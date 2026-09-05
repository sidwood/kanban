//! Herdr session observation: connect, snapshot on startup and
//! reconnect, and append telemetry events (KAN-S8-US1, DR-HB-08).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use kanban_app::{HerdrDiagnostics, TimelineEnvelope};
use kanban_domain::Project;
use kanban_dto::{HerdrConnectionDiagnostics, TimelineEventKind};
use kanban_herdr::{SessionClient, SessionMapping};
use kanban_storage::Database;
use serde_json::json;

/// Why observation could not start for one Project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationError {
    Connect(String),
    Snapshot(String),
    Timeline(String),
}

impl std::fmt::Display for ObservationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(message) | Self::Snapshot(message) | Self::Timeline(message) => {
                write!(f, "{message}")
            }
        }
    }
}

/// Live diagnostics for one Project's Herdr session.
#[derive(Debug, Clone, Default)]
struct SessionDiagnostics {
    session_name: String,
    product_workspace: String,
    herdr_workspace: Option<String>,
    connected: bool,
    last_snapshot_at: Option<String>,
    last_error: Option<String>,
}

/// The service-side Herdr observer.
pub struct HerdrObserver {
    socket_root: PathBuf,
    database: Arc<Database>,
    diagnostics: Arc<Mutex<HashMap<u64, SessionDiagnostics>>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl HerdrObserver {
    /// Start observing every active Project through its per-session
    /// socket. Snapshots on startup are appended as telemetry events.
    pub fn start(database: Arc<Database>, projects: &[Project], socket_root: PathBuf) -> Arc<Self> {
        let observer = Arc::new(Self {
            socket_root,
            database,
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            threads: Mutex::new(Vec::new()),
        });
        for project in projects {
            if project.is_archived() {
                continue;
            }
            observer.observe_project(project);
        }
        observer
    }

    /// Start observing one Project's Herdr session.
    pub fn observe_project(self: &Arc<Self>, project: &Project) {
        if project.is_archived() {
            return;
        }
        let registration = project.registration();
        self.diagnostics.lock().unwrap().insert(
            project.id().value(),
            SessionDiagnostics {
                session_name: registration.herdr_session().to_owned(),
                product_workspace: registration.seed_workspace().to_owned(),
                ..SessionDiagnostics::default()
            },
        );
        let handle = HerdrObserverHandle {
            project_id: project.id().value(),
            mapping: SessionMapping::new(
                registration.herdr_session(),
                registration.seed_workspace(),
            ),
            socket_root: self.socket_root.clone(),
            database: self.database.clone(),
            diagnostics: self.diagnostics.clone(),
        };
        let thread = thread::Builder::new()
            .name(format!("herdr-{}", registration.herdr_session()))
            .spawn(move || handle.run())
            .expect("a Herdr observation thread starts");
        self.threads.lock().unwrap().push(thread);
    }
}

struct HerdrObserverHandle {
    project_id: u64,
    mapping: SessionMapping,
    socket_root: PathBuf,
    database: Arc<Database>,
    diagnostics: Arc<Mutex<HashMap<u64, SessionDiagnostics>>>,
}

impl HerdrObserverHandle {
    fn run(self) {
        let mut first_connect = true;
        loop {
            match SessionClient::connect(self.mapping.clone(), &self.socket_root) {
                Ok(mut client) => {
                    self.mark_connected(true, None);
                    let reason = if first_connect {
                        "startup"
                    } else {
                        "reconnect"
                    };
                    first_connect = false;
                    match self.capture_snapshot(&mut client, reason) {
                        Ok(snapshot) => {
                            self.update_workspace(snapshot.herdr_workspace, snapshot.captured_at)
                        }
                        Err(error) => self.mark_error(error),
                    }
                    if client.subscribe().is_ok() {
                        loop {
                            if client.read_event().is_err() {
                                self.mark_connected(false, Some("disconnected".to_owned()));
                                break;
                            }
                        }
                    }
                }
                Err(error) => self.mark_connected(false, Some(error.to_string())),
            }
            thread::sleep(Duration::from_secs(1));
        }
    }

    fn capture_snapshot(
        &self,
        client: &mut SessionClient,
        reason: &str,
    ) -> Result<kanban_herdr::Snapshot, ObservationError> {
        let snapshot = client
            .snapshot()
            .map_err(|error| ObservationError::Snapshot(error.to_string()))?;
        client
            .mapping()
            .verify_snapshot(&snapshot)
            .map_err(|error| ObservationError::Connect(error.to_string()))?;
        let detail = json!({
            "source": "herdr",
            "event": "snapshot",
            "reason": reason,
            "session": snapshot.session,
            "product_workspace": snapshot.product_workspace,
            "herdr_workspace": snapshot.herdr_workspace,
            "captured_at": snapshot.captured_at,
            "state": snapshot.state,
        });
        let envelope = TimelineEnvelope::project(
            &self.project_id.to_string(),
            TimelineEventKind::Telemetry,
            None,
            detail,
        )
        .map_err(|error| ObservationError::Timeline(error.message))?;
        self.database
            .append_timeline_event(&envelope)
            .map_err(|error| ObservationError::Timeline(error.to_string()))?;
        Ok(snapshot)
    }

    fn mark_connected(&self, connected: bool, last_error: Option<String>) {
        if let Some(entry) = self.diagnostics.lock().unwrap().get_mut(&self.project_id) {
            entry.connected = connected;
            entry.last_error = last_error;
        }
    }

    fn mark_error(&self, error: ObservationError) {
        self.mark_connected(false, Some(error.to_string()));
    }

    fn update_workspace(&self, herdr_workspace: String, captured_at: String) {
        if let Some(entry) = self.diagnostics.lock().unwrap().get_mut(&self.project_id) {
            entry.herdr_workspace = Some(herdr_workspace);
            entry.last_snapshot_at = Some(captured_at);
            entry.connected = true;
            entry.last_error = None;
        }
    }
}

/// Diagnostics served through `herdr.settings.get`.
pub struct LiveHerdrDiagnostics {
    inner: Arc<Mutex<HashMap<u64, SessionDiagnostics>>>,
}

impl LiveHerdrDiagnostics {
    /// Wrap the observer's live diagnostics map.
    pub fn new(observer: &HerdrObserver) -> Self {
        Self {
            inner: observer.diagnostics.clone(),
        }
    }

    /// Seed diagnostics before the observer starts, for tests.
    pub fn empty() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl HerdrDiagnostics for LiveHerdrDiagnostics {
    fn for_project(
        &self,
        project_id: u64,
        session_name: &str,
        product_workspace: &str,
    ) -> HerdrConnectionDiagnostics {
        self.inner
            .lock()
            .unwrap()
            .get(&project_id)
            .cloned()
            .unwrap_or(SessionDiagnostics {
                session_name: session_name.to_owned(),
                product_workspace: product_workspace.to_owned(),
                ..SessionDiagnostics::default()
            })
            .into()
    }
}

impl From<SessionDiagnostics> for HerdrConnectionDiagnostics {
    fn from(value: SessionDiagnostics) -> Self {
        Self {
            session_name: value.session_name,
            product_workspace: value.product_workspace,
            herdr_workspace: value.herdr_workspace,
            connected: value.connected,
            last_snapshot_at: value.last_snapshot_at,
            last_error: value.last_error,
        }
    }
}

/// Resolve the Herdr sessions directory for production observation.
pub fn production_socket_root() -> PathBuf {
    kanban_herdr::herdr_sessions_dir().unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join("Library/Application Support/Herdr/sessions")
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use kanban_app::TimelineStore;
    use kanban_dto::{TimelineEventKind, TimelineQuery, TimelineScope};
    use kanban_herdr::fixture::{ScriptedSession, SessionScript};
    use serde_json::json;
    use tempfile::TempDir;

    use super::{HerdrObserver, production_socket_root};
    use crate::timeline::StorageTimelineStore;
    use kanban_domain::{Project, ProjectId, ProjectRegistration};
    use kanban_storage::{AllowAllMigrations, Database};

    fn project(session: &str, workspace: &str) -> Project {
        let registration = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            workspace,
            "main",
            session,
            None,
        )
        .expect("the registration validates");
        Project::new(ProjectId::new(1), registration)
    }

    #[test]
    fn startup_snapshot_appends_a_telemetry_event() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default(),
        );
        let mut database = Database::open(&dir.path().join("kanban.sqlite"))
            .expect("opening a fresh database succeeds");
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let database = Arc::new(database);
        let _observer = HerdrObserver::start(
            database.clone(),
            &[project("kanban-main", "/workspaces/kanban.seed")],
            socket_root,
        );
        thread::sleep(Duration::from_millis(200));
        let timeline = StorageTimelineStore::new(database);
        let events = timeline
            .query(&TimelineQuery {
                scope: TimelineScope::Project("1".to_owned()),
                entity: None,
                kinds: Some(vec![TimelineEventKind::Telemetry]),
                since: None,
                until: None,
            })
            .expect("telemetry is queryable");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].detail["event"], json!("snapshot"));
        assert_eq!(events[0].detail["reason"], json!("startup"));
    }

    #[test]
    fn production_socket_root_points_at_application_support() {
        let root = production_socket_root();
        assert!(root.ends_with("Herdr/sessions"));
    }
}
