//! Reconciliation (DR-HB-09): full session state is compared on a
//! fixed cadence — every five minutes by default — and every
//! difference the comparison finds is reported, so observation
//! survives the push events a session missed. A per-Project polling
//! fallback (DR-HB-10) tightens that cadence to whole-session
//! captures every ten seconds for Projects that opt in.

use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::Value;

use crate::client::SessionClient;
use crate::error::HerdrError;
use crate::protocol::Snapshot;

/// How often reconciliation compares full session state by default
/// (DR-HB-09): every five minutes.
pub const DEFAULT_RECONCILIATION_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The whole-session polling fallback's interval (DR-HB-10): ten
/// seconds.
pub const DEFAULT_POLLING_FALLBACK_INTERVAL: Duration = Duration::from_secs(10);

/// The per-Project whole-session polling fallback (DR-HB-10):
/// available to every Project, off until one opts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollingFallback {
    enabled: bool,
    interval: Duration,
}

impl PollingFallback {
    /// The disabled fallback: available, waiting for opt-in, and
    /// carrying the ten-second interval an opt-in will run.
    pub fn off() -> Self {
        Self {
            enabled: false,
            interval: DEFAULT_POLLING_FALLBACK_INTERVAL,
        }
    }

    /// An enabled fallback polling the whole session every `interval`.
    pub fn every(interval: Duration) -> Self {
        Self {
            enabled: true,
            interval,
        }
    }

    /// Whether the fallback polls.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The interval the fallback polls on.
    pub fn interval(&self) -> Duration {
        self.interval
    }
}

impl Default for PollingFallback {
    fn default() -> Self {
        Self::off()
    }
}

/// The whole-session comparison cadence one Project follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconciliationPlan {
    interval: Duration,
    fallback: PollingFallback,
}

impl ReconciliationPlan {
    /// A plan comparing every `interval`, fallback off.
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            fallback: PollingFallback::off(),
        }
    }

    /// The same plan with `fallback` deciding the whole-session
    /// cadence while enabled.
    pub fn with_fallback(mut self, fallback: PollingFallback) -> Self {
        self.fallback = fallback;
        self
    }

    /// The interval this plan compares on.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// The fallback this plan carries.
    pub fn fallback(&self) -> PollingFallback {
        self.fallback
    }

    /// The cadence the plan actually follows: the reconciliation
    /// interval, tightened to the polling fallback while one is
    /// enabled. A fallback slower than the reconciliation interval
    /// cannot slow the cadence down.
    pub fn effective_interval(&self) -> Duration {
        if self.fallback.is_enabled() {
            self.fallback.interval().min(self.interval)
        } else {
            self.interval
        }
    }
}

impl Default for ReconciliationPlan {
    fn default() -> Self {
        Self {
            interval: DEFAULT_RECONCILIATION_INTERVAL,
            fallback: PollingFallback::off(),
        }
    }
}

/// One difference between two captures of full session state.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StateDifference {
    /// A top-level state key Herdr started exposing.
    Added {
        /// The state key.
        key: String,
        /// The value it carries.
        value: Value,
    },
    /// A top-level state key Herdr stopped exposing.
    Removed {
        /// The state key.
        key: String,
        /// The value it carried.
        value: Value,
    },
    /// A top-level state key whose value moved.
    Changed {
        /// The state key.
        key: String,
        /// The value the previous capture held.
        from: Value,
        /// The value the current capture holds.
        to: Value,
    },
}

/// The full difference between the previous capture of session state
/// and the current one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotDifference {
    /// When the previous capture was taken, as Herdr reported it.
    pub previous_captured_at: String,
    /// When the current capture was taken, as Herdr reported it.
    pub captured_at: String,
    /// Every difference the comparison found, in comparison order.
    pub changes: Vec<StateDifference>,
}

/// Compare full session state (DR-HB-09): every top-level key of the
/// state Herdr exposed, deep-compared per key. Capture stamps move
/// with every capture and identity is verified by the session
/// mapping, so neither participates in the comparison. State Herdr
/// did not expose as an object is compared whole.
pub fn diff_state(previous: &Value, current: &Value) -> Vec<StateDifference> {
    let (Some(previous), Some(current)) = (previous.as_object(), current.as_object()) else {
        return if previous == current {
            Vec::new()
        } else {
            vec![StateDifference::Changed {
                key: "state".to_owned(),
                from: previous.clone(),
                to: current.clone(),
            }]
        };
    };
    let mut changes = Vec::new();
    for (key, from) in previous {
        match current.get(key) {
            None => changes.push(StateDifference::Removed {
                key: key.clone(),
                value: from.clone(),
            }),
            Some(to) if to != from => changes.push(StateDifference::Changed {
                key: key.clone(),
                from: from.clone(),
                to: to.clone(),
            }),
            Some(_) => {}
        }
    }
    for (key, value) in current {
        if !previous.contains_key(key) {
            changes.push(StateDifference::Added {
                key: key.clone(),
                value: value.clone(),
            });
        }
    }
    changes
}

/// Runs whole-session reconciliation for one observed session: the
/// cadence it follows, when it last captured, and the state baseline
/// each capture is compared against.
pub struct Reconciler {
    plan: ReconciliationPlan,
    last_capture: Option<SystemTime>,
    baseline: Option<Snapshot>,
}

impl Reconciler {
    /// A reconciler with no baseline, due for its first capture.
    pub fn new(plan: ReconciliationPlan) -> Self {
        Self {
            plan,
            last_capture: None,
            baseline: None,
        }
    }

    /// A reconciler whose baseline is an already-captured snapshot —
    /// the observer's startup or reconnect capture — taken at `at`.
    pub fn seeded_with(plan: ReconciliationPlan, snapshot: &Snapshot, at: SystemTime) -> Self {
        Self {
            plan,
            last_capture: Some(at),
            baseline: Some(snapshot.clone()),
        }
    }

    /// The plan this reconciler follows.
    pub fn plan(&self) -> ReconciliationPlan {
        self.plan
    }

    /// Follow `plan` from now on, keeping the baseline and the last
    /// capture: a settings change applies from the next capture the
    /// new cadence calls due.
    pub fn replan(&mut self, plan: ReconciliationPlan) {
        self.plan = plan;
    }

    /// Whether a whole-session capture is due at `now`: immediately
    /// when no baseline exists, and once the plan's effective cadence
    /// has elapsed since the last capture otherwise.
    pub fn due(&self, now: SystemTime) -> bool {
        self.remaining_until(now).is_zero()
    }

    /// How much longer until a whole-session capture is due at `now`,
    /// following the plan's effective cadence. A clock reading from
    /// before the last capture waits out a full interval rather than
    /// firing a capture per tick.
    pub fn remaining_until(&self, now: SystemTime) -> Duration {
        let Some(last) = self.last_capture else {
            return Duration::ZERO;
        };
        match now.duration_since(last) {
            Ok(elapsed) => self.plan.effective_interval().saturating_sub(elapsed),
            Err(_) => self.plan.effective_interval(),
        }
    }

    /// Capture full session state through `client`, compare it with
    /// the previous capture, and adopt it as the new baseline.
    /// Returns the difference, or `None` when nothing changed.
    pub fn reconcile(
        &mut self,
        now: SystemTime,
        client: &mut SessionClient,
    ) -> Result<Option<SnapshotDifference>, HerdrError> {
        let snapshot = client.snapshot()?;
        Ok(self.adopt(now, snapshot))
    }

    /// Adopt an already-captured snapshot as the new baseline and
    /// report its difference from the previous one. The first capture
    /// establishes the baseline: with nothing to compare against it
    /// reports no difference.
    pub fn adopt(&mut self, now: SystemTime, snapshot: Snapshot) -> Option<SnapshotDifference> {
        let difference = self.baseline.as_ref().map(|previous| {
            let changes = diff_state(&previous.state, &snapshot.state);
            (!changes.is_empty()).then(|| SnapshotDifference {
                previous_captured_at: previous.captured_at.clone(),
                captured_at: snapshot.captured_at.clone(),
                changes,
            })
        });
        self.baseline = Some(snapshot);
        self.last_capture = Some(now);
        difference.flatten()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde_json::{Value, json};

    use super::{Reconciler, ReconciliationPlan, StateDifference, diff_state};
    use crate::protocol::Snapshot;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn snapshot(state: Value) -> Snapshot {
        Snapshot {
            session: "kanban-main".to_owned(),
            product_workspace: "/workspaces/kanban.seed".to_owned(),
            herdr_workspace: "kanban.seed".to_owned(),
            state,
            captured_at: "2026-09-05T04:46:00Z".to_owned(),
        }
    }

    #[test]
    fn diff_state_orders_changes_previous_keys_first() {
        let changes = diff_state(
            &json!({ "a": 1, "b": 2, "c": 3 }),
            &json!({ "b": 2, "c": 4, "d": 5 }),
        );
        let keys: Vec<&str> = changes
            .iter()
            .map(|change| match change {
                StateDifference::Added { key, .. }
                | StateDifference::Removed { key, .. }
                | StateDifference::Changed { key, .. } => key,
            })
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["a", "c", "d"]);
    }

    #[test]
    fn a_reconciler_without_a_capture_waits_nothing() {
        let reconciler = Reconciler::new(ReconciliationPlan::new(Duration::from_secs(300)));
        assert!(reconciler.due(at(0)));
        assert_eq!(reconciler.remaining_until(at(0)), Duration::ZERO);
    }

    #[test]
    fn a_clock_behind_the_last_capture_waits_a_full_interval() {
        let mut reconciler = Reconciler::new(ReconciliationPlan::new(Duration::from_secs(300)));
        reconciler.adopt(at(1_000), snapshot(json!({ "roles": [] })));
        assert_eq!(
            reconciler.remaining_until(at(500)),
            Duration::from_secs(300),
            "a clock reading from before the last capture must not fire a capture per tick"
        );
        assert!(!reconciler.due(at(500)));
    }

    #[test]
    fn replan_keeps_the_baseline_and_applies_the_new_cadence() {
        let mut reconciler = Reconciler::seeded_with(
            ReconciliationPlan::new(Duration::from_secs(300)),
            &snapshot(json!({ "roles": [] })),
            at(0),
        );
        assert!(!reconciler.due(at(10)));

        reconciler.replan(
            ReconciliationPlan::new(Duration::from_secs(300))
                .with_fallback(super::PollingFallback::every(Duration::from_secs(10))),
        );

        assert!(
            reconciler.due(at(10)),
            "a plan with a shorter effective cadence applies from the last capture"
        );
        let difference = reconciler
            .adopt(
                at(10),
                snapshot(json!({ "roles": [{ "name": "implementer" }] })),
            )
            .expect("the baseline survives the replan");
        assert_eq!(difference.changes.len(), 1);
    }
}
