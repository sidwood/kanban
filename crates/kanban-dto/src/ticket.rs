//! Ticket payload definitions: the kind, priority, severity, and
//! lifecycle vocabularies, the per-kind creation payload, the Bug's
//! qualification and vendor-neutral facts payloads, and the record
//! every client sees (KAN-S4-US1 through KAN-S4-US3). Each kind sends
//! exactly its own fields on creation — an Implementation attaches to
//! one Spec and carries its slice and story-linked criteria; a Bug
//! carries its quick-capture facts; a Task carries a title and an
//! optional attachment.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The closed Ticket kind vocabulary on the wire (DR-TK-01).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketKind {
    /// A small vertical product slice of exactly one Spec.
    Implementation,
    /// A record of incorrect behaviour, with qualification evidence.
    Bug,
    /// Bounded operational, investigative, or administrative work.
    Task,
}

impl TicketKind {
    /// Every kind, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Implementation, Self::Bug, Self::Task];

    /// The wire name, matching this kind's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Implementation => "implementation",
            Self::Bug => "bug",
            Self::Task => "task",
        }
    }

    /// The kind `wire` names, or `None` outside the vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.as_str() == wire)
    }
}

/// The closed priority vocabulary on the wire (DR-LC-12): urgent,
/// high, normal, low.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketPriority {
    /// Ahead of everything else.
    Urgent,
    /// Ahead of normal work.
    High,
    /// The everyday order.
    Normal,
    /// Behind normal work.
    Low,
}

impl TicketPriority {
    /// Every priority, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Urgent, Self::High, Self::Normal, Self::Low];

    /// The wire name, matching this priority's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Urgent => "urgent",
            Self::High => "high",
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }

    /// The priority `wire` names, or `None` outside the vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|priority| priority.as_str() == wire)
    }
}

/// The closed Ticket lifecycle vocabulary on the wire (DR-LC-01): the
/// canonical states in order, with the terminal states after them.
/// Every Ticket is created into draft; the transition rules are the
/// lifecycle slice's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketState {
    /// Newly created; not yet qualified or planned.
    Draft,
    /// Deliberately set aside.
    Parked,
    /// Waiting on something outside the Ticket.
    Blocked,
    /// Qualified but unavailable until activation.
    Scheduled,
    /// Free to execute when capacity allows.
    Ready,
    /// Executing.
    Active,
    /// Under review.
    InReview,
    /// Review approved, waiting to land.
    Approved,
    /// Landing through the Seed Workspace.
    Landing,
    /// Landed in full.
    Done,
    /// Terminal: will not execute.
    Cancelled,
    /// Terminal: replaced by a reassignment Ticket.
    Superseded,
}

impl TicketState {
    /// Every state, canonical order first, terminal states last.
    pub const ALL: &'static [Self] = &[
        Self::Draft,
        Self::Parked,
        Self::Blocked,
        Self::Scheduled,
        Self::Ready,
        Self::Active,
        Self::InReview,
        Self::Approved,
        Self::Landing,
        Self::Done,
        Self::Cancelled,
        Self::Superseded,
    ];

    /// The wire name, matching this state's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Parked => "parked",
            Self::Blocked => "blocked",
            Self::Scheduled => "scheduled",
            Self::Ready => "ready",
            Self::Active => "active",
            Self::InReview => "in_review",
            Self::Approved => "approved",
            Self::Landing => "landing",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Superseded => "superseded",
        }
    }

    /// The state `wire` names, or `None` outside the vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|state| state.as_str() == wire)
    }
}

/// The closed Bug severity vocabulary on the wire (DR-LC-13):
/// critical, high, medium, low. Severity arrives only inside a
/// qualification (DR-TK-09).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketSeverity {
    /// Data loss, secret exposure, or unauthorised execution.
    Critical,
    /// A failed approved criterion or workflow invariant.
    High,
    /// Usable behaviour, wrongly shaped.
    Medium,
    /// Cosmetic or local clarity.
    Low,
}

impl TicketSeverity {
    /// Every severity, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Critical, Self::High, Self::Medium, Self::Low];

    /// The wire name, matching this severity's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    /// The severity `wire` names, or `None` outside the vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|severity| severity.as_str() == wire)
    }
}

/// One story-linked criterion a Ticket claims: an observable outcome
/// and the User Stories it delivers, named like `CORE-S3-US6` or
/// `S3-US6` (DR-TK-04, DR-PS-13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketCriterion {
    /// The observable outcome the criterion states.
    pub outcome: String,
    /// The User Stories the criterion claims.
    pub stories: Vec<String>,
}

/// One Verification Step a Ticket claims: the command or scripted
/// procedure that demonstrates a criterion (DR-PS-15). Commands live
/// here and never as criteria.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketVerificationStep {
    /// The command as it runs.
    pub command: String,
}

/// One complete Bug qualification (DR-TK-09): the ten facts a Bug
/// needs before it may leave draft, with the severity qualification
/// assigned (DR-LC-13). Sent whole; qualification is one act.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketBugQualification {
    /// The behaviour that should have happened.
    pub expected_behaviour: String,
    /// The reproduction steps or the failing test demonstrating the
    /// defect.
    pub reproduction: String,
    /// Where the defect occurs.
    pub environment: String,
    /// The severity qualification assigned (DR-LC-13).
    pub severity: TicketSeverity,
    /// How often the defect occurs.
    pub frequency: String,
    /// What the defect affects.
    pub affected_scope: String,
    /// What the defect puts at risk.
    pub risk: String,
    /// The story-linked Acceptance Criteria, at least one.
    pub criteria: Vec<TicketCriterion>,
    /// The Verification Steps, at least one.
    pub verification_steps: Vec<TicketVerificationStep>,
}

/// One vendor-neutral External Reference a Bug carries (DR-TK-10): a
/// URI naming an outside link, with an optional label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketExternalReference {
    /// The referenced URI.
    pub uri: String,
    /// The reference's label, when it carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// One Occurrence Snapshot a Bug carries (DR-TK-10): the RFC 3339
/// moment one occurrence was observed and what was seen then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketOccurrenceSnapshot {
    /// The observed moment, RFC 3339.
    pub observed_at: String,
    /// What was observed at that moment.
    pub observation: String,
}

/// The Bug-specific body of a Ticket record: the quick-capture facts,
/// the qualification once one exists, and the vendor-neutral
/// collections the Bug carries (DR-TK-08 to DR-TK-10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketBugRecord {
    /// What the Bug records happening, as the reporter stated it.
    pub actual_behaviour: String,
    /// The evidence the reporter holds, as the reporter stated it.
    pub reporter_evidence: String,
    /// The complete qualification, once one exists (DR-TK-09).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualification: Option<TicketBugQualification>,
    /// The External References, in recording order (DR-TK-10).
    pub external_references: Vec<TicketExternalReference>,
    /// The Occurrence Snapshots, in recording order (DR-TK-10).
    pub occurrence_snapshots: Vec<TicketOccurrenceSnapshot>,
    /// The identities of the Evidence Items this Bug carries
    /// (DR-TK-10).
    pub evidence_ids: Vec<u64>,
}

/// Request payload for the `ticket.create` command. Each kind sends
/// exactly its own fields: an Implementation names its Spec, slice,
/// and criteria; a Bug names its quick-capture facts; a Task names
/// its title. Fields the kind does not carry are simply absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketCreateRequest {
    pub mutation: super::MutationContext,
    /// The Project the new Ticket belongs to.
    pub project_id: u64,
    /// The kind whose schema the Ticket carries.
    pub kind: TicketKind,
    /// The Ticket's priority (DR-LC-12).
    pub priority: TicketPriority,
    /// The Spec this Ticket attaches to. An Implementation must name
    /// exactly one (DR-TK-02); a Bug or Task may name one or none
    /// (DR-TK-03).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_id: Option<u64>,
    /// The Bug or Task title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The Bug's actual behaviour (DR-TK-08); required for a Bug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_behaviour: Option<String>,
    /// The Bug's reporter evidence (DR-TK-08); required for a Bug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporter_evidence: Option<String>,
    /// The Implementation slice description, naming the behaviour
    /// delivered end to end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slice: Option<String>,
    /// The Implementation's story-linked criteria.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<Vec<TicketCriterion>>,
}

/// Request payload for the `ticket.bug.qualify` command: one complete
/// qualification for one Bug (DR-TK-09), replacing any earlier one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketBugQualifyRequest {
    pub mutation: super::MutationContext,
    /// The Bug being qualified.
    pub ticket_id: u64,
    /// The complete qualification, severity included (DR-LC-13).
    pub qualification: TicketBugQualification,
}

/// Request payload for the `ticket.bug.facts` command: the
/// vendor-neutral collections one Bug carries (DR-TK-10), replaced
/// whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketBugFactsRequest {
    pub mutation: super::MutationContext,
    /// The Bug carrying the collections.
    pub ticket_id: u64,
    /// The External References, in recording order.
    pub external_references: Vec<TicketExternalReference>,
    /// The Occurrence Snapshots, in recording order.
    pub occurrence_snapshots: Vec<TicketOccurrenceSnapshot>,
    /// The identities of the Evidence Items attached to this Bug.
    pub evidence_ids: Vec<u64>,
}

/// The Ticket record as every client sees it: the Project it belongs
/// to, the number that Project minted, the kind whose schema it
/// carries, and the kind-specific fields — a title for Bugs and
/// Tasks, a slice and criteria for Implementations, and the Bug body
/// for a Bug.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketRecord {
    /// The immutable, storage-assigned identity.
    pub id: u64,
    /// The Project this Ticket belongs to.
    pub project_id: u64,
    /// The number this Project minted for this Ticket; rendered with
    /// the Project's code, for example `CORE-T17`.
    pub number: u64,
    /// The kind whose schema this Ticket carries.
    pub kind: TicketKind,
    /// The Ticket's priority (DR-LC-12).
    pub priority: TicketPriority,
    /// The lifecycle state (DR-LC-01).
    pub state: TicketState,
    /// The Spec this Ticket attaches to, if it attaches to one.
    pub spec_id: Option<u64>,
    /// The Bug or Task title, if this Ticket carries one.
    pub title: Option<String>,
    /// The Implementation slice description, if this Ticket carries
    /// one.
    pub slice: Option<String>,
    /// The Implementation's story-linked criteria; empty for every
    /// other kind.
    pub criteria: Vec<TicketCriterion>,
    /// The Bug body, if this Ticket carries one.
    pub bug: Option<TicketBugRecord>,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// Request payload for the `ticket.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketListQuery {
    /// The Project whose Tickets are listed, terminal states
    /// included.
    pub project_id: u64,
}

/// Response payload for the `ticket.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketListResponse {
    /// Every Ticket of the Project, oldest first, all lifecycle
    /// states included.
    pub tickets: Vec<TicketRecord>,
}

/// Request payload for the `ticket.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketGetQuery {
    /// The Ticket being read.
    pub ticket_id: u64,
}

/// Request payload for the `ticket.dependency.add` command: the
/// blocking Ticket must land before the waiting Ticket may begin
/// (DR-DE-02). Both endpoints must name registered Tickets; work no
/// Ticket carries is recorded with `ticket.blocker.add` instead
/// (DR-DE-04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketDependencyAddRequest {
    pub mutation: super::MutationContext,
    /// The Ticket that must land first.
    pub from_ticket: u64,
    /// The Ticket that waits on `from_ticket`.
    pub to_ticket: u64,
}

/// Request payload for the `ticket.dependency.remove` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketDependencyRemoveRequest {
    pub mutation: super::MutationContext,
    /// The Ticket that must land first.
    pub from_ticket: u64,
    /// The Ticket that waits on `from_ticket`.
    pub to_ticket: u64,
}

/// Request payload for the `ticket.blocker.add` command: one explicit
/// external blocker, naming the unregistered work a Ticket waits on
/// in prose (DR-DE-04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketBlockerAddRequest {
    pub mutation: super::MutationContext,
    /// The Ticket waiting on the described work.
    pub ticket_id: u64,
    /// The unregistered work, in prose.
    pub description: String,
}

/// Request payload for the `ticket.blocker.remove` command: removing
/// an external blocker is the explicit operator action that clears
/// it (DR-DE-04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketBlockerRemoveRequest {
    pub mutation: super::MutationContext,
    /// The Ticket the blocker was recorded against.
    pub ticket_id: u64,
    /// The recorded blocker being removed.
    pub blocker_id: u64,
}

/// One registered dependency as every client sees it: the blocking
/// Ticket, the Project it belongs to, the number that Project minted
/// for it, and its lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketDependencyRecord {
    /// The blocking Ticket: it must land first (DR-DE-02).
    pub from_ticket_id: u64,
    /// The Project the blocking Ticket belongs to.
    pub from_project_id: u64,
    /// The number the blocking Ticket's Project minted for it;
    /// rendered with the Project's code, for example `CORE-T17`.
    pub from_number: u64,
    /// The blocking Ticket's lifecycle state.
    pub from_state: TicketState,
}

/// One recorded external blocker as every client sees it
/// (DR-DE-04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketBlockerRecord {
    /// The immutable, storage-assigned identity.
    pub id: u64,
    /// The Ticket waiting on the described work.
    pub ticket_id: u64,
    /// The unregistered work, in prose.
    pub description: String,
}

/// Request payload for the `ticket.dependencies` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketDependenciesQuery {
    /// The Ticket whose dependencies and blockers are read.
    pub ticket_id: u64,
}

/// Response payload for the `ticket.dependencies` query and for
/// every dependency and blocker command: the registered
/// dependencies one Ticket waits on, its explicit external blockers,
/// and the Ticket's aggregate version after the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketDependenciesResponse {
    /// The Ticket the dependencies belong to.
    pub ticket_id: u64,
    /// The Ticket's aggregate version, for optimistic checks.
    pub version: u64,
    /// The registered dependencies the Ticket waits on, in
    /// registration order.
    pub dependencies: Vec<TicketDependencyRecord>,
    /// The Ticket's explicit external blockers, in recording order.
    pub blockers: Vec<TicketBlockerRecord>,
}

/// Request payload for the `ticket.readiness` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketReadinessQuery {
    /// The Ticket whose readiness is computed.
    pub ticket_id: u64,
}

/// What still holds one Ticket back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum TicketReadinessBlocker {
    /// A registered dependency whose blocker has not landed.
    Ticket {
        /// The blocking Ticket: it must land first (DR-DE-02).
        from_ticket_id: u64,
        /// The Project the blocking Ticket belongs to.
        from_project_id: u64,
        /// The number the blocking Ticket's Project minted for it.
        from_number: u64,
        /// The blocking Ticket's lifecycle state.
        from_state: TicketState,
    },
    /// An explicit external blocker (DR-DE-04).
    External {
        /// The recorded blocker.
        blocker_id: u64,
        /// The unregistered work, in prose.
        description: String,
    },
}

/// Response payload for the `ticket.readiness` query: the computed
/// readiness projection of one Ticket's dependencies and external
/// blockers (DR-DE-03). The projection never mutates state; dispatch
/// consumes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketReadinessResponse {
    /// The Ticket the readiness was computed for.
    pub ticket_id: u64,
    /// The Ticket's own lifecycle state, for context.
    pub state: TicketState,
    /// Whether nothing holds the Ticket back.
    pub ready: bool,
    /// What holds the Ticket back, dependencies first, then external
    /// blockers.
    pub blocked_by: Vec<TicketReadinessBlocker>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        TicketBlockerAddRequest, TicketBlockerRecord, TicketBlockerRemoveRequest,
        TicketBugFactsRequest, TicketBugQualification, TicketBugQualifyRequest, TicketBugRecord,
        TicketCreateRequest, TicketCriterion, TicketDependenciesQuery, TicketDependenciesResponse,
        TicketDependencyAddRequest, TicketDependencyRecord, TicketDependencyRemoveRequest,
        TicketExternalReference, TicketGetQuery, TicketKind, TicketListQuery, TicketListResponse,
        TicketOccurrenceSnapshot, TicketPriority, TicketReadinessBlocker, TicketReadinessResponse,
        TicketRecord, TicketSeverity, TicketState, TicketVerificationStep,
    };
    use crate::mutation::MutationContext;
    use crate::schema_definitions;

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 3,
            idempotency_key: "key-1".to_owned(),
        }
    }

    fn criterion() -> TicketCriterion {
        TicketCriterion {
            outcome: "Projects register with unique codes.".to_owned(),
            stories: vec!["CORE-S1-US1".to_owned(), "S1-US2".to_owned()],
        }
    }

    fn qualification() -> TicketBugQualification {
        TicketBugQualification {
            expected_behaviour: "The integration branch survives every landing.".to_owned(),
            reproduction: "Re land a reviewed change.".to_owned(),
            environment: "macOS 26.".to_owned(),
            severity: TicketSeverity::High,
            frequency: "Every landing.".to_owned(),
            affected_scope: "Landings.".to_owned(),
            risk: "Lost review state.".to_owned(),
            criteria: vec![criterion()],
            verification_steps: vec![TicketVerificationStep {
                command: "cargo test -p kanban-domain bug_qualification".to_owned(),
            }],
        }
    }

    fn bug_record() -> TicketBugRecord {
        TicketBugRecord {
            actual_behaviour: "The integration branch is dropped.".to_owned(),
            reporter_evidence: "The landing log names the drop.".to_owned(),
            qualification: Some(qualification()),
            external_references: vec![TicketExternalReference {
                uri: "https://example.invalid/issues/12".to_owned(),
                label: Some("The report".to_owned()),
            }],
            occurrence_snapshots: vec![TicketOccurrenceSnapshot {
                observed_at: "2026-09-05T07:41:00Z".to_owned(),
                observation: "The log shows the drop.".to_owned(),
            }],
            evidence_ids: vec![2],
        }
    }

    fn record() -> TicketRecord {
        TicketRecord {
            id: 6,
            project_id: 2,
            number: 17,
            kind: TicketKind::Implementation,
            priority: TicketPriority::High,
            state: TicketState::Draft,
            spec_id: Some(4),
            title: None,
            slice: Some("Registration creates Projects end to end".to_owned()),
            criteria: vec![criterion()],
            bug: None,
            version: 1,
        }
    }

    #[test]
    fn kinds_priorities_severities_and_states_round_trip_their_wire_names() {
        for kind in TicketKind::ALL {
            assert_eq!(
                TicketKind::parse(kind.as_str()),
                Some(*kind),
                "`{}` must survive the round trip",
                kind.as_str()
            );
            assert_eq!(
                serde_json::to_value(kind).expect("the kind encodes"),
                json!(kind.as_str()),
                "the wire name and the serialised name must agree"
            );
        }
        assert_eq!(TicketKind::parse("ghost"), None);

        for priority in TicketPriority::ALL {
            assert_eq!(
                TicketPriority::parse(priority.as_str()),
                Some(*priority),
                "`{}` must survive the round trip",
                priority.as_str()
            );
            assert_eq!(
                serde_json::to_value(priority).expect("the priority encodes"),
                json!(priority.as_str())
            );
        }
        assert_eq!(TicketPriority::parse("ghost"), None);

        for severity in TicketSeverity::ALL {
            assert_eq!(
                TicketSeverity::parse(severity.as_str()),
                Some(*severity),
                "`{}` must survive the round trip",
                severity.as_str()
            );
            assert_eq!(
                serde_json::to_value(severity).expect("the severity encodes"),
                json!(severity.as_str())
            );
        }
        assert_eq!(TicketSeverity::parse("ghost"), None);
        assert_eq!(
            TicketSeverity::parse("urgent"),
            None,
            "priority is not severity"
        );

        for state in TicketState::ALL {
            assert_eq!(
                TicketState::parse(state.as_str()),
                Some(*state),
                "`{}` must survive the round trip",
                state.as_str()
            );
        }
        assert_eq!(TicketState::parse("ghost"), None);
        assert_eq!(
            serde_json::to_value(TicketState::InReview).expect("the state encodes"),
            json!("in_review")
        );
    }

    #[test]
    fn a_record_round_trips_with_its_kind_specific_fields() {
        let encoded = serde_json::to_value(record()).expect("the record serialises");

        assert_eq!(
            encoded,
            json!({
                "id": 6,
                "project_id": 2,
                "number": 17,
                "kind": "implementation",
                "priority": "high",
                "state": "draft",
                "spec_id": 4,
                "title": null,
                "slice": "Registration creates Projects end to end",
                "criteria": [
                    {
                        "outcome": "Projects register with unique codes.",
                        "stories": ["CORE-S1-US1", "S1-US2"],
                    }
                ],
                "bug": null,
                "version": 1,
            })
        );
        let decoded: TicketRecord =
            serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, record());

        let bug = TicketRecord {
            kind: TicketKind::Bug,
            priority: TicketPriority::Urgent,
            spec_id: None,
            title: Some("Landing drops the integration branch".to_owned()),
            slice: None,
            criteria: Vec::new(),
            bug: Some(bug_record()),
            ..record()
        };
        let encoded = serde_json::to_value(&bug).expect("the record serialises");
        assert_eq!(
            encoded["title"],
            json!("Landing drops the integration branch")
        );
        assert_eq!(encoded["spec_id"], json!(null));
        assert_eq!(encoded["criteria"], json!([]));
        assert_eq!(
            encoded["bug"]["qualification"]["severity"],
            json!("high"),
            "severity arrives only inside a qualification (DR-LC-13)"
        );
        assert_eq!(encoded["bug"]["evidence_ids"], json!([2]));
        let decoded: TicketRecord =
            serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, bug);

        let captured = TicketBugRecord {
            qualification: None,
            external_references: Vec::new(),
            occurrence_snapshots: Vec::new(),
            evidence_ids: Vec::new(),
            ..bug_record()
        };
        let encoded = serde_json::to_value(&captured).expect("the record serialises");
        assert!(
            encoded.get("qualification").is_none(),
            "an absent qualification carries no field at all"
        );
    }

    #[test]
    fn every_request_round_trips_and_rejects_unknown_fields() {
        round_trips::<TicketCreateRequest>(json!({
            "mutation": context(),
            "project_id": 2,
            "kind": "implementation",
            "priority": "high",
            "spec_id": 4,
            "slice": "Registration creates Projects end to end",
            "criteria": [criterion()],
        }));
        // A quick-captured Bug sends its three capture facts and
        // nothing else (DR-TK-08).
        round_trips::<TicketCreateRequest>(json!({
            "mutation": context(),
            "project_id": 2,
            "kind": "bug",
            "priority": "normal",
            "title": "Landing drops the integration branch",
            "actual_behaviour": "The integration branch is dropped.",
            "reporter_evidence": "The landing log names the drop.",
        }));

        round_trips::<TicketBugQualifyRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
            "qualification": qualification(),
        }));
        round_trips::<TicketBugFactsRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
            "external_references": [
                { "uri": "https://example.invalid/issues/12", "label": "The report" }
            ],
            "occurrence_snapshots": [
                {
                    "observed_at": "2026-09-05T07:41:00Z",
                    "observation": "The log shows the drop.",
                }
            ],
            "evidence_ids": [2],
        }));

        let list: TicketListQuery =
            serde_json::from_value(json!({ "project_id": 2 })).expect("the list query decodes");
        assert_eq!(list, TicketListQuery { project_id: 2 });

        let get: TicketGetQuery =
            serde_json::from_value(json!({ "ticket_id": 6 })).expect("the get query decodes");
        assert_eq!(get, TicketGetQuery { ticket_id: 6 });

        round_trips::<TicketDependencyAddRequest>(json!({
            "mutation": context(),
            "from_ticket": 3,
            "to_ticket": 9,
        }));
        round_trips::<TicketDependencyRemoveRequest>(json!({
            "mutation": context(),
            "from_ticket": 3,
            "to_ticket": 9,
        }));
        round_trips::<TicketBlockerAddRequest>(json!({
            "mutation": context(),
            "ticket_id": 9,
            "description": "The vendor SDK 4 upgrade",
        }));
        round_trips::<TicketBlockerRemoveRequest>(json!({
            "mutation": context(),
            "ticket_id": 9,
            "blocker_id": 4,
        }));

        let dependencies: TicketDependenciesQuery =
            serde_json::from_value(json!({ "ticket_id": 9 }))
                .expect("the dependencies query decodes");
        assert_eq!(dependencies, TicketDependenciesQuery { ticket_id: 9 });

        let response = TicketListResponse {
            tickets: vec![record()],
        };
        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(encoded["tickets"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn the_dependency_and_readiness_records_round_trip() {
        let dependency = TicketDependencyRecord {
            from_ticket_id: 3,
            from_project_id: 2,
            from_number: 17,
            from_state: TicketState::Active,
        };
        let blocker = TicketBlockerRecord {
            id: 4,
            ticket_id: 9,
            description: "The vendor SDK 4 upgrade".to_owned(),
        };
        let dependencies = TicketDependenciesResponse {
            ticket_id: 9,
            version: 5,
            dependencies: vec![dependency],
            blockers: vec![blocker.clone()],
        };

        let encoded = serde_json::to_value(&dependencies).expect("the response serialises");
        assert_eq!(
            encoded,
            json!({
                "ticket_id": 9,
                "version": 5,
                "dependencies": [{
                    "from_ticket_id": 3,
                    "from_project_id": 2,
                    "from_number": 17,
                    "from_state": "active",
                }],
                "blockers": [{
                    "id": 4,
                    "ticket_id": 9,
                    "description": "The vendor SDK 4 upgrade",
                }],
            })
        );
        let decoded: TicketDependenciesResponse =
            serde_json::from_value(encoded).expect("the response deserialises");
        assert_eq!(decoded, dependencies);
        assert!(
            serde_json::from_value::<TicketDependenciesResponse>(
                serde_json::json!({ "ticket_id": 9, "version": 5, "dependencies": [], "blockers": [], "surprise": true })
            )
            .is_err(),
            "unknown fields are rejected"
        );

        let readiness = TicketReadinessResponse {
            ticket_id: 9,
            state: TicketState::Draft,
            ready: false,
            blocked_by: vec![
                TicketReadinessBlocker::Ticket {
                    from_ticket_id: dependency.from_ticket_id,
                    from_project_id: dependency.from_project_id,
                    from_number: dependency.from_number,
                    from_state: dependency.from_state,
                },
                TicketReadinessBlocker::External {
                    blocker_id: blocker.id,
                    description: blocker.description.clone(),
                },
            ],
        };

        let encoded = serde_json::to_value(&readiness).expect("the readiness serialises");
        assert_eq!(
            encoded,
            json!({
                "ticket_id": 9,
                "state": "draft",
                "ready": false,
                "blocked_by": [
                    { "Ticket": {
                        "from_ticket_id": 3,
                        "from_project_id": 2,
                        "from_number": 17,
                        "from_state": "active",
                    }},
                    { "External": {
                        "blocker_id": 4,
                        "description": "The vendor SDK 4 upgrade",
                    }},
                ],
            })
        );
        let decoded: TicketReadinessResponse =
            serde_json::from_value(encoded).expect("the readiness deserialises");
        assert_eq!(decoded, readiness);
    }

    /// One request wire form decodes typed, re-encodes identically,
    /// and refuses an unknown field.
    fn round_trips<Request>(wire: serde_json::Value)
    where
        Request: serde::de::DeserializeOwned + serde::Serialize,
    {
        let decoded: Request =
            serde_json::from_value(wire.clone()).expect("the request decodes typed");
        let encoded = serde_json::to_value(&decoded).expect("the request re-encodes");
        assert_eq!(encoded, wire, "the wire form round trips");

        let mut refused = wire;
        refused["surprise"] = json!(true);
        assert!(
            serde_json::from_value::<Request>(refused).is_err(),
            "unknown fields are rejected"
        );
    }

    /// The schema of one registered DTO, proving registration.
    fn schema_of(name: &str) -> serde_json::Value {
        let (_, schema) = schema_definitions()
            .into_iter()
            .find(|(schema_name, _)| *schema_name == name)
            .unwrap_or_else(|| panic!("{name} is registered"));
        serde_json::to_value(schema).expect("the schema serialises")
    }

    #[test]
    fn every_ticket_schema_rejects_unknown_fields() {
        for name in [
            "TicketBlockerAddRequest",
            "TicketBlockerRecord",
            "TicketBlockerRemoveRequest",
            "TicketBugFactsRequest",
            "TicketBugQualification",
            "TicketBugQualifyRequest",
            "TicketBugRecord",
            "TicketCreateRequest",
            "TicketCriterion",
            "TicketDependenciesQuery",
            "TicketDependenciesResponse",
            "TicketDependencyAddRequest",
            "TicketDependencyRecord",
            "TicketDependencyRemoveRequest",
            "TicketExternalReference",
            "TicketGetQuery",
            "TicketKind",
            "TicketListQuery",
            "TicketListResponse",
            "TicketOccurrenceSnapshot",
            "TicketPriority",
            "TicketReadinessBlocker",
            "TicketReadinessQuery",
            "TicketReadinessResponse",
            "TicketRecord",
            "TicketSeverity",
            "TicketState",
            "TicketVerificationStep",
        ] {
            let schema = schema_of(name);
            let encoded = serde_json::to_string(&schema).expect("the schema serialises");
            assert!(
                encoded.contains("\"additionalProperties\":false") || encoded.contains("\"enum\":"),
                "{name} should reject unknown fields or close its vocabulary"
            );
        }
    }
}
