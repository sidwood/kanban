//! Live event envelope and payload definitions for the ordered
//! transport stream (ADR-0004).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::comment::CommentRecord;
use crate::evidence::EvidenceRecord;
use crate::initiative::InitiativeRecord;
use crate::project::ProjectRecord;

/// The closed set of live event names the desktop may consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum LiveEventName {
    #[serde(rename = "initiative.created")]
    InitiativeCreated,
    #[serde(rename = "initiative.renamed")]
    InitiativeRenamed,
    #[serde(rename = "initiative.archived")]
    InitiativeArchived,
    #[serde(rename = "project.registered")]
    ProjectRegistered,
    #[serde(rename = "project.archived")]
    ProjectArchived,
    #[serde(rename = "comment.created")]
    CommentCreated,
    #[serde(rename = "comment.edited")]
    CommentEdited,
    #[serde(rename = "ruling.recorded")]
    RulingRecorded,
    #[serde(rename = "ruling.superseded")]
    RulingSuperseded,
    #[serde(rename = "deferral.recorded")]
    DeferralRecorded,
    #[serde(rename = "deferral.superseded")]
    DeferralSuperseded,
    #[serde(rename = "evidence.attached")]
    EvidenceAttached,
    #[serde(rename = "evidence.listed")]
    EvidenceListed,
}

impl LiveEventName {
    /// The wire name every producer and consumer agrees on.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InitiativeCreated => "initiative.created",
            Self::InitiativeRenamed => "initiative.renamed",
            Self::InitiativeArchived => "initiative.archived",
            Self::ProjectRegistered => "project.registered",
            Self::ProjectArchived => "project.archived",
            Self::CommentCreated => "comment.created",
            Self::CommentEdited => "comment.edited",
            Self::RulingRecorded => "ruling.recorded",
            Self::RulingSuperseded => "ruling.superseded",
            Self::DeferralRecorded => "deferral.recorded",
            Self::DeferralSuperseded => "deferral.superseded",
            Self::EvidenceAttached => "evidence.attached",
            Self::EvidenceListed => "evidence.listed",
        }
    }

    /// Parse a wire name into the closed catalogue.
    pub fn parse(name: &str) -> Result<Self, UnknownLiveEventError> {
        match name {
            "initiative.created" => Ok(Self::InitiativeCreated),
            "initiative.renamed" => Ok(Self::InitiativeRenamed),
            "initiative.archived" => Ok(Self::InitiativeArchived),
            "project.registered" => Ok(Self::ProjectRegistered),
            "project.archived" => Ok(Self::ProjectArchived),
            "comment.created" => Ok(Self::CommentCreated),
            "comment.edited" => Ok(Self::CommentEdited),
            "ruling.recorded" => Ok(Self::RulingRecorded),
            "ruling.superseded" => Ok(Self::RulingSuperseded),
            "deferral.recorded" => Ok(Self::DeferralRecorded),
            "deferral.superseded" => Ok(Self::DeferralSuperseded),
            "evidence.attached" => Ok(Self::EvidenceAttached),
            "evidence.listed" => Ok(Self::EvidenceListed),
            other => Err(UnknownLiveEventError {
                event_type: other.to_owned(),
            }),
        }
    }
}

/// The identity carried on ruling live events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulingIdentity {
    pub id: u64,
}

/// The identity carried on deferral live events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeferralIdentity {
    pub id: u64,
}

/// The Project and result count carried by evidence-list live events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceListSummary {
    pub project_id: String,
    pub count: usize,
}

/// One catalogued live event with its typed payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveEvent {
    InitiativeCreated {
        sequence: u64,
        payload: InitiativeRecord,
    },
    InitiativeRenamed {
        sequence: u64,
        payload: InitiativeRecord,
    },
    InitiativeArchived {
        sequence: u64,
        payload: InitiativeRecord,
    },
    ProjectRegistered {
        sequence: u64,
        payload: ProjectRecord,
    },
    ProjectArchived {
        sequence: u64,
        payload: ProjectRecord,
    },
    CommentCreated {
        sequence: u64,
        payload: CommentRecord,
    },
    CommentEdited {
        sequence: u64,
        payload: CommentRecord,
    },
    RulingRecorded {
        sequence: u64,
        payload: RulingIdentity,
    },
    RulingSuperseded {
        sequence: u64,
        payload: RulingIdentity,
    },
    DeferralRecorded {
        sequence: u64,
        payload: DeferralIdentity,
    },
    DeferralSuperseded {
        sequence: u64,
        payload: DeferralIdentity,
    },
    EvidenceAttached {
        sequence: u64,
        payload: EvidenceRecord,
    },
    EvidenceListed {
        sequence: u64,
        payload: EvidenceListSummary,
    },
}

impl LiveEvent {
    /// The wire name of this event.
    pub fn name(&self) -> LiveEventName {
        match self {
            Self::InitiativeCreated { .. } => LiveEventName::InitiativeCreated,
            Self::InitiativeRenamed { .. } => LiveEventName::InitiativeRenamed,
            Self::InitiativeArchived { .. } => LiveEventName::InitiativeArchived,
            Self::ProjectRegistered { .. } => LiveEventName::ProjectRegistered,
            Self::ProjectArchived { .. } => LiveEventName::ProjectArchived,
            Self::CommentCreated { .. } => LiveEventName::CommentCreated,
            Self::CommentEdited { .. } => LiveEventName::CommentEdited,
            Self::RulingRecorded { .. } => LiveEventName::RulingRecorded,
            Self::RulingSuperseded { .. } => LiveEventName::RulingSuperseded,
            Self::DeferralRecorded { .. } => LiveEventName::DeferralRecorded,
            Self::DeferralSuperseded { .. } => LiveEventName::DeferralSuperseded,
            Self::EvidenceAttached { .. } => LiveEventName::EvidenceAttached,
            Self::EvidenceListed { .. } => LiveEventName::EvidenceListed,
        }
    }

    /// The global sequence assigned by the broker.
    pub fn sequence(&self) -> u64 {
        match self {
            Self::InitiativeCreated { sequence, .. }
            | Self::InitiativeRenamed { sequence, .. }
            | Self::InitiativeArchived { sequence, .. }
            | Self::ProjectRegistered { sequence, .. }
            | Self::ProjectArchived { sequence, .. }
            | Self::CommentCreated { sequence, .. }
            | Self::CommentEdited { sequence, .. }
            | Self::RulingRecorded { sequence, .. }
            | Self::RulingSuperseded { sequence, .. }
            | Self::DeferralRecorded { sequence, .. }
            | Self::DeferralSuperseded { sequence, .. }
            | Self::EvidenceAttached { sequence, .. }
            | Self::EvidenceListed { sequence, .. } => *sequence,
        }
    }
}

/// Ordered event frame streamed to every transport subscriber.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventEnvelope {
    pub sequence: u64,
    pub event_type: String,
    pub payload: Value,
}

/// A live event name outside the closed catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownLiveEventError {
    pub event_type: String,
}

impl std::fmt::Display for UnknownLiveEventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown live event type `{}`", self.event_type)
    }
}

impl std::error::Error for UnknownLiveEventError {}

/// A catalogued event whose payload could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidLiveEventPayloadError {
    pub event_type: LiveEventName,
    pub message: String,
}

impl std::fmt::Display for InvalidLiveEventPayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid payload for `{}`: {}",
            self.event_type.as_str(),
            self.message
        )
    }
}

impl std::error::Error for InvalidLiveEventPayloadError {}

/// Decode a wire envelope into one catalogued live event.
pub fn decode_live_event(envelope: &EventEnvelope) -> Result<LiveEvent, DecodeLiveEventError> {
    let name = LiveEventName::parse(&envelope.event_type)?;
    let sequence = envelope.sequence;
    let event = match name {
        LiveEventName::InitiativeCreated => LiveEvent::InitiativeCreated {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::InitiativeRenamed => LiveEvent::InitiativeRenamed {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::InitiativeArchived => LiveEvent::InitiativeArchived {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::ProjectRegistered => LiveEvent::ProjectRegistered {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::ProjectArchived => LiveEvent::ProjectArchived {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::CommentCreated => LiveEvent::CommentCreated {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::CommentEdited => LiveEvent::CommentEdited {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::RulingRecorded => LiveEvent::RulingRecorded {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::RulingSuperseded => LiveEvent::RulingSuperseded {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::DeferralRecorded => LiveEvent::DeferralRecorded {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::DeferralSuperseded => LiveEvent::DeferralSuperseded {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::EvidenceAttached => LiveEvent::EvidenceAttached {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::EvidenceListed => LiveEvent::EvidenceListed {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
    };
    Ok(event)
}

/// Why a wire envelope could not become a catalogued live event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeLiveEventError {
    Unknown(UnknownLiveEventError),
    InvalidPayload(InvalidLiveEventPayloadError),
}

impl std::fmt::Display for DecodeLiveEventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(error) => error.fmt(f),
            Self::InvalidPayload(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DecodeLiveEventError {}

impl From<UnknownLiveEventError> for DecodeLiveEventError {
    fn from(error: UnknownLiveEventError) -> Self {
        Self::Unknown(error)
    }
}

fn decode_payload<T: for<'de> Deserialize<'de>>(
    event_type: LiveEventName,
    payload: &Value,
) -> Result<T, DecodeLiveEventError> {
    serde_json::from_value(payload.clone()).map_err(|error| {
        DecodeLiveEventError::InvalidPayload(InvalidLiveEventPayloadError {
            event_type,
            message: error.to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::TimelineEntityRef;
    use serde_json::json;

    #[test]
    fn unknown_live_event_names_are_refused() {
        let envelope = EventEnvelope {
            sequence: 1,
            event_type: "counter.bumped".to_owned(),
            payload: json!({ "to": 1 }),
        };

        let error = decode_live_event(&envelope).expect_err("unknown names are refused");
        assert_eq!(
            error,
            DecodeLiveEventError::Unknown(UnknownLiveEventError {
                event_type: "counter.bumped".to_owned(),
            })
        );
    }

    #[test]
    fn catalogued_initiative_events_decode_typed_payloads() {
        let envelope = EventEnvelope {
            sequence: 3,
            event_type: "initiative.created".to_owned(),
            payload: json!({
                "id": 1,
                "name": "Alpha",
                "archived": false,
                "version": 1,
            }),
        };

        let event = decode_live_event(&envelope).expect("the envelope decodes");
        assert_eq!(
            event,
            LiveEvent::InitiativeCreated {
                sequence: 3,
                payload: InitiativeRecord {
                    id: 1,
                    name: "Alpha".to_owned(),
                    archived: false,
                    version: 1,
                },
            }
        );
    }

    #[test]
    fn catalogued_project_events_decode_typed_payloads() {
        let envelope = EventEnvelope {
            sequence: 4,
            event_type: "project.registered".to_owned(),
            payload: json!({
                "id": 1,
                "code": "CORE",
                "name": "Control plane",
                "repository": "/repositories/kanban",
                "seed_workspace": "/workspaces/kanban.seed",
                "default_branch": "main",
                "herdr_session": "kanban-main",
                "initiative_id": null,
                "archived": false,
                "counters": { "plan": 0, "spec": 0, "ticket": 0 },
                "version": 1,
            }),
        };

        let event = decode_live_event(&envelope).expect("the envelope decodes");
        assert_eq!(
            event,
            LiveEvent::ProjectRegistered {
                sequence: 4,
                payload: ProjectRecord {
                    id: 1,
                    code: "CORE".to_owned(),
                    name: "Control plane".to_owned(),
                    repository: "/repositories/kanban".to_owned(),
                    seed_workspace: "/workspaces/kanban.seed".to_owned(),
                    default_branch: "main".to_owned(),
                    herdr_session: "kanban-main".to_owned(),
                    initiative_id: None,
                    archived: false,
                    counters: crate::project::ProjectCounters {
                        plan: 0,
                        spec: 0,
                        ticket: 0,
                    },
                    version: 1,
                },
            }
        );
        assert_eq!(
            LiveEventName::parse("project.archived"),
            Ok(LiveEventName::ProjectArchived),
            "both Project events are catalogued"
        );
    }

    #[test]
    fn catalogued_comment_events_decode_typed_payloads() {
        let envelope = EventEnvelope {
            sequence: 2,
            event_type: "comment.edited".to_owned(),
            payload: json!({
                "id": 4,
                "project_id": "kan",
                "target": { "kind": "ticket", "id": "kan-t11" },
                "text": "Updated",
                "version": 2,
            }),
        };

        let event = decode_live_event(&envelope).expect("the envelope decodes");
        assert_eq!(
            event,
            LiveEvent::CommentEdited {
                sequence: 2,
                payload: CommentRecord {
                    id: 4,
                    project_id: "kan".to_owned(),
                    target: TimelineEntityRef {
                        kind: crate::timeline::TimelineEntityKind::Ticket,
                        id: "kan-t11".to_owned(),
                    },
                    text: "Updated".to_owned(),
                    version: 2,
                },
            }
        );
    }

    #[test]
    fn catalogued_evidence_events_decode_typed_payloads() {
        let envelope = EventEnvelope {
            sequence: 5,
            event_type: "evidence.listed".to_owned(),
            payload: json!({ "project_id": "kan-p1", "count": 2 }),
        };

        let event = decode_live_event(&envelope).expect("the envelope decodes");
        assert_eq!(
            event,
            LiveEvent::EvidenceListed {
                sequence: 5,
                payload: EvidenceListSummary {
                    project_id: "kan-p1".to_owned(),
                    count: 2,
                },
            }
        );
    }

    #[test]
    fn invalid_payloads_are_refused() {
        let envelope = EventEnvelope {
            sequence: 1,
            event_type: "ruling.recorded".to_owned(),
            payload: json!({ "surprise": true }),
        };

        let error = decode_live_event(&envelope).expect_err("bad payloads are refused");
        assert!(matches!(
            error,
            DecodeLiveEventError::InvalidPayload(InvalidLiveEventPayloadError {
                event_type: LiveEventName::RulingRecorded,
                ..
            })
        ));
    }
}
