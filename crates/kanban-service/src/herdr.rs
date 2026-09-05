//! Herdr session observation: connect through each Project's
//! effective session, snapshot on startup and reconnect, and append
//! telemetry events (KAN-S8-US1, DR-HB-08, DR-HB-19).

use std::collections::HashMap;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use kanban_app::{HerdrDiagnostics, HerdrProjectObserver, TimelineEnvelope};
use kanban_domain::Project;
use kanban_dto::{HerdrConnectionDiagnostics, TimelineEventKind};
use kanban_herdr::{HerdrError, SessionClient, SessionMapping};
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
    consecutive_failures: u32,
}

/// How failed attempts space out: the base delay doubles with each
/// consecutive failure, capped at the maximum (KAN-T78-AC2).
#[derive(Debug, Clone, Copy)]
pub struct BackoffPolicy {
    base: Duration,
    max: Duration,
}

impl BackoffPolicy {
    /// The production curve: one second doubling up to one minute.
    pub const PRODUCTION: Self = Self {
        base: Duration::from_secs(1),
        max: Duration::from_secs(60),
    };

    /// A custom curve, for tests that need quick redials.
    pub const fn new(base: Duration, max: Duration) -> Self {
        Self { base, max }
    }
}

/// How one observer redials and when a live subscription counts as
/// settled: only a subscription that holds for the settle window
/// resets the backoff and appends its snapshot, so a session that
/// flaps — subscribing and dropping at once — is another failed
/// attempt, never a cycle of resets and reconnect snapshots
/// (KAN-T78-AC2).
#[derive(Debug, Clone, Copy)]
pub struct ObservationTuning {
    /// The backoff failed attempts follow.
    pub backoff: BackoffPolicy,
    /// How long a live subscription must hold before it is settled.
    pub settle: Duration,
}

impl ObservationTuning {
    /// The production tuning: one-second backoff doubling to a minute,
    /// settled after five seconds of a held subscription.
    pub const PRODUCTION: Self = Self {
        backoff: BackoffPolicy::PRODUCTION,
        settle: Duration::from_secs(5),
    };
}

/// Cap the doubling shift so it cannot overflow the base.
const MAX_BACKOFF_SHIFT: u32 = 16;

/// The delay after `failures` consecutive failed attempts: the base
/// doubling per failure, bounded by the policy maximum.
fn backoff_delay(policy: BackoffPolicy, failures: u32) -> Duration {
    let shift = failures.saturating_sub(1).min(MAX_BACKOFF_SHIFT);
    policy.base.saturating_mul(1 << shift).min(policy.max)
}

/// One owned observation: the stop flag its thread polls, the socket
/// duplicate that wakes a blocked read, and the thread itself.
struct Observation {
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<UnixStream>>>,
    handle: Option<JoinHandle<()>>,
}

/// End one observation: signal its thread, wake any blocked read
/// through the socket duplicate, then join the thread so its socket
/// and database references are released before the call returns.
fn end_observation(observation: Observation) {
    observation.stop.store(true, Ordering::Relaxed);
    if let Some(socket) = observation.socket.lock().unwrap().take() {
        let _ = socket.shutdown(Shutdown::Both);
    }
    if let Some(handle) = observation.handle {
        handle.thread().unpark();
        let _ = handle.join();
    }
}

/// The service-side Herdr observer.
pub struct HerdrObserver {
    socket_root: PathBuf,
    database: Arc<Database>,
    diagnostics: Arc<Mutex<HashMap<u64, SessionDiagnostics>>>,
    sessions: Mutex<HashMap<u64, Observation>>,
    backoff: BackoffPolicy,
    settle: Duration,
}

impl HerdrObserver {
    /// Create an observer rooted at `socket_root`.
    pub fn new(database: Arc<Database>, socket_root: PathBuf) -> Arc<Self> {
        Self::with_observation(database, socket_root, ObservationTuning::PRODUCTION)
    }

    /// Create an observer whose failed attempts follow `backoff`.
    pub fn with_backoff(
        database: Arc<Database>,
        socket_root: PathBuf,
        backoff: BackoffPolicy,
    ) -> Arc<Self> {
        Self::with_observation(
            database,
            socket_root,
            ObservationTuning {
                backoff,
                ..ObservationTuning::PRODUCTION
            },
        )
    }

    /// Create an observer following one `tuning`: failed attempts
    /// space out by its backoff, and a live subscription counts as
    /// settled only after its settle window.
    pub fn with_observation(
        database: Arc<Database>,
        socket_root: PathBuf,
        tuning: ObservationTuning,
    ) -> Arc<Self> {
        Arc::new(Self {
            socket_root,
            database,
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            sessions: Mutex::new(HashMap::new()),
            backoff: tuning.backoff,
            settle: tuning.settle,
        })
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
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.contains_key(&project.id().value()) {
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
        let stop = Arc::new(AtomicBool::new(false));
        let socket = Arc::new(Mutex::new(None));
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
            stop: stop.clone(),
            socket: socket.clone(),
            backoff: self.backoff,
            settle: self.settle,
        };
        let thread_name = session
            .as_name()
            .map(|name| format!("herdr-{name}"))
            .unwrap_or_else(|| "herdr-default".to_owned());
        let thread = thread::Builder::new()
            .name(thread_name)
            .spawn(move || handle.run())
            .expect("a Herdr observation thread starts");
        sessions.insert(
            project.id().value(),
            Observation {
                stop,
                socket,
                handle: Some(thread),
            },
        );
    }

    /// Whether this observer still owns one Project's session.
    pub fn is_observing(&self, project_id: u64) -> bool {
        self.sessions.lock().unwrap().contains_key(&project_id)
    }

    /// Consecutive failed attempts one Project's observation is
    /// backing off, for diagnostics (KAN-T78-AC2).
    pub fn consecutive_failures(&self, project_id: u64) -> u32 {
        self.diagnostics
            .lock()
            .unwrap()
            .get(&project_id)
            .map(|entry| entry.consecutive_failures)
            .unwrap_or(0)
    }

    /// Stop observing one Project, release its socket and database
    /// references, and join its thread (KAN-T78-AC1).
    pub fn stop_observing(&self, project_id: u64) {
        let Some(observation) = self.sessions.lock().unwrap().remove(&project_id) else {
            return;
        };
        end_observation(observation);
        self.diagnostics.lock().unwrap().remove(&project_id);
    }

    /// Stop every owned observer and join every thread: shutdown
    /// leaves no session observed and no resource held (KAN-T78-AC1).
    pub fn shutdown(&self) {
        let observations: Vec<Observation> = self
            .sessions
            .lock()
            .unwrap()
            .drain()
            .map(|(_, observation)| observation)
            .collect();
        for observation in observations {
            end_observation(observation);
        }
        self.diagnostics.lock().unwrap().clear();
    }
}

impl HerdrProjectObserver for HerdrObserver {
    fn observe(&self, project: &Project) {
        self.observe_project(project);
    }

    fn stop_observing(&self, project_id: u64) {
        HerdrObserver::stop_observing(self, project_id);
    }
}

struct HerdrObserverHandle {
    project_id: u64,
    mapping: SessionMapping,
    socket_root: PathBuf,
    database: Arc<Database>,
    diagnostics: Arc<Mutex<HashMap<u64, SessionDiagnostics>>>,
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<UnixStream>>>,
    backoff: BackoffPolicy,
    settle: Duration,
}

impl HerdrObserverHandle {
    fn run(self) {
        let mut failures = 0u32;
        let mut live_once = false;
        while !self.stopped() {
            if self.observe_live(&mut live_once) {
                // A settled session that ended is the first failure of
                // the next cycle, not a reset.
                failures = 1;
            } else {
                failures = failures.saturating_add(1);
            }
            self.note_failures(failures);
            if self.stopped() {
                break;
            }
            thread::park_timeout(backoff_delay(self.backoff, failures));
        }
    }

    /// Publish the cycle's failure count, so the settled-live rule is
    /// observable per Project (KAN-T78-AC2).
    fn note_failures(&self, failures: u32) {
        if let Some(entry) = self.diagnostics.lock().unwrap().get_mut(&self.project_id) {
            entry.consecutive_failures = failures;
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Connect, subscribe, and observe the live subscription. Returns
    /// whether the subscription settled and then ended: a failed
    /// connection, subscription, or settle window reports through
    /// diagnostics alone, and the captured snapshot is appended only
    /// once the subscription has held past the settle window
    /// (KAN-T78-AC2, KAN-T78-AC3).
    fn observe_live(&self, live_once: &mut bool) -> bool {
        // Open carries no request traffic: the socket duplicate — the
        // only handle a stop can shut down — is registered before the
        // snapshot handshake blocks, so stopping this thread is always
        // possible, even mid-handshake.
        let mut client = match SessionClient::open(self.mapping.clone(), &self.socket_root) {
            Ok(client) => client,
            Err(error) => {
                self.mark_connected(false, Some(error.to_string()));
                return false;
            }
        };
        // Register the duplicate before anything can block, so a stop
        // can always wake this thread.
        match client.duplicate_socket() {
            Ok(duplicate) => *self.socket.lock().unwrap() = Some(duplicate),
            Err(error) => {
                self.mark_connected(false, Some(error.to_string()));
                return false;
            }
        }
        if self.stopped() {
            return false;
        }
        // Capture before subscribing so no push event can overtake
        // the capture; the append below is gated on the settle window.
        let snapshot = match client.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.mark_connected(false, Some(error.to_string()));
                return false;
            }
        };
        if let Err(error) = client.mapping().verify_snapshot(&snapshot) {
            self.mark_connected(false, Some(error.to_string()));
            return false;
        }
        if let Err(error) = client.subscribe() {
            *self.socket.lock().unwrap() = None;
            self.mark_connected(false, Some(error.to_string()));
            return false;
        }
        self.mark_connected(true, None);
        // Only a subscription that holds for the settle window counts
        // as the live session: one that drops inside the window is
        // another failed attempt, so it neither resets the backoff nor
        // lands its snapshot (KAN-T78-AC2).
        if self.stopped() || !self.settles(&mut client) {
            self.mark_connected(false, Some("disconnected".to_owned()));
            return false;
        }
        let reason = if *live_once { "reconnect" } else { "startup" };
        *live_once = true;
        match self.append_snapshot(&snapshot, reason) {
            Ok(()) => self.record_snapshot(snapshot.captured_at),
            Err(error) => self.mark_error(error),
        }
        while !self.stopped() {
            if client.read_event().is_err() {
                self.mark_connected(false, Some("disconnected".to_owned()));
                break;
            }
        }
        true
    }

    /// Wait out the settle window against the live subscription: a
    /// subscription still answering — pushing events or sitting quiet —
    /// when the window closes is settled, while one whose connection
    /// ends inside it is not (KAN-T78-AC2).
    fn settles(&self, client: &mut SessionClient) -> bool {
        let deadline = Instant::now() + self.settle;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return true;
            }
            match client.read_event_within(remaining) {
                // A pushed event proves the subscription live; the
                // window still has to be waited out.
                Ok(_) => continue,
                // Silence held to the deadline is as settled as
                // traffic.
                Err(HerdrError::TimedOut) => return true,
                // The connection ended inside the window: a flap, not
                // a live session.
                Err(_) => return false,
            }
        }
    }

    /// Append one captured snapshot as a telemetry event.
    fn append_snapshot(
        &self,
        snapshot: &kanban_herdr::Snapshot,
        reason: &str,
    ) -> Result<(), ObservationError> {
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
        let envelope =
            TimelineEnvelope::project(self.project_id, TimelineEventKind::Telemetry, None, detail);
        self.database
            .append_timeline_event(&envelope)
            .map_err(|error| ObservationError::Timeline(error.to_string()))?;
        Ok(())
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use kanban_app::{HerdrDiagnostics, TimelineStore};
    use kanban_dto::{TimelineEventKind, TimelineQuery, TimelineScope};
    use kanban_herdr::fixture::{ScriptedSession, SessionScript};
    use serde_json::json;
    use tempfile::TempDir;

    use super::{
        BackoffPolicy, HerdrObserver, LiveHerdrDiagnostics, ObservationTuning, backoff_delay,
        production_socket_root,
    };
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

    fn migrated_database(dir: &TempDir) -> Arc<Database> {
        let mut database = Database::open(&dir.path().join("kanban.sqlite"))
            .expect("opening a fresh database succeeds");
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        Arc::new(database)
    }

    /// A fast curve so lifecycle tests redial within milliseconds.
    fn fast_backoff() -> BackoffPolicy {
        BackoffPolicy::new(Duration::from_millis(10), Duration::from_millis(40))
    }

    /// A fast tuning so lifecycle tests settle within milliseconds.
    fn fast_observation() -> ObservationTuning {
        ObservationTuning {
            backoff: fast_backoff(),
            settle: Duration::from_millis(100),
        }
    }

    fn telemetry_details(database: &Arc<Database>) -> Vec<serde_json::Value> {
        let timeline = StorageTimelineStore::new(database.clone());
        timeline
            .query(&TimelineQuery {
                scope: TimelineScope::Project(1),
                entity: None,
                kinds: Some(vec![TimelineEventKind::Telemetry]),
                since: None,
                until: None,
            })
            .expect("telemetry is queryable")
            .into_iter()
            .map(|event| event.detail)
            .collect()
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
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(300));
        let details = telemetry_details(&database);
        assert_eq!(details.len(), 1);
        assert_eq!(details[0]["event"], json!("snapshot"));
        assert_eq!(details[0]["reason"], json!("startup"));
        observer.shutdown();
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
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(None, "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(200));
        let timeline = StorageTimelineStore::new(database);
        let events = timeline
            .query(&TimelineQuery {
                scope: TimelineScope::Project(1),
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
        observer.shutdown();
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

    #[test]
    fn observer_backoff_doubles_until_bounded() {
        let policy = BackoffPolicy::PRODUCTION;
        assert_eq!(backoff_delay(policy, 1), Duration::from_secs(1));
        assert_eq!(backoff_delay(policy, 2), Duration::from_secs(2));
        assert_eq!(backoff_delay(policy, 3), Duration::from_secs(4));
        assert_eq!(backoff_delay(policy, 6), Duration::from_secs(32));
        assert_eq!(
            backoff_delay(policy, 7),
            Duration::from_secs(60),
            "the doubling is bounded at the maximum"
        );
        assert_eq!(backoff_delay(policy, 60), Duration::from_secs(60));
    }

    #[test]
    fn observer_shutdown_stops_the_owned_session() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default(),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(300));
        assert!(observer.is_observing(1));
        assert_eq!(telemetry_details(&database).len(), 1);

        observer.shutdown();

        assert!(
            !observer.is_observing(1),
            "shutdown releases the owned session"
        );
        thread::sleep(Duration::from_millis(150));
        assert_eq!(
            telemetry_details(&database).len(),
            1,
            "no thread survives shutdown to append again"
        );
    }

    #[test]
    fn observer_backs_off_failed_connections_without_appending_rows() {
        let dir = TempDir::new().expect("a scratch directory is available");
        // No fixture: every connect fails on the missing socket.
        let socket_root = dir.path().join("sessions");
        let database = migrated_database(&dir);
        let observer = HerdrObserver::with_backoff(database.clone(), socket_root, fast_backoff());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(250));

        assert!(
            observer.is_observing(1),
            "failed connects keep observing under backoff"
        );
        assert_eq!(
            telemetry_details(&database).len(),
            0,
            "a failed connect appends no timeline row"
        );
        let diagnostics = LiveHerdrDiagnostics::new(&observer);
        let state = diagnostics.for_project(
            1,
            Some("kanban-main"),
            "/workspaces/kanban.seed",
            "kanban.seed",
        );
        assert!(!state.connected);
        assert!(
            state
                .last_error
                .expect("the failed connection is reported")
                .contains("not available")
        );

        observer.shutdown();
    }

    #[test]
    fn observer_appends_no_rows_when_subscription_never_succeeds() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_subscribe_error("session is sealed"),
        );
        let database = migrated_database(&dir);
        let observer = HerdrObserver::with_backoff(database.clone(), socket_root, fast_backoff());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(250));

        assert!(
            observer.is_observing(1),
            "failed subscriptions keep observing under backoff"
        );
        assert_eq!(
            telemetry_details(&database).len(),
            0,
            "a connection without a live subscription appends no snapshot row"
        );
        let diagnostics = LiveHerdrDiagnostics::new(&observer);
        let state = diagnostics.for_project(
            1,
            Some("kanban-main"),
            "/workspaces/kanban.seed",
            "kanban.seed",
        );
        assert!(!state.connected);
        assert!(
            state
                .last_error
                .expect("the refused subscription is reported")
                .contains("sealed")
        );

        observer.shutdown();
    }

    #[test]
    fn observer_reconnect_snapshot_lands_only_after_a_live_subscription() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            // The first connection settles — held past the settle
            // window — before it closes, so the second connection is a
            // genuine reconnect of a live session.
            SessionScript::default()
                .with_events(vec![json!({ "kind": "role.output", "text": "working" })])
                .close_after_hold(Duration::from_millis(300)),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(800));

        let reasons: Vec<_> = telemetry_details(&database)
            .iter()
            .map(|detail| detail["reason"].clone())
            .collect();
        assert_eq!(
            reasons,
            vec![json!("startup"), json!("reconnect")],
            "the reconnect snapshot lands after the second settled subscription, and only after it"
        );

        observer.shutdown();
    }

    #[test]
    fn flapping_subscriptions_neither_reset_backoff_nor_append() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_flapping_subscriptions(),
        );
        let database = migrated_database(&dir);
        // A settle window far beyond any flap, so no cycle can settle.
        let observer = HerdrObserver::with_observation(
            database.clone(),
            socket_root,
            ObservationTuning {
                backoff: fast_backoff(),
                settle: Duration::from_millis(500),
            },
        );
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(1200));

        assert!(
            observer.is_observing(1),
            "flapping subscriptions keep observing under backoff"
        );
        assert!(
            telemetry_details(&database).len() <= 1,
            "a subscription that never settles appends no snapshot per cycle"
        );
        assert!(
            observer.consecutive_failures(1) >= 3,
            "flapping is a run of failures, not a reset per cycle"
        );

        observer.shutdown();
    }

    #[test]
    fn observer_threads_stop_on_core_shutdown() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let repository = dir.path().join("wave");
        std::fs::create_dir_all(repository.join(".git")).expect("the scratch repository exists");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "wave-main",
            "/workspaces/wave.seed",
            SessionScript::default(),
        );
        let core = crate::serve_with_herdr_sessions(dir.path(), socket_root)
            .expect("the core boots for the test");
        let mut client = crate::test_client::Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-wave" },
                "code": "WAVE",
                "name": "Wave pool",
                "repository": repository.to_str().expect("the path is UTF-8"),
                "seed_workspace": "/workspaces/wave.seed",
                "default_branch": "main",
                "herdr_session": "wave-main",
                "herdr_workspace": "wave.seed",
            }),
        );
        thread::sleep(Duration::from_millis(200));
        let observer = core.herdr.clone();
        assert!(
            observer.is_observing(1),
            "registration starts observation without a restart"
        );

        core.shutdown();

        assert!(
            !observer.is_observing(1),
            "core shutdown stops the owned observer and joins its thread"
        );
    }

    #[test]
    fn observer_stops_when_the_project_archives() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let repository = dir.path().join("wave");
        std::fs::create_dir_all(repository.join(".git")).expect("the scratch repository exists");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "wave-main",
            "/workspaces/wave.seed",
            SessionScript::default(),
        );
        let core = crate::serve_with_herdr_sessions(dir.path(), socket_root)
            .expect("the core boots for the test");
        let mut client = crate::test_client::Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-wave" },
                "code": "WAVE",
                "name": "Wave pool",
                "repository": repository.to_str().expect("the path is UTF-8"),
                "seed_workspace": "/workspaces/wave.seed",
                "default_branch": "main",
                "herdr_session": "wave-main",
                "herdr_workspace": "wave.seed",
            }),
        );
        thread::sleep(Duration::from_millis(200));
        let observer = core.herdr.clone();
        assert!(observer.is_observing(1));

        client.command(
            "project.archive",
            json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "archive-wave" },
                "project_id": 1,
            }),
        );

        assert!(
            !observer.is_observing(1),
            "the landed archive releases the owned observer"
        );
        thread::sleep(Duration::from_millis(150));
        let answer = client.query_with(
            "timeline.query",
            json!({
                "scope": { "project": "1" },
                "kinds": ["telemetry"],
            }),
        );
        assert_eq!(
            answer["events"].as_array().map(Vec::len),
            Some(1),
            "no snapshot lands after the archive stops observation"
        );

        core.shutdown();
    }

    /// Sleep in small steps until `condition` holds or `limit`
    /// elapses, reporting whether the condition was met.
    fn soon_enough(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        condition()
    }

    /// Run `action` on a helper thread and report whether it finished
    /// within `limit`: a stop that could hang forever fails the test
    /// instead of hanging it.
    fn bounded_within(limit: Duration, action: impl FnOnce() + Send + 'static) -> bool {
        let finished = Arc::new(AtomicBool::new(false));
        let flag = finished.clone();
        thread::spawn(move || {
            action();
            flag.store(true, Ordering::SeqCst);
        });
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if finished.load(Ordering::SeqCst) {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn shutdown_during_the_snapshot_handshake_is_bounded() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_silent_handshake(),
        );
        let database = migrated_database(&dir);
        let observer = HerdrObserver::with_backoff(database, socket_root, fast_backoff());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        assert!(
            soon_enough(Duration::from_secs(2), || fixture.requests_seen() >= 1),
            "the observer reaches the handshake it blocks on"
        );

        assert!(
            bounded_within(Duration::from_secs(5), move || observer.shutdown()),
            "shutdown interrupts the blocked handshake instead of hanging on its join"
        );
    }

    #[test]
    fn archiving_through_dispatch_cannot_deadlock() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let repository = dir.path().join("wave");
        std::fs::create_dir_all(repository.join(".git")).expect("the scratch repository exists");
        let fixture = ScriptedSession::bind(
            &socket_root,
            "wave-main",
            "/workspaces/wave.seed",
            SessionScript::default().with_silent_handshake(),
        );
        let core = crate::serve_with_herdr_sessions(dir.path(), socket_root)
            .expect("the core boots for the test");
        let mut client = crate::test_client::Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-wave" },
                "code": "WAVE",
                "name": "Wave pool",
                "repository": repository.to_str().expect("the path is UTF-8"),
                "seed_workspace": "/workspaces/wave.seed",
                "default_branch": "main",
                "herdr_session": "wave-main",
                "herdr_workspace": "wave.seed",
            }),
        );
        assert!(
            soon_enough(Duration::from_secs(2), || fixture.requests_seen() >= 1),
            "the observer blocks on the handshake before the archive dispatches"
        );

        // The archive must return even though its observer is blocked
        // on the handshake read: the release joins outside the write
        // span and the registered duplicate interrupts the read.
        assert!(
            bounded_within(Duration::from_secs(5), move || {
                client.command(
                    "project.archive",
                    json!({
                        "mutation": { "optimistic_version": 1, "idempotency_key": "archive-wave" },
                        "project_id": 1,
                    }),
                );
            }),
            "the archive returns instead of deadlocking on its observer"
        );

        let mut after = crate::test_client::Client::connect(core.socket_path());
        let listed = after.query("project.list");
        assert_eq!(
            listed["projects"][0]["archived"],
            json!(true),
            "the archive that returned also landed"
        );

        core.shutdown();
    }
}
