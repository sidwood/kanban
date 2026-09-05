//! The durable per-user core process: it wires storage, the
//! application core, and the socket transport together and keeps
//! serving after the desktop UI quits (ADR-0001).

pub mod timeline;

#[cfg(test)]
mod test_client;

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use kanban_app::{Core, TimelineQueryHandler};
use kanban_storage::paths::database_file_name;
use kanban_storage::{
    AllowAllMigrations, Database, RetentionPolicy, SqliteCommentStore, SqliteDeferralStore,
    SqliteEvidenceStore, SqliteIdempotencyStore, SqliteInitiativeStore, SqliteRulingStore,
};
use kanban_transport::{ServerHandle, SocketServer, TransportError};

use timeline::StorageTimelineStore;

/// How many replay outcomes the core keeps. A retry follows its
/// original within seconds and the Operator drives one window, so a
/// five-figure bound covers every retry that could still arrive
/// while keeping the table small enough to ignore.
const RETAINED_OUTCOMES: NonZeroU32 = NonZeroU32::new(10_000).expect("the bound is not zero");

/// The running core process: its open database and its serving
/// socket.
pub struct CoreProcess {
    database: Arc<Database>,
    server: ServerHandle,
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

/// Open (creating if needed) the database inside `data_dir`, bring
/// its schema up to date, and serve the application core on
/// `core.sock` inside the same directory.
pub fn serve(data_dir: &Path) -> Result<CoreProcess, ServiceError> {
    std::fs::create_dir_all(data_dir).map_err(|source| ServiceError::DataDir { source })?;
    let mut database = Database::open(&data_dir.join(database_file_name()))?;
    // Forward-only from the first boot; the verified-backup hook
    // arrives with KAN-T60.
    database.migrate(&AllowAllMigrations)?;
    let initiative_store = Arc::new(SqliteInitiativeStore::new(&database));
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
    let server = SocketServer::bind(data_dir)?;
    let broker = server.broker();
    let mut core = Core::with_health(env!("CARGO_PKG_VERSION"), idempotency_store, broker)?;
    core.register_initiatives(initiative_store)?;
    core.register_comments(comment_store)?;
    core.register_rulings(ruling_store)?;
    core.register_deferrals(deferral_store)?;
    core.register_evidence(evidence_store)?;
    core.register_query(
        "timeline.query",
        Arc::new(TimelineQueryHandler::new(timeline_store)),
    )?;
    let server = server.serve(Arc::new(core))?;
    Ok(CoreProcess { database, server })
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

    use super::{ServiceError, serve};
    use crate::test_client::{Client, boot};

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("the metadata reads")
            .permissions()
            .mode()
            & 0o777
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
                json!({ "kind": "evidence", "id": "1" }),
                json!({ "kind": "ticket", "id": "kan-t10" }),
                Value::Null,
            ],
            "attach references the item, a filtered list references the filter, a Project-wide list references nothing"
        );

        core.shutdown();
    }
}
