//! App gate for deadline detection (KAN-T41, KAN-S8-US4): stall and
//! missing-result deadlines emit attention signals when breached —
//! observation only, so a signal carries facts, never a verdict or a
//! workflow transition.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kanban_app::deadlines::{
    DeadlineConfig, DeadlineMonitor, MISSING_RESULT_DEADLINE_REASON, STALL_DEADLINE_REASON,
};
use kanban_app::telemetry::AttentionSignal;
use kanban_dto::{HerdrGlobalDefaults, HerdrProjectSettings};
use serde_json::{Value, json};

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn deadlines(stall_secs: u64, missing_result_secs: u64) -> DeadlineMonitor {
    DeadlineMonitor::new(DeadlineConfig::from_secs(stall_secs, missing_result_secs))
}

fn output(role: &str) -> Value {
    json!({ "kind": "role.output", "role": role, "text": "working" })
}

fn settled(role: &str) -> Value {
    json!({ "kind": "role.settled", "role": role })
}

fn result(role: &str) -> Value {
    json!({ "kind": "role.result", "role": role, "outcome": "done" })
}

fn exited(role: &str) -> Value {
    json!({ "kind": "role.exited", "role": role, "exit_code": 0 })
}

fn project_settings(stall_secs: u64, missing_result_secs: u64) -> HerdrProjectSettings {
    HerdrProjectSettings {
        reconciliation_interval_secs: 300,
        polling_fallback_enabled: false,
        polling_fallback_interval_secs: 10,
        stall_deadline_secs: stall_secs,
        missing_result_deadline_secs: missing_result_secs,
        version: 1,
    }
}

#[test]
fn deadline_signals_take_their_deadlines_from_project_settings() {
    let from_settings = DeadlineConfig::from(&project_settings(1_800, 3_600));
    assert_eq!(from_settings.stall(), Duration::from_secs(1_800));
    assert_eq!(from_settings.missing_result(), Duration::from_secs(3_600));

    let from_defaults = DeadlineConfig::from(&HerdrGlobalDefaults {
        reconciliation_interval_secs: 300,
        stall_deadline_secs: 60,
        missing_result_deadline_secs: 120,
        version: 1,
    });
    assert_eq!(from_defaults.stall(), Duration::from_secs(60));
    assert_eq!(from_defaults.missing_result(), Duration::from_secs(120));
}

#[test]
fn deadline_signals_emit_attention_when_a_role_stalls() {
    let mut monitor = deadlines(3_600, 7_200);
    monitor.observe_event(
        at(0),
        &json!({ "kind": "role.opened", "role": "implementer" }),
    );

    assert!(
        monitor.evaluate(7, at(3_599)).is_empty(),
        "quiet work inside the stall deadline raises nothing"
    );

    let signals = monitor.evaluate(7, at(3_600));
    assert_eq!(
        signals,
        vec![AttentionSignal {
            project_id: 7,
            reason: STALL_DEADLINE_REASON.to_owned(),
            detail: json!({
                "deadline": "stall",
                "role": "implementer",
                "deadline_secs": 3_600,
                "breached_after_secs": 3_600,
                "last_activity_unix_secs": 0,
            }),
        }],
        "a role quiet for the whole stall deadline raises exactly one stall signal"
    );
}

#[test]
fn deadline_signals_stay_silent_while_work_continues() {
    let mut monitor = deadlines(600, 7_200);
    monitor.observe_event(at(0), &output("implementer"));
    monitor.observe_event(at(400), &output("implementer"));
    monitor.observe_event(at(900), &output("implementer"));

    assert!(
        monitor.evaluate(1, at(900 + 599)).is_empty(),
        "ongoing output keeps pushing the stall deadline back"
    );
}

#[test]
fn deadline_signals_emit_attention_when_a_settled_role_misses_its_result() {
    let mut monitor = deadlines(600, 1_200);
    monitor.observe_event(at(0), &output("implementer"));
    monitor.observe_event(at(100), &settled("implementer"));

    assert!(
        monitor.evaluate(3, at(100 + 1_199)).is_empty(),
        "a settled role inside its missing-result deadline raises nothing"
    );

    let signals = monitor.evaluate(3, at(100 + 1_200));
    assert_eq!(
        signals,
        vec![AttentionSignal {
            project_id: 3,
            reason: MISSING_RESULT_DEADLINE_REASON.to_owned(),
            detail: json!({
                "deadline": "missing_result",
                "role": "implementer",
                "deadline_secs": 1_200,
                "breached_after_secs": 1_200,
                "settled_unix_secs": 100,
            }),
        }],
        "a settled role whose result never arrived raises exactly one missing-result signal"
    );
}

#[test]
fn deadline_signals_never_stall_a_settled_role() {
    let mut monitor = deadlines(600, 1_200);
    monitor.observe_event(at(0), &output("implementer"));
    monitor.observe_event(at(100), &settled("implementer"));

    let signals = monitor.evaluate(3, at(100 + 1_200));
    assert_eq!(signals.len(), 1);
    assert_eq!(
        signals[0].reason, MISSING_RESULT_DEADLINE_REASON,
        "a settled role faces the missing-result deadline, not the stall deadline"
    );
}

#[test]
fn deadline_signals_clear_once_a_result_is_observed() {
    let mut monitor = deadlines(600, 1_200);
    monitor.observe_event(at(0), &output("implementer"));
    monitor.observe_event(at(100), &settled("implementer"));
    monitor.observe_event(at(300), &result("implementer"));

    assert!(
        monitor.evaluate(5, at(300 + 100_000)).is_empty(),
        "an observed result retires the missing-result deadline for good"
    );
}

#[test]
fn deadline_signals_stop_evaluating_exited_roles() {
    let mut monitor = deadlines(600, 1_200);
    monitor.observe_event(at(0), &output("implementer"));
    monitor.observe_event(at(30), &exited("implementer"));

    assert!(
        monitor.evaluate(5, at(30 + 100_000)).is_empty(),
        "a role whose tab exited is no longer anybody's deadline"
    );
}

#[test]
fn deadline_signals_treat_unrecognised_role_events_as_activity() {
    let mut monitor = deadlines(600, 1_200);
    monitor.observe_event(at(0), &output("implementer"));
    monitor.observe_event(
        at(500),
        &json!({ "kind": "role.mumbling", "role": "implementer" }),
    );

    assert!(
        monitor.evaluate(2, at(500 + 599)).is_empty(),
        "a kind Kanban has never seen still proves the role is alive"
    );
    assert_eq!(
        monitor.evaluate(2, at(500 + 600)).len(),
        1,
        "the stall deadline runs from the last sign of life, known kind or not"
    );
}

#[test]
fn deadline_signals_ignore_events_without_a_role() {
    let mut monitor = deadlines(600, 1_200);
    monitor.observe_event(at(0), &json!({ "kind": "session.disconnected" }));
    monitor.observe_event(at(0), &json!({ "kind": "role.opened", "role": "" }));
    monitor.observe_event(at(0), &json!({ "kind": "role.opened", "role": 7 }));

    assert!(
        monitor.evaluate(9, at(100_000)).is_empty(),
        "session-level events and role-less frames raise nothing"
    );
}

#[test]
fn deadline_signals_report_each_breached_role_once_in_role_order() {
    let mut monitor = deadlines(600, 1_200);
    monitor.observe_event(at(0), &output("reviewer"));
    monitor.observe_event(at(0), &output("implementer"));

    let signals = monitor.evaluate(4, at(600));
    let roles: Vec<&str> = signals
        .iter()
        .map(|signal| signal.detail["role"].as_str().expect("the role is named"))
        .collect();
    assert_eq!(
        roles,
        vec!["implementer", "reviewer"],
        "each breached role raises one signal, in a stable order"
    );
}

#[test]
fn deadline_signals_retune_without_forgetting_observed_roles() {
    let mut monitor = deadlines(3_600, 7_200);
    monitor.observe_event(at(0), &output("implementer"));

    monitor.retune(DeadlineConfig::from_secs(600, 1_200));

    assert_eq!(monitor.config(), DeadlineConfig::from_secs(600, 1_200));
    assert!(
        monitor.evaluate(2, at(600)).len() == 1,
        "a role observed before the retune faces the tightened stall deadline"
    );
}

#[test]
fn deadline_signals_default_to_the_global_deadlines() {
    let defaults = DeadlineConfig::default();
    assert_eq!(defaults.stall(), Duration::from_secs(3_600));
    assert_eq!(defaults.missing_result(), Duration::from_secs(7_200));
}
