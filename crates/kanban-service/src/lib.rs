//! The durable per-user core process: it wires storage, the
//! application core, and the socket transport together and keeps
//! serving after the desktop UI quits (ADR-0001).

mod backup_scheduler;
pub mod diagnostics;
pub mod export_files;
pub mod fleet_clone;
pub mod git_observer;
pub mod herdr;
pub mod logs;
pub mod redaction;
mod schedule_scheduler;
pub mod timeline;

#[cfg(test)]
mod test_client;

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kanban_app::{
    ActivationPass, CloneTargetProbe, Core, EventSink, FleetCloneTool, GitObservation,
    ProjectStore, StoredProfileCatalogue, TimelineQueryHandler,
};
use kanban_storage::paths::database_file_name;
use kanban_storage::{
    BackupStore, Database, RetentionPolicy, SqliteCapacityStore, SqliteCloneGuardStore,
    SqliteCommentStore, SqliteDeferralStore, SqliteDependencyStore, SqliteDispatchStore,
    SqliteEvidenceStore, SqliteGraphProposalStore, SqliteHerdrSettingsStore,
    SqliteIdempotencyStore, SqliteInitiativeStore, SqliteLaneStore, SqlitePlanStore,
    SqliteProfileStore, SqliteProjectStore, SqliteRulingStore, SqliteScheduleStore,
    SqliteSpecStore, SqliteTicketStore, SqliteWorkspaceStore, VerifiedBackupHook,
    load_backup_settings,
};
use kanban_transport::{ServerHandle, SocketServer, TransportError};

use herdr::{HerdrObserver, LiveHerdrDiagnostics, ObservationTuning, production_socket_root};
use timeline::StorageTimelineStore;

use backup_scheduler::BackupScheduler;
use diagnostics::DiagnosticsExportHandler;
use fleet_clone::LocalFleetCloneTool;
use git_observer::LocalWorkspaceGitObserver;
use logs::{LogLevel, LogRecord, LogWriter};
use schedule_scheduler::ActivationScheduler;

/// How many replay outcomes the core keeps. A retry follows its
/// original within seconds and the Operator drives one window, so a
/// five-figure bound covers every retry that could still arrive
/// while keeping the table small enough to ignore.
const RETAINED_OUTCOMES: NonZeroU32 = NonZeroU32::new(10_000).expect("the bound is not zero");

/// The minimum age every replay outcome survives, even when a burst
/// exceeds the count bound. A client may retry across restarts and
/// command bursts for up to a day before the table can prune an
/// outcome on count alone.
const MINIMUM_REPLAY_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The service's git observation: a target is a Git repository when
/// its path holds a `.git` entry, a directory for a normal clone or a
/// file for a linked worktree. Registration refuses everything else,
/// keeping non-Git Projects out (DR-PH-08).
#[derive(Debug, Default)]
pub struct LocalRepositories;

impl GitObservation for LocalRepositories {
    fn is_repository(&self, repository: &str) -> bool {
        Path::new(repository).join(".git").exists()
    }
}

/// The service's clone-target probe: a create target is occupied when
/// anything at all already exists at its path. An unregistered clone
/// above all — the shape `git bc-add` treats as success while it
/// syncs extras into it — must be refused before the skill runs, or
/// Kanban records a creation it did not perform (KAN-T118).
#[derive(Debug, Default)]
pub struct LocalCloneTargetProbe;

impl CloneTargetProbe for LocalCloneTargetProbe {
    fn exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }
}

/// The running core process: its open database, its serving
/// socket, its Herdr observer, and its structured logs.
pub struct CoreProcess {
    database: Arc<Database>,
    server: ServerHandle,
    herdr: Arc<HerdrObserver>,
    logs: Arc<LogWriter>,
    _backup_scheduler: BackupScheduler,
    _activation_scheduler: ActivationScheduler,
}

impl CoreProcess {
    /// The path clients connect on.
    pub fn socket_path(&self) -> &Path {
        self.server.socket_path()
    }

    /// The Herdr socket root this core dials.
    #[cfg(test)]
    pub(crate) fn herdr_socket_root(&self) -> &Path {
        self.herdr.socket_root()
    }

    /// Stop serving, stop every Herdr observer, and close the
    /// database.
    pub fn shutdown(self) {
        let Self {
            database,
            server,
            herdr,
            logs,
            _backup_scheduler,
            _activation_scheduler,
        } = self;
        server.shutdown();
        herdr.shutdown();
        // A failing log write must never fail the shutdown it records.
        let _ = logs.append(&LogRecord::new(LogLevel::Info, "service", "core stopped"));
        drop(database);
    }
}

/// Open the durable database and apply every known migration before
/// any socket becomes reachable.
fn prepare_database(data_dir: &Path) -> Result<Database, ServiceError> {
    std::fs::create_dir_all(data_dir).map_err(|source| ServiceError::DataDir { source })?;
    let mut database = Database::open(&data_dir.join(database_file_name()))?;
    let store = BackupStore::new(data_dir.to_path_buf());
    let settings = load_backup_settings(data_dir);
    let hook = VerifiedBackupHook::create_before_migrate(&store, &database, settings.retention);
    database.migrate(&hook)?;
    Ok(database)
}

/// Wire the production application core around a prepared database,
/// the event sink owned by its transport, and `fleet_tool` serving
/// the guarded clone commands. Observation follows `observation`.
fn assemble_core(
    data_dir: &Path,
    database: Database,
    events: Arc<dyn EventSink>,
    herdr_socket_root: PathBuf,
    observation: ObservationTuning,
    fleet_tool: Arc<dyn FleetCloneTool>,
) -> Result<(Arc<Database>, Core, Arc<HerdrObserver>, ActivationPass), ServiceError> {
    let initiative_store = Arc::new(SqliteInitiativeStore::new(&database));
    let project_store = Arc::new(SqliteProjectStore::new(&database));
    let herdr_settings_store = Arc::new(SqliteHerdrSettingsStore::new(&database));
    let plan_store = Arc::new(SqlitePlanStore::new(&database));
    let workspace_store = Arc::new(SqliteWorkspaceStore::new(&database));
    let lane_store = Arc::new(SqliteLaneStore::new(&database));
    let clone_guard_store = Arc::new(SqliteCloneGuardStore::new(&database));
    let spec_store = Arc::new(SqliteSpecStore::new(&database));
    let ticket_store = Arc::new(SqliteTicketStore::new(&database));
    let schedule_store = Arc::new(SqliteScheduleStore::new(&database));
    let dependency_store = Arc::new(SqliteDependencyStore::new(&database));
    let graph_proposal_store = Arc::new(SqliteGraphProposalStore::new(&database));
    let profile_store = Arc::new(SqliteProfileStore::new(&database));
    let capacity_store = Arc::new(SqliteCapacityStore::new(&database));
    let dispatch_store = Arc::new(SqliteDispatchStore::new(&database));
    let comment_store = Arc::new(SqliteCommentStore::new(&database));
    let ruling_store = Arc::new(SqliteRulingStore::new(&database));
    let deferral_store = Arc::new(SqliteDeferralStore::new(&database));
    let idempotency_store = Arc::new(SqliteIdempotencyStore::new(
        &database,
        RetentionPolicy::new(RETAINED_OUTCOMES, MINIMUM_REPLAY_AGE),
    ));
    let evidence_store = Arc::new(SqliteEvidenceStore::new(
        &database,
        data_dir.join("attachments"),
    ));
    let database = Arc::new(database);
    let timeline_store = Arc::new(StorageTimelineStore::new(database.clone()));
    let service_version = env!("CARGO_PKG_VERSION");
    let mut core = Core::with_health(service_version, idempotency_store, events)?;
    let herdr = HerdrObserver::with_observation(database.clone(), herdr_socket_root, observation);
    core.register_initiatives(initiative_store.clone())?;
    let projects = project_store.clone();
    core.register_projects(
        project_store.clone(),
        Arc::new(LocalRepositories),
        initiative_store,
        herdr_settings_store.clone(),
        herdr.clone(),
    )?;
    core.register_workspaces(
        workspace_store.clone(),
        project_store.clone(),
        Arc::new(LocalWorkspaceGitObserver),
    )?;
    core.register_plans(plan_store.clone(), projects.clone(), spec_store.clone())?;
    core.register_specs(spec_store.clone(), projects.clone(), plan_store.clone())?;
    core.register_tickets(
        ticket_store.clone(),
        projects.clone(),
        spec_store.clone(),
        evidence_store.clone(),
    )?;
    core.register_lanes(
        lane_store.clone(),
        projects.clone(),
        workspace_store.clone(),
        ticket_store.clone(),
    )?;
    core.register_clones(
        fleet_tool,
        projects.clone(),
        workspace_store.clone(),
        clone_guard_store,
        Arc::new(LocalCloneTargetProbe),
    )?;
    core.register_dependencies(
        dependency_store.clone(),
        ticket_store.clone(),
        projects.clone(),
    )?;
    // The scheduler's pass shares the serving core's stores, so one
    // activation lands through the same rules every command does.
    let activation_pass = ActivationPass::new(
        ticket_store.clone(),
        dependency_store.clone(),
        schedule_store.clone(),
    );
    core.register_lifecycle(
        ticket_store.clone(),
        dependency_store.clone(),
        projects.clone(),
        schedule_store,
    )?;
    core.register_graph_proposals(
        graph_proposal_store,
        dependency_store.clone(),
        ticket_store.clone(),
        spec_store.clone(),
        projects.clone(),
        profile_store.clone(),
    )?;
    core.register_reassignment(ticket_store.clone(), projects.clone(), spec_store.clone())?;
    core.register_profiles(
        profile_store.clone(),
        ticket_store.clone(),
        projects.clone(),
    )?;
    core.register_plan_diagnostics(
        plan_store.clone(),
        projects.clone(),
        spec_store.clone(),
        Arc::new(StoredProfileCatalogue::new(
            profile_store.clone(),
            ticket_store.clone(),
            spec_store.clone(),
        )),
    )?;
    core.register_capacity(capacity_store.clone(), projects.clone())?;
    core.register_dispatch(
        dispatch_store,
        ticket_store.clone(),
        profile_store.clone(),
        project_store.clone(),
        capacity_store,
        lane_store.clone(),
        dependency_store.clone(),
        herdr.clone(),
    )?;
    core.register_exports(
        plan_store,
        spec_store,
        ticket_store,
        projects,
        Arc::new(export_files::LocalExportFiles),
    )?;
    core.register_comments(comment_store, project_store.clone())?;
    core.register_rulings(ruling_store, project_store.clone())?;
    core.register_deferrals(deferral_store, project_store.clone())?;
    core.register_evidence(evidence_store, project_store.clone())?;
    core.register_query(
        "timeline.query",
        Arc::new(TimelineQueryHandler::new(timeline_store)),
    )?;
    core.register_query(
        "diagnostics.export",
        Arc::new(DiagnosticsExportHandler::new(data_dir, service_version)),
    )?;
    let projects = project_store
        .list()
        .map_err(|cause| ServiceError::ProjectLoad { cause })?;
    herdr.observe_projects(&projects);
    let diagnostics = Arc::new(LiveHerdrDiagnostics::new(&herdr));
    core.register_herdr(herdr_settings_store, diagnostics, project_store)?;
    Ok((database, core, herdr, activation_pass))
}

/// Open (creating if needed) the database inside `data_dir`, bring
/// its schema up to date, and serve the application core on
/// `core.sock` inside the same directory.
pub fn serve(data_dir: &Path) -> Result<CoreProcess, ServiceError> {
    serve_configured(
        data_dir,
        production_socket_root(),
        ObservationTuning::PRODUCTION,
        Arc::new(LocalFleetCloneTool::new(data_dir.to_path_buf())),
    )
}

/// Open the database, bind the socket, and serve a core wired for
/// `herdr_socket_root`, `observation`, and `fleet_tool` until
/// shutdown.
fn serve_configured(
    data_dir: &Path,
    herdr_socket_root: PathBuf,
    observation: ObservationTuning,
    fleet_tool: Arc<dyn FleetCloneTool>,
) -> Result<CoreProcess, ServiceError> {
    let database = prepare_database(data_dir)?;
    let server = SocketServer::bind(data_dir)?;
    let broker = server.broker();
    let (database, core, herdr, activation_pass) = assemble_core(
        data_dir,
        database,
        broker.clone(),
        herdr_socket_root,
        observation,
        fleet_tool,
    )?;
    let logs =
        Arc::new(LogWriter::open(data_dir).map_err(|source| ServiceError::LogOpen { source })?);
    let backup_scheduler =
        BackupScheduler::spawn(data_dir.to_path_buf(), database.clone(), logs.clone());
    let activation_scheduler = ActivationScheduler::spawn(activation_pass, broker, logs.clone());
    let server = server.serve(Arc::new(core))?;
    let socket_path = server.socket_path().to_path_buf();
    // The startup record names the live socket, which is the fact a
    // support session wants first; a failing write never blocks serve.
    let _ = logs.append(&LogRecord::new(
        LogLevel::Info,
        "service",
        format!("core serving on {}", socket_path.display()),
    ));
    Ok(CoreProcess {
        database,
        server,
        herdr,
        logs,
        _backup_scheduler: backup_scheduler,
        _activation_scheduler: activation_scheduler,
    })
}

#[cfg(test)]
pub(crate) fn serve_with_herdr_sessions(
    data_dir: &Path,
    herdr_socket_root: PathBuf,
) -> Result<CoreProcess, ServiceError> {
    serve_configured(
        data_dir,
        herdr_socket_root,
        fast_observation(),
        Arc::new(LocalFleetCloneTool::default()),
    )
}

/// Serve a test core whose guarded clone commands run through
/// `fleet_tool`, over an isolated Herdr socket root (KAN-T120).
#[cfg(test)]
pub(crate) fn serve_with_fleet_clone_tool(
    data_dir: &Path,
    herdr_socket_root: PathBuf,
    fleet_tool: Arc<dyn FleetCloneTool>,
) -> Result<CoreProcess, ServiceError> {
    serve_configured(data_dir, herdr_socket_root, fast_observation(), fleet_tool)
}

/// Observation tuned fast, so core-level tests settle and redial
/// within their own budgets.
#[cfg(test)]
fn fast_observation() -> ObservationTuning {
    ObservationTuning {
        backoff: herdr::BackoffPolicy::new(Duration::from_millis(10), Duration::from_millis(40)),
        settle: Duration::from_millis(50),
        io_timeout: Duration::from_millis(100),
    }
}

/// Why the core process could not start.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// The data directory could not be created.
    #[error("the data directory could not be created: {source}")]
    DataDir {
        /// The underlying failure.
        source: std::io::Error,
    },
    /// The managed logs directory could not be opened.
    #[error("the logs directory could not be opened: {source}")]
    LogOpen {
        /// The underlying failure.
        source: std::io::Error,
    },
    /// Storage refused.
    #[error(transparent)]
    Storage(#[from] kanban_storage::StorageError),
    /// Transport refused.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// A handler could not be registered.
    #[error(transparent)]
    Registration(#[from] kanban_app::RegistrationError),
    /// Persisted Projects could not be loaded during assembly.
    #[error("project load failed: {}", .cause.message)]
    ProjectLoad {
        /// The structured application refusal.
        cause: kanban_dto::ApiError,
    },
}

/// Serve from the managed application data directory until killed.
/// Another core already serving the managed socket is not a failure:
/// the caller's goal, a serving core, is already met.
pub fn run_managed() -> Result<(), ServiceError> {
    let data_dir = kanban_storage::paths::managed_data_dir()?;
    match serve(&data_dir) {
        Ok(core) => {
            eprintln!("kanban core serving {}", core.socket_path().display());
            // The core has no stop path of its own yet; explicit
            // stop with capability warnings lands in KAN-T63.
            loop {
                std::thread::park();
            }
        }
        Err(ServiceError::Transport(TransportError::SocketInUse { path })) => {
            eprintln!("another kanban core is already serving {}", path.display());
            Ok(())
        }
        Err(failure) => Err(failure),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use serde_json::{Value, json};
    use tempfile::TempDir;

    use std::sync::Arc;

    use kanban_app::{GitObservation, NoopEventSink, assert_registered_matches_exposed_catalogue};

    use super::{
        CloneTargetProbe, LocalCloneTargetProbe, LocalRepositories, ObservationTuning,
        RETAINED_OUTCOMES, ServiceError, assemble_core, prepare_database, serve,
        serve_with_herdr_sessions,
    };
    use crate::herdr::production_socket_root;
    use crate::test_client::{Client, boot};
    use kanban_herdr::fixture::{ScriptedSession, SessionScript};
    use std::thread;
    use std::time::Duration;

    /// A scratch directory standing in for a Git repository the
    /// service's own observation accepts.
    fn scratch_repository(dir: &TempDir, name: &str) -> String {
        let repository = dir.path().join(name);
        std::fs::create_dir_all(repository.join(".git"))
            .expect("the scratch repository is created");
        repository
            .canonicalize()
            .expect("the repository path canonicalises")
            .to_str()
            .expect("the path is UTF-8")
            .to_owned()
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("the metadata reads")
            .permissions()
            .mode()
            & 0o777
    }

    /// KAN-T82-AC1: test boots inject an isolated Herdr socket root
    /// and never default to the Operator's live session directory.
    #[test]
    fn test_boot_never_selects_the_production_herdr_socket_root() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let core = boot(&dir);
        let root = core.herdr_socket_root();
        assert!(
            root.starts_with(dir.path()),
            "tests must inject an isolated socket root under the scratch directory"
        );
        assert_ne!(
            root,
            production_socket_root().as_path(),
            "tests must never default to the production Herdr directory"
        );
        core.shutdown();
    }

    /// KAN-T82-AC2: only the managed production serve path selects
    /// the installed Herdr session directory.
    #[test]
    fn production_serve_selects_the_production_herdr_socket_root() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let core = serve(dir.path()).expect("the production core boots");
        assert_eq!(
            core.herdr_socket_root(),
            production_socket_root().as_path(),
            "the managed production entry point supplies the production root"
        );
        core.shutdown();
    }

    #[test]
    fn registered_catalogue_matches_the_exposed_catalogue() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let database = prepare_database(dir.path()).expect("the production database prepares");
        let (_, core, _, _) = assemble_core(
            dir.path(),
            database,
            Arc::new(NoopEventSink),
            dir.path().join("herdr-sessions"),
            ObservationTuning::PRODUCTION,
            Arc::new(crate::LocalFleetCloneTool::default()),
        )
        .expect("the production core wires");

        assert_registered_matches_exposed_catalogue(&core.registered_operations());
    }

    #[test]
    fn boot_answers_health_over_the_socket() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let core = boot(&dir);

        let mut client = Client::connect(core.socket_path());
        let health = client.query("health.get");

        assert_eq!(
            health,
            json!({ "connected": true, "service_version": env!("CARGO_PKG_VERSION") }),
            "the boot smoke test drives the real socket"
        );

        core.shutdown();
    }

    #[test]
    fn boot_owns_the_documented_files_in_the_data_directory() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let core = boot(&dir);

        let socket_path = core.socket_path();
        assert_eq!(socket_path.file_name(), Some("core.sock".as_ref()));
        assert!(
            dir.path().join("kanban.sqlite").is_file(),
            "the authoritative database lives in the data directory"
        );
        assert_eq!(mode_of(dir.path()), 0o700, "the directory is owner-only");
        assert_eq!(mode_of(socket_path), 0o600, "the socket is owner-only");

        core.shutdown();
    }

    #[test]
    fn reboot_against_an_existing_database_is_idempotent() {
        let dir = TempDir::new().expect("a scratch directory is available");
        boot(&dir).shutdown();

        let core = boot(&dir);
        let mut client = Client::connect(core.socket_path());
        assert_eq!(
            client.query("health.get")["connected"],
            json!(true),
            "a second boot against the same files keeps serving"
        );

        core.shutdown();
    }

    #[test]
    fn a_second_core_refuses_to_take_the_live_socket() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let core = boot(&dir);

        let refusal = serve_with_herdr_sessions(dir.path(), dir.path().join("herdr-sessions"));
        assert!(
            matches!(
                refusal,
                Err(ServiceError::Transport(
                    kanban_transport::TransportError::SocketInUse { .. }
                ))
            ),
            "two cores must never share one socket"
        );

        core.shutdown();
    }

    #[test]
    fn restart_replay_returns_the_recorded_response_without_reapplying() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let request = json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": "restart-1" },
            "name": "Reliability",
        });

        let core = boot(&dir);
        let created =
            Client::connect(core.socket_path()).command("initiative.create", request.clone());
        core.shutdown();

        let rebooted = boot(&dir);
        let mut client = Client::connect(rebooted.socket_path());
        let replayed = client.command("initiative.create", request.clone());

        assert_eq!(
            replayed, created,
            "a retry after a restart replays the original response"
        );
        assert_eq!(
            client.query("initiative.list"),
            json!({
                "initiatives": [
                    { "id": 1, "name": "Reliability", "archived": false, "version": 1 }
                ]
            }),
            "the replay must not have applied the mutation a second time"
        );

        let refused = client.command_error(
            "initiative.create",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "restart-1" },
                "name": "Recovery",
            }),
        );
        assert_eq!(
            refused["code"],
            json!("duplicate_idempotency_key"),
            "the spent key refuses a different request across the restart"
        );

        rebooted.shutdown();
    }

    /// KAN-T135-AC4: a pre-scheme row seeded in the durable store —
    /// aggregate and body, no operation — fails closed after a real
    /// core restart, on the real SQLite file, while the modern row
    /// recorded beside it still replays (AC3) and the ambiguous row
    /// survives every refusal for audit (AC1).
    #[test]
    fn a_legacy_outcome_fails_closed_across_a_durable_restart() {
        use rusqlite::params;

        let dir = TempDir::new().expect("a scratch directory is available");
        let modern_request = json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": "modern-restart-1" },
            "name": "Modern",
        });
        let legacy_request = json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": "legacy-restart-1" },
            "name": "Legacy",
        });

        let core = boot(&dir);
        let mut client = Client::connect(core.socket_path());
        let created = client.command("initiative.create", modern_request.clone());
        core.shutdown();

        // Seed the row exactly as the pre-scheme core recorded it: the
        // aggregate and the body with no operation, and the outcome
        // that used to replay for any operation sharing the shape.
        let conn = rusqlite::Connection::open(dir.path().join("kanban.sqlite"))
            .expect("the database reopens for seeding");
        conn.execute(
            "INSERT INTO idempotency_outcomes (idempotency_key, fingerprint, response)
             VALUES (?1, ?2, ?3)",
            params![
                "legacy-restart-1",
                "initiative:{\"name\":\"Legacy\"}",
                "{\"id\":7,\"name\":\"Legacy\",\"archived\":false,\"version\":1}",
            ],
        )
        .expect("the pre-scheme row inserts");
        drop(conn);

        let rebooted = boot(&dir);
        let mut client = Client::connect(rebooted.socket_path());

        // The retried create cannot prove it is the operation that
        // spent the key, so the durable row must not replay for it;
        // the guard refuses the reuse and demands a fresh key.
        let refused = client.command_error("initiative.create", legacy_request.clone());
        assert_eq!(
            refused["code"],
            json!("ambiguous_idempotency_key"),
            "a durable legacy row fails closed after a restart: {refused}"
        );
        assert!(
            refused["message"]
                .as_str()
                .expect("the refusal carries a message")
                .contains("fresh idempotency key"),
            "the refusal should require a fresh key: {refused}"
        );

        // The refusal applied nothing: the request lands under a fresh
        // key, and the modern row replays unchanged across the same
        // restart, so the closed door is the legacy row's alone.
        let landed = client.command(
            "initiative.create",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "legacy-restart-2" },
                "name": "Legacy",
            }),
        );
        assert_eq!(landed["name"], json!("Legacy"));
        let replayed = client.command("initiative.create", modern_request.clone());
        assert_eq!(
            replayed, created,
            "an operation-aware retry still replays across the restart"
        );
        assert_eq!(
            client.query("initiative.list"),
            json!({
                "initiatives": [
                    { "id": 1, "name": "Modern", "archived": false, "version": 1 },
                    { "id": 2, "name": "Legacy", "archived": false, "version": 1 },
                ]
            }),
            "exactly one create per request landed"
        );
        rebooted.shutdown();

        // The ambiguous row survives every refusal exactly as recorded:
        // preserved for audit, never rewritten and never replayed.
        let conn = rusqlite::Connection::open(dir.path().join("kanban.sqlite"))
            .expect("the database reopens for audit");
        let (fingerprint, response): (String, String) = conn
            .query_row(
                "SELECT fingerprint, response FROM idempotency_outcomes
                 WHERE idempotency_key = ?1",
                ["legacy-restart-1"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the seeded row is still recorded");
        assert_eq!(
            fingerprint, "initiative:{\"name\":\"Legacy\"}",
            "the ambiguous row keeps its operation-blind fingerprint for audit"
        );
        assert_eq!(
            response, "{\"id\":7,\"name\":\"Legacy\",\"archived\":false,\"version\":1}",
            "the ambiguous row keeps its recorded outcome for audit"
        );
    }

    #[test]
    fn restart_replay_survives_a_burst_above_the_count_bound() {
        use rusqlite::params;

        let dir = TempDir::new().expect("a scratch directory is available");
        let request = json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": "burst-replay-1" },
            "name": "Reliability",
        });

        let core = boot(&dir);
        let created =
            Client::connect(core.socket_path()).command("initiative.create", request.clone());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let completed = std::fs::read_to_string(dir.path().join(".backup-scheduler.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|state| state["last_success_unix_secs"].as_u64())
                .is_some();
            if completed {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the initial scheduled backup finishes before burst seeding"
            );
            thread::sleep(Duration::from_millis(10));
        }
        core.shutdown();

        let conn = rusqlite::Connection::open(dir.path().join("kanban.sqlite"))
            .expect("the database reopens for seeding");
        for index in 0..RETAINED_OUTCOMES.get() {
            conn.execute(
                "INSERT INTO idempotency_outcomes (idempotency_key, fingerprint, response, recorded_at)
                 VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![
                    format!("burst-{index}"),
                    format!("fingerprint:{index}"),
                    "{}",
                ],
            )
            .expect("the burst row inserts");
        }

        let rebooted = boot(&dir);
        let mut client = Client::connect(rebooted.socket_path());
        client.command(
            "initiative.create",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "burst-trigger" },
                "name": "Trigger",
            }),
        );
        let replayed = client.command("initiative.create", request.clone());

        assert_eq!(
            replayed, created,
            "a retry after a burst above the count bound replays when the outcome is still inside the minimum age"
        );
        assert_eq!(
            client.query("initiative.list"),
            json!({
                "initiatives": [
                    { "id": 1, "name": "Reliability", "archived": false, "version": 1 },
                    { "id": 2, "name": "Trigger", "archived": false, "version": 1 },
                ]
            }),
            "the replay must not have applied the mutation a second time"
        );

        rebooted.shutdown();
    }

    /// The desktop shell's request deadline (`CoreLink::REQUEST_TIMEOUT`,
    /// unchanged by this slice): a live command that cannot answer
    /// inside it is reported as an unresponsive core, so a backup
    /// copying under the live connection's mutex is a defect, not a
    /// slow case. `Client`'s own read timeout mirrors the same five
    /// seconds.
    const REQUEST_DEADLINE: Duration = Duration::from_secs(5);

    /// Enough pages that a copying snapshot's throttled steps alone
    /// hold the copy for far longer than the request deadline.
    const PAGES_PAST_THE_REQUEST_DEADLINE: i64 = 1300;

    #[test]
    fn live_commands_answer_inside_the_request_deadline_while_a_backup_copies() {
        let dir = TempDir::new().expect("a scratch directory is available");
        grow_the_database_past_the_request_deadline(&dir);

        let core = boot(&dir);
        wait_for_the_scheduled_backup_to_start_copying(&dir);

        let mut client = Client::connect(core.socket_path());
        let started = std::time::Instant::now();
        client.command(
            "initiative.create",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "overlap-1" },
                "name": "Responsive",
            }),
        );
        let listed = client.query("initiative.list");
        assert_eq!(
            listed["initiatives"][0]["name"],
            json!("Responsive"),
            "the overlapped command must land"
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < REQUEST_DEADLINE,
            "a live command must answer inside the unchanged {REQUEST_DEADLINE:?} request deadline while a backup copies, took {elapsed:?}"
        );

        wait_for_the_scheduled_backup_to_settle(&dir);
        core.shutdown();
    }

    /// Migrates the scratch database and grows it past
    /// [`PAGES_PAST_THE_REQUEST_DEADLINE`] pages, so a backup of it
    /// necessarily spans hundreds of throttled snapshot steps.
    fn grow_the_database_past_the_request_deadline(dir: &TempDir) {
        use rusqlite::params;

        let mut database = kanban_storage::Database::open(
            &dir.path().join(kanban_storage::paths::database_file_name()),
        )
        .expect("the scratch database opens");
        database
            .migrate(&kanban_storage::AllowAllMigrations)
            .expect("the migrations apply");
        drop(database);

        let conn = rusqlite::Connection::open(
            dir.path().join(kanban_storage::paths::database_file_name()),
        )
        .expect("the database reopens for growth");
        conn.execute_batch("CREATE TABLE backup_overlap_filler (payload BLOB)")
            .expect("the filler table creates");
        let blob = vec![0_u8; 8192];
        loop {
            let pages: i64 = conn
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .expect("the page count reads");
            assert!(
                pages < 100_000,
                "the filler must stay bounded while growing toward the target"
            );
            if pages >= PAGES_PAST_THE_REQUEST_DEADLINE {
                break;
            }
            for _ in 0..25 {
                conn.execute(
                    "INSERT INTO backup_overlap_filler (payload) VALUES (?1)",
                    params![&blob],
                )
                .expect("the filler row inserts");
            }
        }
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .expect("the grown database checkpoints");
        drop(conn);
    }

    /// Waits until the scheduled startup backup is actively copying
    /// pages, so the overlapped command lands mid-copy and not before
    /// or after it.
    fn wait_for_the_scheduled_backup_to_start_copying(dir: &TempDir) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let copying = std::fs::read_dir(dir.path().join("backups"))
                .map(|entries| {
                    entries.filter_map(|entry| entry.ok()).any(|entry| {
                        entry.path().is_dir()
                            && std::fs::metadata(
                                entry
                                    .path()
                                    .join(kanban_storage::paths::database_file_name()),
                            )
                            .map(|metadata| metadata.len() > 0)
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            if copying {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the scheduled startup backup must begin copying"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Waits for the startup backup to succeed, mirroring the burst
    /// fixture's settle, so shutdown never races a copying backup.
    fn wait_for_the_scheduled_backup_to_settle(dir: &TempDir) {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let settled = std::fs::read_to_string(dir.path().join(".backup-scheduler.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|state| state["last_success_unix_secs"].as_u64())
                .is_some();
            if settled {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the scheduled backup must settle before shutdown"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_refused_command_leaves_the_core_writable() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let core = boot(&dir);
        let mut client = Client::connect(core.socket_path());

        let refused = client.command_error(
            "initiative.create",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "blank" },
                "name": "   ",
            }),
        );
        assert_eq!(refused["code"], json!("invalid_request"));

        let created = client.command(
            "initiative.create",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "after-refusal" },
                "name": "Reliability",
            }),
        );

        assert_eq!(
            created,
            json!({ "id": 1, "name": "Reliability", "archived": false, "version": 1 }),
            "the discarded span released the write it never landed"
        );

        core.shutdown();
    }

    #[test]
    fn the_initiative_lifecycle_serves_over_the_socket() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let core = boot(&dir);
        let mut client = Client::connect(core.socket_path());

        let created = client.command(
            "initiative.create",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "boot-create" },
                "name": "Reliability",
            }),
        );
        assert_eq!(created["name"], json!("Reliability"));
        assert_eq!(created["version"], json!(1));

        let renamed = client.command(
            "initiative.rename",
            json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "boot-rename" },
                "initiative_id": 1,
                "name": "Reliability and Recovery",
            }),
        );
        assert_eq!(renamed["name"], json!("Reliability and Recovery"));
        assert_eq!(renamed["version"], json!(2));

        let archived = client.command(
            "initiative.archive",
            json!({
                "mutation": { "optimistic_version": 2, "idempotency_key": "boot-archive" },
                "initiative_id": 1,
            }),
        );
        assert_eq!(archived["archived"], json!(true));

        let listed = client.query("initiative.list");
        assert_eq!(
            listed,
            json!({
                "initiatives": [
                    {
                        "id": 1,
                        "name": "Reliability and Recovery",
                        "archived": true,
                        "version": 3,
                    }
                ]
            }),
            "archiving preserves every recorded fact over the wire"
        );

        // The recorded facts are durable: a fresh core over the same
        // database still lists the archived Initiative.
        core.shutdown();
        let rebooted = boot(&dir);
        let mut second = Client::connect(rebooted.socket_path());
        assert_eq!(
            second.query("initiative.list"),
            listed,
            "every recorded fact survives a restart"
        );

        rebooted.shutdown();
    }

    #[test]
    fn the_project_registration_lifecycle_serves_over_the_socket() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = scratch_repository(&dir, "kanban");
        let core = boot(&dir);
        let mut client = Client::connect(core.socket_path());

        let registered = client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-core" },
                "code": "CORE",
                "name": "Control plane",
                "repository": repository,
                "seed_workspace": "/workspaces/kanban.seed",
                "default_branch": "main",
                "herdr_session": "kanban-main",
                "herdr_workspace": "kanban.seed",
            }),
        );
        assert_eq!(registered["code"], json!("CORE"));
        assert_eq!(registered["version"], json!(1));
        assert_eq!(
            registered["counters"],
            json!({ "plan": 0, "spec": 0, "ticket": 0 })
        );

        // Session names are no longer exclusive: a second Project may
        // select the same one, and a Project may name none at all.
        let shared_session = client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-wave" },
                "code": "WAVE",
                "name": "Wave pool",
                "repository": repository,
                "seed_workspace": "/workspaces/wave.seed",
                "default_branch": "main",
                "herdr_session": "kanban-main",
                "herdr_workspace": "wave.seed",
            }),
        );
        assert_eq!(shared_session["code"], json!("WAVE"));
        let sessionless = client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-bare" },
                "code": "BARE",
                "name": "Default session",
                "repository": repository,
                "seed_workspace": "/workspaces/bare.seed",
                "default_branch": "main",
                "herdr_workspace": "bare.seed",
            }),
        );
        assert_eq!(sessionless["herdr_session"], json!(null));
        assert_eq!(sessionless["herdr_workspace"], json!("bare.seed"));
        // A target that is not a Git repository never registers.
        let plain = dir.path().join("not-a-repository");
        std::fs::create_dir_all(&plain).expect("the plain directory is created");
        let plain = plain
            .canonicalize()
            .expect("the plain path canonicalises")
            .to_str()
            .expect("the path is UTF-8")
            .to_owned();
        let non_git = client.command_error(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-plain" },
                "code": "PLAIN",
                "name": "Plain directory",
                "repository": plain,
                "seed_workspace": "/workspaces/plain.seed",
                "default_branch": "main",
                "herdr_session": "plain-main",
                "herdr_workspace": "plain.seed",
            }),
        );
        assert_eq!(non_git["code"], json!("invalid_request"));

        // Archiving is terminal and preserves the code, the counters,
        // and the record itself.
        let archived = client.command(
            "project.archive",
            json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "archive-core" },
                "project_id": 1,
            }),
        );
        assert_eq!(archived["archived"], json!(true));
        assert_eq!(archived["code"], json!("CORE"));
        let listed = client.query("project.list");
        assert_eq!(
            listed,
            json!({
                "projects": [
                    {
                        "id": 1,
                        "code": "CORE",
                        "name": "Control plane",
                        "repository": repository,
                        "seed_workspace": "/workspaces/kanban.seed",
                        "default_branch": "main",
                        "herdr_session": "kanban-main",
                        "herdr_workspace": "kanban.seed",
                        "initiative_id": null,
                        "archived": true,
                        "counters": { "plan": 0, "spec": 0, "ticket": 0 },
                        "version": 2,
                    },
                    {
                        "id": 2,
                        "code": "WAVE",
                        "name": "Wave pool",
                        "repository": repository,
                        "seed_workspace": "/workspaces/wave.seed",
                        "default_branch": "main",
                        "herdr_session": "kanban-main",
                        "herdr_workspace": "wave.seed",
                        "initiative_id": null,
                        "archived": false,
                        "counters": { "plan": 0, "spec": 0, "ticket": 0 },
                        "version": 1,
                    },
                    {
                        "id": 3,
                        "code": "BARE",
                        "name": "Default session",
                        "repository": repository,
                        "seed_workspace": "/workspaces/bare.seed",
                        "default_branch": "main",
                        "herdr_session": null,
                        "herdr_workspace": "bare.seed",
                        "initiative_id": null,
                        "archived": false,
                        "counters": { "plan": 0, "spec": 0, "ticket": 0 },
                        "version": 1,
                    }
                ]
            }),
            "archiving preserves every recorded fact over the wire, and the shared session stays"
        );

        // The recorded facts are durable: a fresh core over the same
        // database still lists the archived Project.
        core.shutdown();
        let rebooted = boot(&dir);
        let mut second = Client::connect(rebooted.socket_path());
        assert_eq!(
            second.query("project.list"),
            listed,
            "every recorded fact survives a restart"
        );

        rebooted.shutdown();
    }

    #[test]
    fn registering_a_project_starts_herdr_observation_without_a_restart() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let repository = scratch_repository(&dir, "wave");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "wave-main",
            "/workspaces/wave.seed",
            SessionScript::default(),
        );
        let core = serve_with_herdr_sessions(dir.path(), socket_root)
            .expect("the core boots for the test");
        let mut client = Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-wave" },
                "code": "WAVE",
                "name": "Wave pool",
                "repository": repository,
                "seed_workspace": "/workspaces/wave.seed",
                "default_branch": "main",
                "herdr_session": "wave-main",
                "herdr_workspace": "wave.seed",
            }),
        );
        // The snapshot lands when the observer settles, not at a
        // fixed tick: poll for it so a loaded test run is still one
        // exact assertion, not a race.
        let events = {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                let answer = client.query_with(
                    "timeline.query",
                    json!({
                        "scope": { "project": 1 },
                        "kinds": ["telemetry"],
                    }),
                );
                let events = answer["events"]
                    .as_array()
                    .expect("telemetry is queryable on the live registration path")
                    .clone();
                if !events.is_empty() || std::time::Instant::now() > deadline {
                    break events;
                }
                thread::sleep(Duration::from_millis(10));
            }
        };
        assert_eq!(
            events.len(),
            1,
            "the startup snapshot lands without a restart, and it is the only telemetry row"
        );
        assert_eq!(events[0]["detail"]["event"], json!("snapshot"));
        assert_eq!(events[0]["detail"]["reason"], json!("startup"));
        core.shutdown();
    }

    #[test]
    fn local_repositories_observes_the_git_entry() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let observation = LocalRepositories;

        let clone = dir.path().join("clone");
        std::fs::create_dir_all(clone.join(".git")).expect("the clone is created");
        assert!(observation.is_repository(clone.to_str().expect("the path is UTF-8")));

        let worktree = dir.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("the worktree is created");
        std::fs::write(worktree.join(".git"), "gitdir: ../clone/.git/worktrees/w")
            .expect("the worktree pointer is written");
        assert!(observation.is_repository(worktree.to_str().expect("the path is UTF-8")));

        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).expect("the plain directory is created");
        assert!(
            !observation.is_repository(plain.to_str().expect("the path is UTF-8")),
            "a directory without .git is not a repository"
        );
        assert!(
            !observation.is_repository(
                dir.path()
                    .join("missing")
                    .to_str()
                    .expect("the path is UTF-8")
            ),
            "a missing path is not a repository"
        );
    }

    /// KAN-T118: occupancy is existence, whatever occupies the target —
    /// a full clone, a plain directory, or a lone file — because any of
    /// them means the creation Kanban would record is not the one that
    /// lands.
    #[test]
    fn local_clone_target_probe_reports_only_what_exists() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let probe = LocalCloneTargetProbe;

        let occupied = dir.path().join("kanban.fleet-t34");
        std::fs::create_dir_all(&occupied).expect("the clone directory is created");
        assert!(probe.exists(occupied.to_str().expect("the path is UTF-8")));

        let file = dir.path().join("stray-file");
        std::fs::write(&file, "stray\n").expect("the stray file is written");
        assert!(
            probe.exists(file.to_str().expect("the path is UTF-8")),
            "a file occupying the target is occupancy too"
        );

        assert!(
            !probe.exists(
                dir.path()
                    .join("missing")
                    .to_str()
                    .expect("the path is UTF-8")
            ),
            "a free target is free"
        );
    }

    /// KAN-T8-AC4: a Project's registration and archive are its own
    /// durable history, queryable on the Project's timeline. Every row
    /// must decode through the typed timeline path as it was written,
    /// with no migration repairing it into the vocabulary.
    #[test]
    fn project_history_is_queryable_on_the_projects_own_timeline() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = scratch_repository(&dir, "kanban");
        let core = boot(&dir);
        let mut client = Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-core" },
                "code": "CORE",
                "name": "Control plane",
                "repository": repository,
                "seed_workspace": "/workspaces/kanban.seed",
                "default_branch": "main",
                "herdr_session": "kanban-main",
                "herdr_workspace": "kanban.seed",
            }),
        );
        client.command(
            "project.archive",
            json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "archive-core" },
                "project_id": 1,
            }),
        );

        let answer = client.query_with("timeline.query", json!({ "scope": { "project": 1 } }));

        let events = answer["events"]
            .as_array()
            .expect("the Project timeline query answers with events");
        let recorded: Vec<(Value, Value, Value, Value)> = events
            .iter()
            .map(|event| {
                (
                    event["scope"].clone(),
                    event["kind"].clone(),
                    event["entity"].clone(),
                    event["detail"]["action"].clone(),
                )
            })
            .collect();
        assert_eq!(
            recorded,
            vec![
                (
                    json!({ "project": 1 }),
                    json!("transition"),
                    json!({ "kind": "project", "id": "1" }),
                    json!("registered"),
                ),
                (
                    json!({ "project": 1 }),
                    json!("transition"),
                    json!({ "kind": "project", "id": "1" }),
                    json!("archived"),
                ),
            ],
            "both Project history rows decode as typed transitions"
        );
        assert_eq!(events[0]["detail"]["code"], json!("CORE"));
        assert_eq!(events[0]["detail"]["herdr_session"], json!("kanban-main"));
        let narrowed = client.query_with(
            "timeline.query",
            json!({
                "scope": { "project": 1 },
                "entity": { "kind": "project", "id": "1" },
                "kinds": ["transition"],
            }),
        );
        assert_eq!(
            narrowed["events"].as_array().map(Vec::len),
            Some(2),
            "the entity and kind filters reach the same rows"
        );

        // The rows are durable as written: a fresh core over the same
        // file applies no migration and still decodes every row.
        core.shutdown();
        let stored_kinds: Vec<String> = {
            let conn = rusqlite::Connection::open(dir.path().join("kanban.sqlite"))
                .expect("a second connection opens");
            let mut statement = conn
                .prepare(
                    "SELECT kind FROM timeline_events
                     WHERE scope = 'project' AND project_id = '1' ORDER BY id",
                )
                .expect("the timeline is readable");
            statement
                .query_map([], |row| row.get(0))
                .expect("the query runs")
                .collect::<Result<Vec<_>, _>>()
                .expect("the kinds decode")
        };
        assert_eq!(
            stored_kinds,
            vec!["transition".to_owned(), "transition".to_owned()],
            "the stored kinds are already inside the closed vocabulary"
        );
        let rebooted = boot(&dir);
        let mut second = Client::connect(rebooted.socket_path());
        assert_eq!(
            second.query_with("timeline.query", json!({ "scope": { "project": 1 } })),
            answer,
            "a restart serves the same typed history"
        );

        rebooted.shutdown();
    }

    /// KAN-T10-AC3: every evidence command leaves the per-Project
    /// timeline readable. KAN-T79: the evidence now resolves the
    /// registered Project's numeric identity, so its rows join the
    /// Project's own history. KAN-T91: evidence listing is a safe
    /// read and does not append timeline rows.
    #[test]
    fn evidence_commands_keep_the_project_timeline_readable() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = scratch_repository(&dir, "kanban");
        let core = boot(&dir);
        let mut client = Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "evidence-project" },
                "code": "CORE",
                "name": "Control plane",
                "repository": repository,
                "seed_workspace": "/workspaces/kanban.seed",
                "default_branch": "main",
                "herdr_session": "kanban-main",
                "herdr_workspace": "kanban.seed",
            }),
        );

        let attached = client.command(
            "evidence.attach",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "evidence-attach" },
                "project_id": 1,
                "entity_kind": "ticket",
                "entity_id": "kan-t10",
                "evidence_kind": "managed_file",
                "content_base64": "cHJvb2YgYnl0ZXM=",
            }),
        );
        assert_eq!(attached["id"], json!(1));
        client.query_with(
            "evidence.list",
            json!({
                "project_id": 1,
                "entity_kind": "ticket",
                "entity_id": "kan-t10",
            }),
        );
        client.query_with(
            "evidence.list",
            json!({
                "project_id": 1,
            }),
        );

        let timeline = client.query_with("timeline.query", json!({ "scope": { "project": 1 } }));
        let events = timeline["events"]
            .as_array()
            .expect("the timeline query answers with events");
        let entities: Vec<Value> = events
            .iter()
            .map(|event| event.get("entity").cloned().unwrap_or(Value::Null))
            .collect();
        assert_eq!(
            entities,
            vec![
                json!({ "kind": "project", "id": "1" }),
                json!({ "kind": "ticket", "id": "kan-t10" }),
            ],
            "registration and attach land on the timeline; listing stays a safe read"
        );

        core.shutdown();
    }

    /// KAN-T79-AC2: comments, rulings, deferrals, evidence, Project
    /// transitions, and Herdr telemetry all derive one canonical
    /// timeline scope, so one query answers a Project's complete
    /// history. KAN-T79-AC1: every command on the way there resolved
    /// the numeric Project identity through the store.
    #[test]
    fn project_timeline_holds_every_project_scoped_row_under_one_scope() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let repository = scratch_repository(&dir, "kanban");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default(),
        );
        let core = serve_with_herdr_sessions(dir.path(), socket_root)
            .expect("the core boots for the test");
        let mut client = Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-core" },
                "code": "CORE",
                "name": "Control plane",
                "repository": repository,
                "seed_workspace": "/workspaces/kanban.seed",
                "default_branch": "main",
                "herdr_session": "kanban-main",
                "herdr_workspace": "kanban.seed",
            }),
        );
        client.command(
            "comment.create",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "comment-1" },
                "project_id": 1,
                "target": { "kind": "ticket", "id": "kan-t10" },
                "text": "One timeline",
            }),
        );
        client.command(
            "ruling.record",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "ruling-1" },
                "project_id": 1,
                "summary": "Allow landing",
            }),
        );
        client.command(
            "deferral.record",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "deferral-1" },
                "project_id": 1,
                "finding_id": "finding-1",
                "reason": "Cosmetic only",
            }),
        );
        client.command(
            "evidence.attach",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "evidence-1" },
                "project_id": 1,
                "entity_kind": "ticket",
                "entity_id": "kan-t10",
                "evidence_kind": "managed_file",
                "content_base64": "cHJvb2YgYnl0ZXM=",
            }),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let timeline = client.query_with(
                "timeline.query",
                json!({ "scope": { "project": 1 }, "kinds": ["telemetry"] }),
            );
            if !timeline["events"]
                .as_array()
                .expect("events are an array")
                .is_empty()
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the live subscription settles before archive"
            );
            thread::sleep(Duration::from_millis(10));
        }
        client.command(
            "project.archive",
            json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "archive-core" },
                "project_id": 1,
            }),
        );
        thread::sleep(Duration::from_millis(200));

        let answer = client.query_with("timeline.query", json!({ "scope": { "project": 1 } }));
        let events = answer["events"]
            .as_array()
            .expect("the timeline query answers with events");
        // Herdr telemetry lands on its own observation thread, so the
        // row order around it races; the contract is the company every
        // row keeps, not its exact position.
        let mut recorded: Vec<Value> = events
            .iter()
            .map(|event| json!({ "scope": event["scope"], "kind": event["kind"] }))
            .collect();
        recorded.sort_by_key(|row| row["kind"].as_str().expect("the kind is text").to_owned());
        assert_eq!(
            recorded,
            vec![
                json!({ "kind": "comment", "scope": { "project": 1 } }),
                json!({ "kind": "deferral", "scope": { "project": 1 } }),
                json!({ "kind": "evidence", "scope": { "project": 1 } }),
                json!({ "kind": "ruling", "scope": { "project": 1 } }),
                json!({ "kind": "telemetry", "scope": { "project": 1 } }),
                json!({ "kind": "transition", "scope": { "project": 1 } }),
                json!({ "kind": "transition", "scope": { "project": 1 } }),
            ],
            "every project-scoped writer lands under the one canonical scope"
        );

        core.shutdown();
    }

    /// KAN-T13-AC2, KAN-T13-AC3: a Plan composes, freezes at
    /// activation, replans with an auditable replacement version, and
    /// every frozen version stays queryable over the socket and across
    /// a restart.
    #[test]
    fn the_plan_lifecycle_serves_over_the_socket() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = scratch_repository(&dir, "kanban");
        let core = boot(&dir);
        let mut client = Client::connect(core.socket_path());

        let registered = client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-core" },
                "code": "CORE",
                "name": "Control plane",
                "repository": repository,
                "seed_workspace": "/workspaces/kanban.seed",
                "default_branch": "main",
                "herdr_session": "kanban-main",
                "herdr_workspace": "kanban.seed",
            }),
        );
        assert_eq!(registered["version"], json!(1));
        // Author four Specs through the served core: Specs 1, 2, and 3
        // join the Plan below, while Spec 4 stays minted but never a
        // member, so an edge reaching it leaves the single Plan.
        for (name, number) in [
            ("Registration", 1),
            ("Timeline", 2),
            ("Review", 3),
            ("Landing", 4),
        ] {
            let authored = client.command(
                "spec.create",
                json!({
                    "mutation": {
                        "optimistic_version": 0,
                        "idempotency_key": format!("spec-create-{number}"),
                    },
                    "project_id": 1,
                    "content": {
                        "name": name,
                        "short_description": "One behaviour area",
                        "problem_statement": "",
                        "solution": "",
                        "user_stories": "",
                        "implementation_decisions": "",
                        "testing_decisions": "",
                        "out_of_scope": "",
                        "further_notes": "",
                    },
                }),
            );
            assert_eq!(authored["number"], json!(number));
            assert_eq!(authored["execution"], json!("unplanned"));
        }

        let created = client.command(
            "plan.create",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "plan-create" },
                "project_id": 1,
            }),
        );
        assert_eq!(created["number"], json!(1));
        assert_eq!(created["state"], json!("draft"));

        let mut version = created["version"]
            .as_u64()
            .expect("the version is a number");
        for spec in [1, 3, 2] {
            let response = client.command(
                "plan.spec.add",
                json!({
                    "mutation": {
                        "optimistic_version": version,
                        "idempotency_key": format!("plan-add-{spec}"),
                    },
                    "plan_id": 1,
                    "spec_number": spec,
                }),
            );
            version = response["version"]
                .as_u64()
                .expect("the version is a number");
        }
        for (from, to) in [(1, 2), (3, 2)] {
            let response = client.command(
                "plan.edge.add",
                json!({
                    "mutation": {
                        "optimistic_version": version,
                        "idempotency_key": format!("plan-edge-{from}-{to}"),
                    },
                    "plan_id": 1,
                    "from_spec": from,
                    "to_spec": to,
                }),
            );
            version = response["version"]
                .as_u64()
                .expect("the version is a number");
        }

        // An edge leaving the single Plan is refused over the wire.
        let refused = client.command_error(
            "plan.edge.add",
            json!({
                "mutation": {
                    "optimistic_version": version,
                    "idempotency_key": "plan-edge-outside",
                },
                "plan_id": 1,
                "from_spec": 2,
                "to_spec": 1,
                "surprise": true,
            }),
        );
        assert_eq!(refused["code"], json!("unknown_field"));
        let outside = client.command_error(
            "plan.edge.add",
            json!({
                "mutation": {
                    "optimistic_version": version,
                    "idempotency_key": "plan-edge-outside-2",
                },
                "plan_id": 1,
                "from_spec": 1,
                "to_spec": 4,
            }),
        );
        assert_eq!(outside["code"], json!("invalid_request"));
        assert!(
            outside["message"]
                .as_str()
                .expect("the message is text")
                .contains("within one Plan"),
            "the refusal names the rule: {outside}"
        );

        let activated = client.command(
            "plan.activate",
            json!({
                "mutation": { "optimistic_version": version, "idempotency_key": "plan-activate" },
                "plan_id": 1,
            }),
        );
        assert_eq!(activated["state"], json!("active"));

        let replanned = client.command(
            "plan.replan",
            json!({
                "mutation": { "optimistic_version": activated["version"], "idempotency_key": "plan-replan" },
                "plan_id": 1,
            }),
        );
        assert_eq!(replanned["state"], json!("draft"));
        let mut version = replanned["version"]
            .as_u64()
            .expect("the version is a number");
        let moved = client.command(
            "plan.spec.move",
            json!({
                "mutation": { "optimistic_version": version, "idempotency_key": "plan-move" },
                "plan_id": 1,
                "spec_number": 2,
                "position": 0,
            }),
        );
        version = moved["version"].as_u64().expect("the version is a number");
        let reactivated = client.command(
            "plan.activate",
            json!({
                "mutation": { "optimistic_version": version, "idempotency_key": "plan-reactivate" },
                "plan_id": 1,
            }),
        );
        assert_eq!(reactivated["state"], json!("active"));

        let detail = client.query_with("plan.get", json!({ "plan_id": 1 }));
        assert_eq!(
            detail["versions"]
                .as_array()
                .expect("the versions are a list")
                .iter()
                .map(|entry| entry["number"].clone())
                .collect::<Vec<_>>(),
            vec![json!(1), json!(2)],
            "the replacement is minted while the first version stays queryable"
        );
        assert_eq!(detail["versions"][0]["spec_numbers"], json!([1, 3, 2]));
        assert_eq!(detail["versions"][1]["spec_numbers"], json!([2, 1, 3]));
        assert_eq!(detail["plan"]["state"], json!("active"));

        // The frozen history is durable: a fresh core over the same
        // database still serves both versions.
        core.shutdown();
        let rebooted = boot(&dir);
        let mut second = Client::connect(rebooted.socket_path());
        assert_eq!(
            second.query_with("plan.get", json!({ "plan_id": 1 })),
            detail,
            "every frozen version survives a restart"
        );

        rebooted.shutdown();
    }

    mod repository {
        use std::path::PathBuf;
        use std::sync::Mutex;

        use serde_json::json;
        use tempfile::TempDir;

        use crate::test_client::{Client, boot};

        static CWD_LOCK: Mutex<()> = Mutex::new(());

        fn scratch_repository(dir: &TempDir, name: &str) -> String {
            let repository = dir.path().join(name);
            std::fs::create_dir_all(repository.join(".git"))
                .expect("the scratch repository is created");
            repository
                .canonicalize()
                .expect("the repository path canonicalises")
                .to_str()
                .expect("the path is UTF-8")
                .to_owned()
        }

        #[test]
        fn repository_anchor_is_canonicalised_before_validation() {
            let _guard = CWD_LOCK.lock().expect("the cwd lock is sound");
            let dir = TempDir::new().expect("a scratch directory is available");
            let absolute = scratch_repository(&dir, "kanban");
            let nested = dir.path().join("nested");
            std::fs::create_dir_all(&nested).expect("the nested directory is created");
            std::env::set_current_dir(&nested).expect("the cwd moves beside the repository");

            let core = boot(&dir);
            let mut client = Client::connect(core.socket_path());
            let registered = client.command(
                "project.register",
                json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "register-relative" },
                    "code": "CORE",
                    "name": "Control plane",
                    "repository": "../kanban",
                    "seed_workspace": "/workspaces/kanban.seed",
                    "default_branch": "main",
                    "herdr_session": "kanban-main",
                    "herdr_workspace": "kanban.seed",
                }),
            );
            assert_eq!(
                registered["repository"],
                json!(absolute),
                "a relative anchor is stored as its absolute path"
            );
            assert!(
                PathBuf::from(registered["repository"].as_str().expect("the path is text"))
                    .is_absolute(),
                "the stored anchor must not depend on the registration cwd"
            );

            core.shutdown();
        }

        #[test]
        fn repository_anchor_persists_across_cwd_changes() {
            let _guard = CWD_LOCK.lock().expect("the cwd lock is sound");
            let dir = TempDir::new().expect("a scratch directory is available");
            let absolute = scratch_repository(&dir, "kanban");
            let nested = dir.path().join("nested");
            std::fs::create_dir_all(&nested).expect("the nested directory is created");
            std::env::set_current_dir(&nested).expect("the cwd moves beside the repository");

            let core = boot(&dir);
            let mut client = Client::connect(core.socket_path());
            client.command(
                "project.register",
                json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "register-relative" },
                    "code": "CORE",
                    "name": "Control plane",
                    "repository": "../kanban",
                    "seed_workspace": "/workspaces/kanban.seed",
                    "default_branch": "main",
                    "herdr_session": "kanban-main",
                    "herdr_workspace": "kanban.seed",
                }),
            );
            core.shutdown();

            let elsewhere = dir.path().join("elsewhere");
            std::fs::create_dir_all(&elsewhere).expect("another cwd is created");
            std::env::set_current_dir(&elsewhere).expect("the cwd moves away from the repository");

            let rebooted = boot(&dir);
            let mut second = Client::connect(rebooted.socket_path());
            let listed = second.query("project.list");
            assert_eq!(
                listed["projects"][0]["repository"],
                json!(absolute),
                "the stored anchor stays stable after the cwd changes"
            );

            rebooted.shutdown();
        }

        #[test]
        fn missing_repository_anchor_is_refused() {
            let _guard = CWD_LOCK.lock().expect("the cwd lock is sound");
            let dir = TempDir::new().expect("a scratch directory is available");
            std::env::set_current_dir(dir.path()).expect("the cwd moves into the scratch tree");

            let core = boot(&dir);
            let mut client = Client::connect(core.socket_path());
            let refused = client.command_error(
                "project.register",
                json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "register-missing" },
                    "code": "CORE",
                    "name": "Control plane",
                    "repository": "./missing",
                    "seed_workspace": "/workspaces/kanban.seed",
                    "default_branch": "main",
                    "herdr_session": "kanban-main",
                    "herdr_workspace": "kanban.seed",
                }),
            );
            assert_eq!(refused["code"], json!("invalid_request"));
            assert_eq!(
                refused["message"],
                json!("the target repository at `./missing` does not exist")
            );

            core.shutdown();
        }
    }

    mod project_load {
        use kanban_app::NoopEventSink;
        use kanban_dto::ErrorCode;
        use std::sync::Arc;

        use super::{
            ObservationTuning, ServiceError, assemble_core, corrupt_project_fixture,
            prepare_database, serve,
        };
        use crate::herdr::production_socket_root;
        use tempfile::TempDir;

        #[test]
        fn project_load_preserves_the_structured_application_cause() {
            let dir = TempDir::new().expect("a scratch directory is available");
            let database = corrupt_project_fixture(&dir);

            let failure = match assemble_core(
                dir.path(),
                database,
                Arc::new(NoopEventSink),
                production_socket_root(),
                ObservationTuning::PRODUCTION,
                Arc::new(crate::LocalFleetCloneTool::default()),
            ) {
                Err(error) => error,
                Ok(_) => panic!("corrupt data refuses assembly"),
            };

            let cause = match failure {
                ServiceError::ProjectLoad { cause } => cause,
                other => panic!("expected ProjectLoad, got {other}"),
            };
            assert_eq!(cause.code, ErrorCode::Internal);
            assert!(
                cause
                    .message
                    .contains("stored Project row failed validation"),
                "the structured cause names the corruption: {cause:?}"
            );
        }

        #[test]
        fn project_load_surfaces_through_serve() {
            let dir = TempDir::new().expect("a scratch directory is available");
            corrupt_project_fixture(&dir);

            let failure = match serve(dir.path()) {
                Err(error) => error,
                Ok(_) => panic!("corrupt data refuses serve"),
            };

            let cause = match failure {
                ServiceError::ProjectLoad { cause } => cause,
                other => panic!("expected ProjectLoad, got {other}"),
            };
            assert_eq!(cause.code, ErrorCode::Internal);
            assert!(
                cause
                    .message
                    .contains("stored Project row failed validation"),
                "serve preserves the application cause: {cause:?}"
            );
        }

        #[test]
        fn project_load_lists_the_storage_cause_before_socket_bind() {
            let dir = TempDir::new().expect("a scratch directory is available");
            corrupt_project_fixture(&dir);

            let failure = match assemble_core(
                dir.path(),
                prepare_database(dir.path()).expect("the database prepares"),
                Arc::new(NoopEventSink),
                production_socket_root(),
                ObservationTuning::PRODUCTION,
                Arc::new(crate::LocalFleetCloneTool::default()),
            ) {
                Err(error) => error,
                Ok(_) => panic!("assembly fails before the socket is bound"),
            };

            assert!(
                matches!(failure, ServiceError::ProjectLoad { .. }),
                "the load refusal is dedicated, not transport: {failure}"
            );
        }
    }

    mod startup {
        use kanban_app::{NoopEventSink, RegistrationError};
        use kanban_dto::ErrorCode;
        use std::sync::Arc;

        use super::{ObservationTuning, ServiceError, assemble_core, corrupt_project_fixture};
        use crate::herdr::production_socket_root;
        use tempfile::TempDir;

        #[test]
        fn startup_corrupt_project_row_is_not_a_catalogue_registration_error() {
            let dir = TempDir::new().expect("a scratch directory is available");
            let database = corrupt_project_fixture(&dir);

            let failure = match assemble_core(
                dir.path(),
                database,
                Arc::new(NoopEventSink),
                production_socket_root(),
                ObservationTuning::PRODUCTION,
                Arc::new(crate::LocalFleetCloneTool::default()),
            ) {
                Err(error) => error,
                Ok(_) => panic!("corrupt data refuses assembly"),
            };

            let catalogue = ServiceError::Registration(RegistrationError::Uncatalogued(
                "stored Project row failed validation".to_owned(),
            ));
            assert!(
                catalogue.to_string().contains("is not in the catalog"),
                "catalogue registration errors name the catalogue: {catalogue}"
            );
            assert!(
                matches!(failure, ServiceError::ProjectLoad { .. }),
                "corrupt data must surface as project load, not registration: {failure}"
            );
            assert!(
                !failure.to_string().contains("is not in the catalog"),
                "startup must not mislabel corruption as an uncatalogued operation: {failure}"
            );
            let cause = match failure {
                ServiceError::ProjectLoad { cause } => cause,
                other => panic!("expected ProjectLoad, got {other}"),
            };
            assert_eq!(cause.code, ErrorCode::Internal);
        }
    }

    /// One prepared database whose sole Project row fails domain
    /// validation on read.
    fn corrupt_project_fixture(dir: &TempDir) -> kanban_storage::Database {
        let database = prepare_database(dir.path()).expect("the database prepares");
        let conn = rusqlite::Connection::open(dir.path().join("kanban.sqlite"))
            .expect("a second connection opens");
        conn.execute(
            "INSERT INTO projects
                 (code, name, repository, seed_workspace, default_branch,
                  herdr_workspace, herdr_session, archived, version)
             VALUES ('CORE', 'Control plane', '/repositories/kanban',
                     '/workspaces/kanban.seed', 'main', 'kanban.seed',
                     'kanban-main', 0, 1)",
            [],
        )
        .expect("the fixture Project lands");
        conn.execute("UPDATE projects SET code = 'bad' WHERE id = 1", [])
            .expect("the row is corrupted");
        database
    }
}
