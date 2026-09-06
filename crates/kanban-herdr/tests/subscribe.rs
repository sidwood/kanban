mod support;

use std::net::Shutdown;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use kanban_domain::HerdrSession;
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{HerdrError, SessionClient, SessionMapping, WaitRequest};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn subscribe_receives_scripted_push_events() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_events(vec![json!({
            "kind": "role.output",
            "role": "implementer",
            "text": "working"
        })]),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );
    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the session connects through its socket");

    client
        .subscribe()
        .expect("subscription starts on the socket");
    let event = client
        .read_event()
        .expect("push events arrive after subscribe");

    assert_eq!(event["kind"], json!("role.output"));
    assert_eq!(event["role"], json!("implementer"));
}

#[test]
fn wait_returns_the_scripted_result() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_wait(true, json!({ "role": "implementer" })),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );
    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the session connects through its socket");

    let (met, detail) = client
        .wait(WaitRequest {
            condition: "role.settled".to_owned(),
            timeout_ms: 500,
        })
        .expect("waiting is supported per session");

    assert!(met);
    assert_eq!(detail["role"], json!("implementer"));
}

#[test]
fn shutting_a_duplicate_socket_wakes_a_blocked_read() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default(),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );
    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the session connects through its socket");
    client
        .subscribe()
        .expect("subscription starts on the socket");
    let duplicate = client
        .duplicate_socket()
        .expect("the socket duplicates for its owner");

    let (wake, woke) = mpsc::channel();
    let blocked = thread::spawn(move || {
        let outcome = client.read_event().err();
        wake.send(outcome).expect("the reader reports waking");
    });
    thread::sleep(Duration::from_millis(100));
    let _ = duplicate.shutdown(Shutdown::Both);

    let woken = woke
        .recv_timeout(Duration::from_secs(2))
        .expect("shutting the duplicate down unblocks the read");
    assert_eq!(woken, Some(HerdrError::Disconnected));
    blocked.join().expect("the reader thread finishes");
}

#[test]
fn a_refused_subscription_reports_the_remote_error() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_subscribe_error("session is sealed"),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );
    let mut client = SessionClient::connect(mapping, dir.path())
        .expect("the session connects through its socket");

    let refusal = client
        .subscribe()
        .expect_err("a sealed session refuses subscriptions");

    assert_eq!(
        refusal,
        HerdrError::Remote {
            message: "session is sealed".to_owned()
        }
    );
}

#[test]
fn closing_after_events_ends_the_first_connection_only() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default()
            .with_events(vec![json!({ "kind": "role.output", "text": "working" })])
            .close_after_events(),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );
    let mut first =
        SessionClient::connect(mapping.clone(), dir.path()).expect("the first connection opens");
    first.subscribe().expect("the first subscription starts");
    first
        .read_event()
        .expect("the scripted event arrives before the close");
    assert_eq!(
        first.read_event().err(),
        Some(HerdrError::Disconnected),
        "the fixture closes the first connection after its events"
    );

    let mut second =
        SessionClient::connect(mapping, dir.path()).expect("a later connection still opens");
    second
        .subscribe()
        .expect("a later subscription still starts");
}

#[test]
fn a_hold_before_close_ends_every_connection() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().close_after_hold_every(Duration::from_millis(100)),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );
    let mut first =
        SessionClient::connect(mapping.clone(), dir.path()).expect("the first connection opens");
    first.subscribe().expect("the first subscription starts");
    assert_eq!(
        first.read_event_within(Duration::from_secs(2)).err(),
        Some(HerdrError::Disconnected),
        "the fixture closes the first connection after its hold"
    );

    let mut second = SessionClient::connect(mapping, dir.path()).expect("a later connection opens");
    second.subscribe().expect("a later subscription starts");
    assert_eq!(
        second.read_event_within(Duration::from_secs(2)).err(),
        Some(HerdrError::Disconnected),
        "the hold closes every later connection too, so a test can script reconnect after reconnect"
    );
}

/// Every connection can carry its own script, the final one
/// repeating: a test gives one connection a refusal, the next a held
/// subscription that closes, so an observer loop is scripted cycle by
/// cycle instead of one behaviour split across every connection
/// (KAN-T130-AC1).
#[test]
fn connection_scripts_serve_each_connection_in_order() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_connection_scripts(vec![
            SessionScript::default().with_subscribe_error("first connection is sealed"),
            SessionScript::default().close_after_hold_every(Duration::from_millis(100)),
        ]),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );

    // The refusal script keeps its connection open for further
    // requests, so the client must close it before the fixture's
    // sequential accept loop can serve the next script.
    {
        let mut first = SessionClient::connect(mapping.clone(), dir.path())
            .expect("the first connection opens");
        assert_eq!(
            first.subscribe().err(),
            Some(HerdrError::Remote {
                message: "first connection is sealed".to_owned()
            }),
            "the first connection serves the first script"
        );
    }

    let mut second =
        SessionClient::connect(mapping.clone(), dir.path()).expect("the second connection opens");
    second
        .subscribe()
        .expect("the second connection serves the second script");
    assert_eq!(
        second.read_event_within(Duration::from_secs(2)).err(),
        Some(HerdrError::Disconnected),
        "the second script's hold closes its own connection"
    );

    let mut third =
        SessionClient::connect(mapping, dir.path()).expect("the third connection opens");
    third
        .subscribe()
        .expect("the final script repeats for every later connection");
    assert_eq!(
        third.read_event_within(Duration::from_secs(2)).err(),
        Some(HerdrError::Disconnected),
        "the repeated final script closes after its hold, where the base script would hold the connection open"
    );
}

/// A stream that drops inside a bounded read must surface the drop:
/// restoring the request deadline afterwards touches a socket the
/// drop already killed, and that control failure must not stand in
/// for the disconnection the caller needs to see.
#[test]
fn a_windowed_read_reports_the_streams_disconnection() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().close_after_hold(Duration::from_millis(100)),
    );
    let mapping = SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    );
    let mut client = SessionClient::connect(mapping, dir.path()).expect("the session connects");
    client.subscribe().expect("the subscription starts");

    assert_eq!(
        client.read_event_within(Duration::from_secs(2)).err(),
        Some(HerdrError::Disconnected),
        "the dropped stream reports its disconnection, not the failed deadline restore"
    );
}
