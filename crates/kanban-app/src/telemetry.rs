//! The Herdr telemetry projection (KAN-S8): push events become
//! timeline rows and attention signals, never workflow verdicts
//! (DR-HB-03, DR-HB-04, DR-HB-07).

use kanban_dto::TimelineEventKind;
use serde_json::{Map, Value};

use crate::timeline::TimelineEnvelope;

/// What one piece of Herdr telemetry becomes. The vocabulary is
/// closed here — a timeline row or an attention signal — and no
/// variant can carry a verdict or a workflow transition, so the
/// never-a-verdict rule holds by construction (DR-HB-04).
#[derive(Debug, Clone, PartialEq)]
pub enum TelemetryProjection {
    /// An append-only telemetry row on one Project's timeline.
    Timeline(TimelineEnvelope),
    /// An operator-facing signal. Deadline breaches land here in
    /// KAN-T41 and missing submissions in KAN-T45; push events
    /// themselves never raise one.
    Attention(AttentionSignal),
}

/// One operator-facing signal raised by observing a Project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionSignal {
    /// The Project whose observation raised the signal.
    pub project_id: u64,
    /// Why the signal was raised.
    pub reason: String,
    /// The structured facts behind the signal.
    pub detail: Value,
}

/// The metadata a role tab carries (DR-HB-03): Project, Ticket,
/// Lane, reviewer slot, run, harness, and model.
const ROLE_TAB_FIELDS: [&str; 7] = [
    "project",
    "ticket",
    "lane",
    "reviewer_slot",
    "run",
    "harness",
    "model",
];

/// Project one Herdr push event onto one Project's timeline as
/// telemetry. Every event lands: telemetry records what Herdr
/// reported without judging it, so agent exits, disconnects, and
/// stalls are rows, never verdicts (DR-HB-04, DR-HB-07).
pub fn project_herdr_event(project_id: u64, event: &Value) -> Vec<TelemetryProjection> {
    vec![TelemetryProjection::Timeline(TimelineEnvelope::project(
        project_id,
        TimelineEventKind::Telemetry,
        None,
        push_event_detail(event),
    ))]
}

/// The telemetry detail for one push event: provenance, the kind
/// Herdr reported, the role and its tab metadata when the event
/// carries them, and the payload unchanged.
fn push_event_detail(event: &Value) -> Value {
    let mut detail = Map::new();
    detail.insert("source".to_owned(), Value::from("herdr"));
    detail.insert(
        "event".to_owned(),
        event.get("kind").cloned().unwrap_or(Value::Null),
    );
    if let Some(role) = event.get("role") {
        detail.insert("role".to_owned(), role.clone());
    }
    if let Some(tab) = role_tab(event) {
        detail.insert("tab".to_owned(), tab);
    }
    detail.insert("payload".to_owned(), event.clone());
    Value::Object(detail)
}

/// The role tab metadata an event carries, exactly as reported:
/// only the fields Herdr sent, never invented values (DR-HB-03).
fn role_tab(event: &Value) -> Option<Value> {
    let object = event.as_object()?;
    let tab: Map<String, Value> = ROLE_TAB_FIELDS
        .iter()
        .filter_map(|field| {
            object
                .get(*field)
                .map(|value| ((*field).to_owned(), value.clone()))
        })
        .collect();
    if tab.is_empty() {
        None
    } else {
        Some(Value::Object(tab))
    }
}

#[cfg(test)]
mod telemetry_projection {
    use kanban_dto::{TimelineEventKind, TimelineScope};
    use serde_json::{Value, json};

    use super::TelemetryProjection;
    use super::project_herdr_event;
    use crate::timeline::TimelineEnvelope;

    fn timeline_row(project_id: u64, event: &Value) -> TimelineEnvelope {
        match project_herdr_event(project_id, event).as_slice() {
            [TelemetryProjection::Timeline(envelope)] => envelope.clone(),
            other => panic!("a push event projects to exactly one timeline row: {other:?}"),
        }
    }

    #[test]
    fn a_push_event_projects_to_a_telemetry_row_in_its_project() {
        let envelope = timeline_row(
            7,
            &json!({ "kind": "role.output", "role": "implementer", "text": "working" }),
        );

        assert_eq!(envelope.scope(), &TimelineScope::Project(7));
        assert_eq!(envelope.kind(), TimelineEventKind::Telemetry);
        assert_eq!(envelope.entity(), None);
        assert_eq!(envelope.detail()["source"], json!("herdr"));
        assert_eq!(envelope.detail()["event"], json!("role.output"));
        assert_eq!(envelope.detail()["role"], json!("implementer"));
    }

    #[test]
    fn role_tab_metadata_is_carried_whole() {
        let envelope = timeline_row(
            1,
            &json!({
                "kind": "role.opened",
                "role": "reviewer",
                "project": "CORE",
                "ticket": "KAN-T40",
                "lane": "review",
                "reviewer_slot": "primary",
                "run": "run-1",
                "harness": "claude-code",
                "model": "opus-5"
            }),
        );

        assert_eq!(
            envelope.detail()["tab"],
            json!({
                "project": "CORE",
                "ticket": "KAN-T40",
                "lane": "review",
                "reviewer_slot": "primary",
                "run": "run-1",
                "harness": "claude-code",
                "model": "opus-5"
            })
        );
    }

    #[test]
    fn partial_role_tab_metadata_is_preserved_as_reported() {
        let envelope = timeline_row(
            1,
            &json!({ "kind": "role.opened", "role": "implementer", "ticket": "KAN-T40" }),
        );

        assert_eq!(
            envelope.detail()["tab"],
            json!({ "ticket": "KAN-T40" }),
            "fields Herdr did not report are absent, not invented"
        );
    }

    #[test]
    fn an_event_without_role_metadata_carries_no_tab() {
        let envelope = timeline_row(1, &json!({ "kind": "session.disconnected" }));

        assert!(envelope.detail().get("tab").is_none());
        assert!(envelope.detail().get("role").is_none());
    }

    #[test]
    fn an_event_without_a_kind_still_lands() {
        let payload = json!({ "note": "frame without a kind" });
        let envelope = timeline_row(1, &payload);

        assert_eq!(envelope.detail()["event"], Value::Null);
        assert_eq!(envelope.detail()["payload"], payload);
    }

    #[test]
    fn verdict_shaped_events_project_as_telemetry_only() {
        let hostile = [
            json!({ "kind": "role.exited", "role": "implementer", "exit_code": 0 }),
            json!({ "kind": "role.stalled", "role": "implementer" }),
            json!({ "kind": "run.passed", "verdict": "pass" }),
            json!({ "kind": "ticket.transition", "to": "done" }),
            json!({ "kind": "spec.version.approved", "version": 2 }),
        ];

        for event in hostile {
            for projection in project_herdr_event(1, &event) {
                match projection {
                    TelemetryProjection::Timeline(envelope) => assert_eq!(
                        envelope.kind(),
                        TimelineEventKind::Telemetry,
                        "no event type implies a verdict or a transition"
                    ),
                    TelemetryProjection::Attention(signal) => {
                        panic!("a push event raised a signal: {signal:?}")
                    }
                }
            }
        }
    }
}
