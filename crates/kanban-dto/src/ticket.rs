//! Ticket payload definitions: the kind, priority, severity, and
//! lifecycle vocabularies, the per-kind creation payload, the Bug's
//! qualification and vendor-neutral facts payloads, the lifecycle
//! command payloads — a drag, the named human commands, and the
//! audited emergency override (DR-LC-07 to DR-LC-10) — and the record
//! every client sees (KAN-S4-US1 through KAN-S4-US6). Each kind sends
//! exactly its own fields on creation — an Implementation attaches to
//! one Spec and carries its slice and story-linked criteria; a Bug
//! carries its quick-capture facts; a Task carries a title, one
//! subtype of the closed set, a human-or-agent mode, completion
//! criteria, and optional schedule or due-date timing.

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

/// The closed Task subtype vocabulary on the wire (DR-TK-06): the
/// seven bounded flavours of non-story work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskSubtype {
    /// Keeping the product and its operations running.
    Operational,
    /// Finding out what is true before acting on it.
    Investigative,
    /// Bookkeeping and record-keeping work.
    Administrative,
    /// Learning without a deliverable committed up front.
    Research,
    /// Proving an approach before committing to it.
    Prototype,
    /// Moving work or data from one place to another.
    Migration,
    /// Work done by hand, on purpose.
    Manual,
}

impl TaskSubtype {
    /// Every subtype, in vocabulary order.
    pub const ALL: &'static [Self] = &[
        Self::Operational,
        Self::Investigative,
        Self::Administrative,
        Self::Research,
        Self::Prototype,
        Self::Migration,
        Self::Manual,
    ];

    /// The wire name, matching this subtype's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Operational => "operational",
            Self::Investigative => "investigative",
            Self::Administrative => "administrative",
            Self::Research => "research",
            Self::Prototype => "prototype",
            Self::Migration => "migration",
            Self::Manual => "manual",
        }
    }

    /// The subtype `wire` names, or `None` outside the closed set.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|subtype| subtype.as_str() == wire)
    }
}

/// The closed Task mode vocabulary on the wire (KAN-S4-US4): whether
/// a human or an agent executes the bounded work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskMode {
    /// Sid executes the work.
    Human,
    /// An agent executes the work under dispatch.
    Agent,
}

impl TaskMode {
    /// Every mode, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Human, Self::Agent];

    /// The wire name, matching this mode's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
        }
    }

    /// The mode `wire` names, or `None` outside the closed set.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|mode| mode.as_str() == wire)
    }
}

/// One explicit human review decision on the wire (DR-LC-09): the
/// decision the lifecycle records when a review resolves. The review
/// flows that stage findings are KAN-S10's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketReviewDecision {
    /// The review approves; the Ticket waits to land.
    Approve,
    /// The review rejects; the Ticket returns to work.
    Reject,
}

impl TicketReviewDecision {
    /// Every decision, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Approve, Self::Reject];

    /// The wire name, matching this decision's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }

    /// The decision `wire` names, or `None` outside the vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|decision| decision.as_str() == wire)
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
/// its title, subtype, mode, completion criteria, and optional
/// timing. Fields the kind does not carry are simply absent.
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
    /// The Implementation's story-linked criteria. A Task never
    /// carries these (DR-TK-07); it sends `completion` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<Vec<TicketCriterion>>,
    /// The Task's subtype; a Task names exactly one (DR-TK-06).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtype: Option<TaskSubtype>,
    /// The Task's human-or-agent mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<TaskMode>,
    /// The Task's completion criteria: observable outcomes that bound
    /// the work, with no story links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<Vec<String>>,
    /// The Task's one-time activation instant, RFC 3339; stored for
    /// KAN-S11, whose activation behaviour is KAN-T53 and KAN-T54.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_for: Option<String>,
    /// The Task's due date, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
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

/// Request payload for the `ticket.assign` command: the Ticket's
/// assignment names one catalogue entry by reference, and an unknown
/// name is rejected (DR-EP-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketAssignRequest {
    pub mutation: super::MutationContext,
    /// The Ticket being assigned.
    pub ticket_id: u64,
    /// The Execution Profile the assignment names, by its catalogue
    /// name.
    pub profile: String,
}

/// Request payload for the `ticket.transition` command: a drag moves a
/// Task Ticket to a legal target along the canonical lifecycle
/// (DR-LC-07). A drag of an Implementation or Bug Ticket is refused
/// with the explanation that those transitions are agent-owned
/// (DR-LC-08).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketTransitionRequest {
    pub mutation: super::MutationContext,
    /// The Ticket being dragged.
    pub ticket_id: u64,
    /// The state the drag names.
    pub to: TicketState,
}

/// Request payload for the `ticket.park` command: set aside work that
/// has not started executing (DR-LC-09).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketParkRequest {
    pub mutation: super::MutationContext,
    /// The Ticket being parked.
    pub ticket_id: u64,
}

/// Request payload for the `ticket.unpark` command: return parked work
/// to circulation (DR-LC-09).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketUnparkRequest {
    pub mutation: super::MutationContext,
    /// The Ticket being unparked.
    pub ticket_id: u64,
}

/// Request payload for the `ticket.schedule` command: hold qualified
/// work until its activation (DR-LC-09). The optional Schedule facts
/// — activation instant, timezone, and eligible profile — arrive
/// together or not at all, and land the one-time Schedule that makes
/// the Ticket ready when its moment arrives (DR-SA-01, DR-SA-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketScheduleRequest {
    pub mutation: super::MutationContext,
    /// The Ticket being scheduled.
    pub ticket_id: u64,
    /// The one-time activation instant, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<String>,
    /// The IANA timezone the Schedule lives in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// The Execution Profile eligible once the activation fires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// Request payload for the `ticket.cancel` command: end the Ticket.
/// Cancelled is terminal and absent from the active board (DR-LC-02,
/// DR-LC-09).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketCancelRequest {
    pub mutation: super::MutationContext,
    /// The Ticket being cancelled.
    pub ticket_id: u64,
}

/// Request payload for the `ticket.review` command: record one
/// explicit human review decision (DR-LC-09).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketReviewRequest {
    pub mutation: super::MutationContext,
    /// The Ticket the decision resolves.
    pub ticket_id: u64,
    /// The decision recorded.
    pub decision: TicketReviewDecision,
}

/// Request payload for the `ticket.prioritise` command: set the
/// Ticket's priority from the closed vocabulary (DR-LC-09, DR-LC-12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketPrioritiseRequest {
    pub mutation: super::MutationContext,
    /// The Ticket being prioritised.
    pub ticket_id: u64,
    /// The priority being set.
    pub priority: TicketPriority,
}

/// Request payload for the `ticket.edit` command: edit the
/// human-authored description of an open Ticket (DR-LC-09) — the
/// title a Bug or Task carries or the slice description an
/// Implementation carries. Each kind sends exactly the field it owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketEditRequest {
    pub mutation: super::MutationContext,
    /// The Ticket being edited.
    pub ticket_id: u64,
    /// The new title, for a Bug or Task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The new slice description, for an Implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slice: Option<String>,
}

/// Request payload for the `ticket.emergency.override` command:
/// recovery moves a Ticket to any state past the rules, on the
/// strength of the justification the audit row carries — who ran it
/// and why (DR-LC-10). This command is the only way past the rules;
/// no unrestricted drag exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketEmergencyOverrideRequest {
    pub mutation: super::MutationContext,
    /// The Ticket being recovered.
    pub ticket_id: u64,
    /// The state recovery names.
    pub to: TicketState,
    /// Who ran the override, recorded on the timeline.
    pub who: String,
    /// Why the override ran, recorded on the timeline.
    pub why: String,
}

/// Request payload for the `ticket.spec.move` command: a draft,
/// unpinned Ticket moves its Spec attachment to another Spec of its
/// own Project (DR-DE-05). A pinned or executed Ticket stays where it
/// stands (DR-DE-06).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketSpecMoveRequest {
    pub mutation: super::MutationContext,
    /// The Ticket whose Spec attachment moves.
    pub ticket_id: u64,
    /// The Spec of the same Project the Ticket moves to.
    pub spec_id: u64,
}

/// The closed Ticket graph proposal lifecycle on the wire (DR-PS-16,
/// DR-PS-17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketGraphState {
    /// Recorded against an approved Spec version, awaiting the human
    /// approval gate.
    Proposed,
    /// Approved; every Ticket in the graph is pinned to the Spec
    /// content version the proposal named.
    Approved,
}

impl TicketGraphState {
    /// Every state, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Proposed, Self::Approved];

    /// The wire name, matching this state's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Approved => "approved",
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

/// One proposed dependency edge inside a Ticket graph: the blocking
/// Ticket must land before the waiting Ticket may begin (DR-DE-02).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketGraphEdgeProposal {
    /// The Ticket that must land first.
    pub from_ticket: u64,
    /// The Ticket that waits on `from_ticket`.
    pub to_ticket: u64,
}

/// Request payload for the `ticket.graph.propose` command: one
/// complete Ticket graph proposed against an approved Spec version
/// (DR-PS-16). Every named Ticket must be attached to the Spec; the
/// completeness of the set is the approval gate's to judge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketGraphProposeRequest {
    pub mutation: super::MutationContext,
    /// The Spec the graph proposes for.
    pub spec_id: u64,
    /// The approved Spec content version the graph is proposed
    /// against.
    pub spec_version: u64,
    /// Every Ticket the graph holds.
    pub tickets: Vec<u64>,
    /// The dependency edges between the Tickets the graph holds.
    pub edges: Vec<TicketGraphEdgeProposal>,
}

/// Request payload for the `ticket.graph.approve` command: the human
/// gate's decision on one proposed graph (DR-PS-17). Approval pins
/// every Ticket in the graph to the Spec version it named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketGraphApproveRequest {
    pub mutation: super::MutationContext,
    /// The proposal being approved.
    pub proposal_id: u64,
}

/// One dependency edge of a recorded Ticket graph, as every client
/// sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketGraphEdgeRecord {
    /// The Ticket that must land first (DR-DE-02).
    pub from_ticket: u64,
    /// The Ticket that waits on `from_ticket`.
    pub to_ticket: u64,
}

/// One Ticket graph proposal as every client sees it: the Spec
/// version it is proposed against, the Tickets and edges it holds,
/// and its lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketGraphRecord {
    /// The immutable, storage-assigned identity.
    pub id: u64,
    /// The Spec the graph proposes for.
    pub spec_id: u64,
    /// The Spec content version the graph is proposed against.
    pub spec_version: u64,
    /// The proposal's lifecycle state.
    pub state: TicketGraphState,
    /// Every Ticket the graph holds, in proposal order.
    pub tickets: Vec<u64>,
    /// The dependency edges between the Tickets the graph holds, in
    /// proposal order.
    pub edges: Vec<TicketGraphEdgeRecord>,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// Request payload for the `ticket.graph.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketGraphListQuery {
    /// The Spec whose proposed graphs are listed, every state
    /// included.
    pub spec_id: u64,
}

/// Response payload for the `ticket.graph.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketGraphListResponse {
    /// Every proposal recorded against the Spec, oldest first.
    pub proposals: Vec<TicketGraphRecord>,
}

/// Request payload for the `ticket.reassign` command (DR-DE-07):
/// reassignment creates a replacement Ticket under its kind's schema
/// and supersedes the named original. The replacement carries the
/// full creation shape — each kind sends exactly its own fields, as
/// `ticket.create` does — because a reassignment states its changed
/// plan whole, never a silent copy of the original. The replacement
/// references its predecessor in the record the command returns, and
/// the superseded original keeps its history and its number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketReassignRequest {
    pub mutation: super::MutationContext,
    /// The Ticket being replaced. It moves to the terminal superseded
    /// state from any open state; a terminal or landed original is
    /// refused.
    pub ticket_id: u64,
    /// The kind whose schema the replacement carries.
    pub kind: TicketKind,
    /// The replacement's priority (DR-LC-12).
    pub priority: TicketPriority,
    /// The Spec the replacement attaches to. An Implementation must
    /// name exactly one (DR-TK-02); a Bug or Task may name one or
    /// none (DR-TK-03).
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
    /// The Implementation's story-linked criteria. A Task never
    /// carries these (DR-TK-07); it sends `completion` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<Vec<TicketCriterion>>,
    /// The Task's subtype; a Task names exactly one (DR-TK-06).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtype: Option<TaskSubtype>,
    /// The Task's human-or-agent mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<TaskMode>,
    /// The Task's completion criteria: observable outcomes that bound
    /// the work, with no story links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<Vec<String>>,
    /// The Task's one-time activation instant, RFC 3339; stored for
    /// KAN-S11.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_for: Option<String>,
    /// The Task's due date, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
}

/// The Ticket record as every client sees it: the Project it belongs
/// to, the number that Project minted, the kind whose schema it
/// carries, the kind-specific fields — a title for Bugs and Tasks, a
/// slice and criteria for Implementations, the Bug body for a Bug,
/// and for Tasks the subtype, mode, completion criteria, and optional
/// timing — and the Execution Profile the assignment names, if it
/// carries one.
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
    /// The Task's subtype, if this Ticket carries one.
    pub subtype: Option<TaskSubtype>,
    /// The Task's human-or-agent mode, if this Ticket carries one.
    pub mode: Option<TaskMode>,
    /// The Task's completion criteria; empty for every other kind.
    pub completion: Vec<String>,
    /// The Task's one-time activation instant, if it carries one;
    /// RFC 3339 in UTC, stored for KAN-S11.
    pub scheduled_for: Option<String>,
    /// The Task's due date, if it carries one; RFC 3339 in UTC.
    pub due: Option<String>,
    /// The Execution Profile this Ticket's assignment names, by its
    /// catalogue name. The reference keeps its name through every
    /// later catalogue change (DR-EP-05).
    pub profile: Option<String>,
    /// The Spec content version an approved Ticket graph pinned this
    /// Ticket to, if one did (DR-DE-06); a pinned Ticket stays with
    /// its Spec and version.
    pub pinned_spec_version: Option<u64>,
    /// The Ticket this one replaced, when reassignment created it as a
    /// replacement (DR-DE-07); an ordinary Ticket references none and
    /// the field stays absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_id: Option<u64>,
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
        TaskMode, TaskSubtype, TicketAssignRequest, TicketBlockerAddRequest, TicketBlockerRecord,
        TicketBlockerRemoveRequest, TicketBugFactsRequest, TicketBugQualification,
        TicketBugQualifyRequest, TicketBugRecord, TicketCancelRequest, TicketCreateRequest,
        TicketCriterion, TicketDependenciesQuery, TicketDependenciesResponse,
        TicketDependencyAddRequest, TicketDependencyRecord, TicketDependencyRemoveRequest,
        TicketEditRequest, TicketEmergencyOverrideRequest, TicketExternalReference, TicketGetQuery,
        TicketGraphApproveRequest, TicketGraphEdgeProposal, TicketGraphEdgeRecord,
        TicketGraphListQuery, TicketGraphListResponse, TicketGraphProposeRequest,
        TicketGraphRecord, TicketGraphState, TicketKind, TicketListQuery, TicketListResponse,
        TicketOccurrenceSnapshot, TicketParkRequest, TicketPrioritiseRequest, TicketPriority,
        TicketReadinessBlocker, TicketReadinessResponse, TicketReassignRequest, TicketRecord,
        TicketReviewDecision, TicketReviewRequest, TicketScheduleRequest, TicketSeverity,
        TicketSpecMoveRequest, TicketState, TicketTransitionRequest, TicketUnparkRequest,
        TicketVerificationStep,
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
            subtype: None,
            mode: None,
            completion: Vec::new(),
            scheduled_for: None,
            due: None,
            profile: None,
            pinned_spec_version: None,
            predecessor_id: None,
            version: 1,
        }
    }

    #[test]
    fn subtypes_and_modes_round_trip_through_their_wire_names() {
        assert_eq!(TaskSubtype::ALL.len(), 7);
        for subtype in TaskSubtype::ALL {
            assert_eq!(
                TaskSubtype::parse(subtype.as_str()),
                Some(*subtype),
                "`{}` must survive the round trip",
                subtype.as_str()
            );
            assert_eq!(
                serde_json::to_value(subtype).expect("the subtype encodes"),
                json!(subtype.as_str()),
                "the wire name and the serialised name must agree"
            );
        }
        assert_eq!(TaskSubtype::parse("ghost"), None);

        assert_eq!(TaskMode::ALL.len(), 2);
        for mode in TaskMode::ALL {
            assert_eq!(TaskMode::parse(mode.as_str()), Some(*mode));
            assert_eq!(
                serde_json::to_value(mode).expect("the mode encodes"),
                json!(mode.as_str())
            );
        }
        assert_eq!(TaskMode::parse("ghost"), None);
    }

    #[test]
    fn a_task_record_round_trips_with_its_bounded_fields() {
        let task = TicketRecord {
            kind: TicketKind::Task,
            priority: TicketPriority::Low,
            spec_id: None,
            title: Some("Archive the old register".to_owned()),
            slice: None,
            criteria: Vec::new(),
            subtype: Some(TaskSubtype::Migration),
            mode: Some(TaskMode::Agent),
            completion: vec!["The register archive is restorable.".to_owned()],
            scheduled_for: Some("2026-10-01T00:00:00.000Z".to_owned()),
            due: Some("2026-09-30T17:00:00.000Z".to_owned()),
            ..record()
        };

        let encoded = serde_json::to_value(&task).expect("the record serialises");
        assert_eq!(
            encoded,
            json!({
                "id": 6,
                "project_id": 2,
                "number": 17,
                "kind": "task",
                "priority": "low",
                "state": "draft",
                "spec_id": null,
                "title": "Archive the old register",
                "slice": null,
                "criteria": [],
                "bug": null,
                "subtype": "migration",
                "mode": "agent",
                "completion": ["The register archive is restorable."],
                "scheduled_for": "2026-10-01T00:00:00.000Z",
                "due": "2026-09-30T17:00:00.000Z",
                "profile": null,
                "pinned_spec_version": null,
                "version": 1,
            })
        );
        let decoded: TicketRecord =
            serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, task);
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
                "subtype": null,
                "mode": null,
                "completion": [],
                "scheduled_for": null,
                "due": null,
                "profile": null,
                "pinned_spec_version": null,
                "version": 1,
            })
        );
        assert!(
            encoded.get("predecessor_id").is_none(),
            "an ordinary Ticket references no predecessor"
        );
        let decoded: TicketRecord =
            serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, record());
        let assigned = TicketRecord {
            profile: Some("standard".to_owned()),
            ..record()
        };
        let encoded = serde_json::to_value(&assigned).expect("the record serialises");
        assert_eq!(encoded["profile"], json!("standard"));

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
        assert_eq!(
            encoded["subtype"],
            json!(null),
            "a Bug carries no Task fields"
        );
        assert_eq!(encoded["completion"], json!([]));
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

        // A replacement references its predecessor (DR-DE-07); the
        // field arrives only when one exists.
        let replacement = TicketRecord {
            id: 9,
            number: 18,
            predecessor_id: Some(6),
            ..record()
        };
        let encoded = serde_json::to_value(&replacement).expect("the record serialises");
        assert_eq!(encoded["predecessor_id"], json!(6));
        let decoded: TicketRecord =
            serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, replacement);
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
        // A Task sends its bounded fields and never story-linked
        // criteria: `criteria` stays absent and `completion` carries
        // the outcomes.
        round_trips::<TicketCreateRequest>(json!({
            "mutation": context(),
            "project_id": 2,
            "kind": "task",
            "priority": "normal",
            "title": "Archive the old register",
            "subtype": "migration",
            "mode": "agent",
            "completion": ["The register archive is restorable."],
            "scheduled_for": "2026-10-01T00:00:00Z",
            "due": "2026-09-30T17:00:00Z",
        }));
        round_trips::<TicketAssignRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
            "profile": "standard",
        }));
        round_trips::<TicketSpecMoveRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
            "spec_id": 4,
        }));
        round_trips::<TicketGraphProposeRequest>(json!({
            "mutation": context(),
            "spec_id": 4,
            "spec_version": 2,
            "tickets": [17, 19],
            "edges": [TicketGraphEdgeProposal {
                from_ticket: 17,
                to_ticket: 19,
            }],
        }));
        round_trips::<TicketGraphApproveRequest>(json!({
            "mutation": context(),
            "proposal_id": 3,
        }));

        let graphs: TicketGraphListQuery =
            serde_json::from_value(json!({ "spec_id": 4 })).expect("the graph query decodes");
        assert_eq!(graphs, TicketGraphListQuery { spec_id: 4 });

        // The lifecycle command surface: a drag names its target, the
        // named commands name their Ticket, a review names its
        // decision, an edit sends exactly its kind's field, and the
        // emergency override carries the who and why its audit row
        // records (DR-LC-07 to DR-LC-10).
        round_trips::<TicketTransitionRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
            "to": "ready",
        }));
        round_trips::<TicketParkRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
        }));
        round_trips::<TicketUnparkRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
        }));
        round_trips::<TicketScheduleRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
        }));
        round_trips::<TicketScheduleRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
            "activation": "2026-09-10T11:00:00+02:00",
            "timezone": "Europe/Amsterdam",
            "profile": "standard",
        }));
        round_trips::<TicketCancelRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
        }));
        round_trips::<TicketReviewRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
            "decision": "approve",
        }));
        round_trips::<TicketPrioritiseRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
            "priority": "urgent",
        }));
        round_trips::<TicketEditRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
            "title": "Landing drops every branch",
        }));
        round_trips::<TicketEditRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
            "slice": "Registration creates Projects end to end",
        }));
        round_trips::<TicketEmergencyOverrideRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
            "to": "ready",
            "who": "Sid Wood",
            "why": "Recovery after the core crashed mid move",
        }));
        // A reassignment names its original and states the
        // replacement whole, exactly as a creation would (DR-DE-07).
        round_trips::<TicketReassignRequest>(json!({
            "mutation": context(),
            "ticket_id": 6,
            "kind": "task",
            "priority": "high",
            "title": "Replan the register archive",
            "subtype": "migration",
            "mode": "agent",
            "completion": ["The register moves and restores."],
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
    fn the_graph_record_round_trips_with_its_tickets_and_edges() {
        let proposal = TicketGraphRecord {
            id: 3,
            spec_id: 4,
            spec_version: 2,
            state: TicketGraphState::Proposed,
            tickets: vec![17, 19],
            edges: vec![TicketGraphEdgeRecord {
                from_ticket: 17,
                to_ticket: 19,
            }],
            version: 1,
        };

        let encoded = serde_json::to_value(&proposal).expect("the record serialises");
        assert_eq!(
            encoded,
            json!({
                "id": 3,
                "spec_id": 4,
                "spec_version": 2,
                "state": "proposed",
                "tickets": [17, 19],
                "edges": [{ "from_ticket": 17, "to_ticket": 19 }],
                "version": 1,
            })
        );
        let decoded: TicketGraphRecord =
            serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, proposal);

        for state in TicketGraphState::ALL {
            assert_eq!(TicketGraphState::parse(state.as_str()), Some(*state));
            assert_eq!(
                serde_json::to_value(state).expect("the state encodes"),
                json!(state.as_str())
            );
        }
        assert_eq!(TicketGraphState::parse("ghost"), None);

        let approved = TicketGraphRecord {
            state: TicketGraphState::Approved,
            ..proposal
        };
        let encoded = serde_json::to_value(&approved).expect("the record serialises");
        assert_eq!(encoded["state"], json!("approved"));

        let response = TicketGraphListResponse {
            proposals: vec![approved],
        };
        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(
            encoded["proposals"].as_array().map(Vec::len),
            Some(1),
            "the proposals list round trips"
        );
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

    #[test]
    fn review_decisions_round_trip_their_wire_names() {
        assert_eq!(TicketReviewDecision::ALL.len(), 2);
        for decision in TicketReviewDecision::ALL {
            assert_eq!(
                TicketReviewDecision::parse(decision.as_str()),
                Some(*decision),
                "`{}` must survive the round trip",
                decision.as_str()
            );
            assert_eq!(
                serde_json::to_value(decision).expect("the decision encodes"),
                json!(decision.as_str()),
                "the wire name and the serialised name must agree"
            );
        }
        assert_eq!(TicketReviewDecision::parse("ghost"), None);
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
            "TaskMode",
            "TaskSubtype",
            "TicketAssignRequest",
            "TicketCancelRequest",
            "TicketGraphApproveRequest",
            "TicketGraphEdgeProposal",
            "TicketGraphEdgeRecord",
            "TicketGraphListQuery",
            "TicketGraphListResponse",
            "TicketGraphProposeRequest",
            "TicketGraphRecord",
            "TicketGraphState",
            "TicketSpecMoveRequest",
            "TicketCreateRequest",
            "TicketCriterion",
            "TicketDependenciesQuery",
            "TicketDependenciesResponse",
            "TicketDependencyAddRequest",
            "TicketDependencyRecord",
            "TicketDependencyRemoveRequest",
            "TicketEditRequest",
            "TicketEmergencyOverrideRequest",
            "TicketExternalReference",
            "TicketGetQuery",
            "TicketKind",
            "TicketListQuery",
            "TicketListResponse",
            "TicketOccurrenceSnapshot",
            "TicketParkRequest",
            "TicketPrioritiseRequest",
            "TicketPriority",
            "TicketReadinessBlocker",
            "TicketReadinessQuery",
            "TicketReadinessResponse",
            "TicketReassignRequest",
            "TicketRecord",
            "TicketReviewDecision",
            "TicketReviewRequest",
            "TicketScheduleRequest",
            "TicketSeverity",
            "TicketState",
            "TicketTransitionRequest",
            "TicketUnparkRequest",
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
