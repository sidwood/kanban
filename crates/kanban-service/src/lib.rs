//! The durable per-user core process: it wires storage, the
//! application core, and the socket transport together and keeps
//! serving after the desktop UI quits (ADR-0001).

pub mod herdr;
pub mod timeline;

#[cfg(test)]
mod test_client;

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kanban_app::{
    Core, EventSink, GitObservation, ProjectStore, RegistrationError, TimelineQueryHandler,
};
use kanban_storage::paths::database_file_name;
use kanban_storage::{
    AllowAllMigrations, Database, RetentionPolicy, SqliteCommentStore, SqliteDeferralStore,
    SqliteEvidenceStore, SqliteHerdrSettingsStore, SqliteIdempotencyStore, SqliteInitiativeStore,
    SqlitePlanStore, SqliteProjectStore, SqliteRulingStore,
};
use kanban_transport::{ServerHandle, SocketServer, TransportError};

use herdr::{HerdrObserver, LiveHerdrDiagnostics, production_socket_root};
use timeline::StorageTimelineStore;

/// How many replay outcomes the core keeps. A retry follows its
/// original within seconds and the Operator drives one window, so a
/// five-figure bound covers every retry that could still arrive
/// while keeping the table small enough to ignore.
const RETAINED_OUTCOMES: NonZeroU32 = NonZeroU32::new(10_000).expect("the bound is not zero");

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

/// The running core process: its open database, its serving
/// socket, and its Herdr observer.
pub struct CoreProcess {
    database: Arc<Database>,
    server: ServerHandle,
    _herdr: Arc<HerdrObserver>,
}

impl CoreProcess {
    /// The path clients connect on.
    pub fn socket_path(&self) -> &Path {
        self.server.socket_path()
    }

    /// Stop serving and close the database.
    pub fn shutdown(self) {
        self.server.shutdown();
        drop(self.database);
    }
}

/// Open the durable database and apply every known migration before
/// any socket becomes reachable.
fn prepare_database(data_dir: &Path) -> Result<Database, ServiceError> {
    std::fs::create_dir_all(data_dir).map_err(|source| ServiceError::DataDir { source })?;
    let mut database = Database::open(&data_dir.join(database_file_name()))?;
    // Forward-only from the first boot; the verified-backup hook
    // arrives with KAN-T60.
    database.migrate(&AllowAllMigrations)?;
    Ok(database)
}

/// Wire the production application core around a prepared database
/// and the event sink owned by its transport.
fn assemble_core(
    data_dir: &Path,
    database: Database,
    events: Arc<dyn EventSink>,
    herdr_socket_root: PathBuf,
) -> Result<(Arc<Database>, Core, Arc<HerdrObserver>), ServiceError> {
    let initiative_store = Arc::new(SqliteInitiativeStore::new(&database));
    let project_store = Arc::new(SqliteProjectStore::new(&database));
    let herdr_settings_store = Arc::new(SqliteHerdrSettingsStore::new(&database));
    let plan_store = Arc::new(SqlitePlanStore::new(&database));
    let comment_store = Arc::new(SqliteCommentStore::new(&database));
    let ruling_store = Arc::new(SqliteRulingStore::new(&database));
    let deferral_store = Arc::new(SqliteDeferralStore::new(&database));
    let idempotency_store = Arc::new(SqliteIdempotencyStore::new(
        &database,
        RetentionPolicy::keep_most_recent(RETAINED_OUTCOMES),
    ));
    let evidence_store = Arc::new(SqliteEvidenceStore::new(
        &database,
        data_dir.join("attachments"),
    ));
    let database = Arc::new(database);
    let timeline_store = Arc::new(StorageTimelineStore::new(database.clone()));
    let mut core = Core::with_health(env!("CARGO_PKG_VERSION"), idempotency_store, events)?;
    let herdr = HerdrObserver::new(database.clone(), herdr_socket_root);
    core.register_initiatives(initiative_store.clone())?;
    let projects = project_store.clone();
    core.register_projects(
        project_store.clone(),
        Arc::new(LocalRepositories),
        initiative_store,
        herdr_settings_store.clone(),
        herdr.clone(),
    )?;
    core.register_plans(plan_store, projects)?;
    core.register_comments(comment_store)?;
    core.register_rulings(ruling_store)?;
    core.register_deferrals(deferral_store)?;
    core.register_evidence(evidence_store)?;
    core.register_query(
        "timeline.query",
        Arc::new(TimelineQueryHandler::new(timeline_store)),
    )?;
    let projects = project_store.list().map_err(|error| {
        ServiceError::Registration(RegistrationError::Uncatalogued(error.message))
    })?;
    herdr.observe_projects(&projects);
    let diagnostics = Arc::new(LiveHerdrDiagnostics::new(&herdr));
    core.register_herdr(herdr_settings_store, diagnostics, project_store)?;
    Ok((database, core, herdr))
}

/// Open (creating if needed) the database inside `data_dir`, bring
/// its schema up to date, and serve the application core on
/// `core.sock` inside the same directory.
pub fn serve(data_dir: &Path) -> Result<CoreProcess, ServiceError> {
    let database = prepare_database(data_dir)?;
    let server = SocketServer::bind(data_dir)?;
    let broker = server.broker();
    let (database, core, herdr) =
        assemble_core(data_dir, database, broker, production_socket_root())?;
    let server = server.serve(Arc::new(core))?;
    Ok(CoreProcess {
        database,
        server,
        _herdr: herdr,
    })
}

#[cfg(test)]
pub(crate) fn serve_with_herdr_sessions(
    data_dir: &Path,
    herdr_socket_root: PathBuf,
) -> Result<CoreProcess, ServiceError> {
    let database = prepare_database(data_dir)?;
    let server = SocketServer::bind(data_dir)?;
    let broker = server.broker();
    let (database, core, herdr) = assemble_core(data_dir, database, broker, herdr_socket_root)?;
    let server = server.serve(Arc::new(core))?;
    Ok(CoreProcess {
        database,
        server,
        _herdr: herdr,
    })
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
    /// Storage refused.
    #[error(transparent)]
    Storage(#[from] kanban_storage::StorageError),
    /// Transport refused.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// A handler could not be registered.
    #[error(transparent)]
    Registration(#[from] kanban_app::RegistrationError),
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
        LocalRepositories, ServiceError, assemble_core, prepare_database, serve,
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
        repository.to_str().expect("the path is UTF-8").to_owned()
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("the metadata reads")
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn registered_catalogue_matches_the_exposed_catalogue() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let database = prepare_database(dir.path()).expect("the production database prepares");
        let (_, core, _) = assemble_core(
            dir.path(),
            database,
            Arc::new(NoopEventSink),
            production_socket_root(),
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

        let refusal = serve(dir.path());
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
            }),
        );
        assert_eq!(registered["code"], json!("CORE"));
        assert_eq!(registered["version"], json!(1));
        assert_eq!(
            registered["counters"],
            json!({ "plan": 0, "spec": 0, "ticket": 0 })
        );

        // The same session name is refused for another Project, and a
        // target that is not a Git repository never registers.
        let duplicate_session = client.command_error(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-wave" },
                "code": "WAVE",
                "name": "Wave pool",
                "repository": repository,
                "seed_workspace": "/workspaces/wave.seed",
                "default_branch": "main",
                "herdr_session": "kanban-main",
            }),
        );
        assert_eq!(duplicate_session["code"], json!("invalid_request"));
        assert!(
            duplicate_session["message"]
                .as_str()
                .expect("the message is text")
                .contains("kanban-main"),
            "the refusal names the session: {duplicate_session}"
        );
        let non_git = client.command_error(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-plain" },
                "code": "PLAIN",
                "name": "Plain directory",
                "repository": dir.path().join("not-a-repository"),
                "seed_workspace": "/workspaces/plain.seed",
                "default_branch": "main",
                "herdr_session": "plain-main",
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
                        "initiative_id": null,
                        "archived": true,
                        "counters": { "plan": 0, "spec": 0, "ticket": 0 },
                        "version": 2,
                    }
                ]
            }),
            "archiving preserves every recorded fact over the wire"
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
            }),
        );
        thread::sleep(Duration::from_millis(200));
        let answer = client.query_with(
            "timeline.query",
            json!({
                "scope": { "project": "1" },
                "kinds": ["telemetry"],
            }),
        );
        let events = answer["events"]
            .as_array()
            .expect("telemetry is queryable on the live registration path");
        assert_eq!(events.len(), 1);
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
            }),
        );
        client.command(
            "project.archive",
            json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "archive-core" },
                "project_id": 1,
            }),
        );

        let answer = client.query_with("timeline.query", json!({ "scope": { "project": "1" } }));

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
                    json!({ "project": "1" }),
                    json!("transition"),
                    json!({ "kind": "project", "id": "1" }),
                    json!("registered"),
                ),
                (
                    json!({ "project": "1" }),
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
                "scope": { "project": "1" },
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
            second.query_with("timeline.query", json!({ "scope": { "project": "1" } })),
            answer,
            "a restart serves the same typed history"
        );

        rebooted.shutdown();
    }

    /// KAN-T10-AC3: every evidence command leaves the per-Project
    /// timeline readable. A list row once carried half an entity
    /// reference, which the timeline decoder refuses, so one
    /// `evidence.list` used to make the whole Project query fail.
    #[test]
    fn evidence_commands_keep_the_project_timeline_readable() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let core = boot(&dir);
        let mut client = Client::connect(core.socket_path());

        let attached = client.command(
            "evidence.attach",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "evidence-attach" },
                "project_id": "kan-p1",
                "entity_kind": "ticket",
                "entity_id": "kan-t10",
                "evidence_kind": "managed_file",
                "content_base64": "cHJvb2YgYnl0ZXM=",
            }),
        );
        assert_eq!(attached["id"], json!(1));
        client.command(
            "evidence.list",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "evidence-list-ticket" },
                "project_id": "kan-p1",
                "entity_kind": "ticket",
                "entity_id": "kan-t10",
            }),
        );
        client.command(
            "evidence.list",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "evidence-list-project" },
                "project_id": "kan-p1",
            }),
        );

        let timeline = client.query_with(
            "timeline.query",
            json!({ "scope": { "project": "kan-p1" } }),
        );
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
                json!({ "kind": "ticket", "id": "kan-t10" }),
                json!({ "kind": "ticket", "id": "kan-t10" }),
                Value::Null,
            ],
            "attach references the subject entity, a filtered list references the filter, a Project-wide list references nothing"
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
            }),
        );
        assert_eq!(registered["version"], json!(1));
        // Stand in for the Spec authoring that lands in KAN-T14: the
        // Project mints four Spec numbers, so Spec 4 exists without
        // belonging to this Plan.
        {
            let conn = rusqlite::Connection::open(dir.path().join("kanban.sqlite"))
                .expect("a second connection opens");
            conn.execute("UPDATE projects SET spec_counter = 4 WHERE id = 1", [])
                .expect("the Spec numbers are minted");
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
}
