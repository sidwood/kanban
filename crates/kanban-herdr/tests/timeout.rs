mod support;

use std::time::{Duration, Instant};

use kanban_domain::HerdrSession;
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{HerdrError, HerdrResponse, PromptRequest, SessionClient, SessionMapping};

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

/// How far apart a stalling peer places the pieces of its answer.
/// A `thread::sleep` on this platform can overshoot its request by
/// well over a tenth of a second, so the gap is chosen small and the
/// window it must fit inside generously: what the test needs is that
/// every single read is answered in time, not that the pacing is
/// exact.
const PACE: Duration = Duration::from_millis(50);

/// The request deadline the paced scenarios give one round trip.
/// Several times the worst observed pacing gap, so no individual
/// read is what ends the request.
const PACED_DEADLINE: Duration = Duration::from_millis(500);

/// How many pieces the stalling peer sends. Bounded and finite: the
/// fixture stops on its own, and even at the pacing's exact lower
/// bound the stall it scripts outlasts the deadline.
const PIECES: usize = 16;

/// KAN-T142-AC2: one request, one deadline. A peer that keeps
/// feeding complete event frames answers every individual read well
/// inside its window, so only a deadline over the whole round trip
/// can stop it holding the request — and everything queued behind
/// it — open.
#[test]
fn a_request_deadline_covers_the_event_frames_that_arrive_before_its_answer() {
    let dir = tempfile::TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default()
            .with_prompt_accepted(true)
            .with_paced_prompt(PIECES, 1, PACE),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::open_with_io_timeout(mapping, dir.path(), PACED_DEADLINE)
        .expect("the session socket opens");
    client.snapshot().expect("the handshake answers");
    client.subscribe().expect("the subscription answers");

    let started = Instant::now();
    let answered = client.prompt(PromptRequest {
        role: "coordinator".to_owned(),
        message: "resume once".to_owned(),
    });
    let elapsed = started.elapsed();

    assert_eq!(
        answered,
        Err(HerdrError::TimedOut),
        "a request that outlives its deadline fails, however timely each read was"
    );
    assert!(
        elapsed < PACED_DEADLINE * 3,
        "the request returned inside its own deadline, took {elapsed:?}"
    );
}

/// KAN-T142-AC2: a response line arriving one fragment at a time
/// spends that same one deadline. Each fragment is timely; the line
/// they build is not.
#[test]
fn a_request_deadline_covers_a_response_that_arrives_in_fragments() {
    let dir = tempfile::TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default()
            .with_prompt_accepted(true)
            .with_paced_prompt(0, PIECES, PACE),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::open_with_io_timeout(mapping, dir.path(), PACED_DEADLINE)
        .expect("the session socket opens");
    client.snapshot().expect("the handshake answers");

    let started = Instant::now();
    let answered = client.prompt(PromptRequest {
        role: "coordinator".to_owned(),
        message: "resume once".to_owned(),
    });
    let elapsed = started.elapsed();

    assert_eq!(
        answered,
        Err(HerdrError::TimedOut),
        "a partly arrived answer cannot outlive the request's deadline"
    );
    assert!(
        elapsed < PACED_DEADLINE * 3,
        "the request returned inside its own deadline, took {elapsed:?}"
    );
}

/// KAN-T142-AC2: an answer that arrives after its own request gave
/// up must never be read as the next request's. A prompt result is
/// an acknowledgement of delivery, so a stale one satisfying a later
/// prompt would record a delivery nobody accepted.
#[test]
fn a_late_answer_cannot_satisfy_the_request_that_follows_a_timeout() {
    let dir = tempfile::TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default()
            .with_prompt_accepted(true)
            .with_paced_prompt(0, 1, TEST_IO_TIMEOUT * 3),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::open_with_io_timeout(mapping, dir.path(), TEST_IO_TIMEOUT)
        .expect("the session socket opens");
    client.snapshot().expect("the handshake answers");

    assert_eq!(
        client.prompt(PromptRequest {
            role: "coordinator".to_owned(),
            message: "resume recovery one".to_owned(),
        }),
        Err(HerdrError::TimedOut),
        "the first prompt gives up before its answer arrives"
    );

    let second = client.prompt(PromptRequest {
        role: "coordinator".to_owned(),
        message: "resume recovery two".to_owned(),
    });

    assert_eq!(
        second,
        Err(HerdrError::Disconnected),
        "a connection with an answer still in flight carries no further request"
    );
}

#[test]
fn an_incompatible_frame_abandons_a_subscribed_request_connection() {
    let dir = tempfile::TempDir::new().expect("a scratch directory is available");
    let fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default()
            .with_prompt_accepted(true)
            .with_incompatible_prompt_frame(
                serde_json::json!({ "sequence": 1 }),
                TEST_IO_TIMEOUT * 3,
            ),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::open_with_io_timeout(mapping, dir.path(), TEST_IO_TIMEOUT * 10)
        .expect("the session socket opens");
    client.snapshot().expect("the handshake answers");
    client.subscribe().expect("the subscription answers");

    let rejected = client.prompt(PromptRequest {
        role: "coordinator".to_owned(),
        message: "resume recovery one".to_owned(),
    });
    let queued = client
        .read_event()
        .expect("the valid queued event survives");
    let second = client.prompt(PromptRequest {
        role: "coordinator".to_owned(),
        message: "resume recovery two".to_owned(),
    });
    let third = client.snapshot().map(|_| true);

    assert!(
        matches!(rejected, Err(HerdrError::Decode(_))),
        "the incompatible event version is rejected: {rejected:?}"
    );
    assert_eq!(queued, serde_json::json!({ "sequence": 1 }));
    assert_eq!(second, Err(HerdrError::Disconnected));
    assert_eq!(third, Err(HerdrError::Disconnected));
    let late_answer_bound = Instant::now() + Duration::from_secs(2);
    while fixture.late_prompt_answers_attempted() == 0 && Instant::now() < late_answer_bound {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        fixture.late_prompt_answers_attempted(),
        1,
        "the original prompt answer was attempted after the mismatch"
    );
}

#[test]
fn an_unexpected_response_kind_abandons_the_request_connection() {
    let dir = tempfile::TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_prompt_response(HerdrResponse::WakeResult { accepted: true }),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client =
        SessionClient::open_with_io_timeout(mapping, dir.path(), Duration::from_secs(5))
            .expect("the session socket opens");
    client.snapshot().expect("the handshake answers");

    let rejected = client.prompt(PromptRequest {
        role: "coordinator".to_owned(),
        message: "resume recovery one".to_owned(),
    });
    let second = client.prompt(PromptRequest {
        role: "coordinator".to_owned(),
        message: "resume recovery two".to_owned(),
    });
    let third = client.snapshot().map(|_| true);

    assert!(
        matches!(rejected, Err(HerdrError::Decode(_))),
        "the incompatible response kind is rejected: {rejected:?}"
    );
    assert_eq!(second, Err(HerdrError::Disconnected));
    assert_eq!(third, Err(HerdrError::Disconnected));
}

#[test]
fn a_buffered_response_that_decodes_past_the_deadline_times_out_and_abandons() {
    let dir = tempfile::TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_prompt_accepted(true),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::open_with_io_timeout(mapping, dir.path(), TEST_IO_TIMEOUT)
        .expect("the session socket opens");
    client.snapshot().expect("the handshake answers");
    client.buffer_response_for_test(
        HerdrResponse::PromptResult { accepted: true },
        TEST_IO_TIMEOUT * 2,
    );

    let answered = client.prompt(PromptRequest {
        role: "coordinator".to_owned(),
        message: "resume recovery one".to_owned(),
    });
    let later = client.prompt(PromptRequest {
        role: "coordinator".to_owned(),
        message: "resume recovery two".to_owned(),
    });

    assert_eq!(
        answered,
        Err(HerdrError::TimedOut),
        "decode and return spend the same absolute request deadline"
    );
    assert_eq!(
        later,
        Err(HerdrError::Disconnected),
        "an answer decoded after expiry cannot leave the socket reusable"
    );
}

#[test]
fn an_oversized_response_frame_is_rejected_and_abandons() {
    const MAX_RESPONSE_FRAME_BYTES: usize = 4 * 1024 * 1024;

    let dir = tempfile::TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_prompt_accepted(true),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client =
        SessionClient::open_with_io_timeout(mapping, dir.path(), Duration::from_secs(5))
            .expect("the session socket opens");
    client.snapshot().expect("the handshake answers");
    let mut frame = format!(
        r#"{{"kind":"prompt_result","accepted":true,"padding":"{}"}}"#,
        "x".repeat(MAX_RESPONSE_FRAME_BYTES)
    );
    frame.push('\n');
    client.buffer_raw_response_for_test(frame.as_bytes());

    let rejected = client.prompt(PromptRequest {
        role: "coordinator".to_owned(),
        message: "resume recovery one".to_owned(),
    });
    let later = client.prompt(PromptRequest {
        role: "coordinator".to_owned(),
        message: "resume recovery two".to_owned(),
    });

    assert!(
        matches!(
            rejected,
            Err(HerdrError::Decode(ref message)) if message.contains("exceeds")
        ),
        "the oversized frame is refused before JSON decode: {rejected:?}"
    );
    assert_eq!(later, Err(HerdrError::Disconnected));
}

#[test]
fn a_paced_prompt_uses_the_requested_non_empty_fragment_count() {
    const FRAGMENTS: usize = 40;

    let dir = tempfile::TempDir::new().expect("a scratch directory is available");
    let fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default()
            .with_prompt_accepted(true)
            .with_paced_prompt(0, FRAGMENTS, Duration::from_millis(1)),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client =
        SessionClient::open_with_io_timeout(mapping, dir.path(), Duration::from_secs(5))
            .expect("the session socket opens");
    client.snapshot().expect("the handshake answers");

    assert_eq!(
        client.prompt(PromptRequest {
            role: "coordinator".to_owned(),
            message: "resume recovery once".to_owned(),
        }),
        Ok(true)
    );
    assert_eq!(
        fixture.paced_prompt_writes(),
        FRAGMENTS,
        "the requested feasible fragment count is the actual non-empty write count"
    );
}

/// DR-HB-11, KAN-T78-AC2: the subscription is not a request. Its
/// caller's window is the only bound on it, so a quiet session that
/// pushes an event long after a request would have expired is still
/// observed rather than torn down.
#[test]
fn a_subscription_waits_past_the_request_deadline_for_its_next_event() {
    let dir = tempfile::TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default()
            .with_events(vec![serde_json::json!({ "role": "implementer" })])
            .with_delayed_events(TEST_IO_TIMEOUT * 3),
    );
    let mapping = named_mapping("kanban-main", "/workspaces/kanban.seed");
    let mut client = SessionClient::open_with_io_timeout(mapping, dir.path(), TEST_IO_TIMEOUT)
        .expect("the session socket opens");
    client.subscribe().expect("the subscription answers");

    let started = Instant::now();
    let event = client.read_event_within(TEST_IO_TIMEOUT * 20);
    let elapsed = started.elapsed();

    assert!(
        event.is_ok(),
        "the observation stream keeps waiting for its window: {event:?}"
    );
    assert!(
        elapsed > TEST_IO_TIMEOUT,
        "the event arrived past one request deadline, at {elapsed:?}"
    );
}
