//! The durable per-user core process: it wires storage, the
//! application core, and the socket transport together and keeps
//! serving after the desktop UI quits (ADR-0001).

use std::path::Path;
use std::sync::Arc;

use kanban_app::{Core, MemoryIdempotencyStore};
use kanban_storage::paths::database_file_name;
use kanban_storage::{AllowAllMigrations, Database};
use kanban_transport::{ServerHandle, SocketServer, TransportError};

/// The running core process: its open database and its serving
/// socket.
pub struct CoreProcess {
    database: Database,
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
    let server = SocketServer::bind(data_dir)?;
    let broker = server.broker();
    let core = Core::with_health(
        env!("CARGO_PKG_VERSION"),
        Arc::new(MemoryIdempotencyStore::new()),
        broker,
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
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::time::Duration;

    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::{CoreProcess, ServiceError, serve};

    /// Boot the core against a scratch data directory.
    fn boot(dir: &TempDir) -> CoreProcess {
        serve(dir.path()).expect("the core boots on a scratch data directory")
    }

    /// One line-based client, mirroring what every real client does.
    struct Client {
        reader: BufReader<UnixStream>,
        stream: UnixStream,
    }

    impl Client {
        fn connect(socket_path: &Path) -> Self {
            let stream = UnixStream::connect(socket_path).expect("the client connects");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("the read timeout applies");
            let reader = BufReader::new(stream.try_clone().expect("the stream clones"));
            Self { reader, stream }
        }

        fn query(&mut self, operation: &str) -> Value {
            writeln!(
                self.stream,
                "{}",
                json!({ "kind": "query", "operation": operation, "payload": {} })
            )
            .expect("the client writes");
            self.stream.flush().expect("the client flushes");
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).expect("the client reads");
            assert!(read > 0, "the core answers the query");
            let frame: Value = serde_json::from_str(line.trim_end()).expect("a frame decodes");
            assert_eq!(frame["kind"], "response", "the query succeeds: {frame}");
            frame["payload"].clone()
        }
    }

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
}
