//! The shell's socket client against a real in-process core, and its
//! channel lifecycle against a stub core that controls when bytes
//! arrive.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kanban_app::{
    CommandHandler, Core, EventSink, MemoryIdempotencyStore, OperationDescriptor, OperationKind,
    ParsedCommand, parse_payload,
};
use kanban_desktop_lib::core_link::{CoreLink, REQUEST_TIMEOUT, forward_events};
use kanban_dto::{ApiError, ErrorCode, EventEnvelope, MutationContext};
use kanban_transport::{ResponseFrame, SocketServer};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The same one-aggregate fixture the app and transport tests use,
/// so the link is proven against a command that streams events, plus
/// the health query the shell's typed command serves.
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
        name: "health.get",
        kind: OperationKind::Query,
        request_schema: "HealthQuery",
        response_schema: "HealthResponse",
        mcp_tool_name: "health_get",
        description: "Test fixture: the health query.",
    },
];

/// The test core's health answer.
const TEST_VERSION: &str = "0.1.0-link-test";

/// Serves the fixture's health query.
struct Health;

impl kanban_app::QueryHandler for Health {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        parse_payload::<kanban_dto::HealthQuery>(payload)?;
        Ok(json!({ "connected": true, "service_version": TEST_VERSION }))
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BumpRequest {
    /// Present only so the frame parses exactly as the real command
    /// payloads do; the fixture itself never reads it.
    #[expect(dead_code)]
    mutation: MutationContext,
    step: i64,
}

#[derive(Debug, Default)]
struct Counter {
    value: Mutex<i64>,
    version: Mutex<u64>,
}

impl CommandHandler for Counter {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<BumpRequest>(payload)?;
        ParsedCommand::lift("counter", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(*self.version.lock().expect("the version lock is sound"))
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: BumpRequest = parse_payload(&command.payload)?;
        let mut value = self.value.lock().expect("the value lock is sound");
        let mut version = self.version.lock().expect("the version lock is sound");
        *value += request.step;
        *version += 1;
        events.emit("counter.bumped", json!({ "to": *value }));
        Ok(json!({ "value": *value, "version": *version }))
    }
}

/// Serve a core with the counter command and health query.
fn served(dir: &TempDir) -> kanban_transport::ServerHandle {
    let server = SocketServer::bind(dir.path()).expect("the server binds");
    let broker = server.broker();
    let mut core = Core::new(
        TEST_CATALOG,
        Arc::new(MemoryIdempotencyStore::new()),
        broker,
    );
    core.register_command("counter.bump", Arc::new(Counter::default()))
        .expect("the test command registers");
    core.register_query("health.get", Arc::new(Health))
        .expect("the test query registers");
    server.serve(Arc::new(core)).expect("the server serves")
}

fn bump(step: i64, key: &str, version: u64) -> Value {
    json!({
        "mutation": { "optimistic_version": version, "idempotency_key": key },
        "step": step,
    })
}

/// A client that keeps a connection open without subscribing, so the
/// link's channels can be proven to stay request-shaped.
struct IdleClient {
    _stream: UnixStream,
}

impl IdleClient {
    fn connect(socket_path: &Path) -> Self {
        let stream = UnixStream::connect(socket_path).expect("the idle client connects");
        Self { _stream: stream }
    }
}

#[test]
fn queries_and_commands_round_trip_through_the_link() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let handle = served(&dir);
    let link = CoreLink::connect(handle.socket_path()).expect("the link connects");

    let health = link
        .query("health.get", &json!({}))
        .expect("the health query round trips");
    assert_eq!(health["connected"], json!(true));
    assert_eq!(health["service_version"], json!(TEST_VERSION));

    let response = link
        .command("counter.bump", &bump(2, "key-1", 0))
        .expect("the command round trips");
    assert_eq!(response, json!({ "value": 2, "version": 1 }));

    let _idle = IdleClient::connect(handle.socket_path());
    let again = link
        .command("counter.bump", &bump(3, "key-2", 1))
        .expect("the link keeps serving across other clients");
    assert_eq!(again, json!({ "value": 5, "version": 2 }));

    handle.shutdown();
}

#[test]
fn an_unknown_operation_surfaces_as_an_api_error() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let handle = served(&dir);
    let link = CoreLink::connect(handle.socket_path()).expect("the link connects");

    let refused = link.query("nope.get", &json!({}));
    assert!(
        matches!(
            &refused,
            Err(error) if error.code == ErrorCode::NotFound
        ),
        "unknown operations arrive as typed errors, got {refused:?}"
    );

    handle.shutdown();
}

#[test]
fn events_arrive_in_order_through_the_forwarder() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let handle = served(&dir);

    let seen: Arc<Mutex<Vec<EventEnvelope>>> = Arc::default();
    let collector = seen.clone();
    let forward = {
        let socket_path = handle.socket_path().to_owned();
        std::thread::spawn(move || {
            forward_events(&socket_path, move |envelope| {
                collector
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(envelope);
            })
        })
    };

    // Give the forwarder a moment to subscribe, then drive commands.
    std::thread::sleep(Duration::from_millis(200));
    let link = CoreLink::connect(handle.socket_path()).expect("the link connects");
    link.command("counter.bump", &bump(1, "key-1", 0))
        .expect("the first bump lands");
    link.command("counter.bump", &bump(1, "key-2", 1))
        .expect("the second bump lands");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let count = seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        if count >= 2 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the forwarder never delivered both events"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let seen = seen
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert_eq!(
        seen.iter().map(|event| event.sequence).collect::<Vec<_>>(),
        vec![1, 2],
        "events keep their global order"
    );
    assert_eq!(
        seen.iter()
            .map(|event| event.event_type.clone())
            .collect::<Vec<_>>(),
        vec!["counter.bumped".to_owned(), "counter.bumped".to_owned()],
        "the envelopes carry their types"
    );

    // Ending the server ends the forwarder cleanly.
    handle.shutdown();
    forward
        .join()
        .expect("the forwarder thread finishes")
        .expect("a closed socket ends the stream without an error");
}

#[test]
fn a_closed_socket_ends_the_forwarder_cleanly() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let handle = served(&dir);

    let socket_path = handle.socket_path().to_owned();
    let forward = std::thread::spawn(move || forward_events(&socket_path, |_| {}));
    std::thread::sleep(Duration::from_millis(200));
    handle.shutdown();

    forward
        .join()
        .expect("the forwarder thread finishes")
        .expect("shutdown ends the stream without an error");
}

/// What one stub connection answers, and when.
enum Reply {
    /// A well-formed answer, but only once the link's read timeout
    /// has already expired.
    Late(Value),
    /// A well-formed answer, at once.
    Prompt(Value),
    /// A line that is not a frame.
    Malformed,
    /// A frame only a subscribed connection may carry.
    Unexpected,
    /// No answer at all: the connection closes.
    EarlyClose,
}

/// A stub core on a real socket, answering each accepted connection
/// from a fixed script. The real core cannot be asked to answer late
/// or badly, and this fixture exists only to control when bytes
/// arrive; it holds no domain behaviour.
struct StubCore {
    socket_path: PathBuf,
    accepted: Arc<AtomicUsize>,
    delivered: Receiver<()>,
}

impl StubCore {
    /// Serve `script`: one reply per accepted connection, in order.
    /// The stub stops accepting once the script is spent.
    fn serve(dir: &TempDir, script: Vec<Reply>) -> Self {
        let socket_path = dir.path().join("core.sock");
        let listener = UnixListener::bind(&socket_path).expect("the stub core binds");
        let accepted = Arc::new(AtomicUsize::new(0));
        let counted = accepted.clone();
        let (sent, delivered) = channel();
        std::thread::spawn(move || {
            for reply in script {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                counted.fetch_add(1, Ordering::SeqCst);
                // One thread per connection: a late answer must not
                // hold up the reconnection that follows it.
                let sent = sent.clone();
                std::thread::spawn(move || answer(stream, reply, &sent));
            }
        });
        Self {
            socket_path,
            accepted,
            delivered,
        }
    }

    fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// How many connections the stub has accepted.
    fn connections(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// Block until the next scripted reply has been delivered, so a
    /// late answer is provably on the wire before the next request.
    fn await_delivery(&self) {
        self.delivered
            .recv_timeout(REQUEST_TIMEOUT * 4)
            .expect("the stub core delivered its reply");
    }
}

/// Read this connection's one request line, then reply as scripted.
fn answer(stream: UnixStream, reply: Reply, delivered: &Sender<()>) {
    let mut writer = stream.try_clone().expect("the stub core clones its stream");
    let mut reader = BufReader::new(stream);
    let mut request = String::new();
    if reader.read_line(&mut request).unwrap_or(0) == 0 {
        return;
    }
    let line = match reply {
        Reply::Late(payload) => {
            std::thread::sleep(REQUEST_TIMEOUT + Duration::from_millis(500));
            Some(frame(payload))
        }
        Reply::Prompt(payload) => Some(frame(payload)),
        Reply::Malformed => Some("this is not a frame".to_owned()),
        Reply::Unexpected => Some(
            serde_json::to_string(&ResponseFrame::Event {
                event: EventEnvelope {
                    sequence: 1,
                    event_type: "counter.bumped".to_owned(),
                    payload: json!({ "to": 1 }),
                },
            })
            .expect("the event frame encodes"),
        ),
        Reply::EarlyClose => None,
    };
    let Some(line) = line else {
        return;
    };
    // A late answer may land on a socket the link has already closed;
    // refusing that write is the fix working.
    let _ = writeln!(writer, "{line}").and_then(|_| writer.flush());
    let _ = delivered.send(());
    // A real core keeps the connection open after answering, so the
    // stub does too: a link that kept this channel would find the
    // answer above waiting, ready to be read as the next request's.
    let mut ignored = String::new();
    let _ = reader.read_line(&mut ignored);
}

/// Encode one well-formed response frame.
fn frame(payload: Value) -> String {
    serde_json::to_string(&ResponseFrame::Response { payload }).expect("the response frame encodes")
}

/// Drive one request against a stub whose first connection answers
/// badly and whose second answers well, and prove the second request
/// reconnected and read only its own answer.
fn reconnects_after(first: Reply) {
    let dir = TempDir::new().expect("a scratch directory is available");
    let stub = StubCore::serve(&dir, vec![first, Reply::Prompt(json!({ "value": 2 }))]);
    let link = CoreLink::connect(stub.socket_path()).expect("the link connects");

    let refused = link.query("health.get", &json!({}));
    assert!(
        refused.is_err(),
        "an uncertain answer fails the request, got {refused:?}"
    );

    let answered = link
        .query("health.get", &json!({}))
        .expect("the next request is answered");
    assert_eq!(
        answered,
        json!({ "value": 2 }),
        "the next request reads only its own answer"
    );
    assert_eq!(stub.connections(), 2, "the link reconnected exactly once");
}

#[test]
fn core_link_timeout_reconnect_never_delivers_a_late_answer() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let stub = StubCore::serve(
        &dir,
        vec![
            Reply::Late(json!({ "value": 1 })),
            Reply::Prompt(json!({ "value": 2 })),
        ],
    );
    let link = CoreLink::connect(stub.socket_path()).expect("the link connects");

    let timed_out = link.query("health.get", &json!({}));
    assert!(
        timed_out.is_err(),
        "an answer past the read timeout fails the request, got {timed_out:?}"
    );

    // Only now is the first answer on the wire: a channel the link
    // kept would have it queued and ready to be read as the next
    // request's answer.
    stub.await_delivery();

    let answered = link
        .query("health.get", &json!({}))
        .expect("the next request is answered");
    assert_eq!(
        answered,
        json!({ "value": 2 }),
        "the next request reads only its own answer"
    );
    assert_eq!(stub.connections(), 2, "the link reconnected exactly once");
}

#[test]
fn core_link_reconnects_after_a_malformed_answer() {
    reconnects_after(Reply::Malformed);
}

#[test]
fn core_link_reconnects_after_an_unexpected_frame() {
    reconnects_after(Reply::Unexpected);
}

#[test]
fn core_link_reconnects_after_an_early_close() {
    reconnects_after(Reply::EarlyClose);
}
