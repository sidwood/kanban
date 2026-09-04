//! The shell's socket client against a real in-process core.

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kanban_app::{
    CommandHandler, Core, EventSink, MemoryIdempotencyStore, OperationDescriptor, OperationKind,
    ParsedCommand, parse_payload,
};
use kanban_desktop_lib::core_link::{CoreLink, forward_events};
use kanban_dto::{ApiError, ErrorCode, EventEnvelope, MutationContext};
use kanban_transport::SocketServer;
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
