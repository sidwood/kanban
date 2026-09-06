//! Live event envelope and payload definitions for the ordered
//! transport stream (ADR-0004).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::clone::{CloneCreatedRecord, CloneRemovedRecord};
use crate::comment::CommentRecord;
use crate::evidence::EvidenceRecord;
use crate::initiative::InitiativeRecord;
use crate::lane::LaneRecord;
use crate::plan::PlanRecord;
use crate::profile::ProfileRecord;
use crate::project::ProjectRecord;
use crate::spec::SpecRecord;
use crate::ticket::TicketRecord;
use crate::workspace::WorkspaceRecord;

macro_rules! define_live_event_catalogue {
    (
        $(
            $variant:ident @ $wire:literal => {
                payload: $payload:literal,
                description: $description:literal,
            }
        ),* $(,)?
    ) => {
        /// The closed set of live event names the desktop may consume.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        pub enum LiveEventName {
            $(
                #[serde(rename = $wire)]
                $variant,
            )*
        }

        /// Metadata for one catalogued live event.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct LiveEventDescriptor {
            pub name: LiveEventName,
            pub payload_schema: &'static str,
            pub description: &'static str,
        }

        impl LiveEventName {
            /// The wire name every producer and consumer agrees on.
            pub fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $wire, )*
                }
            }

            /// Parse a wire name into the closed catalogue.
            pub fn parse(name: &str) -> Result<Self, UnknownLiveEventError> {
                match name {
                    $( $wire => Ok(Self::$variant), )*
                    other => Err(UnknownLiveEventError {
                        event_type: other.to_owned(),
                    }),
                }
            }

            /// Every catalogued live event.
            pub const ALL: &[Self] = &[$( Self::$variant ),*];
        }

        /// The authoritative live-event catalogue.
        pub fn live_event_catalog() -> &'static [LiveEventDescriptor] {
            &[
                $(
                    LiveEventDescriptor {
                        name: LiveEventName::$variant,
                        payload_schema: $payload,
                        description: $description,
                    },
                )*
            ]
        }

        /// Look up one catalogued live event by typed name.
        pub fn event_descriptor(name: LiveEventName) -> &'static LiveEventDescriptor {
            match name {
                $(
                    LiveEventName::$variant => &LiveEventDescriptor {
                        name: LiveEventName::$variant,
                        payload_schema: $payload,
                        description: $description,
                    },
                )*
            }
        }
    };
}

define_live_event_catalogue! {
    InitiativeCreated @ "initiative.created" => {
        payload: "InitiativeRecord",
        description: "An Initiative was created.",
    },
    InitiativeRenamed @ "initiative.renamed" => {
        payload: "InitiativeRecord",
        description: "An Initiative was renamed.",
    },
    InitiativeArchived @ "initiative.archived" => {
        payload: "InitiativeRecord",
        description: "An Initiative was archived.",
    },
    ProjectRegistered @ "project.registered" => {
        payload: "ProjectRecord",
        description: "A Project was registered.",
    },
    ProjectArchived @ "project.archived" => {
        payload: "ProjectRecord",
        description: "A Project was archived.",
    },
    PlanCreated @ "plan.created" => {
        payload: "PlanRecord",
        description: "A Plan was created.",
    },
    PlanActivated @ "plan.activated" => {
        payload: "PlanRecord",
        description: "A Plan was activated, freezing a version.",
    },
    PlanReplanned @ "plan.replanned" => {
        payload: "PlanRecord",
        description: "A Plan was replanned, reserving its replacement version.",
    },
    PlanCompleted @ "plan.completed" => {
        payload: "PlanRecord",
        description: "A Plan was completed.",
    },
    PlanCancelled @ "plan.cancelled" => {
        payload: "PlanRecord",
        description: "A Plan was cancelled.",
    },
    PlanArchived @ "plan.archived" => {
        payload: "PlanRecord",
        description: "A Plan was archived.",
    },
    SpecCreated @ "spec.created" => {
        payload: "SpecRecord",
        description: "A Spec was authored.",
    },
    SpecPlanned @ "spec.planned" => {
        payload: "SpecRecord",
        description: "A Spec joined a Plan, planning its execution.",
    },
    SpecVersionApproved @ "spec.version.approved" => {
        payload: "SpecRecord",
        description: "A Spec content version was approved.",
    },
    SpecVersionSuperseded @ "spec.version.superseded" => {
        payload: "SpecRecord",
        description: "A Spec content version was superseded.",
    },
    SpecExecutionMoved @ "spec.execution.moved" => {
        payload: "SpecRecord",
        description: "A Spec's execution moved along its state set.",
    },
    TicketCreated @ "ticket.created" => {
        payload: "TicketRecord",
        description: "A Ticket was created under its kind's schema.",
    },
    TicketAssigned @ "ticket.assigned" => {
        payload: "TicketRecord",
        description: "A Ticket's assignment named an Execution Profile.",
    },
    TicketStateChanged @ "ticket.state.changed" => {
        payload: "TicketRecord",
        description: "A Ticket's lifecycle state moved through a command, a drag, or the emergency override.",
    },
    TicketEdited @ "ticket.edited" => {
        payload: "TicketRecord",
        description: "A Ticket's title, slice description, or priority was edited.",
    },
    ProfileDefined @ "profile.defined" => {
        payload: "ProfileRecord",
        description: "An Execution Profile was defined in the catalogue.",
    },
    ProfileUpdated @ "profile.updated" => {
        payload: "ProfileRecord",
        description: "An Execution Profile's definition was replaced under its name.",
    },
    ProfileRetired @ "profile.retired" => {
        payload: "ProfileRecord",
        description: "An Execution Profile was retired. The entry is preserved, never deleted.",
    },
    CommentCreated @ "comment.created" => {
        payload: "CommentRecord",
        description: "A Comment was created.",
    },
    CommentEdited @ "comment.edited" => {
        payload: "CommentRecord",
        description: "A Comment was edited.",
    },
    RulingRecorded @ "ruling.recorded" => {
        payload: "RulingIdentity",
        description: "A ruling was recorded.",
    },
    RulingSuperseded @ "ruling.superseded" => {
        payload: "RulingIdentity",
        description: "A ruling was superseded.",
    },
    DeferralRecorded @ "deferral.recorded" => {
        payload: "DeferralIdentity",
        description: "A deferral was recorded.",
    },
    DeferralSuperseded @ "deferral.superseded" => {
        payload: "DeferralIdentity",
        description: "A deferral was superseded.",
    },
    EvidenceAttached @ "evidence.attached" => {
        payload: "EvidenceRecord",
        description: "Evidence was attached to a subject entity.",
    },
    EvidenceListed @ "evidence.listed" => {
        payload: "EvidenceListSummary",
        description: "Evidence was listed for a Project.",
    },
    WorkspaceRegistered @ "workspace.registered" => {
        payload: "WorkspaceRecord",
        description: "A Workspace was registered.",
    },
    WorkspaceObserved @ "workspace.observed" => {
        payload: "WorkspaceRecord",
        description: "A Workspace was observed and its health updated.",
    },
    WorkspaceRetired @ "workspace.retired" => {
        payload: "WorkspaceRecord",
        description: "A Workspace was retired. The record is preserved, never deleted.",
    },
    LaneCreated @ "lane.created" => {
        payload: "LaneRecord",
        description: "A Lane was created as a durable execution slot.",
    },
    LaneWorkspaceAssigned @ "lane.workspace.assigned" => {
        payload: "LaneRecord",
        description: "A Workspace was claimed by a Lane.",
    },
    LaneWorkspaceReleased @ "lane.workspace.released" => {
        payload: "LaneRecord",
        description: "A Lane released its Workspace claim.",
    },
    LaneTicketAssigned @ "lane.ticket.assigned" => {
        payload: "LaneRecord",
        description: "A Ticket became the active occupant of a Lane.",
    },
    LaneTicketReleased @ "lane.ticket.released" => {
        payload: "LaneRecord",
        description: "A Lane released its active Ticket.",
    },
    CloneCreated @ "clone.created" => {
        payload: "CloneCreatedRecord",
        description: "A branch clone was created through the guarded fleet skill.",
    },
    CloneRemoved @ "clone.removed" => {
        payload: "CloneRemovedRecord",
        description: "A branch clone was removed through the guarded fleet skill. The Workspace record is preserved.",
    },
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
    pub project_id: u64,
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
    PlanCreated {
        sequence: u64,
        payload: PlanRecord,
    },
    PlanActivated {
        sequence: u64,
        payload: PlanRecord,
    },
    PlanReplanned {
        sequence: u64,
        payload: PlanRecord,
    },
    PlanCompleted {
        sequence: u64,
        payload: PlanRecord,
    },
    PlanCancelled {
        sequence: u64,
        payload: PlanRecord,
    },
    PlanArchived {
        sequence: u64,
        payload: PlanRecord,
    },
    SpecCreated {
        sequence: u64,
        payload: SpecRecord,
    },
    SpecPlanned {
        sequence: u64,
        payload: SpecRecord,
    },
    SpecVersionApproved {
        sequence: u64,
        payload: SpecRecord,
    },
    SpecVersionSuperseded {
        sequence: u64,
        payload: SpecRecord,
    },
    SpecExecutionMoved {
        sequence: u64,
        payload: SpecRecord,
    },
    TicketCreated {
        sequence: u64,
        payload: Box<TicketRecord>,
    },
    TicketAssigned {
        sequence: u64,
        payload: Box<TicketRecord>,
    },
    TicketStateChanged {
        sequence: u64,
        payload: Box<TicketRecord>,
    },
    TicketEdited {
        sequence: u64,
        payload: Box<TicketRecord>,
    },
    ProfileDefined {
        sequence: u64,
        payload: ProfileRecord,
    },
    ProfileUpdated {
        sequence: u64,
        payload: ProfileRecord,
    },
    ProfileRetired {
        sequence: u64,
        payload: ProfileRecord,
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
    WorkspaceRegistered {
        sequence: u64,
        payload: WorkspaceRecord,
    },
    WorkspaceObserved {
        sequence: u64,
        payload: WorkspaceRecord,
    },
    WorkspaceRetired {
        sequence: u64,
        payload: WorkspaceRecord,
    },
    LaneCreated {
        sequence: u64,
        payload: LaneRecord,
    },
    LaneWorkspaceAssigned {
        sequence: u64,
        payload: LaneRecord,
    },
    LaneWorkspaceReleased {
        sequence: u64,
        payload: LaneRecord,
    },
    LaneTicketAssigned {
        sequence: u64,
        payload: LaneRecord,
    },
    LaneTicketReleased {
        sequence: u64,
        payload: LaneRecord,
    },
    CloneCreated {
        sequence: u64,
        payload: CloneCreatedRecord,
    },
    CloneRemoved {
        sequence: u64,
        payload: CloneRemovedRecord,
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
            Self::PlanCreated { .. } => LiveEventName::PlanCreated,
            Self::PlanActivated { .. } => LiveEventName::PlanActivated,
            Self::PlanReplanned { .. } => LiveEventName::PlanReplanned,
            Self::PlanCompleted { .. } => LiveEventName::PlanCompleted,
            Self::PlanCancelled { .. } => LiveEventName::PlanCancelled,
            Self::PlanArchived { .. } => LiveEventName::PlanArchived,
            Self::SpecCreated { .. } => LiveEventName::SpecCreated,
            Self::SpecPlanned { .. } => LiveEventName::SpecPlanned,
            Self::SpecVersionApproved { .. } => LiveEventName::SpecVersionApproved,
            Self::SpecVersionSuperseded { .. } => LiveEventName::SpecVersionSuperseded,
            Self::SpecExecutionMoved { .. } => LiveEventName::SpecExecutionMoved,
            Self::TicketCreated { .. } => LiveEventName::TicketCreated,
            Self::TicketAssigned { .. } => LiveEventName::TicketAssigned,
            Self::TicketStateChanged { .. } => LiveEventName::TicketStateChanged,
            Self::TicketEdited { .. } => LiveEventName::TicketEdited,
            Self::ProfileDefined { .. } => LiveEventName::ProfileDefined,
            Self::ProfileUpdated { .. } => LiveEventName::ProfileUpdated,
            Self::ProfileRetired { .. } => LiveEventName::ProfileRetired,
            Self::CommentCreated { .. } => LiveEventName::CommentCreated,
            Self::CommentEdited { .. } => LiveEventName::CommentEdited,
            Self::RulingRecorded { .. } => LiveEventName::RulingRecorded,
            Self::RulingSuperseded { .. } => LiveEventName::RulingSuperseded,
            Self::DeferralRecorded { .. } => LiveEventName::DeferralRecorded,
            Self::DeferralSuperseded { .. } => LiveEventName::DeferralSuperseded,
            Self::EvidenceAttached { .. } => LiveEventName::EvidenceAttached,
            Self::EvidenceListed { .. } => LiveEventName::EvidenceListed,
            Self::WorkspaceRegistered { .. } => LiveEventName::WorkspaceRegistered,
            Self::WorkspaceObserved { .. } => LiveEventName::WorkspaceObserved,
            Self::WorkspaceRetired { .. } => LiveEventName::WorkspaceRetired,
            Self::LaneCreated { .. } => LiveEventName::LaneCreated,
            Self::LaneWorkspaceAssigned { .. } => LiveEventName::LaneWorkspaceAssigned,
            Self::LaneWorkspaceReleased { .. } => LiveEventName::LaneWorkspaceReleased,
            Self::LaneTicketAssigned { .. } => LiveEventName::LaneTicketAssigned,
            Self::LaneTicketReleased { .. } => LiveEventName::LaneTicketReleased,
            Self::CloneCreated { .. } => LiveEventName::CloneCreated,
            Self::CloneRemoved { .. } => LiveEventName::CloneRemoved,
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
            | Self::PlanCreated { sequence, .. }
            | Self::PlanActivated { sequence, .. }
            | Self::PlanReplanned { sequence, .. }
            | Self::PlanCompleted { sequence, .. }
            | Self::PlanCancelled { sequence, .. }
            | Self::PlanArchived { sequence, .. }
            | Self::SpecCreated { sequence, .. }
            | Self::SpecPlanned { sequence, .. }
            | Self::SpecVersionApproved { sequence, .. }
            | Self::SpecVersionSuperseded { sequence, .. }
            | Self::SpecExecutionMoved { sequence, .. }
            | Self::TicketCreated { sequence, .. }
            | Self::TicketAssigned { sequence, .. }
            | Self::TicketStateChanged { sequence, .. }
            | Self::TicketEdited { sequence, .. }
            | Self::ProfileDefined { sequence, .. }
            | Self::ProfileUpdated { sequence, .. }
            | Self::ProfileRetired { sequence, .. }
            | Self::CommentCreated { sequence, .. }
            | Self::CommentEdited { sequence, .. }
            | Self::RulingRecorded { sequence, .. }
            | Self::RulingSuperseded { sequence, .. }
            | Self::DeferralRecorded { sequence, .. }
            | Self::DeferralSuperseded { sequence, .. }
            | Self::EvidenceAttached { sequence, .. }
            | Self::EvidenceListed { sequence, .. }
            | Self::WorkspaceRegistered { sequence, .. }
            | Self::WorkspaceObserved { sequence, .. }
            | Self::WorkspaceRetired { sequence, .. }
            | Self::LaneCreated { sequence, .. }
            | Self::LaneWorkspaceAssigned { sequence, .. }
            | Self::LaneWorkspaceReleased { sequence, .. }
            | Self::LaneTicketAssigned { sequence, .. }
            | Self::LaneTicketReleased { sequence, .. }
            | Self::CloneCreated { sequence, .. }
            | Self::CloneRemoved { sequence, .. } => *sequence,
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
        LiveEventName::PlanCreated => LiveEvent::PlanCreated {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::PlanActivated => LiveEvent::PlanActivated {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::PlanReplanned => LiveEvent::PlanReplanned {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::PlanCompleted => LiveEvent::PlanCompleted {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::PlanCancelled => LiveEvent::PlanCancelled {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::PlanArchived => LiveEvent::PlanArchived {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::SpecCreated => LiveEvent::SpecCreated {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::SpecPlanned => LiveEvent::SpecPlanned {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::SpecVersionApproved => LiveEvent::SpecVersionApproved {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::SpecVersionSuperseded => LiveEvent::SpecVersionSuperseded {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::SpecExecutionMoved => LiveEvent::SpecExecutionMoved {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::TicketCreated => LiveEvent::TicketCreated {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::TicketAssigned => LiveEvent::TicketAssigned {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::TicketStateChanged => LiveEvent::TicketStateChanged {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::TicketEdited => LiveEvent::TicketEdited {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::ProfileDefined => LiveEvent::ProfileDefined {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::ProfileUpdated => LiveEvent::ProfileUpdated {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::ProfileRetired => LiveEvent::ProfileRetired {
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
        LiveEventName::WorkspaceRegistered => LiveEvent::WorkspaceRegistered {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::WorkspaceObserved => LiveEvent::WorkspaceObserved {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::WorkspaceRetired => LiveEvent::WorkspaceRetired {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::LaneCreated => LiveEvent::LaneCreated {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::LaneWorkspaceAssigned => LiveEvent::LaneWorkspaceAssigned {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::LaneWorkspaceReleased => LiveEvent::LaneWorkspaceReleased {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::LaneTicketAssigned => LiveEvent::LaneTicketAssigned {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::LaneTicketReleased => LiveEvent::LaneTicketReleased {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::CloneCreated => LiveEvent::CloneCreated {
            sequence,
            payload: decode_payload(name, &envelope.payload)?,
        },
        LiveEventName::CloneRemoved => LiveEvent::CloneRemoved {
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
                "herdr_workspace": "kanban.seed",
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
                    herdr_session: Some("kanban-main".to_owned()),
                    herdr_workspace: "kanban.seed".to_owned(),
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
    fn catalogued_profile_events_decode_typed_payloads() {
        let envelope = EventEnvelope {
            sequence: 6,
            event_type: "profile.defined".to_owned(),
            payload: json!({
                "name": "standard",
                "harness": "claude-code",
                "model": "opus",
                "effort": "high",
                "usage_pool": "operator",
                "fallback": null,
                "retired": false,
                "version": 1,
            }),
        };

        let event = decode_live_event(&envelope).expect("the envelope decodes");
        assert_eq!(
            event,
            LiveEvent::ProfileDefined {
                sequence: 6,
                payload: ProfileRecord {
                    name: "standard".to_owned(),
                    harness: "claude-code".to_owned(),
                    model: "opus".to_owned(),
                    effort: "high".to_owned(),
                    usage_pool: "operator".to_owned(),
                    fallback: None,
                    retired: false,
                    version: 1,
                },
            }
        );
        assert_eq!(
            LiveEventName::parse("profile.updated")
                .expect("the name parses")
                .as_str(),
            "profile.updated"
        );
        assert_eq!(
            LiveEventName::parse("profile.retired")
                .expect("the name parses")
                .as_str(),
            "profile.retired"
        );
        assert_eq!(
            LiveEventName::parse("ticket.assigned")
                .expect("the name parses")
                .as_str(),
            "ticket.assigned"
        );
    }

    #[test]
    fn catalogued_comment_events_decode_typed_payloads() {
        let envelope = EventEnvelope {
            sequence: 2,
            event_type: "comment.edited".to_owned(),
            payload: json!({
                "id": 4,
                "project_id": 1,
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
                    project_id: 1,
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
    fn catalogued_spec_events_decode_typed_payloads() {
        let envelope = EventEnvelope {
            sequence: 7,
            event_type: "spec.version.approved".to_owned(),
            payload: json!({
                "id": 6,
                "project_id": 2,
                "number": 4,
                "name": "Plans and specifications",
                "execution": "unplanned",
                "plan_id": null,
                "version": 3,
            }),
        };

        let event = decode_live_event(&envelope).expect("the envelope decodes");
        assert_eq!(
            event,
            LiveEvent::SpecVersionApproved {
                sequence: 7,
                payload: SpecRecord {
                    id: 6,
                    project_id: 2,
                    number: 4,
                    name: "Plans and specifications".to_owned(),
                    execution: crate::spec::SpecExecutionState::Unplanned,
                    plan_id: None,
                    version: 3,
                },
            }
        );
        assert_eq!(
            LiveEventName::parse("spec.planned")
                .expect("the name parses")
                .as_str(),
            "spec.planned"
        );
    }

    #[test]
    fn catalogued_evidence_events_decode_typed_payloads() {
        let envelope = EventEnvelope {
            sequence: 5,
            event_type: "evidence.listed".to_owned(),
            payload: json!({ "project_id": 1, "count": 2 }),
        };

        let event = decode_live_event(&envelope).expect("the envelope decodes");
        assert_eq!(
            event,
            LiveEvent::EvidenceListed {
                sequence: 5,
                payload: EvidenceListSummary {
                    project_id: 1,
                    count: 2,
                },
            }
        );
    }

    #[test]
    fn catalogued_lane_events_decode_typed_payloads() {
        let envelope = EventEnvelope {
            sequence: 6,
            event_type: "lane.workspace.assigned".to_owned(),
            payload: json!({
                "id": 1,
                "project_id": 2,
                "workspace_id": 3,
                "ticket_id": null,
                "version": 2,
            }),
        };

        let event = decode_live_event(&envelope).expect("the envelope decodes");
        assert_eq!(
            event,
            LiveEvent::LaneWorkspaceAssigned {
                sequence: 6,
                payload: LaneRecord {
                    id: 1,
                    project_id: 2,
                    workspace_id: Some(3),
                    ticket_id: None,
                    version: 2,
                },
            }
        );
    }

    #[test]
    fn catalogued_clone_events_decode_typed_payloads() {
        let created = EventEnvelope {
            sequence: 8,
            event_type: "clone.created".to_owned(),
            payload: json!({
                "project_id": 1,
                "path": "/workspaces/kanban.fleet-t34",
                "branch": "fleet/kan-t34",
            }),
        };

        let event = decode_live_event(&created).expect("the envelope decodes");
        assert_eq!(
            event,
            LiveEvent::CloneCreated {
                sequence: 8,
                payload: CloneCreatedRecord {
                    project_id: 1,
                    path: "/workspaces/kanban.fleet-t34".to_owned(),
                    branch: "fleet/kan-t34".to_owned(),
                },
            }
        );

        let removed = EventEnvelope {
            sequence: 9,
            event_type: "clone.removed".to_owned(),
            payload: json!({
                "project_id": 1,
                "workspace_id": 2,
                "path": "/workspaces/kanban.fleet-t34",
                "branch": null,
            }),
        };

        let event = decode_live_event(&removed).expect("the envelope decodes");
        assert_eq!(
            event,
            LiveEvent::CloneRemoved {
                sequence: 9,
                payload: CloneRemovedRecord {
                    project_id: 1,
                    workspace_id: 2,
                    path: "/workspaces/kanban.fleet-t34".to_owned(),
                    branch: None,
                },
            }
        );
    }

    #[test]
    fn live_event_catalogue_lists_every_variant() {
        assert_eq!(live_event_catalog().len(), LiveEventName::ALL.len());
        for name in LiveEventName::ALL {
            let descriptor = event_descriptor(*name);
            assert_eq!(descriptor.name, *name);
            assert_eq!(descriptor.name.as_str(), name.as_str());
        }
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
