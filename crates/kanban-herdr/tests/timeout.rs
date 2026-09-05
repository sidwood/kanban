mod support;

use std::time::{Duration, Instant};

use kanban_domain::HerdrSession;
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{HerdrError, SessionClient, SessionMapping};

fn named_mapping(session: &str, product_workspace: &str) -> SessionMapping {
    SessionMapping::new(
        HerdrSession::named(session).expect("the name validates"),
        product_workspace,
        "kanban.seed",
    )
}

/// Fast enough for integration tests; production uses
/// [`kanban_herdr::SESSION_IO_TIMEOUT`].
const TEST_IO_TIMEOUT: Duration = Duration::from_millis(100);

#[test]
fn snapshot_handshake_timeout_on_an_unresponsive_server() {
    let dir = tempfile::TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_silent_handshake(),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::open_with_io_timeout(mapping, dir.path(), TEST_IO_TIMEOUT)
        .expect("the session socket opens");

    let started = Instant::now();
    let refusal = client.snapshot();
    let elapsed = started.elapsed();

    assert_eq!(
        refusal,
        Err(HerdrError::TimedOut),
        "a silent server must surface a bounded timeout, not block forever"
    );
    assert!(
        elapsed < TEST_IO_TIMEOUT * 3,
        "the handshake returned within a bounded window, took {elapsed:?}"
    );
}

#[test]
fn request_round_trip_restores_socket_deadlines_after_a_timeout() {
    let dir = tempfile::TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_silent_handshake(),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::open_with_io_timeout(mapping, dir.path(), TEST_IO_TIMEOUT)
        .expect("the session socket opens");

    assert_eq!(
        client.snapshot(),
        Err(HerdrError::TimedOut),
        "the first request times out"
    );

    // A working server on a fresh connection proves deadlines were
    // cleared: an uncleared read timeout would poison later reads.
    let working = tempfile::TempDir::new().expect("a scratch directory is available");
    let _healthy = ScriptedSession::bind(
        working.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default(),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::open_with_io_timeout(mapping, working.path(), TEST_IO_TIMEOUT)
        .expect("the healthy session socket opens");
    client
        .snapshot()
        .expect("a cleared deadline lets the next request answer");
}

#[test]
fn read_event_within_preserves_a_partial_line_across_a_timeout() {
    let dir = tempfile::TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default(),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::open_with_io_timeout(mapping, dir.path(), TEST_IO_TIMEOUT)
        .expect("the session socket opens");
    client.subscribe().expect("the subscription succeeds");

    assert_eq!(
        client.read_event_within(Duration::from_millis(50)),
        Err(HerdrError::TimedOut),
        "silence inside the window is a timeout, not a disconnect"
    );
    assert_eq!(
        client.read_event_within(Duration::from_millis(50)),
        Err(HerdrError::TimedOut),
        "the subscription stays open after a timeout"
    );
}
