//! The current-user-only Unix socket server (ADR-0003).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use crate::broker::EventBroker;
use crate::error::TransportError;
use crate::frame::{FrameKind, RequestFrame, ResponseFrame};
use kanban_dto::ApiError;

/// The socket file name inside the managed data directory.
pub const SOCKET_FILE_NAME: &str = "core.sock";

/// The mode a current-user-only directory must carry: read, write,
/// and search for the owner, nothing for anyone else.
const OWNER_ONLY_DIRECTORY: u32 = 0o700;

/// The mode the socket file must carry: read and write for the
/// owner, nothing for anyone else.
const OWNER_ONLY_SOCKET: u32 = 0o600;

/// A bound, not-yet-serving socket in a current-user-only directory.
pub struct SocketServer {
    listener: UnixListener,
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    connections: Arc<Mutex<Vec<ConnectionEntry>>>,
    next_connection_id: Arc<AtomicU64>,
    broker: Arc<EventBroker>,
}

struct ConnectionEntry {
    id: u64,
    stream: Arc<UnixStream>,
    reader: std::thread::JoinHandle<()>,
}

/// Forget one connection's registry entry, closing its socket when
/// this was the last handle.
fn forget_connection(connections: &Arc<Mutex<Vec<ConnectionEntry>>>, id: u64) {
    connections
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .retain(|entry| entry.id != id);
}

/// A running server. Dropping it without [`ServerHandle::shutdown`]
/// leaves its threads serving until the process ends; call
/// `shutdown` for a clean stop.
pub struct ServerHandle {
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    accept: std::thread::JoinHandle<()>,
}

impl SocketServer {
    /// Bind `core.sock` inside `directory`, creating the directory if
    /// needed and tightening it and the socket to owner-only before
    /// accepting a single connection. A socket left behind by a
    /// crashed core is replaced; a socket another core is serving,
    /// or any other file, is refused.
    pub fn bind(directory: &Path) -> Result<Self, TransportError> {
        prepare_socket_directory(directory)?;
        let socket_path = directory.join(SOCKET_FILE_NAME);
        clear_stale_socket(&socket_path)?;
        let listener = UnixListener::bind(&socket_path).map_err(|source| TransportError::Bind {
            path: socket_path.clone(),
            source,
        })?;
        apply_mode(&socket_path, OWNER_ONLY_SOCKET)?;
        Ok(Self {
            listener,
            socket_path,
            shutdown: Arc::default(),
            connections: Arc::default(),
            next_connection_id: Arc::default(),
            broker: Arc::new(EventBroker::new()),
        })
    }

    /// The broker that sequences this server's event stream.
    pub fn broker(&self) -> Arc<EventBroker> {
        self.broker.clone()
    }

    /// The bound socket path.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Serve `core` on the bound socket until
    /// [`ServerHandle::shutdown`] is called.
    pub fn serve(self, core: Arc<kanban_app::Core>) -> ServerHandle {
        let Self {
            listener,
            socket_path,
            shutdown,
            connections,
            next_connection_id,
            broker,
        } = self;
        let accept_shutdown = shutdown.clone();
        let accept_connections = connections.clone();
        let accept = std::thread::Builder::new()
            .name("kanban-transport-accept".to_owned())
            .spawn(move || {
                for connection in listener.incoming() {
                    if accept_shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    match connection {
                        Ok(stream) => spawn_connection(
                            stream,
                            &core,
                            &broker,
                            &accept_connections,
                            &next_connection_id,
                        ),
                        Err(_) => continue,
                    }
                }
                // Take the entries out and drop the lock before
                // joining: readers deregister themselves through the
                // same mutex on the way out, so joining under the
                // lock would deadlock.
                let entries: Vec<ConnectionEntry> = {
                    let mut entries = accept_connections
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    entries.drain(..).collect()
                };
                for entry in entries {
                    let _ = entry.stream.shutdown(std::net::Shutdown::Both);
                    let _ = entry.reader.join();
                }
            })
            .expect("spawning the accept thread succeeds");
        ServerHandle {
            socket_path,
            shutdown,
            accept,
        }
    }
}

impl ServerHandle {
    /// The socket path this server holds.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Stop serving: unblock the accept loop, end every connection,
    /// and wait for the server's threads to finish.
    pub fn shutdown(self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // Wake the accept loop; the extra connection is dropped
        // unread on purpose.
        let _ = UnixStream::connect(&self.socket_path);
        let _ = self.accept.join();
    }
}

/// Create the socket directory if needed and tighten it to
/// owner-only.
fn prepare_socket_directory(directory: &Path) -> Result<(), TransportError> {
    std::fs::create_dir_all(directory).map_err(|source| TransportError::Directory {
        path: directory.to_owned(),
        source,
    })?;
    apply_mode(directory, OWNER_ONLY_DIRECTORY)
}

/// Set `path`'s permission bits to `mode`.
fn apply_mode(path: &Path, mode: u32) -> Result<(), TransportError> {
    let permissions = std::fs::metadata(path)
        .map_err(|source| TransportError::Directory {
            path: path.to_owned(),
            source,
        })?
        .permissions();
    let mut permissions = permissions;
    permissions.set_mode(mode);
    std::fs::set_permissions(path, permissions).map_err(|source| match mode {
        OWNER_ONLY_DIRECTORY => TransportError::Directory {
            path: path.to_owned(),
            source,
        },
        _ => TransportError::Secure {
            path: path.to_owned(),
            source,
        },
    })
}

/// Decide what to do about an existing socket path: refuse a live
/// core's socket and any non-socket file, remove a crashed core's
/// stale socket.
fn clear_stale_socket(socket_path: &Path) -> Result<(), TransportError> {
    let metadata = match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(()),
    };
    if !metadata.file_type().is_socket() {
        return Err(TransportError::SocketPathOccupied {
            path: socket_path.to_owned(),
        });
    }
    match UnixStream::connect(socket_path) {
        Ok(_) => Err(TransportError::SocketInUse {
            path: socket_path.to_owned(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            std::fs::remove_file(socket_path).map_err(|source| TransportError::StaleSocket {
                path: socket_path.to_owned(),
                source,
            })
        }
        Err(_) => Err(TransportError::SocketInUse {
            path: socket_path.to_owned(),
        }),
    }
}

/// Accept one connection's threads into the registry and start its
/// reader.
fn spawn_connection(
    stream: UnixStream,
    core: &Arc<kanban_app::Core>,
    broker: &Arc<EventBroker>,
    connections: &Arc<Mutex<Vec<ConnectionEntry>>>,
    next_connection_id: &Arc<AtomicU64>,
) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let shared_write = Arc::new(Mutex::new(write_half));
    let stream = Arc::new(stream);
    let id = next_connection_id.fetch_add(1, Ordering::Relaxed);
    let reader = std::thread::Builder::new()
        .name("kanban-transport-connection".to_owned())
        .spawn({
            let core = core.clone();
            let broker = broker.clone();
            let shared_write = shared_write.clone();
            let connections = connections.clone();
            move || {
                serve_connection(read_half, shared_write, &core, &broker);
                // Forget the connection so its socket closes for the
                // client instead of lingering until server shutdown.
                forget_connection(&connections, id);
            }
        })
        .expect("spawning a connection thread succeeds");
    connections
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(ConnectionEntry { id, stream, reader });
}

/// One connection's live event subscription: the broker-side id,
/// the flag that retires the writer without announcing anything, and
/// the writer's handle.
struct ActiveSubscription {
    id: u64,
    end_quietly: Arc<AtomicBool>,
    writer: std::thread::JoinHandle<()>,
}

/// End one connection's subscription: tell its writer to stop
/// writing, detach it from the broker, and wait for the writer to
/// finish, so no line of this subscription can appear after the
/// call returns.
fn end_subscription(active: ActiveSubscription, broker: &EventBroker) {
    // Draining without writing: the connection itself asked for
    // this end, so there is nothing to announce to the client.
    active.end_quietly.store(true, Ordering::Release);
    broker.unsubscribe(active.id);
    let _ = active.writer.join();
}

/// Serve request lines from one client until it disconnects or the
/// server shuts down.
fn serve_connection(
    read_half: UnixStream,
    shared_write: Arc<Mutex<UnixStream>>,
    core: &Arc<kanban_app::Core>,
    broker: &Arc<EventBroker>,
) {
    let mut reader = BufReader::new(read_half);
    let mut subscription: Option<ActiveSubscription> = None;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }

        let request: RequestFrame = match serde_json::from_str(trimmed) {
            Ok(request) => request,
            Err(error) => {
                // A line that is not a frame is a protocol breach:
                // answer it, then close the connection.
                let _ = write_frame(
                    &shared_write,
                    &ResponseFrame::Error {
                        error: ApiError::invalid_request(&format!("malformed frame: {error}")),
                    },
                );
                break;
            }
        };

        match request.kind {
            FrameKind::Subscribe => {
                // Retire any previous subscription and wait for its
                // writer to finish before starting the replacement:
                // two writers on one connection could interleave
                // their lines.
                if let Some(previous) = subscription.take() {
                    end_subscription(previous, broker);
                }
                // Acknowledge before attaching, so the
                // acknowledgement always precedes this
                // subscription's first event frame: an attached
                // writer races this write for the socket.
                if write_frame(&shared_write, &ResponseFrame::Subscribed {}).is_err() {
                    break;
                }
                let (id, events) = broker.subscribe();
                let end_quietly = Arc::new(AtomicBool::new(false));
                let writer =
                    spawn_event_writer(events, shared_write.clone(), Arc::clone(&end_quietly))
                        .expect("spawning an event writer succeeds");
                subscription = Some(ActiveSubscription {
                    id,
                    end_quietly,
                    writer,
                });
            }
            FrameKind::Query | FrameKind::Command => {
                let Some(operation) = request.operation.as_deref() else {
                    let _ = write_frame(
                        &shared_write,
                        &ResponseFrame::Error {
                            error: ApiError::invalid_request(&format!(
                                "a {} frame must name an operation",
                                match request.kind {
                                    FrameKind::Query => "query",
                                    FrameKind::Command => "command",
                                    FrameKind::Subscribe => "subscribe",
                                }
                            )),
                        },
                    );
                    continue;
                };
                let payload = request
                    .payload
                    .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
                let outcome = match request.kind {
                    FrameKind::Query => core.query(operation, &payload),
                    FrameKind::Command => core.command(operation, &payload),
                    FrameKind::Subscribe => continue,
                };
                let frame = match outcome {
                    Ok(payload) => ResponseFrame::Response { payload },
                    Err(error) => ResponseFrame::Error { error },
                };
                if write_frame(&shared_write, &frame).is_err() {
                    break;
                }
            }
        }
    }
    if let Some(active) = subscription.take() {
        end_subscription(active, broker);
    }
}

/// Write one frame as one line. The shared lock keeps lines whole
/// when a subscription's writer thread is writing alongside this
/// one.
fn write_frame(
    shared_write: &Arc<Mutex<UnixStream>>,
    frame: &ResponseFrame,
) -> std::io::Result<()> {
    let line = serde_json::to_string(frame).expect("a response frame encodes");
    let mut stream = shared_write
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    writeln!(stream, "{line}")?;
    stream.flush()
}

/// Drain one subscriber's event lines onto its connection until the
/// subscription ends. When [`ActiveSubscription::end_quietly`] is
/// raised the writer drains its queue without writing, because the
/// connection itself ended the subscription; any other end is the
/// broker evicting a subscriber that stopped reading, and is
/// announced so the client can subscribe again.
fn spawn_event_writer(
    events: Receiver<String>,
    shared_write: Arc<Mutex<UnixStream>>,
    end_quietly: Arc<AtomicBool>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("kanban-transport-writer".to_owned())
        .spawn(move || {
            while let Ok(line) = events.recv() {
                if end_quietly.load(Ordering::Acquire) {
                    continue;
                }
                let mut stream = shared_write
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if writeln!(stream, "{line}")
                    .and_then(|_| stream.flush())
                    .is_err()
                {
                    return;
                }
            }
            // The queue disconnected: the subscription was evicted,
            // unless the connection ended it quietly above.
            if !end_quietly.load(Ordering::Acquire) {
                let _ = write_frame(&shared_write, &ResponseFrame::Evicted {});
            }
        })
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::{SOCKET_FILE_NAME, ServerHandle, SocketServer};
    use crate::error::TransportError;
    use crate::frame::{FrameKind, RequestFrame, ResponseFrame};
    use kanban_app::{
        CommandHandler, Core, EventSink, MemoryIdempotencyStore, OperationDescriptor,
        OperationKind, ParsedCommand, parse_payload,
    };
    use kanban_dto::{ApiError, ErrorCode, EventEnvelope, MutationContext};

    const TEST_CATALOG: &[OperationDescriptor] = &[
        OperationDescriptor {
            name: "counter.bump",
            kind: OperationKind::Command,
            request_schema: "MutationContext",
            response_schema: "HealthResponse",
            mcp_tool_name: "counter_bump",
            description: "Test fixture: bump a versioned counter.",
        },
        OperationDescriptor {
            name: "counter.pad",
            kind: OperationKind::Command,
            request_schema: "MutationContext",
            response_schema: "HealthResponse",
            mcp_tool_name: "counter_pad",
            description: "Test fixture: emit one padded event.",
        },
    ];

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct BumpRequest {
        mutation: MutationContext,
        step: i64,
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PadRequest {
        mutation: MutationContext,
        bytes: usize,
    }

    /// The same one-aggregate fixture the app tests use, wired to
    /// the broker so commands stream events.
    #[derive(Debug, Default)]
    struct Counter {
        value: std::sync::Mutex<i64>,
        version: std::sync::Mutex<u64>,
    }

    impl CommandHandler for Counter {
        fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
            parse_payload::<BumpRequest>(payload)?;
            ParsedCommand::lift("counter", payload)
        }

        fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
            Ok(*self.version.lock().expect("the version lock is sound"))
        }

        fn apply(
            &self,
            command: &ParsedCommand,
            events: &dyn EventSink,
        ) -> Result<Value, ApiError> {
            let request: BumpRequest = parse_payload(&command.payload)?;
            debug_assert_eq!(
                request.mutation.optimistic_version, command.optimistic_version,
                "the typed DTO and the lift agree on the mutation context"
            );
            let mut value = self.value.lock().expect("the value lock is sound");
            let mut version = self.version.lock().expect("the version lock is sound");
            *value += request.step;
            *version += 1;
            events.emit("counter.bumped", json!({ "to": *value }));
            Ok(json!({ "value": *value, "version": *version }))
        }
    }

    /// Emits one event padded to `bytes`, for tests that need event
    /// lines no socket buffer can hold.
    #[derive(Debug, Default)]
    struct Pad;

    impl CommandHandler for Pad {
        fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
            parse_payload::<PadRequest>(payload)?;
            ParsedCommand::lift("counter", payload)
        }

        fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
            Ok(0)
        }

        fn apply(
            &self,
            command: &ParsedCommand,
            events: &dyn EventSink,
        ) -> Result<Value, ApiError> {
            let request: PadRequest = parse_payload(&command.payload)?;
            debug_assert_eq!(
                request.mutation.optimistic_version, command.optimistic_version,
                "the typed DTO and the lift agree on the mutation context"
            );
            events.emit(
                "counter.padded",
                json!({ "pad": "x".repeat(request.bytes) }),
            );
            Ok(json!({ "bytes": request.bytes }))
        }
    }

    /// A served core with a test command, plus its socket path.
    fn served(dir: &Path) -> ServerHandle {
        let server = SocketServer::bind(dir).expect("the server binds");
        let broker = server.broker();
        let mut core = Core::new(
            TEST_CATALOG,
            Arc::new(MemoryIdempotencyStore::new()),
            broker,
        );
        core.register_command("counter.bump", Arc::new(Counter::default()))
            .expect("the test command registers");
        core.register_command("counter.pad", Arc::new(Pad))
            .expect("the test command registers");
        server.serve(Arc::new(core))
    }

    fn bump(step: i64, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "step": step,
        })
    }

    fn pad(bytes: usize, key: &str) -> Value {
        json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "bytes": bytes,
        })
    }

    /// A raw line client with a guard against hanging forever.
    struct TestClient {
        reader: BufReader<UnixStream>,
        stream: UnixStream,
    }

    impl TestClient {
        fn connect(path: &Path) -> Self {
            let stream = UnixStream::connect(path).expect("the client connects");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("the read timeout applies");
            let reader = BufReader::new(stream.try_clone().expect("the stream clones"));
            Self { reader, stream }
        }

        fn send_raw(&mut self, line: &str) {
            writeln!(self.stream, "{line}").expect("the client writes");
            self.stream.flush().expect("the client flushes");
        }

        fn send(&mut self, frame: &RequestFrame) {
            self.send_raw(&serde_json::to_string(frame).expect("the frame encodes"));
        }

        /// Read one frame, failing on EOF or a malformed line.
        fn recv(&mut self) -> ResponseFrame {
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).expect("the client reads");
            assert!(read > 0, "the server closed the connection early");
            serde_json::from_str(line.trim_end()).expect("the frame decodes")
        }

        /// Read one frame, or `None` when the server has closed the
        /// connection.
        fn try_recv(&mut self) -> Option<ResponseFrame> {
            let mut line = String::new();
            let read = self
                .reader
                .read_line(&mut line)
                .expect("the client reads until the server closes");
            if read == 0 {
                return None;
            }
            Some(serde_json::from_str(line.trim_end()).expect("the frame decodes"))
        }

        fn query(&mut self, operation: &str) -> ResponseFrame {
            self.send(&RequestFrame {
                kind: FrameKind::Query,
                operation: Some(operation.to_owned()),
                payload: Some(json!({})),
            });
            self.recv()
        }

        fn command(&mut self, payload: Value) -> ResponseFrame {
            self.send(&RequestFrame {
                kind: FrameKind::Command,
                operation: Some("counter.bump".to_owned()),
                payload: Some(payload),
            });
            self.recv()
        }

        fn pad(&mut self, payload: Value) -> ResponseFrame {
            self.send(&RequestFrame {
                kind: FrameKind::Command,
                operation: Some("counter.pad".to_owned()),
                payload: Some(payload),
            });
            self.recv()
        }

        fn subscribe(&mut self) {
            self.send(&RequestFrame {
                kind: FrameKind::Subscribe,
                operation: None,
                payload: None,
            });
            assert_eq!(self.recv(), ResponseFrame::Subscribed {});
        }

        fn events(&mut self, count: usize) -> Vec<ResponseFrame> {
            (0..count).map(|_| self.recv()).collect()
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
    fn binding_creates_an_owner_only_directory_and_socket() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_dir = dir.path().join("Kanban");
        std::fs::create_dir(&socket_dir).expect("the directory is created");
        std::fs::set_permissions(&socket_dir, std::fs::Permissions::from_mode(0o755))
            .expect("the loose mode applies");

        let server = SocketServer::bind(&socket_dir).expect("the server binds");
        let socket_path = server.socket_path().to_owned();
        drop(server);

        assert_eq!(mode_of(&socket_dir), 0o700, "the directory is owner-only");
        assert_eq!(mode_of(&socket_path), 0o600, "the socket is owner-only");
        assert_eq!(socket_path.file_name(), Some(SOCKET_FILE_NAME.as_ref()));
    }

    #[test]
    fn a_query_round_trips_over_the_socket() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let server = SocketServer::bind(dir.path()).expect("the server binds");
        let broker = server.broker();
        let core = Core::with_health(
            "0.1.0-test",
            Arc::new(MemoryIdempotencyStore::new()),
            broker,
        )
        .expect("the core wires");
        let handle = server.serve(Arc::new(core));

        let mut client = TestClient::connect(handle.socket_path());
        let response = client.query("health.get");

        assert_eq!(
            response,
            ResponseFrame::Response {
                payload: json!({ "connected": true, "service_version": "0.1.0-test" })
            }
        );

        handle.shutdown();
    }

    #[test]
    fn a_command_round_trips_and_streams_ordered_events() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let handle = served(dir.path());

        let mut subscriber = TestClient::connect(handle.socket_path());
        subscriber.subscribe();

        let mut caller = TestClient::connect(handle.socket_path());
        let response = caller.command(bump(1, "key-1", 0));
        assert_eq!(
            response,
            ResponseFrame::Response {
                payload: json!({ "value": 1, "version": 1 })
            }
        );

        let second = caller.command(bump(2, "key-2", 1));
        assert_eq!(
            second,
            ResponseFrame::Response {
                payload: json!({ "value": 3, "version": 2 })
            }
        );

        let events = subscriber.events(2);
        assert_eq!(
            events,
            vec![
                ResponseFrame::Event {
                    event: EventEnvelope {
                        sequence: 1,
                        event_type: "counter.bumped".to_owned(),
                        payload: json!({ "to": 1 }),
                    }
                },
                ResponseFrame::Event {
                    event: EventEnvelope {
                        sequence: 2,
                        event_type: "counter.bumped".to_owned(),
                        payload: json!({ "to": 3 }),
                    }
                },
            ],
            "events arrive in sequence order"
        );

        handle.shutdown();
    }

    #[test]
    fn every_subscriber_sees_the_same_ordered_events() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let handle = served(dir.path());

        let mut first = TestClient::connect(handle.socket_path());
        let mut second = TestClient::connect(handle.socket_path());
        first.subscribe();
        second.subscribe();

        let mut caller = TestClient::connect(handle.socket_path());
        caller.command(bump(1, "key-1", 0));
        caller.command(bump(1, "key-2", 1));

        assert_eq!(first.events(2), second.events(2));

        handle.shutdown();
    }

    #[test]
    fn resubscription_retires_the_old_writer_first() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let handle = served(dir.path());

        let mut subscriber = TestClient::connect(handle.socket_path());
        subscriber.subscribe();

        // Six padded events, each far larger than any socket buffer,
        // stall the first writer mid-line with the rest queued behind
        // it: only the in-flight line can reach the wire before the
        // resubscription retires the writer.
        let mut caller = TestClient::connect(handle.socket_path());
        for i in 1..=6 {
            caller.pad(pad(2 * 1024 * 1024, &format!("big-{i}")));
        }

        // Resubscribe while the first writer is stalled.
        subscriber.send(&RequestFrame {
            kind: FrameKind::Subscribe,
            operation: None,
            payload: None,
        });

        // Drain until the resubscription is acknowledged; the only
        // event ahead of the acknowledgement is the one the old
        // writer already had in flight. The queued remainder of the
        // retired subscription is dropped, not delivered.
        let mut before_ack = Vec::new();
        loop {
            match subscriber.recv() {
                ResponseFrame::Subscribed {} => break,
                ResponseFrame::Event { event } => before_ack.push(event.sequence),
                other => panic!("unexpected frame {other:?}"),
            }
        }
        assert_eq!(
            before_ack,
            vec![1],
            "only the in-flight event precedes the acknowledgement"
        );

        // The replacement subscription delivers from its own start.
        let response = caller.command(bump(1, "after", 0));
        assert!(
            matches!(response, ResponseFrame::Response { .. }),
            "the bump succeeds, got {response:?}"
        );
        match subscriber.recv() {
            ResponseFrame::Event { event } => assert_eq!(event.sequence, 7),
            other => panic!("the new subscription delivers, got {other:?}"),
        }

        handle.shutdown();
    }

    #[test]
    fn the_acknowledgement_precedes_the_first_event() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let handle = served(dir.path());

        // A second client keeps events flowing while fresh
        // connections subscribe, so an attached writer has every
        // chance to race the acknowledgement onto the wire.
        let hammer_path = handle.socket_path().to_owned();
        let hammer = std::thread::spawn(move || {
            let mut client = TestClient::connect(&hammer_path);
            for i in 1..=5_000u32 {
                client.pad(pad(0, &format!("hammer-{i}")));
            }
        });

        for _ in 0..20 {
            let mut subscriber = TestClient::connect(handle.socket_path());
            subscriber.send(&RequestFrame {
                kind: FrameKind::Subscribe,
                operation: None,
                payload: None,
            });
            match subscriber.recv() {
                ResponseFrame::Subscribed {} => {}
                other => panic!("the acknowledgement must be the first frame, got {other:?}"),
            }
            drop(subscriber);
        }

        hammer.join().expect("the hammering client finishes");
        handle.shutdown();
    }

    #[test]
    fn an_evicted_subscriber_is_told_and_can_resubscribe() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let handle = served(dir.path());

        let mut subscriber = TestClient::connect(handle.socket_path());
        subscriber.subscribe();

        // This subscriber never reads while a second client buries
        // it: once its queue and the socket buffer fill, the broker
        // evicts it.
        let mut caller = TestClient::connect(handle.socket_path());
        for i in 1..=2_000 {
            caller.pad(pad(0, &format!("flood-{i}")));
        }

        // Draining eventually reaches the eviction notice; the
        // events that never fitted the queue are simply gone.
        let mut delivered = 0;
        loop {
            match subscriber.recv() {
                ResponseFrame::Event { .. } => delivered += 1,
                ResponseFrame::Evicted {} => break,
                other => panic!("unexpected frame {other:?}"),
            }
        }
        assert!(
            delivered < 2_000,
            "the evicted subscriber missed the tail of the flood"
        );

        // The connection itself stays alive and answers commands.
        let response = subscriber.command(bump(1, "after-eviction", 0));
        assert!(
            matches!(response, ResponseFrame::Response { .. }),
            "the connection outlives its subscription, got {response:?}"
        );

        // Resubscribing resumes the stream from that point.
        subscriber.subscribe();
        caller.pad(pad(0, "resumed"));
        match subscriber.recv() {
            ResponseFrame::Event { event } => assert_eq!(event.sequence, 2_002),
            other => panic!("the new subscription delivers, got {other:?}"),
        }

        handle.shutdown();
    }

    #[test]
    fn late_subscribers_miss_earlier_events() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let handle = served(dir.path());

        let mut caller = TestClient::connect(handle.socket_path());
        caller.command(bump(1, "key-1", 0));

        let mut subscriber = TestClient::connect(handle.socket_path());
        subscriber.subscribe();
        caller.command(bump(1, "key-2", 1));

        let events = subscriber.events(1);
        assert_eq!(
            events,
            vec![ResponseFrame::Event {
                event: EventEnvelope {
                    sequence: 2,
                    event_type: "counter.bumped".to_owned(),
                    payload: json!({ "to": 2 }),
                }
            }],
            "the stream is live-only; sequence numbers stay global"
        );

        handle.shutdown();
    }

    #[test]
    fn a_stale_optimistic_version_error_reaches_the_client() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let handle = served(dir.path());

        let mut client = TestClient::connect(handle.socket_path());
        client.command(bump(1, "key-1", 0));

        let stale = client.command(bump(1, "key-2", 0));

        assert_eq!(
            stale,
            ResponseFrame::Error {
                error: ApiError::stale_version(0, 1)
            },
            "the client receives the code and the current version"
        );

        handle.shutdown();
    }

    #[test]
    fn unknown_fields_are_rejected_over_the_socket() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let handle = served(dir.path());

        let mut client = TestClient::connect(handle.socket_path());
        let response = client.command(json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
            "step": 1,
            "surprise": true,
        }));

        assert_eq!(
            response,
            ResponseFrame::Error {
                error: ApiError::unknown_field("surprise")
            }
        );

        handle.shutdown();
    }

    #[test]
    fn a_frame_without_an_operation_is_rejected() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let handle = served(dir.path());

        let mut client = TestClient::connect(handle.socket_path());
        client.send(&RequestFrame {
            kind: FrameKind::Query,
            operation: None,
            payload: Some(json!({})),
        });

        match client.recv() {
            ResponseFrame::Error { error } => {
                assert_eq!(error.code, ErrorCode::InvalidRequest);
            }
            other => panic!("an invalid request earns an error frame, got {other:?}"),
        }

        handle.shutdown();
    }

    #[test]
    fn a_malformed_line_is_answered_then_disconnected() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let handle = served(dir.path());

        let mut client = TestClient::connect(handle.socket_path());
        client.send_raw("this is not json");

        match client.recv() {
            ResponseFrame::Error { error } => {
                assert_eq!(error.code, ErrorCode::InvalidRequest);
            }
            other => panic!("a malformed line earns an error frame, got {other:?}"),
        }
        assert!(
            client.try_recv().is_none(),
            "the connection closes after a protocol breach"
        );

        handle.shutdown();
    }

    #[test]
    fn connection_is_refused_without_directory_permission() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let handle = served(dir.path());

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o000))
            .expect("the directory is closed");

        let refused = UnixStream::connect(handle.socket_path());
        assert!(
            matches!(
                refused,
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied
            ),
            "a client without search permission on the socket directory is refused"
        );

        // Restore so the tempdir can clean up and the server still
        // serves the authorised client.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .expect("the directory is restored");

        let mut client = TestClient::connect(handle.socket_path());
        client.command(bump(1, "key-1", 0));

        handle.shutdown();
    }

    #[test]
    fn a_stale_socket_is_replaced() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_path = dir.path().join(SOCKET_FILE_NAME);
        let crashed = UnixListener::bind(&socket_path).expect("the abandoned socket binds");
        drop(crashed);

        let handle = served(dir.path());
        let mut client = TestClient::connect(handle.socket_path());
        client.command(bump(1, "key-1", 0));

        handle.shutdown();
    }

    #[test]
    fn a_live_socket_refuses_a_second_server() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let handle = served(dir.path());

        let second = SocketServer::bind(dir.path());
        assert!(
            matches!(second, Err(TransportError::SocketInUse { .. })),
            "a second core must not take the socket from a live one"
        );

        let mut client = TestClient::connect(handle.socket_path());
        client.command(bump(1, "key-1", 0));

        handle.shutdown();
    }

    #[test]
    fn a_non_socket_file_blocks_binding() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_path = dir.path().join(SOCKET_FILE_NAME);
        std::fs::write(&socket_path, b"not a socket").expect("the file is written");

        let refused = SocketServer::bind(dir.path());
        assert!(
            matches!(refused, Err(TransportError::SocketPathOccupied { .. })),
            "the server must not delete a file it does not own"
        );
        assert!(socket_path.is_file(), "the file is untouched");

        drop(dir);
    }
}
