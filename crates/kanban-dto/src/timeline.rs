use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The closed vocabulary of timeline event kinds (DR-AE-01).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimelineEventKind {
    Transition,
    Run,
    Telemetry,
    Review,
    Finding,
    Evidence,
    Comment,
    Deferral,
    Ruling,
}

impl TimelineEventKind {
    /// Every event kind the Plan records, in vocabulary order.
    pub const ALL: &'static [Self] = &[
        Self::Transition,
        Self::Run,
        Self::Telemetry,
        Self::Review,
        Self::Finding,
        Self::Evidence,
        Self::Comment,
        Self::Deferral,
        Self::Ruling,
    ];

    /// The wire name, matching this kind's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Transition => "transition",
            Self::Run => "run",
            Self::Telemetry => "telemetry",
            Self::Review => "review",
            Self::Finding => "finding",
            Self::Evidence => "evidence",
            Self::Comment => "comment",
            Self::Deferral => "deferral",
            Self::Ruling => "ruling",
        }
    }

    /// The kind `wire` names, or `None` outside the vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.as_str() == wire)
    }
}

/// Entity kinds that may appear on the activity timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimelineEntityKind {
    Initiative,
    Project,
    Plan,
    Spec,
    Ticket,
    Run,
    Review,
    Finding,
    Evidence,
    Comment,
}

impl TimelineEntityKind {
    /// Every entity kind the timeline may reference, in vocabulary
    /// order.
    pub const ALL: &'static [Self] = &[
        Self::Initiative,
        Self::Project,
        Self::Plan,
        Self::Spec,
        Self::Ticket,
        Self::Run,
        Self::Review,
        Self::Finding,
        Self::Evidence,
        Self::Comment,
    ];

    /// The wire name, matching this kind's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Initiative => "initiative",
            Self::Project => "project",
            Self::Plan => "plan",
            Self::Spec => "spec",
            Self::Ticket => "ticket",
            Self::Run => "run",
            Self::Review => "review",
            Self::Finding => "finding",
            Self::Evidence => "evidence",
            Self::Comment => "comment",
        }
    }

    /// The kind `wire` names, or `None` outside the vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.as_str() == wire)
    }
}

/// A timeline-visible entity reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineEntityRef {
    pub kind: TimelineEntityKind,
    pub id: String,
}

/// One append-only timeline row as returned by queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineEventRecord {
    pub id: u64,
    pub project_id: String,
    pub kind: TimelineEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<TimelineEntityRef>,
    pub recorded_at: String,
    pub detail: Value,
}

/// Filters for the per-Project timeline query surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineQuery {
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<TimelineEntityRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<TimelineEventKind>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
}

/// The timeline query answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineQueryResponse {
    pub events: Vec<TimelineEventRecord>,
}

#[cfg(test)]
mod vocabulary {
    use super::{TimelineEntityKind, TimelineEventKind};

    #[test]
    fn event_kinds_round_trip_through_their_wire_names() {
        for kind in TimelineEventKind::ALL {
            assert_eq!(
                TimelineEventKind::parse(kind.as_str()),
                Some(*kind),
                "`{}` must survive the round trip",
                kind.as_str()
            );
            assert_eq!(
                serde_json::to_value(kind).expect("the kind encodes"),
                serde_json::Value::from(kind.as_str()),
                "the wire name and the serialised name must agree"
            );
        }
    }

    #[test]
    fn event_kinds_cover_every_category_the_plan_records() {
        let names: Vec<&str> = TimelineEventKind::ALL
            .iter()
            .map(TimelineEventKind::as_str)
            .collect();
        assert_eq!(
            names,
            vec![
                "transition",
                "run",
                "telemetry",
                "review",
                "finding",
                "evidence",
                "comment",
                "deferral",
                "ruling",
            ]
        );
    }

    #[test]
    fn entity_kinds_round_trip_through_their_wire_names() {
        for kind in TimelineEntityKind::ALL {
            assert_eq!(
                TimelineEntityKind::parse(kind.as_str()),
                Some(*kind),
                "`{}` must survive the round trip",
                kind.as_str()
            );
            assert_eq!(
                serde_json::to_value(kind).expect("the kind encodes"),
                serde_json::Value::from(kind.as_str()),
                "the wire name and the serialised name must agree"
            );
        }
    }

    #[test]
    fn parsing_refuses_a_value_outside_the_vocabulary() {
        assert_eq!(TimelineEventKind::parse("initiative.created"), None);
        assert_eq!(TimelineEntityKind::parse("ghost"), None);
    }
}
