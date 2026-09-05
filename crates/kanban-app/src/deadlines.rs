//! Stall and missing-result deadlines (KAN-S8-US4, DR-HB-11): a
//! Project's configured deadlines turn observed role activity into
//! attention signals when breached. Evaluation is a projection —
//! signals carry facts about observation, never a verdict or a
//! workflow transition (DR-HB-04) — and the consumer that persists
//! them as Attention items lands in KAN-S11.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use kanban_dto::{HerdrGlobalDefaults, HerdrProjectSettings};
use serde_json::{Value, json};

use crate::telemetry::AttentionSignal;

/// The reason a stall deadline carries (KAN-T41-AC3).
pub const STALL_DEADLINE_REASON: &str = "stall_deadline_breached";

/// The reason a missing-result deadline carries (KAN-T41-AC3).
pub const MISSING_RESULT_DEADLINE_REASON: &str = "missing_result_deadline_breached";

/// The deadlines one Project faces: per-Project settings with global
/// defaults (KAN-S8 implementation decisions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadlineConfig {
    stall: Duration,
    missing_result: Duration,
}

impl DeadlineConfig {
    /// Deadlines given directly in seconds.
    pub fn from_secs(stall_secs: u64, missing_result_secs: u64) -> Self {
        Self {
            stall: Duration::from_secs(stall_secs),
            missing_result: Duration::from_secs(missing_result_secs),
        }
    }

    /// After this much quiet a working role is stalled.
    pub fn stall(&self) -> Duration {
        self.stall
    }

    /// After this much time without a result a settled role is
    /// result-missing.
    pub fn missing_result(&self) -> Duration {
        self.missing_result
    }
}

impl From<&HerdrProjectSettings> for DeadlineConfig {
    fn from(settings: &HerdrProjectSettings) -> Self {
        Self::from_secs(
            settings.stall_deadline_secs,
            settings.missing_result_deadline_secs,
        )
    }
}

impl From<&HerdrGlobalDefaults> for DeadlineConfig {
    fn from(defaults: &HerdrGlobalDefaults) -> Self {
        Self::from_secs(
            defaults.stall_deadline_secs,
            defaults.missing_result_deadline_secs,
        )
    }
}

impl Default for DeadlineConfig {
    /// The global defaults (KAN-S8-US4): an hour of quiet before a
    /// working role is stalled, two hours without a result before a
    /// settled one is result-missing.
    fn default() -> Self {
        Self::from_secs(3_600, 7_200)
    }
}

/// What one observed role's deadlines hang on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RoleDeadlines {
    last_activity: Option<SystemTime>,
    settled_at: Option<SystemTime>,
    result_at: Option<SystemTime>,
    exited_at: Option<SystemTime>,
}

/// Watches one Project's observed session and emits attention
/// signals as its deadlines breach (KAN-T41-AC3).
///
/// Events arrive as observed push telemetry with the observation
/// time as a value; kinds Kanban does not recognise still count as
/// activity, so no unseen Herdr event can fake a stall. A role that
/// settled faces the missing-result deadline instead of the stall
/// deadline, a role whose result arrived faces neither, and a role
/// whose tab exited faces nothing. Evaluation is pure: a breach is
/// reported on every `evaluate` while it holds, and deduplication
/// into Attention items belongs to the KAN-S11 consumer.
pub struct DeadlineMonitor {
    config: DeadlineConfig,
    roles: HashMap<String, RoleDeadlines>,
}

impl DeadlineMonitor {
    /// A monitor enforcing `config`.
    pub fn new(config: DeadlineConfig) -> Self {
        Self {
            config,
            roles: HashMap::new(),
        }
    }

    /// The deadlines this monitor enforces.
    pub fn config(&self) -> DeadlineConfig {
        self.config
    }

    /// Enforce `config` from now on, keeping every observed role:
    /// settings changes apply to the live observation without a
    /// reconnect, and the roles already watched stay watched.
    pub fn retune(&mut self, config: DeadlineConfig) {
        self.config = config;
    }

    /// Observe one push event at `at`. Events without a named role —
    /// session-level telemetry or malformed frames — change nothing.
    pub fn observe_event(&mut self, at: SystemTime, event: &Value) {
        let Some(role) = event.get("role").and_then(Value::as_str) else {
            return;
        };
        if role.is_empty() {
            return;
        }
        let kind = event
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let entry = self.roles.entry(role.to_owned()).or_default();
        entry.last_activity = Some(at);
        match kind {
            "role.settled" => entry.settled_at = Some(at),
            "role.result" => entry.result_at = Some(at),
            "role.exited" => entry.exited_at = Some(at),
            _ => {}
        }
    }

    /// Reconcile the watched roles against one authoritative capture
    /// of session state, taken at `at`: a role the capture's `roles`
    /// list no longer names retires, and so does a listed role whose
    /// entry reports `result` (any non-null value) or `exited`
    /// (true). Push events swallowed by a disconnected gap cannot be
    /// replayed — the subscription carries no resume cursor — so this
    /// is how a result, an exit, or a disappearance that landed while
    /// disconnected retires a pending deadline instead of
    /// phantom-breaching forever. A listed role keeps its observed
    /// anchors, because a capture proves existence, not activity, and
    /// a role no push event ever named is not watched. State without
    /// a `roles` array is not authoritative about roles and changes
    /// nothing.
    pub fn observe_snapshot(&mut self, at: SystemTime, state: &Value) {
        let Some(entries) = state.get("roles").and_then(Value::as_array) else {
            return;
        };
        let present: HashSet<&str> = entries.iter().filter_map(snapshot_role).collect();
        self.roles.retain(|role, _| present.contains(role.as_str()));
        for entry in entries {
            let Some(role) = snapshot_role(entry) else {
                continue;
            };
            let Some(tracked) = self.roles.get_mut(role) else {
                continue;
            };
            if entry.get("result").is_some_and(|value| !value.is_null()) {
                tracked.result_at = Some(at);
            }
            if entry.get("exited").and_then(Value::as_bool) == Some(true) {
                tracked.exited_at = Some(at);
            }
        }
    }

    /// Every attention signal breached at `now`, one per role and
    /// deadline, in role order.
    pub fn evaluate(&self, project_id: u64, now: SystemTime) -> Vec<AttentionSignal> {
        let mut roles: Vec<&String> = self.roles.keys().collect();
        roles.sort_unstable();
        roles
            .into_iter()
            .filter_map(|role| self.evaluate_role(project_id, role, now))
            .collect()
    }

    /// The one signal, if any, `role`'s deadlines raise at `now`.
    fn evaluate_role(
        &self,
        project_id: u64,
        role: &str,
        now: SystemTime,
    ) -> Option<AttentionSignal> {
        let entry = self.roles.get(role)?;
        if entry.exited_at.is_some() {
            return None;
        }
        if entry.result_at.is_some() {
            return None;
        }
        if let Some(settled_at) = entry.settled_at {
            let elapsed = secs_since(now, settled_at);
            return (elapsed >= self.config.missing_result().as_secs()).then(|| AttentionSignal {
                project_id,
                reason: MISSING_RESULT_DEADLINE_REASON.to_owned(),
                detail: json!({
                    "deadline": "missing_result",
                    "role": role,
                    "deadline_secs": self.config.missing_result().as_secs(),
                    "breached_after_secs": elapsed,
                    "settled_unix_secs": unix_secs(settled_at),
                }),
            });
        }
        let last_activity = entry.last_activity?;
        let elapsed = secs_since(now, last_activity);
        (elapsed >= self.config.stall().as_secs()).then(|| AttentionSignal {
            project_id,
            reason: STALL_DEADLINE_REASON.to_owned(),
            detail: json!({
                "deadline": "stall",
                "role": role,
                "deadline_secs": self.config.stall().as_secs(),
                "breached_after_secs": elapsed,
                "last_activity_unix_secs": unix_secs(last_activity),
            }),
        })
    }
}

/// Whole seconds from `anchor` to `now`, saturating at zero when the
/// clock reads from before the anchor.
fn secs_since(now: SystemTime, anchor: SystemTime) -> u64 {
    now.duration_since(anchor)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// The role one snapshot entry names: a bare string, or an object
/// carrying its name in `name`.
fn snapshot_role(entry: &Value) -> Option<&str> {
    match entry {
        Value::String(role) => Some(role.as_str()),
        Value::Object(object) => match object.get("name") {
            Some(Value::String(role)) => Some(role.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// Whole seconds from the Unix epoch, for signal detail.
fn unix_secs(at: SystemTime) -> u64 {
    at.duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::{DeadlineConfig, DeadlineMonitor};

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn a_role_known_only_by_settling_still_faces_missing_result() {
        let mut monitor = DeadlineMonitor::new(DeadlineConfig::from_secs(600, 1_200));
        monitor.observe_event(
            at(50),
            &json!({ "kind": "role.settled", "role": "reviewer" }),
        );

        assert!(
            monitor.evaluate(1, at(50 + 1_200)).len() == 1,
            "settling without earlier output still starts the missing-result deadline"
        );
    }

    #[test]
    fn a_clock_before_the_anchor_saturates_at_zero() {
        let mut monitor = DeadlineMonitor::new(DeadlineConfig::from_secs(0, 1_200));
        monitor.observe_event(
            at(1_000),
            &json!({ "kind": "role.output", "role": "reviewer" }),
        );

        let signals = monitor.evaluate(1, at(500));
        assert_eq!(
            signals.len(),
            1,
            "a zero stall deadline with a rewound clock raises the stall once, not a loop"
        );
        assert_eq!(signals[0].detail["breached_after_secs"], json!(0));
    }
}
