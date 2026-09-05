//! Herdr session observation: connect through each Project's
//! effective session, snapshot on startup and reconnect, and append
//! telemetry events (KAN-S8-US1, DR-HB-08, DR-HB-19).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use kanban_app::{HerdrDiagnostics, HerdrProjectObserver, TimelineEnvelope};
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

/// Live diagnostics for one Project's Herdr binding: the session the
/// Project selected (absent for Herdr's default), the product
/// workspace, and the target Herdr workspace resolved inside that
/// session.
#[derive(Debug, Clone, Default)]
struct SessionDiagnostics {
    session_name: Option<String>,
    product_workspace: String,
    herdr_workspace: String,
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
    /// Create an observer rooted at `socket_root`.
    pub fn new(database: Arc<Database>, socket_root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            socket_root,
            database,
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            threads: Mutex::new(Vec::new()),
        })
    }

    /// Start observing every active Project through its per-session
    /// socket. Snapshots on startup are appended as telemetry events.
    pub fn start(database: Arc<Database>, projects: &[Project], socket_root: PathBuf) -> Arc<Self> {
        let observer = Self::new(database, socket_root);
        observer.observe_projects(projects);
        observer
    }

    /// Start observing every active Project in `projects`.
    pub fn observe_projects(&self, projects: &[Project]) {
        for project in projects {
            if project.is_archived() {
                continue;
            }
            self.observe_project(project);
        }
    }

    /// Start observing one Project's Herdr binding through its
    /// effective session.
    pub fn observe_project(&self, project: &Project) {
        if project.is_archived() {
            return;
        }
        let registration = project.registration();
        let session = registration.effective_herdr_session();
        self.diagnostics.lock().unwrap().insert(
            project.id().value(),
            SessionDiagnostics {
                session_name: registration.herdr_session().map(str::to_owned),
                product_workspace: registration.seed_workspace().to_owned(),
                herdr_workspace: registration.herdr_workspace().to_owned(),
                ..SessionDiagnostics::default()
            },
        );
        let handle = HerdrObserverHandle {
            project_id: project.id().value(),
            mapping: SessionMapping::new(
                session.clone(),
                registration.seed_workspace(),
                registration.herdr_workspace(),
            ),
            socket_root: self.socket_root.clone(),
            database: self.database.clone(),
            diagnostics: self.diagnostics.clone(),
        };
        let thread_name = session
            .as_name()
            .map(|name| format!("herdr-{name}"))
            .unwrap_or_else(|| "herdr-default".to_owned());
        let thread = thread::Builder::new()
            .name(thread_name)
            .spawn(move || handle.run())
            .expect("a Herdr observation thread starts");
        self.threads.lock().unwrap().push(thread);
    }
}

impl HerdrProjectObserver for HerdrObserver {
    fn observe(&self, project: &Project) {
        self.observe_project(project);
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
                        Ok(snapshot) => self.record_snapshot(snapshot.captured_at),
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

    fn record_snapshot(&self, captured_at: String) {
        if let Some(entry) = self.diagnostics.lock().unwrap().get_mut(&self.project_id) {
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
        session: Option<&str>,
        product_workspace: &str,
        herdr_workspace: &str,
    ) -> HerdrConnectionDiagnostics {
        self.inner
            .lock()
            .unwrap()
            .get(&project_id)
            .cloned()
            .unwrap_or(SessionDiagnostics {
                session_name: session.map(str::to_owned),
                product_workspace: product_workspace.to_owned(),
                herdr_workspace: herdr_workspace.to_owned(),
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

/// Resolve Herdr's config root for default and named session observation.
pub fn production_socket_root() -> PathBuf {
    kanban_herdr::herdr_sessions_dir().unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/herdr")
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

    fn project(session: Option<&str>, workspace: &str) -> Project {
        let registration = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            workspace,
            "main",
            "kanban.seed",
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
            &[project(Some("kanban-main"), "/workspaces/kanban.seed")],
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

    /// KAN-T100-AC4: a Project without a session is observed through
    /// Herdr's default session socket, with no session selection.
    #[test]
    fn a_sessionless_project_is_observed_through_the_default_session_socket() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind_default(
            &socket_root,
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
            &[project(None, "/workspaces/kanban.seed")],
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
        assert_eq!(
            events.len(),
            1,
            "the default session serves the sessionless Project"
        );
        assert_eq!(events[0].detail["event"], json!("snapshot"));
    }

    #[test]
    fn production_socket_root_matches_the_installed_cli_config_root() {
        let root = production_socket_root();
        assert_eq!(
            root,
            std::path::PathBuf::from(std::env::var_os("HOME").expect("home is known"))
                .join(".config/herdr")
        );
    }
}
