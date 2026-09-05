//! Client gate for the whole-session polling fallback (KAN-T41,
//! DR-HB-10): a per-Project ten-second cadence of full-session
//! captures, available to any Project that opts in and off for every
//! Project that has not.

mod support;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kanban_domain::HerdrSession;
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{
    DEFAULT_POLLING_FALLBACK_INTERVAL, PollingFallback, Reconciler, ReconciliationPlan,
    SessionClient, SessionMapping, StateDifference,
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn mapping() -> SessionMapping {
    SessionMapping::new(
        HerdrSession::named("kanban-main").expect("the name validates"),
        "/workspaces/kanban.seed",
        "kanban.seed",
    )
}

fn snapshot_of(state: Value) -> kanban_herdr::Snapshot {
    kanban_herdr::Snapshot {
        session: "kanban-main".to_owned(),
        product_workspace: "/workspaces/kanban.seed".to_owned(),
        herdr_workspace: "kanban.seed".to_owned(),
        state,
        captured_at: "2026-09-05T04:46:00Z".to_owned(),
    }
}

#[test]
fn polling_fallback_is_off_by_default() {
    assert!(
        !PollingFallback::default().is_enabled(),
        "the polling fallback is off unless a Project opts in (DR-HB-10)"
    );
    assert_eq!(
        ReconciliationPlan::default().effective_interval(),
        ReconciliationPlan::default().interval(),
        "a disabled fallback leaves the reconciliation cadence untouched"
    );
}

#[test]
fn polling_fallback_carries_the_ten_second_interval() {
    assert_eq!(
        DEFAULT_POLLING_FALLBACK_INTERVAL,
        Duration::from_secs(10),
        "the whole-session polling fallback polls every ten seconds (DR-HB-10)"
    );
    assert_eq!(
        PollingFallback::off().interval(),
        DEFAULT_POLLING_FALLBACK_INTERVAL
    );
}

#[test]
fn polling_fallback_tightens_the_cadence_when_enabled() {
    let plan = ReconciliationPlan::default()
        .with_fallback(PollingFallback::every(DEFAULT_POLLING_FALLBACK_INTERVAL));
    assert_eq!(plan.effective_interval(), Duration::from_secs(10));

    let plan = ReconciliationPlan::new(Duration::from_secs(5))
        .with_fallback(PollingFallback::every(DEFAULT_POLLING_FALLBACK_INTERVAL));
    assert_eq!(
        plan.effective_interval(),
        Duration::from_secs(5),
        "a fallback slower than reconciliation cannot slow the cadence down"
    );
}

#[test]
fn polling_fallback_stays_off_until_a_capture_is_due() {
    let reconciler = Reconciler::seeded_with(
        ReconciliationPlan::default(),
        &snapshot_of(json!({ "roles": [] })),
        at(0),
    );

    assert!(
        !reconciler.due(at(10)),
        "ten quiet seconds do not capture anything while the fallback is off"
    );
    assert!(
        !reconciler.due(at(299)),
        "nothing before the five-minute interval captures while the fallback is off"
    );
    assert!(reconciler.due(at(300)));
}

#[test]
fn polling_fallback_captures_the_whole_session_on_its_cadence() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let _fixture = ScriptedSession::bind(
        dir.path(),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_snapshot_states(vec![
            json!({ "roles": [] }),
            json!({ "roles": [{ "name": "implementer" }] }),
        ]),
    );
    let mut client =
        SessionClient::open(mapping(), dir.path()).expect("the session opens through its socket");
    let baseline = client.snapshot().expect("the baseline captures");
    let plan = ReconciliationPlan::new(Duration::from_secs(300))
        .with_fallback(PollingFallback::every(Duration::from_secs(10)));
    let mut reconciler = Reconciler::seeded_with(plan, &baseline, at(0));

    assert!(
        !reconciler.due(at(9)),
        "the fallback cadence is waited out like any other"
    );
    let difference = reconciler
        .reconcile(at(10), &mut client)
        .expect("the fallback captures through the socket")
        .expect("the whole session changed between captures");

    assert_eq!(
        difference.changes,
        vec![StateDifference::Changed {
            key: "roles".to_owned(),
            from: json!([]),
            to: json!([{ "name": "implementer" }]),
        }],
        "an enabled fallback polls the whole session and reports what it finds"
    );
    assert_eq!(
        reconciler.remaining_until(at(10)),
        Duration::from_secs(10),
        "each capture resets the fallback cadence"
    );
}
