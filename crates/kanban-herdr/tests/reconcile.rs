//! Client gate for reconciliation (KAN-T41, DR-HB-09): full session
//! state is compared on a fixed cadence — every five minutes by
//! default — and every difference the comparison finds is reported,
//! so observation survives the push events a session missed.

mod support;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kanban_domain::HerdrSession;
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{
    DEFAULT_RECONCILIATION_INTERVAL, Reconciler, ReconciliationPlan, SessionClient, SessionMapping,
    Snapshot, StateDifference,
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn snapshot_of(state: Value) -> Snapshot {
    Snapshot {
        session: "kanban-main".to_owned(),
        product_workspace: "/workspaces/kanban.seed".to_owned(),
        herdr_workspace: "kanban.seed".to_owned(),
        state,
        captured_at: "2026-09-05T04:46:00Z".to_owned(),
    }
}

fn quiet_session(dir: &TempDir, states: Vec<Value>) -> ScriptedSession {
    ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_snapshot_states(states),
    )
}

fn mapping() -> SessionMapping {
    SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    )
}

#[test]
fn reconcile_defaults_to_five_minutes() {
    assert_eq!(
        DEFAULT_RECONCILIATION_INTERVAL,
        Duration::from_secs(5 * 60),
        "reconciliation compares full session state every five minutes by default (DR-HB-09)"
    );
    assert_eq!(
        ReconciliationPlan::default().interval(),
        DEFAULT_RECONCILIATION_INTERVAL
    );
}

#[test]
fn reconcile_is_due_once_its_interval_has_elapsed() {
    let reconciler = Reconciler::seeded_with(
        ReconciliationPlan::new(Duration::from_secs(300)),
        &snapshot_of(json!({ "roles": [] })),
        at(1_000),
    );

    assert!(!reconciler.due(at(1_299)), "one second early is not due");
    assert!(reconciler.due(at(1_300)), "the interval elapsing is due");
    assert!(
        !reconciler.due(at(1_000)),
        "a capture that already happened is not due again"
    );
}

#[test]
fn reconcile_without_a_baseline_is_due_immediately() {
    let reconciler = Reconciler::new(ReconciliationPlan::default());
    assert!(
        reconciler.due(at(0)),
        "a reconciler with no baseline must establish one before comparing"
    );
}

#[test]
fn reconcile_reports_the_difference_between_captures() {
    let mut reconciler = Reconciler::new(ReconciliationPlan::default());
    assert_eq!(
        reconciler.adopt(at(60), snapshot_of(json!({ "roles": [] }))),
        None,
        "the first capture is the baseline, not a difference"
    );

    let difference = reconciler
        .adopt(
            at(360),
            snapshot_of(json!({ "roles": [{ "name": "implementer" }] })),
        )
        .expect("changed full session state is a difference");

    assert_eq!(
        difference.changes,
        vec![StateDifference::Changed {
            key: "roles".to_owned(),
            from: json!({ "roles": [] })["roles"].clone(),
            to: json!([{ "name": "implementer" }]),
        }],
        "the difference names the state key that moved and both of its values"
    );
}

#[test]
fn reconcile_ignores_the_capture_stamp_when_comparing() {
    let mut reconciler = Reconciler::new(ReconciliationPlan::default());
    let mut recaptured = snapshot_of(json!({ "roles": [] }));
    recaptured.captured_at = "2026-09-05T04:51:00Z".to_owned();
    reconciler.adopt(at(60), snapshot_of(json!({ "roles": [] })));

    assert_eq!(
        reconciler.adopt(at(360), recaptured),
        None,
        "a recapture of identical state is not a difference, whatever its stamp"
    );
}

#[test]
fn reconcile_reports_added_and_removed_state_keys() {
    let mut reconciler = Reconciler::new(ReconciliationPlan::default());
    reconciler.adopt(
        at(60),
        snapshot_of(json!({ "roles": [], "locks": 1, "queue": "idle" })),
    );

    let difference = reconciler
        .adopt(
            at(360),
            snapshot_of(json!({ "roles": [], "agents": 2, "queue": "idle" })),
        )
        .expect("the state changed");

    assert_eq!(
        difference.changes,
        vec![
            StateDifference::Removed {
                key: "locks".to_owned(),
                value: json!(1),
            },
            StateDifference::Added {
                key: "agents".to_owned(),
                value: json!(2),
            },
        ],
        "keys Herdr stopped and started exposing are differences, untouched keys are not"
    );
}

#[test]
fn reconcile_compares_non_object_state_whole() {
    let mut reconciler = Reconciler::new(ReconciliationPlan::default());
    reconciler.adopt(at(60), snapshot_of(json!([1, 2, 3])));

    let difference = reconciler
        .adopt(at(360), snapshot_of(json!([1, 2])))
        .expect("the whole state changed");

    assert_eq!(
        difference.changes,
        vec![StateDifference::Changed {
            key: "state".to_owned(),
            from: json!([1, 2, 3]),
            to: json!([1, 2]),
        }],
        "state Herdr did not expose as an object is compared whole"
    );
}

#[test]
fn reconcile_captures_through_the_session_socket() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = quiet_session(
        &dir,
        vec![
            json!({ "roles": [] }),
            json!({ "roles": [{ "name": "implementer" }] }),
        ],
    );
    let mut client =
        SessionClient::open(mapping(), dir.path()).expect("the session opens through its socket");
    let baseline = client.snapshot().expect("the baseline captures");
    let mut reconciler = Reconciler::seeded_with(ReconciliationPlan::default(), &baseline, at(0));

    let difference = reconciler
        .reconcile(at(300), &mut client)
        .expect("reconciliation captures through the socket")
        .expect("the session state changed between captures");

    assert_eq!(difference.changes.len(), 1);
    assert_eq!(
        difference.changes[0],
        StateDifference::Changed {
            key: "roles".to_owned(),
            from: json!([]),
            to: json!([{ "name": "implementer" }]),
        }
    );
    assert!(
        !reconciler.due(at(300)),
        "the capture that just happened resets the cadence"
    );
}

#[test]
fn reconcile_captures_on_a_live_subscription() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default()
            .with_snapshot_states(vec![
                json!({ "roles": [] }),
                json!({ "roles": [{ "name": "reviewer" }] }),
            ])
            .with_events(vec![json!({
                "kind": "role.output",
                "role": "implementer",
                "text": "working"
            })]),
    );
    let mut client =
        SessionClient::open(mapping(), dir.path()).expect("the session opens through its socket");
    let baseline = client.snapshot().expect("the baseline captures");
    client.subscribe().expect("the subscription starts");
    let event = client.read_event().expect("the pushed event arrives");
    assert_eq!(event["kind"], json!("role.output"));

    let mut reconciler = Reconciler::seeded_with(ReconciliationPlan::default(), &baseline, at(0));
    let difference = reconciler
        .reconcile(at(300), &mut client)
        .expect("reconciliation captures beside an open subscription")
        .expect("the session state changed between captures");

    assert_eq!(
        difference.changes,
        vec![StateDifference::Changed {
            key: "roles".to_owned(),
            from: json!([]),
            to: json!([{ "name": "reviewer" }]),
        }],
        "a capture on the subscribed connection reports what the push stream missed"
    );
    assert_eq!(
        client.read_event_within(Duration::from_millis(50)),
        Err(kanban_herdr::HerdrError::TimedOut),
        "the subscription stays usable after a reconciliation capture"
    );
}
