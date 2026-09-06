//! The Ticket entity: one independently grabbable unit of work
//! (CONTEXT.md). Every Ticket belongs to exactly one Project (DR-TK-01)
//! and is named by the number that Project minted for it. Kinds carry
//! different obligations: an Implementation Ticket is a small vertical
//! slice delivering the behaviour of exactly one Spec, named end to end
//! by its slice description and claiming that Spec's User Stories
//! through story-linked criteria (DR-TK-02, DR-TK-04); a Bug records
//! incorrect behaviour and may attach to one Spec or stand alone
//! (DR-TK-03), captured quickly with title, actual behaviour, and
//! reporter evidence (DR-TK-08) and staying draft until a complete
//! qualification exists (DR-TK-09), carrying vendor-neutral External
//! References, Occurrence Snapshots, and Evidence Items while it waits
//! (DR-TK-10); a Task is bounded non-story work with the same optional
//! attachment, named by one subtype of the closed set and a
//! human-or-agent mode, bounded by completion criteria instead of
//! story-linked criteria, and carrying optional schedule or due-date
//! timing stored for KAN-S11 (DR-TK-06, DR-TK-07). Every kind carries
//! the closed priority vocabulary urgent, high, normal, low (DR-LC-12)
//! and the Bug's severity the closed critical, high, medium, low
//! (DR-LC-13); every kind starts its lifecycle in draft (DR-LC-01).
//! The lifecycle's transitions and readiness rules land in KAN-T21,
//! dependencies in KAN-T20, reassignment in KAN-T22, and graph
//! approval pinning in KAN-T23; this module owns the shape a Ticket
//! is created with, the Bug's capture and qualification rules, and
//! the Task's bounds.

use std::fmt;

use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use crate::coverage::{AcceptanceCriterion, UserStoryRef, VerificationStep};
use crate::evidence::EvidenceId;

use crate::plan::SpecNumber;
use crate::profile::ProfileName;
use crate::project::ProjectId;
use crate::spec::SpecId;
use crate::timeline_time::stored_format;

/// The identity of one Ticket. Assigned once by storage and immutable
/// afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TicketId(u64);

impl TicketId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TicketId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a Ticket number was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketNumberError {
    /// Ticket numbers start at one; zero names no Ticket.
    Zero,
}

impl fmt::Display for TicketNumberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a Ticket number starts at one")
    }
}

impl std::error::Error for TicketNumberError {}

/// One Ticket of one Project, named by the number that Project minted
/// for it, for example the `17` of `CORE-T17`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TicketNumber(u64);

impl TicketNumber {
    /// Accept any positive number.
    pub fn new(value: u64) -> Result<Self, TicketNumberError> {
        if value == 0 {
            return Err(TicketNumberError::Zero);
        }
        Ok(Self(value))
    }

    /// The minted number this Ticket is named by.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TicketNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The closed Ticket kind vocabulary (DR-TK-01): the three
/// obligations a Ticket is created under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    /// The stored and wire name of this kind.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Implementation => "implementation",
            Self::Bug => "bug",
            Self::Task => "task",
        }
    }

    /// The kind a stored row names, or `None` outside the vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.wire_name() == stored)
    }
}

/// The closed priority vocabulary (DR-LC-12): urgent, high, normal,
/// low. Priority drives deterministic card ordering; there is no
/// manual ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Priority {
    /// Ahead of everything else.
    Urgent,
    /// Ahead of normal work.
    High,
    /// The everyday order.
    Normal,
    /// Behind normal work.
    Low,
}

impl Priority {
    /// Every priority, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Urgent, Self::High, Self::Normal, Self::Low];

    /// The stored and wire name of this priority.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Urgent => "urgent",
            Self::High => "high",
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }

    /// The priority a stored row names, or `None` outside the
    /// vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|priority| priority.wire_name() == stored)
    }
}

/// The closed Bug severity vocabulary (DR-LC-13): critical, high,
/// medium, low. Severity is set by qualification alone (DR-TK-09): a
/// quick-captured Bug carries none until it is qualified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// Data loss, secret exposure, or unauthorised execution.
    Critical,
    /// A failed approved criterion or workflow invariant.
    High,
    /// Usable behaviour, wrongly shaped.
    Medium,
    /// Cosmetic or local clarity.
    Low,
}

impl Severity {
    /// Every severity, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Critical, Self::High, Self::Medium, Self::Low];

    /// The stored and wire name of this severity.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    /// The severity a stored row names, or `None` outside the
    /// vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|severity| severity.wire_name() == stored)
    }
}

/// The closed Ticket lifecycle vocabulary (DR-LC-01): the canonical
/// states in order, with the terminal states after them. A Ticket is
/// created into draft; the transitions between states and their
/// ownership rules are KAN-T21.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// Terminal in the canonical order: landed in full.
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

    /// Whether this state accepts no further lifecycle movement.
    /// Cancelled and superseded are the terminal states (DR-LC-02).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Superseded)
    }

    /// The stored and wire name of this state.
    pub fn wire_name(self) -> &'static str {
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

    /// The state a stored row names, or `None` outside the
    /// vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|state| state.wire_name() == stored)
    }
}

/// The closed Task subtype vocabulary (DR-TK-06, CONTEXT.md): the
/// seven bounded flavours of non-story work a Task names exactly one
/// of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    /// The stored and wire name of this subtype.
    pub fn wire_name(self) -> &'static str {
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

    /// The subtype a stored row names, or `None` outside the closed
    /// set.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|subtype| subtype.wire_name() == stored)
    }
}

/// The closed Task mode vocabulary (KAN-S4-US4): whether a human or
/// an agent executes the bounded work. Transition ownership follows
/// the mode's kind rules in KAN-T21, not this value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskMode {
    /// Sid executes the work.
    Human,
    /// An agent executes the work under dispatch.
    Agent,
}

impl TaskMode {
    /// Every mode, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Human, Self::Agent];

    /// The stored and wire name of this mode.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
        }
    }

    /// The mode a stored row names, or `None` outside the closed set.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|mode| mode.wire_name() == stored)
    }
}

/// One Task completion criterion (DR-TK-07): the observable outcome
/// that bounds the Task. Completion criteria state outcomes alone — a
/// Task never claims a User Story through story-linked criteria, so
/// this value carries no story links at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionCriterion {
    outcome: String,
}

impl CompletionCriterion {
    /// Assemble a criterion, refusing an outcome that states nothing.
    pub fn new(outcome: impl Into<String>) -> Result<Self, TicketError> {
        let outcome = outcome.into();
        if outcome.trim().is_empty() {
            return Err(TicketError::Blank("completion criterion"));
        }
        Ok(Self { outcome })
    }

    /// The observable outcome that bounds the Task.
    pub fn outcome(&self) -> &str {
        &self.outcome
    }
}

/// One Task's optional timing (KAN-S4-US4): a one-time activation —
/// the schedule a Task stores for KAN-S11's activation behaviour —
/// and a due date. Both are RFC 3339 instants, normalised to the UTC
/// shape storage keeps; both may sit absent. Activation semantics are
/// KAN-T53 and KAN-T54; this value stores validated timing only.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskTiming {
    scheduled_for: Option<String>,
    due: Option<String>,
}

impl TaskTiming {
    /// Assemble timing from raw instants, refusing a value that is
    /// not valid RFC 3339 and normalising the rest to the stored UTC
    /// shape.
    pub fn new(scheduled_for: Option<String>, due: Option<String>) -> Result<Self, TicketError> {
        Ok(Self {
            scheduled_for: scheduled_for
                .map(|raw| instant("activation", raw))
                .transpose()?,
            due: due.map(|raw| instant("due date", raw)).transpose()?,
        })
    }

    /// The absent timing: no activation, no due date.
    pub fn none() -> Self {
        Self::default()
    }

    /// The one-time activation instant, if the Task carries one.
    pub fn scheduled_for(&self) -> Option<&str> {
        self.scheduled_for.as_deref()
    }

    /// The due date, if the Task carries one.
    pub fn due(&self) -> Option<&str> {
        self.due.as_deref()
    }
}

/// Parse one raw instant as RFC 3339 and render it in the stored UTC
/// shape, naming the field a refusal reports.
fn instant(field: &'static str, raw: String) -> Result<String, TicketError> {
    let parsed = OffsetDateTime::parse(&raw, &Rfc3339)
        .map(|parsed| parsed.to_offset(UtcOffset::UTC))
        .map_err(|_| TicketError::MalformedTiming {
            field,
            value: raw.clone(),
        })?;
    parsed
        .format(stored_format())
        .map_err(|_| TicketError::MalformedTiming { field, value: raw })
}

/// Why a Ticket was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketError {
    /// A text field holds nothing but whitespace. The value names the
    /// field.
    Blank(&'static str),
    /// An Implementation Ticket attaches to exactly one Spec
    /// (DR-TK-02); it cannot be created unattached.
    UnattachedSpec,
    /// An Implementation Ticket claims User Stories through
    /// story-linked criteria (DR-TK-04); an empty claim list delivers
    /// nothing owned.
    Unclaimed,
    /// A criterion linked a User Story of another Spec. An
    /// Implementation Ticket claims the stories of the Spec it
    /// delivers.
    ForeignStory {
        /// The story that named another Spec.
        story: UserStoryRef,
    },
    /// Only a Bug Ticket carries qualification and Bug facts
    /// (DR-TK-05); another kind accepts neither.
    NotABug,
    /// An External Reference named no URI. The reference is the
    /// vendor-neutral carrier for every outside link a Bug holds
    /// (DR-TK-10).
    InvalidReference,
    /// An Occurrence Snapshot named no RFC 3339 moment, or stated no
    /// observation (DR-TK-10).
    InvalidSnapshot {
        /// Why the snapshot was refused.
        reason: SnapshotError,
    },
    /// A Bug named an Evidence Item by an identity no attachment can
    /// hold; storage-assigned identities start at one (DR-TK-10).
    InvalidEvidenceId,
    /// A Task names one subtype of the closed set (DR-TK-06); work
    /// without a subtype is unbounded.
    UnspecifiedSubtype,
    /// A Task names its human-or-agent mode (KAN-S4-US4).
    UnstatedMode,
    /// A Task is bounded by completion criteria (DR-TK-07); an empty
    /// list bounds nothing.
    Unbounded,
    /// A Task timing field was not a valid RFC 3339 instant. The
    /// value names the field and the raw text refused.
    MalformedTiming {
        /// The field the refused instant belongs to.
        field: &'static str,
        /// The raw value that named no instant.
        value: String,
    },
    /// A Ticket pins once: to the Spec content version its approved
    /// graph named (DR-DE-06).
    AlreadyPinned,
    /// A pinned Ticket stays with the Spec and version it was
    /// approved against (DR-DE-06); it moves between Specs no more.
    Pinned,
    /// Only a draft Ticket moves between Specs (DR-DE-05); execution
    /// past draft pins the Ticket where it stands.
    MoveRequiresDraft,
    /// A terminal Ticket — cancelled or superseded — accepts no
    /// further changes.
    Terminal,
    /// Only a Bug or Task Ticket carries a title to edit; an
    /// Implementation is named by its slice description.
    NotTitled,
    /// Only an Implementation Ticket carries a slice description to
    /// edit; the other kinds are named by their titles.
    NotSliced,
}

/// Why an Occurrence Snapshot was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotError {
    /// The timestamp is not valid RFC 3339.
    MalformedTime,
    /// The observation states nothing.
    BlankObservation,
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedTime => write!(f, "an Occurrence Snapshot names an RFC 3339 moment"),
            Self::BlankObservation => write!(f, "an Occurrence Snapshot states its observation"),
        }
    }
}

impl std::error::Error for SnapshotError {}

impl fmt::Display for TicketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blank(field) => write!(f, "a Ticket {field} cannot be blank"),
            Self::UnattachedSpec => {
                write!(f, "an Implementation Ticket attaches to exactly one Spec")
            }
            Self::Unclaimed => {
                write!(f, "an Implementation Ticket carries story-linked criteria")
            }
            Self::ForeignStory { story } => write!(
                f,
                "an Implementation Ticket claims the stories of the Spec it delivers; \
                 {} names another Spec",
                story.wire_name()
            ),
            Self::NotABug => write!(f, "only a Bug Ticket carries qualification and Bug facts"),
            Self::InvalidReference => write!(
                f,
                "an External Reference names a URI with a scheme, like `https://example.invalid/1`"
            ),
            Self::InvalidSnapshot { reason } => write!(f, "{reason}"),
            Self::InvalidEvidenceId => {
                write!(f, "an Evidence Item identity starts at one")
            }
            Self::UnspecifiedSubtype => {
                write!(f, "a Task Ticket names one subtype of the closed set")
            }
            Self::UnstatedMode => write!(f, "a Task Ticket names a human or agent mode"),
            Self::Unbounded => write!(f, "a Task Ticket carries completion criteria"),
            Self::MalformedTiming { field, value } => {
                write!(f, "a Task {field} must be an RFC 3339 instant: `{value}`")
            }
            Self::AlreadyPinned => {
                write!(
                    f,
                    "a Ticket pins once, to the version its approved graph named"
                )
            }
            Self::Pinned => write!(
                f,
                "a pinned Ticket stays with the Spec version it was approved against"
            ),
            Self::MoveRequiresDraft => {
                write!(f, "only a draft Ticket moves between Specs")
            }
            Self::Terminal => write!(f, "a terminal Ticket accepts no further changes"),
            Self::NotTitled => write!(f, "only a Bug or Task Ticket carries a title"),
            Self::NotSliced => {
                write!(
                    f,
                    "only an Implementation Ticket carries a slice description"
                )
            }
        }
    }
}

impl std::error::Error for TicketError {}

/// One Implementation Ticket's kind-specific schema (DR-TK-02,
/// DR-TK-04): the one Spec the slice delivers, the slice description
/// naming the behaviour delivered end to end, and the story-linked
/// criteria claiming that Spec's User Stories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplementationTicket {
    spec: SpecId,
    slice: String,
    criteria: Vec<AcceptanceCriterion>,
}

impl ImplementationTicket {
    /// Rehydrate a stored body exactly as it was recorded.
    pub fn restore(
        spec: SpecId,
        slice: impl Into<String>,
        criteria: Vec<AcceptanceCriterion>,
    ) -> Self {
        Self {
            spec,
            slice: slice.into(),
            criteria,
        }
    }

    /// The one Spec this slice delivers.
    pub fn spec(&self) -> SpecId {
        self.spec
    }

    /// The slice description, naming the behaviour delivered end to
    /// end.
    pub fn slice(&self) -> &str {
        &self.slice
    }

    /// The story-linked criteria, in the order they were linked.
    pub fn criteria(&self) -> &[AcceptanceCriterion] {
        &self.criteria
    }

    /// Replace the slice description, for the edit command alone. The
    /// Ticket checks the new value; this mutator trusts its caller.
    pub(crate) fn redescribe(&mut self, slice: String) {
        self.slice = slice;
    }
}

/// One vendor-neutral External Reference a Bug may carry (DR-TK-10):
/// a URI naming an outside link — a tracker item, a chat message, a
/// dashboard — with an optional label. No vendor owns a field here;
/// provenance that names one arrives with a later slice, never this
/// shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalReference {
    uri: String,
    label: Option<String>,
}

impl ExternalReference {
    /// Assemble one reference, refusing anything but a URI with a
    /// scheme and, when a label is present, a label that names
    /// something.
    pub fn new(uri: impl Into<String>, label: Option<String>) -> Result<Self, TicketError> {
        let uri = uri.into().trim().to_owned();
        if !is_uri(&uri) {
            return Err(TicketError::InvalidReference);
        }
        let label = label
            .map(|raw: String| raw.trim().to_owned())
            .map(|trimmed| {
                if trimmed.is_empty() {
                    Err(TicketError::Blank("External Reference label"))
                } else {
                    Ok(trimmed)
                }
            })
            .transpose()?;
        Ok(Self { uri, label })
    }

    /// The referenced URI, trimmed as stored.
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// The reference's label, if it carries one.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
}

/// Whether `text` is a URI with a scheme: an RFC 3986 scheme — a
/// letter first, then letters, digits, `+`, `-`, or `.` — before the
/// first colon, with something following it.
fn is_uri(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once(':') else {
        return false;
    };
    if rest.is_empty() || scheme.is_empty() {
        return false;
    }
    let mut characters = scheme.chars();
    let first = characters.next().expect("the scheme is non-empty");
    first.is_ascii_alphabetic()
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
        && !text.chars().any(char::is_whitespace)
}

/// One Occurrence Snapshot a Bug may carry (DR-TK-10): the RFC 3339
/// moment one occurrence was observed and what was seen then. Time
/// arrives as a value; the domain owns no clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccurrenceSnapshot {
    observed_at: String,
    observation: String,
}

impl OccurrenceSnapshot {
    /// Assemble one snapshot, refusing a moment that is not RFC 3339
    /// and an observation that states nothing.
    pub fn new(
        observed_at: impl Into<String>,
        observation: impl Into<String>,
    ) -> Result<Self, TicketError> {
        let observed_at = observed_at.into().trim().to_owned();
        if time::OffsetDateTime::parse(&observed_at, &Rfc3339).is_err() {
            return Err(TicketError::InvalidSnapshot {
                reason: SnapshotError::MalformedTime,
            });
        }
        let observation = observation.into();
        if observation.trim().is_empty() {
            return Err(TicketError::InvalidSnapshot {
                reason: SnapshotError::BlankObservation,
            });
        }
        Ok(Self {
            observed_at,
            observation,
        })
    }

    /// The observed moment, RFC 3339 as it was given.
    pub fn observed_at(&self) -> &str {
        &self.observed_at
    }

    /// What was observed at that moment.
    pub fn observation(&self) -> &str {
        &self.observation
    }
}

/// The vendor-neutral provenance collections one Bug carries
/// (DR-TK-10): External References, Occurrence Snapshots, and the
/// identities of the Evidence Items attached to it. Quick capture
/// needs none of them; they gather while the Bug waits for
/// qualification.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BugFacts {
    external_references: Vec<ExternalReference>,
    occurrence_snapshots: Vec<OccurrenceSnapshot>,
    evidence_items: Vec<EvidenceId>,
}

impl BugFacts {
    /// Assemble the collections, refusing every malformed entry.
    pub fn new(
        external_references: Vec<ExternalReference>,
        occurrence_snapshots: Vec<OccurrenceSnapshot>,
        evidence_items: Vec<EvidenceId>,
    ) -> Result<Self, TicketError> {
        if evidence_items.iter().any(|item| item.value() == 0) {
            return Err(TicketError::InvalidEvidenceId);
        }
        Ok(Self {
            external_references,
            occurrence_snapshots,
            evidence_items,
        })
    }

    /// An empty set of facts: what quick capture starts from.
    pub fn empty() -> Self {
        Self::default()
    }

    /// The External References, in the order they were recorded.
    pub fn external_references(&self) -> &[ExternalReference] {
        &self.external_references
    }

    /// The Occurrence Snapshots, in the order they were recorded.
    pub fn occurrence_snapshots(&self) -> &[OccurrenceSnapshot] {
        &self.occurrence_snapshots
    }

    /// The identities of the Evidence Items this Bug carries.
    pub fn evidence_items(&self) -> &[EvidenceId] {
        &self.evidence_items
    }
}

/// One complete Bug qualification (DR-TK-09): the ten facts a Bug
/// needs before it may leave draft. Every text field must state
/// something, the criteria must be present and story-linked, and the
/// verification steps must be present; severity arrives only here
/// (DR-LC-13). Assembled whole, never piecemeal — qualification is
/// one act, not ten edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BugQualification {
    expected_behaviour: String,
    reproduction: String,
    environment: String,
    severity: Severity,
    frequency: String,
    affected_scope: String,
    risk: String,
    criteria: Vec<AcceptanceCriterion>,
    verification_steps: Vec<VerificationStep>,
}

impl BugQualification {
    /// Assemble one qualification, refusing any blank text field, an
    /// empty claim of criteria, and an empty list of verification
    /// steps. `reproduction` carries the reproduction steps or the
    /// failing test that demonstrates the defect — one slot, either
    /// content.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        expected_behaviour: impl Into<String>,
        reproduction: impl Into<String>,
        environment: impl Into<String>,
        severity: Severity,
        frequency: impl Into<String>,
        affected_scope: impl Into<String>,
        risk: impl Into<String>,
        criteria: Vec<AcceptanceCriterion>,
        verification_steps: Vec<VerificationStep>,
    ) -> Result<Self, TicketError> {
        let fields: [(&'static str, String); 6] = [
            ("expected behaviour", expected_behaviour.into()),
            ("reproduction or failing test", reproduction.into()),
            ("environment", environment.into()),
            ("frequency", frequency.into()),
            ("affected scope", affected_scope.into()),
            ("risk", risk.into()),
        ];
        for (name, value) in &fields {
            if value.trim().is_empty() {
                return Err(TicketError::Blank(name));
            }
        }
        let [
            expected_behaviour,
            reproduction,
            environment,
            frequency,
            affected_scope,
            risk,
        ] = fields.map(|(_, value)| value);
        if criteria.is_empty() {
            return Err(TicketError::Blank("Acceptance Criteria claim"));
        }
        if verification_steps.is_empty() {
            return Err(TicketError::Blank("Verification Steps claim"));
        }
        Ok(Self {
            expected_behaviour,
            reproduction,
            environment,
            severity,
            frequency,
            affected_scope,
            risk,
            criteria,
            verification_steps,
        })
    }

    /// The behaviour that should have happened.
    pub fn expected_behaviour(&self) -> &str {
        &self.expected_behaviour
    }

    /// The reproduction steps or the failing test demonstrating the
    /// defect.
    pub fn reproduction(&self) -> &str {
        &self.reproduction
    }

    /// Where the defect occurs.
    pub fn environment(&self) -> &str {
        &self.environment
    }

    /// The severity qualification assigned (DR-LC-13).
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// How often the defect occurs.
    pub fn frequency(&self) -> &str {
        &self.frequency
    }

    /// What the defect affects.
    pub fn affected_scope(&self) -> &str {
        &self.affected_scope
    }

    /// What the defect puts at risk.
    pub fn risk(&self) -> &str {
        &self.risk
    }

    /// The story-linked Acceptance Criteria, in claim order.
    pub fn criteria(&self) -> &[AcceptanceCriterion] {
        &self.criteria
    }

    /// The Verification Steps, in run order.
    pub fn verification_steps(&self) -> &[VerificationStep] {
        &self.verification_steps
    }
}

/// One Bug Ticket's schema (DR-TK-03, DR-TK-08, DR-TK-09, DR-TK-10):
/// the quick-capture facts it is created with — title, actual
/// behaviour, and reporter evidence — an optional Spec attachment,
/// the vendor-neutral collections it carries while it waits, and the
/// qualification that completes it. Creation demands only the capture
/// facts; qualification arrives later as one whole act.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BugTicket {
    title: String,
    spec: Option<SpecId>,
    actual_behaviour: String,
    reporter_evidence: String,
    qualification: Option<BugQualification>,
    facts: BugFacts,
}

impl BugTicket {
    /// Rehydrate a stored body exactly as it was recorded.
    pub fn restore(
        title: impl Into<String>,
        spec: Option<SpecId>,
        actual_behaviour: impl Into<String>,
        reporter_evidence: impl Into<String>,
        qualification: Option<BugQualification>,
        facts: BugFacts,
    ) -> Self {
        Self {
            title: title.into(),
            spec,
            actual_behaviour: actual_behaviour.into(),
            reporter_evidence: reporter_evidence.into(),
            qualification,
            facts,
        }
    }

    /// The Bug's title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The Spec this Bug attaches to, if any; a Bug may stand alone.
    pub fn spec(&self) -> Option<SpecId> {
        self.spec
    }

    /// What the Bug records happening, as the reporter stated it.
    pub fn actual_behaviour(&self) -> &str {
        &self.actual_behaviour
    }

    /// The evidence the reporter holds, as the reporter stated it.
    pub fn reporter_evidence(&self) -> &str {
        &self.reporter_evidence
    }

    /// The qualification, once one exists (DR-TK-09).
    pub fn qualification(&self) -> Option<&BugQualification> {
        self.qualification.as_ref()
    }

    /// Whether a complete qualification exists.
    pub fn is_qualified(&self) -> bool {
        self.qualification.is_some()
    }

    /// The severity qualification assigned, or `None` until the Bug
    /// is qualified (DR-LC-13).
    pub fn severity(&self) -> Option<Severity> {
        self.qualification.as_ref().map(|record| record.severity())
    }

    /// The vendor-neutral collections this Bug carries (DR-TK-10).
    pub fn facts(&self) -> &BugFacts {
        &self.facts
    }

    /// Replace the title, for the edit command alone. The Ticket
    /// checks the new value; this mutator trusts its caller.
    pub(crate) fn retitle(&mut self, title: String) {
        self.title = title;
    }
}

/// One Task Ticket's creation schema (DR-TK-06, DR-TK-07): a title,
/// an optional Spec attachment, one subtype of the closed set, a
/// human-or-agent mode, the completion criteria that bound the work —
/// never story-linked criteria — and optional schedule or due-date
/// timing stored for KAN-S11.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskTicket {
    title: String,
    spec: Option<SpecId>,
    subtype: TaskSubtype,
    mode: TaskMode,
    completion: Vec<CompletionCriterion>,
    timing: TaskTiming,
}

impl TaskTicket {
    /// Rehydrate a stored body exactly as it was recorded.
    pub fn restore(
        title: impl Into<String>,
        spec: Option<SpecId>,
        subtype: TaskSubtype,
        mode: TaskMode,
        completion: Vec<CompletionCriterion>,
        timing: TaskTiming,
    ) -> Self {
        Self {
            title: title.into(),
            spec,
            subtype,
            mode,
            completion,
            timing,
        }
    }

    /// The Task's title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The Spec this Task attaches to, if any.
    pub fn spec(&self) -> Option<SpecId> {
        self.spec
    }

    /// The one subtype this Task names.
    pub fn subtype(&self) -> TaskSubtype {
        self.subtype
    }

    /// The Task's human-or-agent mode.
    pub fn mode(&self) -> TaskMode {
        self.mode
    }

    /// The completion criteria that bound this Task.
    pub fn completion(&self) -> &[CompletionCriterion] {
        &self.completion
    }

    /// The Task's optional timing.
    pub fn timing(&self) -> &TaskTiming {
        &self.timing
    }

    /// Replace the title, for the edit command alone. The Ticket
    /// checks the new value; this mutator trusts its caller.
    pub(crate) fn retitle(&mut self, title: String) {
        self.title = title;
    }
}

/// The kind-specific schema one Ticket carries (KAN-S4-US1): each kind
/// holds exactly its own obligations, so a field that names no kind —
/// a Bug's qualification evidence, a Task's completion criteria —
/// arrives with the slice that owns it, never here early.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketBody {
    /// A vertical slice of exactly one Spec.
    Implementation(ImplementationTicket),
    /// Incorrect behaviour, capture facts now, qualification later.
    Bug(Box<BugTicket>),
    /// Bounded non-story work.
    Task(TaskTicket),
}

impl TicketBody {
    /// Assemble an Implementation body, refusing an unattached slice,
    /// a blank slice description, an empty claim of criteria, and any
    /// criterion linked to another Spec's story. `of` is the number of
    /// the Spec the slice delivers, the Spec whose stories the
    /// criteria must claim.
    pub fn implementation(
        spec: Option<SpecId>,
        of: SpecNumber,
        slice: impl Into<String>,
        criteria: Vec<AcceptanceCriterion>,
    ) -> Result<Self, TicketError> {
        let spec = spec.ok_or(TicketError::UnattachedSpec)?;
        let slice = slice.into();
        if slice.trim().is_empty() {
            return Err(TicketError::Blank("slice description"));
        }
        if criteria.is_empty() {
            return Err(TicketError::Unclaimed);
        }
        for criterion in &criteria {
            if let Some(foreign) = criterion
                .stories()
                .iter()
                .copied()
                .find(|story| story.spec() != of)
            {
                return Err(TicketError::ForeignStory { story: foreign });
            }
        }
        Ok(Self::Implementation(ImplementationTicket {
            spec,
            slice,
            criteria,
        }))
    }

    /// Assemble a Bug body from its quick-capture facts (DR-TK-08),
    /// refusing a blank title, actual behaviour, or reporter evidence.
    /// Nothing else is required: the Spec attachment, the Bug facts,
    /// and the qualification all arrive later.
    pub fn bug(
        title: impl Into<String>,
        spec: Option<SpecId>,
        actual_behaviour: impl Into<String>,
        reporter_evidence: impl Into<String>,
    ) -> Result<Self, TicketError> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(TicketError::Blank("title"));
        }
        let actual_behaviour = actual_behaviour.into();
        if actual_behaviour.trim().is_empty() {
            return Err(TicketError::Blank("actual behaviour"));
        }
        let reporter_evidence = reporter_evidence.into();
        if reporter_evidence.trim().is_empty() {
            return Err(TicketError::Blank("reporter evidence"));
        }
        Ok(Self::Bug(Box::new(BugTicket {
            title,
            spec,
            actual_behaviour,
            reporter_evidence,
            qualification: None,
            facts: BugFacts::empty(),
        })))
    }

    /// Assemble a Task body, refusing a blank title, a missing
    /// subtype or mode, and an empty completion list. Completion
    /// criteria state outcomes alone; a Task never carries
    /// story-linked criteria (DR-TK-07).
    pub fn task(
        title: impl Into<String>,
        spec: Option<SpecId>,
        subtype: Option<TaskSubtype>,
        mode: Option<TaskMode>,
        completion: Vec<CompletionCriterion>,
        timing: TaskTiming,
    ) -> Result<Self, TicketError> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(TicketError::Blank("title"));
        }
        let subtype = subtype.ok_or(TicketError::UnspecifiedSubtype)?;
        let mode = mode.ok_or(TicketError::UnstatedMode)?;
        if completion.is_empty() {
            return Err(TicketError::Unbounded);
        }
        Ok(Self::Task(TaskTicket {
            title,
            spec,
            subtype,
            mode,
            completion,
            timing,
        }))
    }

    /// The kind whose schema this body carries.
    pub fn kind(&self) -> TicketKind {
        match self {
            Self::Implementation(_) => TicketKind::Implementation,
            Self::Bug(_) => TicketKind::Bug,
            Self::Task(_) => TicketKind::Task,
        }
    }

    /// The Spec this Ticket attaches to: exactly one for an
    /// Implementation, zero or one for a Bug or Task (DR-TK-02,
    /// DR-TK-03).
    pub fn spec(&self) -> Option<SpecId> {
        match self {
            Self::Implementation(implementation) => Some(implementation.spec()),
            Self::Bug(bug) => bug.spec(),
            Self::Task(task) => task.spec(),
        }
    }
}

/// One Ticket aggregate: the Project it belongs to, the number that
/// Project minted for it, its priority, its lifecycle state, the
/// kind-specific body, the Execution Profile its assignment names by
/// reference (DR-EP-03), the Spec content version an approved graph
/// pinned it to (DR-DE-06), and, when reassignment created it as a
/// replacement, the predecessor it references (DR-DE-07). The version
/// counts applied changes: creation lands at 1 and every later legal
/// change bumps it, so a stored version is all a caller needs for
/// optimistic checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    id: TicketId,
    project: ProjectId,
    number: TicketNumber,
    priority: Priority,
    state: TicketState,
    body: TicketBody,
    predecessor: Option<TicketId>,
    profile: Option<ProfileName>,
    pin: Option<u64>,
    version: u64,
}

impl Ticket {
    /// A fresh Ticket: created into draft, at version 1, carrying its
    /// kind's schema, no assignment, and no pin. The body's own
    /// constructors hold the kind-specific rules; a body rehydrated
    /// from storage passes through unchanged.
    pub fn new(
        id: TicketId,
        project: ProjectId,
        number: TicketNumber,
        priority: Priority,
        body: TicketBody,
    ) -> Self {
        Self {
            id,
            project,
            number,
            priority,
            state: TicketState::Draft,
            body,
            predecessor: None,
            profile: None,
            version: 1,
        }
    }

    /// A replacement Ticket (DR-DE-07): created into draft, at
    /// version 1, referencing `predecessor`, the Ticket this one
    /// replaces. The reference is one-directional and immutable — set
    /// here, carried forever — and the supersession of the
    /// predecessor is [`crate::reassignment`]'s act, never this
    /// constructor's.
    pub fn replacement(
        id: TicketId,
        project: ProjectId,
        number: TicketNumber,
        priority: Priority,
        predecessor: TicketId,
        body: TicketBody,
    ) -> Self {
        Self {
            id,
            project,
            number,
            priority,
            state: TicketState::Draft,
            body,
            predecessor: Some(predecessor),
            profile: None,
            pin: None,
            version: 1,
        }
    }

    /// Rehydrate a stored Ticket exactly as it was recorded.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        id: TicketId,
        project: ProjectId,
        number: TicketNumber,
        priority: Priority,
        state: TicketState,
        body: TicketBody,
        predecessor: Option<TicketId>,
        profile: Option<ProfileName>,
        pin: Option<u64>,
        version: u64,
    ) -> Self {
        Self {
            id,
            project,
            number,
            priority,
            state,
            body,
            predecessor,
            profile,
            pin,
            version,
        }
    }

    /// The immutable identity.
    pub fn id(&self) -> TicketId {
        self.id
    }

    /// The Project this Ticket belongs to.
    pub fn project(&self) -> ProjectId {
        self.project
    }

    /// The Ticket this one replaced, when reassignment created it as a
    /// replacement (DR-DE-07); an ordinary Ticket references nothing.
    pub fn predecessor(&self) -> Option<TicketId> {
        self.predecessor
    }

    /// The number this Project minted for this Ticket, rendered with
    /// the Project's code as `CORE-T17`.
    pub fn number(&self) -> TicketNumber {
        self.number
    }

    /// The kind whose schema this Ticket carries.
    pub fn kind(&self) -> TicketKind {
        self.body.kind()
    }

    /// The Ticket's priority (DR-LC-12).
    pub fn priority(&self) -> Priority {
        self.priority
    }

    /// The lifecycle state (DR-LC-01).
    pub fn state(&self) -> TicketState {
        self.state
    }

    /// The kind-specific body.
    pub fn body(&self) -> &TicketBody {
        &self.body
    }

    /// The Spec this Ticket attaches to, if it attaches to one.
    pub fn spec(&self) -> Option<SpecId> {
        self.body.spec()
    }

    /// The Bug or Task title, if this Ticket carries one.
    pub fn title(&self) -> Option<&str> {
        match &self.body {
            TicketBody::Bug(bug) => Some(bug.title()),
            TicketBody::Task(task) => Some(task.title()),
            TicketBody::Implementation(_) => None,
        }
    }

    /// The Implementation slice description, if this Ticket carries
    /// one.
    pub fn slice(&self) -> Option<&str> {
        match &self.body {
            TicketBody::Implementation(implementation) => Some(implementation.slice()),
            _ => None,
        }
    }

    /// The Implementation's story-linked criteria; empty for every
    /// other kind. A Task claims no User Story through these
    /// (DR-TK-07).
    pub fn criteria(&self) -> &[AcceptanceCriterion] {
        match &self.body {
            TicketBody::Implementation(implementation) => implementation.criteria(),
            _ => &[],
        }
    }

    /// The Bug body, if this Ticket carries one.
    pub fn bug(&self) -> Option<&BugTicket> {
        match &self.body {
            TicketBody::Bug(bug) => Some(bug),
            _ => None,
        }
    }

    /// Qualify this Bug with one complete qualification (DR-TK-09),
    /// replacing any earlier one. Refused for every other kind; a
    /// refusal changes nothing. The applied change bumps the version.
    pub fn qualify(&mut self, qualification: BugQualification) -> Result<(), TicketError> {
        self.with_bug(|bug| bug.qualification = Some(qualification))
    }

    /// Replace the vendor-neutral collections this Bug carries
    /// (DR-TK-10). Refused for every other kind; a refusal changes
    /// nothing. The applied change bumps the version.
    pub fn record_bug_facts(&mut self, facts: BugFacts) -> Result<(), TicketError> {
        self.with_bug(|bug| bug.facts = facts)
    }

    /// Apply `change` to the Bug body alone, bumping the version only
    /// when the body is a Bug's.
    fn with_bug(&mut self, change: impl FnOnce(&mut BugTicket)) -> Result<(), TicketError> {
        let TicketBody::Bug(bug) = &mut self.body else {
            return Err(TicketError::NotABug);
        };
        change(bug);
        self.version += 1;
        Ok(())
    }

    /// The Task's subtype, if this Ticket carries one.
    pub fn subtype(&self) -> Option<TaskSubtype> {
        match &self.body {
            TicketBody::Task(task) => Some(task.subtype()),
            _ => None,
        }
    }

    /// The Task's human-or-agent mode, if this Ticket carries one.
    pub fn task_mode(&self) -> Option<TaskMode> {
        match &self.body {
            TicketBody::Task(task) => Some(task.mode()),
            _ => None,
        }
    }

    /// The Task's completion criteria; empty for every other kind.
    pub fn completion(&self) -> &[CompletionCriterion] {
        match &self.body {
            TicketBody::Task(task) => task.completion(),
            _ => &[],
        }
    }

    /// The Task's one-time activation instant, if it carries one.
    pub fn scheduled_for(&self) -> Option<&str> {
        match &self.body {
            TicketBody::Task(task) => task.timing().scheduled_for(),
            _ => None,
        }
    }

    /// The Task's due date, if it carries one.
    pub fn due(&self) -> Option<&str> {
        match &self.body {
            TicketBody::Task(task) => task.timing().due(),
            _ => None,
        }
    }

    /// The Execution Profile this Ticket's assignment names, if it
    /// carries one. The reference keeps its name through every later
    /// catalogue change (DR-EP-05).
    pub fn profile(&self) -> Option<&ProfileName> {
        self.profile.as_ref()
    }

    /// Assign this Ticket to the Execution Profile `name` references.
    /// A terminal Ticket — cancelled or superseded — accepts no
    /// further changes; whether the name resolves to a catalogue
    /// entry is the catalogue's rule, not the Ticket's.
    pub fn assign(&mut self, name: ProfileName) -> Result<(), TicketError> {
        if self.state.is_terminal() {
            return Err(TicketError::Terminal);
        }
        self.profile = Some(name);
        self.version += 1;
        Ok(())
    }

    /// The Spec content version an approved Ticket graph pinned this
    /// Ticket to, if one did (DR-DE-06).
    pub fn pinned_version(&self) -> Option<u64> {
        self.pin
    }

    /// Pin this Ticket to the Spec content version `version` its
    /// approved graph named (DR-DE-06). A Ticket pins once — a second
    /// graph approval names no Ticket of an already-pinned graph —
    /// and a terminal Ticket accepts no further changes. The applied
    /// change bumps the version.
    pub fn pin_to(&mut self, version: u64) -> Result<(), TicketError> {
        if self.state.is_terminal() {
            return Err(TicketError::Terminal);
        }
        if self.pin.is_some() {
            return Err(TicketError::AlreadyPinned);
        }
        self.pin = Some(version);
        self.version += 1;
        Ok(())
    }

    /// Move this Ticket's Spec attachment to `spec`, the Spec whose
    /// minted number is `number` (DR-DE-05). Only a draft, unpinned
    /// Ticket moves: a pinned Ticket stays with the version it was
    /// approved against, and execution past draft pins the Ticket
    /// where it stands. An Implementation keeps claiming the stories
    /// of the Spec it delivers, so a move is refused while any
    /// criterion names the story of another Spec. A refusal changes
    /// nothing.
    pub fn move_to_spec(&mut self, spec: SpecId, number: SpecNumber) -> Result<(), TicketError> {
        if self.state.is_terminal() {
            return Err(TicketError::Terminal);
        }
        if self.pin.is_some() {
            return Err(TicketError::Pinned);
        }
        if self.state != TicketState::Draft {
            return Err(TicketError::MoveRequiresDraft);
        }
        match &mut self.body {
            TicketBody::Implementation(implementation) => {
                if let Some(foreign) = implementation
                    .criteria
                    .iter()
                    .flat_map(|criterion| criterion.stories())
                    .copied()
                    .find(|story| story.spec() != number)
                {
                    return Err(TicketError::ForeignStory { story: foreign });
                }
                implementation.spec = spec;
            }
            TicketBody::Bug(bug) => bug.spec = Some(spec),
            TicketBody::Task(task) => task.spec = Some(spec),
        }
        self.version += 1;
        Ok(())
    }

    /// Whether the kind-specific readiness facts are complete: a Bug
    /// needs its full qualification before it may leave draft
    /// (DR-TK-09); the other kinds are created complete. The lifecycle
    /// rules read this; it never moves state itself.
    pub fn is_qualified(&self) -> bool {
        match &self.body {
            TicketBody::Bug(bug) => bug.is_qualified(),
            _ => true,
        }
    }

    /// Prioritise the Ticket (DR-LC-09, DR-LC-12). A terminal Ticket
    /// accepts no further changes; the applied change bumps the
    /// version.
    pub fn prioritise(&mut self, priority: Priority) -> Result<(), TicketError> {
        if self.state.is_terminal() {
            return Err(TicketError::Terminal);
        }
        self.priority = priority;
        self.version += 1;
        Ok(())
    }

    /// Edit the title of a Bug or Task Ticket (DR-LC-09), refusing a
    /// blank title and an Implementation, which a slice description
    /// names instead. The applied change bumps the version.
    pub fn retitle(&mut self, title: impl Into<String>) -> Result<(), TicketError> {
        if self.state.is_terminal() {
            return Err(TicketError::Terminal);
        }
        let title = title.into();
        if title.trim().is_empty() {
            return Err(TicketError::Blank("title"));
        }
        match &mut self.body {
            TicketBody::Bug(bug) => bug.retitle(title),
            TicketBody::Task(task) => task.retitle(title),
            TicketBody::Implementation(_) => return Err(TicketError::NotTitled),
        }
        self.version += 1;
        Ok(())
    }

    /// Edit the slice description of an Implementation Ticket
    /// (DR-LC-09), refusing a blank slice and every other kind, which
    /// a title names instead. The applied change bumps the version.
    pub fn redescribe(&mut self, slice: impl Into<String>) -> Result<(), TicketError> {
        if self.state.is_terminal() {
            return Err(TicketError::Terminal);
        }
        let slice = slice.into();
        if slice.trim().is_empty() {
            return Err(TicketError::Blank("slice description"));
        }
        match &mut self.body {
            TicketBody::Implementation(implementation) => implementation.redescribe(slice),
            _ => return Err(TicketError::NotSliced),
        }
        self.version += 1;
        Ok(())
    }

    /// Move the lifecycle state, counting the change. The rules live
    /// in [`crate::lifecycle`]; this mutator exists for that module
    /// and its command surface alone, so no caller moves a Ticket
    /// around them.
    pub(crate) fn transition_state(&mut self, to: TicketState) {
        self.state = to;
        self.version += 1;
    }

    /// The number of applied changes, for optimistic version checks.
    pub fn version(&self) -> u64 {
        self.version
    }
}

#[cfg(test)]
mod ticket_kinds {
    use super::{
        BugFacts, BugTicket, CompletionCriterion, Priority, TaskMode, TaskSubtype, TaskTicket,
        TaskTiming, Ticket, TicketBody, TicketError, TicketId, TicketKind, TicketNumber,
        TicketState,
    };
    use crate::coverage::{AcceptanceCriterion, UserStoryRef};
    use crate::plan::SpecNumber;
    use crate::project::ProjectId;
    use crate::spec::SpecId;

    fn story(spec: u64, ordinal: u64) -> UserStoryRef {
        UserStoryRef::new(
            SpecNumber::new(spec).expect("the fixture number is positive"),
            ordinal,
        )
        .expect("the fixture ordinal is positive")
    }

    fn criterion(spec: u64, ordinal: u64, outcome: &str) -> AcceptanceCriterion {
        AcceptanceCriterion::new(outcome, vec![story(spec, ordinal)])
            .expect("the fixture criterion links")
    }

    fn number(value: u64) -> TicketNumber {
        TicketNumber::new(value).expect("the fixture number is positive")
    }

    /// An Implementation body delivering Spec 1's behaviour.
    fn implementation(
        spec: Option<SpecId>,
        criteria: Vec<AcceptanceCriterion>,
    ) -> Result<TicketBody, TicketError> {
        TicketBody::implementation(
            spec,
            SpecNumber::new(1).expect("the fixture number is positive"),
            "Registration creates Projects end to end",
            criteria,
        )
    }

    #[test]
    fn kinds_priorities_and_states_round_trip_through_their_wire_names() {
        assert_eq!(TicketKind::ALL.len(), 3);
        for kind in TicketKind::ALL {
            assert_eq!(
                TicketKind::parse(kind.wire_name()),
                Some(*kind),
                "`{}` must survive the round trip",
                kind.wire_name()
            );
        }
        assert_eq!(TicketKind::parse("ghost"), None);

        assert_eq!(Priority::ALL.len(), 4);
        for priority in Priority::ALL {
            assert_eq!(
                Priority::parse(priority.wire_name()),
                Some(*priority),
                "`{}` must survive the round trip",
                priority.wire_name()
            );
        }
        assert_eq!(Priority::parse("ghost"), None);

        assert_eq!(TicketState::ALL.len(), 12);
        for state in TicketState::ALL {
            assert_eq!(
                TicketState::parse(state.wire_name()),
                Some(*state),
                "`{}` must survive the round trip",
                state.wire_name()
            );
        }
        assert_eq!(TicketState::parse("ghost"), None);
        assert_eq!(TicketState::InReview.wire_name(), "in_review");
    }

    #[test]
    fn terminal_states_are_cancelled_and_superseded() {
        assert!(TicketState::Cancelled.is_terminal());
        assert!(TicketState::Superseded.is_terminal());
        for open in [
            TicketState::Draft,
            TicketState::Parked,
            TicketState::Blocked,
            TicketState::Scheduled,
            TicketState::Ready,
            TicketState::Active,
            TicketState::InReview,
            TicketState::Approved,
            TicketState::Landing,
            TicketState::Done,
        ] {
            assert!(!open.is_terminal());
        }
    }

    #[test]
    fn ticket_numbers_start_at_one() {
        assert_eq!(
            TicketNumber::new(0).unwrap_err(),
            super::TicketNumberError::Zero
        );
        let minted = number(23);
        assert_eq!(minted.value(), 23);
        assert_eq!(minted.to_string(), "23");
    }

    #[test]
    fn a_fresh_implementation_ticket_is_a_draft_claiming_its_spec() {
        let ticket = Ticket::new(
            TicketId::new(9),
            ProjectId::new(4),
            number(2),
            Priority::High,
            implementation(
                Some(SpecId::new(7)),
                vec![criterion(1, 4, "Projects register.")],
            )
            .expect("the body validates"),
        );

        assert_eq!(ticket.id(), TicketId::new(9));
        assert_eq!(ticket.project(), ProjectId::new(4));
        assert_eq!(ticket.number().value(), 2);
        assert_eq!(ticket.kind(), TicketKind::Implementation);
        assert_eq!(ticket.priority(), Priority::High);
        assert_eq!(ticket.state(), TicketState::Draft);
        assert_eq!(ticket.spec(), Some(SpecId::new(7)));
        assert_eq!(
            ticket.slice(),
            Some("Registration creates Projects end to end")
        );
        assert_eq!(ticket.criteria().len(), 1);
        assert_eq!(
            ticket.title(),
            None,
            "an Implementation Ticket carries a slice, not a title"
        );
        assert_eq!(ticket.version(), 1);
    }

    #[test]
    fn an_implementation_ticket_attaches_to_exactly_one_spec() {
        assert_eq!(
            implementation(None, vec![criterion(1, 4, "Projects register.")]).unwrap_err(),
            TicketError::UnattachedSpec
        );
    }

    #[test]
    fn a_blank_slice_description_is_refused() {
        let refused = TicketBody::implementation(
            Some(SpecId::new(7)),
            SpecNumber::new(1).expect("the fixture number is positive"),
            "   ",
            vec![criterion(1, 4, "Projects register.")],
        )
        .unwrap_err();

        assert_eq!(refused, TicketError::Blank("slice description"));
    }

    #[test]
    fn an_implementation_ticket_carries_story_linked_criteria() {
        assert_eq!(
            implementation(Some(SpecId::new(7)), Vec::new()).unwrap_err(),
            TicketError::Unclaimed
        );
    }

    #[test]
    fn criteria_claim_the_stories_of_the_spec_the_slice_delivers() {
        assert_eq!(
            implementation(
                Some(SpecId::new(7)),
                vec![criterion(
                    9,
                    4,
                    "Another Spec's story is well linked, just not here."
                )]
            )
            .unwrap_err(),
            TicketError::ForeignStory { story: story(9, 4) }
        );
        assert_eq!(
            TicketError::ForeignStory { story: story(9, 4) }.to_string(),
            "an Implementation Ticket claims the stories of the Spec it delivers; \
             S9-US4 names another Spec"
        );
    }

    #[test]
    fn bugs_and_tasks_attach_to_zero_or_one_spec() {
        let standing = TicketBody::bug(
            "Landing drops the integration branch",
            None,
            "The integration branch is dropped on landing.",
            "The landing log shows the drop.",
        )
        .expect("a Bug may stand alone");
        let attached = TicketBody::task(
            "Archive the old register",
            Some(SpecId::new(3)),
            Some(TaskSubtype::Operational),
            Some(TaskMode::Human),
            vec![CompletionCriterion::new("The register is archived.").expect("the outcome binds")],
            TaskTiming::none(),
        )
        .expect("a Task may attach to one Spec");

        assert_eq!(
            standing,
            TicketBody::Bug(Box::new(BugTicket::restore(
                "Landing drops the integration branch",
                None,
                "The integration branch is dropped on landing.",
                "The landing log shows the drop.",
                None,
                BugFacts::empty(),
            )))
        );
        assert_eq!(
            attached,
            TicketBody::Task(TaskTicket::restore(
                "Archive the old register",
                Some(SpecId::new(3)),
                TaskSubtype::Operational,
                TaskMode::Human,
                vec![
                    CompletionCriterion::new("The register is archived.")
                        .expect("the outcome binds")
                ],
                TaskTiming::none(),
            ))
        );
        assert_eq!(standing.kind(), TicketKind::Bug);
        assert_eq!(attached.kind(), TicketKind::Task);
    }

    #[test]
    fn a_blank_title_is_refused() {
        assert_eq!(
            TicketBody::bug("  ", None, "It drops the branch.", "The log shows it.").unwrap_err(),
            TicketError::Blank("title")
        );
        assert_eq!(
            TicketBody::task(
                "\t",
                None,
                Some(TaskSubtype::Manual),
                Some(TaskMode::Human),
                vec![CompletionCriterion::new("Done.").expect("the outcome binds")],
                TaskTiming::none(),
            )
            .unwrap_err(),
            TicketError::Blank("title")
        );
    }

    #[test]
    fn restore_rehydrates_every_recorded_fact() {
        let ticket = Ticket::restore(
            TicketId::new(5),
            ProjectId::new(2),
            number(11),
            Priority::Urgent,
            TicketState::Active,
            TicketBody::implementation(
                Some(SpecId::new(6)),
                SpecNumber::new(3).expect("the fixture number is positive"),
                "Specs approve end to end",
                vec![
                    criterion(3, 1, "Approval freezes content."),
                    criterion(3, 2, "Supersession stays explicit."),
                ],
            )
            .expect("the fixture body validates"),
            None,
            Some(crate::profile::ProfileName::new("standard").expect("the name validates")),
            None,
            7,
        );

        assert_eq!(ticket.id().value(), 5);
        assert_eq!(ticket.project().value(), 2);
        assert_eq!(ticket.number().value(), 11);
        assert_eq!(ticket.kind(), TicketKind::Implementation);
        assert_eq!(ticket.priority(), Priority::Urgent);
        assert_eq!(ticket.state(), TicketState::Active);
        assert_eq!(ticket.spec(), Some(SpecId::new(6)));
        assert_eq!(ticket.criteria()[1].stories(), [story(3, 2)].as_slice());
        assert_eq!(ticket.profile().map(|name| name.as_str()), Some("standard"));
        assert_eq!(ticket.version(), 7);
    }

    #[test]
    fn an_assignment_names_a_profile_and_bumps_the_version() {
        let mut ticket = Ticket::new(
            TicketId::new(1),
            ProjectId::new(1),
            number(4),
            Priority::Normal,
            TicketBody::bug(
                "Landing drops the integration branch",
                None,
                "The integration branch is dropped on landing.",
                "The landing log shows the drop.",
            )
            .expect("the fixture body validates"),
        );
        assert_eq!(
            ticket.profile(),
            None,
            "a fresh Ticket carries no assignment"
        );

        let named = crate::profile::ProfileName::new("standard").expect("the name validates");
        ticket.assign(named).expect("an open Ticket assigns");

        assert_eq!(ticket.profile().map(|name| name.as_str()), Some("standard"));
        assert_eq!(ticket.version(), 2);
    }

    #[test]
    fn a_terminal_ticket_accepts_no_assignment() {
        let mut ticket = Ticket::new(
            TicketId::new(1),
            ProjectId::new(1),
            number(4),
            Priority::Normal,
            TicketBody::bug(
                "Landing drops the integration branch",
                None,
                "The integration branch is dropped on landing.",
                "The landing log shows the drop.",
            )
            .expect("the fixture body validates"),
        );
        ticket.state = TicketState::Cancelled;

        let named = crate::profile::ProfileName::new("standard").expect("the name validates");
        assert_eq!(ticket.assign(named), Err(TicketError::Terminal));
        assert_eq!(ticket.profile(), None, "the refusal changed nothing");
        assert_eq!(ticket.version(), 1, "the refusal changed nothing");
    }

    #[test]
    fn prioritising_takes_the_closed_vocabulary_and_bumps_the_version() {
        let mut ticket = Ticket::new(
            TicketId::new(1),
            ProjectId::new(1),
            number(4),
            Priority::Normal,
            TicketBody::bug(
                "Landing drops the integration branch",
                None,
                "The integration branch is dropped on landing.",
                "The landing log shows the drop.",
            )
            .expect("the fixture body validates"),
        );

        ticket
            .prioritise(Priority::Urgent)
            .expect("an open Ticket prioritises");

        assert_eq!(ticket.priority(), Priority::Urgent);
        assert_eq!(ticket.version(), 2);

        ticket.state = TicketState::Cancelled;
        assert_eq!(
            ticket.prioritise(Priority::Low),
            Err(TicketError::Terminal),
            "a terminal Ticket accepts no priority change"
        );
        assert_eq!(
            ticket.priority(),
            Priority::Urgent,
            "the refusal changed nothing"
        );
    }

    #[test]
    fn editing_titles_and_slices_serves_only_the_kind_that_carries_them() {
        let mut bug = Ticket::new(
            TicketId::new(1),
            ProjectId::new(1),
            number(4),
            Priority::Normal,
            TicketBody::bug(
                "Landing drops the integration branch",
                None,
                "The integration branch is dropped on landing.",
                "The landing log shows the drop.",
            )
            .expect("the fixture body validates"),
        );
        bug.retitle("Landing drops every branch")
            .expect("a Bug retitles");
        assert_eq!(bug.title(), Some("Landing drops every branch"));
        assert_eq!(bug.version(), 2);
        assert_eq!(
            bug.redescribe("A slice"),
            Err(TicketError::NotSliced),
            "a Bug carries no slice description"
        );

        let mut slice = Ticket::new(
            TicketId::new(2),
            ProjectId::new(1),
            number(5),
            Priority::Normal,
            implementation(
                Some(SpecId::new(7)),
                vec![criterion(1, 4, "Projects register.")],
            )
            .expect("the fixture body validates"),
        );
        slice
            .redescribe("Registration creates Projects end to end, again")
            .expect("an Implementation carries an edited slice");
        assert_eq!(
            slice.slice(),
            Some("Registration creates Projects end to end, again")
        );
        assert_eq!(slice.version(), 2);
        assert_eq!(
            slice.retitle("A title"),
            Err(TicketError::NotTitled),
            "an Implementation carries no title"
        );
    }

    #[test]
    fn a_blank_edit_is_refused_and_a_terminal_ticket_accepts_none() {
        let mut bug = Ticket::new(
            TicketId::new(1),
            ProjectId::new(1),
            number(4),
            Priority::Normal,
            TicketBody::bug(
                "Landing drops the integration branch",
                None,
                "The integration branch is dropped on landing.",
                "The landing log shows the drop.",
            )
            .expect("the fixture body validates"),
        );
        assert_eq!(bug.retitle("  "), Err(TicketError::Blank("title")));
        assert_eq!(bug.title(), Some("Landing drops the integration branch"));

        bug.state = TicketState::Superseded;
        assert_eq!(
            bug.retitle("A fresh title"),
            Err(TicketError::Terminal),
            "a terminal Ticket accepts no edits"
        );
        assert_eq!(bug.version(), 1, "the refusals changed nothing");

        let mut slice = Ticket::new(
            TicketId::new(2),
            ProjectId::new(1),
            number(5),
            Priority::Normal,
            TicketBody::implementation(
                Some(SpecId::new(7)),
                crate::plan::SpecNumber::new(1).expect("the fixture number is positive"),
                "Specs approve end to end",
                vec![criterion(1, 4, "Projects register.")],
            )
            .expect("the fixture body validates"),
        );
        assert_eq!(
            slice.redescribe("\t"),
            Err(TicketError::Blank("slice description"))
        );
    }
}

#[cfg(test)]
mod ticket_pinning {
    use super::{Priority, Ticket, TicketBody, TicketError, TicketId, TicketState};
    use crate::coverage::{AcceptanceCriterion, UserStoryRef};
    use crate::plan::SpecNumber;
    use crate::project::ProjectId;
    use crate::spec::SpecId;

    fn number(value: u64) -> super::TicketNumber {
        super::TicketNumber::new(value).expect("the fixture number is positive")
    }

    fn spec(value: u64) -> SpecNumber {
        SpecNumber::new(value).expect("the fixture number is positive")
    }

    fn story(spec_number: u64, ordinal: u64) -> UserStoryRef {
        UserStoryRef::new(spec(spec_number), ordinal).expect("the fixture ordinal is positive")
    }

    /// A quick-captured Bug attached to Spec 1, in the state a test
    /// chooses.
    fn bug(state: TicketState, spec: Option<SpecId>) -> Ticket {
        let mut ticket = Ticket::new(
            TicketId::new(1),
            ProjectId::new(1),
            number(4),
            Priority::Normal,
            TicketBody::bug(
                "Landing drops the integration branch",
                spec,
                "The integration branch is dropped on landing.",
                "The landing log shows the drop.",
            )
            .expect("the fixture body validates"),
        );
        ticket.state = state;
        ticket
    }

    /// An Implementation delivering Spec 1's behaviour.
    fn implementation() -> Ticket {
        Ticket::new(
            TicketId::new(1),
            ProjectId::new(1),
            number(4),
            Priority::Normal,
            TicketBody::implementation(
                Some(SpecId::new(1)),
                spec(1),
                "Specs mint unique numbers end to end",
                vec![
                    AcceptanceCriterion::new("Specs mint unique numbers.", vec![story(1, 1)])
                        .expect("the fixture criterion links"),
                ],
            )
            .expect("the fixture body validates"),
        )
    }

    #[test]
    fn graph_approval_pins_the_ticket_to_its_spec_version() {
        let mut ticket = bug(TicketState::Draft, Some(SpecId::new(1)));

        ticket.pin_to(2).expect("the approval pins its Tickets");

        assert_eq!(ticket.pinned_version(), Some(2));
        assert_eq!(ticket.version(), 2, "the pin is one applied change");
    }

    #[test]
    fn a_ticket_pins_once() {
        let mut ticket = bug(TicketState::Draft, Some(SpecId::new(1)));
        ticket.pin_to(2).expect("the first approval pins");

        assert_eq!(
            ticket.pin_to(3).unwrap_err(),
            TicketError::AlreadyPinned,
            "a second graph approval names no Ticket of an approved graph"
        );
        assert_eq!(
            ticket.pinned_version(),
            Some(2),
            "the refusal changed nothing"
        );
        assert_eq!(ticket.version(), 2, "the refusal changed nothing");
        assert_eq!(
            TicketError::AlreadyPinned.to_string(),
            "a Ticket pins once, to the version its approved graph named"
        );
    }

    #[test]
    fn a_terminal_ticket_accepts_no_pin() {
        let mut ticket = bug(TicketState::Superseded, Some(SpecId::new(1)));

        assert_eq!(ticket.pin_to(2), Err(TicketError::Terminal));
        assert_eq!(ticket.pinned_version(), None, "the refusal changed nothing");
    }

    #[test]
    fn a_draft_ticket_moves_between_specs_before_approval() {
        let mut ticket = bug(TicketState::Draft, Some(SpecId::new(1)));

        ticket
            .move_to_spec(SpecId::new(4), spec(4))
            .expect("a draft, unpinned Ticket moves inside its Project");

        assert_eq!(ticket.spec(), Some(SpecId::new(4)));
        assert_eq!(ticket.version(), 2, "the move is one applied change");

        // An unattached Bug or Task attaches on the move the same way.
        let mut standing = bug(TicketState::Draft, None);
        standing
            .move_to_spec(SpecId::new(4), spec(4))
            .expect("a standing Ticket attaches by the same move");
        assert_eq!(standing.spec(), Some(SpecId::new(4)));
    }

    #[test]
    fn a_pinned_ticket_stays_with_its_spec_and_version() {
        let mut ticket = bug(TicketState::Draft, Some(SpecId::new(1)));
        ticket.pin_to(2).expect("the approval pins");

        assert_eq!(
            ticket.move_to_spec(SpecId::new(4), spec(4)).unwrap_err(),
            TicketError::Pinned,
            "approved Tickets stay pinned (DR-DE-06)"
        );
        assert_eq!(ticket.spec(), Some(SpecId::new(1)));
        assert_eq!(ticket.pinned_version(), Some(2));
        assert_eq!(ticket.version(), 2, "the refusal changed nothing");
        assert_eq!(
            TicketError::Pinned.to_string(),
            "a pinned Ticket stays with the Spec version it was approved against"
        );
    }

    #[test]
    fn an_executed_ticket_never_moves() {
        for executed in [
            TicketState::Parked,
            TicketState::Blocked,
            TicketState::Scheduled,
            TicketState::Ready,
            TicketState::Active,
            TicketState::InReview,
            TicketState::Approved,
            TicketState::Landing,
            TicketState::Done,
            TicketState::Cancelled,
            TicketState::Superseded,
        ] {
            let mut ticket = bug(executed, Some(SpecId::new(1)));

            assert_eq!(
                ticket.move_to_spec(SpecId::new(4), spec(4)).unwrap_err(),
                if executed.is_terminal() {
                    TicketError::Terminal
                } else {
                    TicketError::MoveRequiresDraft
                },
                "`{}` is past the draft move (DR-DE-05)",
                executed.wire_name()
            );
            assert_eq!(ticket.spec(), Some(SpecId::new(1)));
            assert_eq!(ticket.version(), 1, "the refusal changed nothing");
        }
        assert_eq!(
            TicketError::MoveRequiresDraft.to_string(),
            "only a draft Ticket moves between Specs"
        );
    }

    #[test]
    fn an_implementation_keeps_claiming_the_spec_it_delivers() {
        let mut ticket = implementation();

        let refused = ticket.move_to_spec(SpecId::new(4), spec(4)).unwrap_err();

        assert_eq!(
            refused,
            TicketError::ForeignStory { story: story(1, 1) },
            "the slice's criteria claim Spec 1's stories, not Spec 4's (DR-TK-04)"
        );
        assert_eq!(ticket.spec(), Some(SpecId::new(1)));
        assert_eq!(ticket.version(), 1, "the refusal changed nothing");

        // Naming the Spec the claims already deliver is the one
        // implementation move that holds its claims true.
        ticket
            .move_to_spec(SpecId::new(1), spec(1))
            .expect("the claims still name the destination's stories");
        assert_eq!(ticket.spec(), Some(SpecId::new(1)));
    }
}

#[cfg(test)]
mod bug_qualification {
    use super::{
        BugFacts, BugQualification, BugTicket, CompletionCriterion, ExternalReference,
        OccurrenceSnapshot, Severity, SnapshotError, TaskMode, TaskSubtype, TaskTiming, Ticket,
        TicketBody, TicketError, TicketId, TicketKind, TicketState,
    };
    use crate::coverage::{AcceptanceCriterion, UserStoryRef, VerificationStep};
    use crate::evidence::EvidenceId;
    use crate::plan::SpecNumber;
    use crate::project::ProjectId;
    use crate::spec::SpecId;

    fn story(spec: u64, ordinal: u64) -> UserStoryRef {
        UserStoryRef::new(
            SpecNumber::new(spec).expect("the fixture number is positive"),
            ordinal,
        )
        .expect("the fixture ordinal is positive")
    }

    /// A quick-captured Bug: the three capture facts and nothing else.
    fn captured() -> Ticket {
        let body = TicketBody::bug(
            "Landing drops the integration branch",
            None,
            "The integration branch is dropped after a review lands.",
            "The landing log names the drop immediately after the merge.",
        )
        .expect("quick capture accepts the three facts");
        Ticket::new(
            TicketId::new(3),
            ProjectId::new(1),
            super::TicketNumber::new(4).expect("the fixture number is positive"),
            super::Priority::Normal,
            body,
        )
    }

    fn criterion(outcome: &str) -> AcceptanceCriterion {
        AcceptanceCriterion::new(outcome, vec![story(1, 3)]).expect("the fixture criterion links")
    }

    fn step(command: &str) -> VerificationStep {
        VerificationStep::new(command).expect("the fixture step carries its command")
    }

    /// One complete qualification, with `severity` and optional
    /// overrides for the fields a test varies.
    fn qualified(severity: Severity) -> BugQualification {
        BugQualification::new(
            "The integration branch survives every landing.",
            "Re land a reviewed change; the branch list still names it.",
            "macOS 26, Kanban 0.1.0, SQLite 3.50.",
            severity,
            "Every landing so far.",
            "All landing reviews of every Project.",
            "Duplicate landings and lost review state.",
            vec![criterion("The integration branch survives a landing.")],
            vec![step("cargo test -p kanban-storage tickets")],
        )
        .expect("the fixture qualification is complete")
    }

    #[test]
    fn quick_capture_needs_only_title_actual_behaviour_and_reporter_evidence() {
        let bug = captured();

        assert_eq!(bug.kind(), TicketKind::Bug);
        assert_eq!(bug.state(), TicketState::Draft, "capture lands in draft");
        let body = bug.bug().expect("the Bug body is reachable");
        assert_eq!(body.title(), "Landing drops the integration branch");
        assert_eq!(
            body.actual_behaviour(),
            "The integration branch is dropped after a review lands."
        );
        assert_eq!(
            body.reporter_evidence(),
            "The landing log names the drop immediately after the merge."
        );
        assert_eq!(body.spec(), None, "a captured Bug may stand alone");
        assert!(
            !body.is_qualified(),
            "capture requires no qualification (DR-TK-08)"
        );
        assert_eq!(
            body.severity(),
            None,
            "severity arrives only with qualification (DR-LC-13)"
        );
        assert_eq!(
            body.facts(),
            &BugFacts::empty(),
            "capture requires no references, snapshots, or evidence"
        );
    }

    #[test]
    fn quick_capture_refuses_a_blank_capture_fact() {
        let evidence = "The landing log names the drop.";
        let behaviour = "The integration branch is dropped.";
        assert_eq!(
            TicketBody::bug("  ", None, behaviour, evidence).unwrap_err(),
            TicketError::Blank("title")
        );
        assert_eq!(
            TicketBody::bug("A title", None, "   ", evidence).unwrap_err(),
            TicketError::Blank("actual behaviour")
        );
        assert_eq!(
            TicketBody::bug("A title", None, behaviour, "\n\t").unwrap_err(),
            TicketError::Blank("reporter evidence")
        );
    }

    #[test]
    fn a_bug_stays_draft_until_a_complete_qualification_exists() {
        let mut bug = captured();

        assert_eq!(bug.state(), TicketState::Draft);
        assert_eq!(bug.version(), 1);
        bug.qualify(qualified(Severity::High))
            .expect("the Bug qualifies");

        assert_eq!(
            bug.state(),
            TicketState::Draft,
            "qualification completes a Bug; readiness is computed, never a state change"
        );
        assert_eq!(bug.version(), 2, "the applied change bumps the version");
        let body = bug.bug().expect("the Bug body is reachable");
        assert!(body.is_qualified());
        assert_eq!(body.severity(), Some(Severity::High));
        assert_eq!(
            body.qualification()
                .expect("the qualification exists")
                .expected_behaviour(),
            "The integration branch survives every landing."
        );
    }

    #[test]
    fn qualification_refuses_any_missing_fact() {
        let full = |expected: &str,
                    reproduction: &str,
                    environment: &str,
                    frequency: &str,
                    scope: &str,
                    risk: &str| {
            BugQualification::new(
                expected,
                reproduction,
                environment,
                Severity::Medium,
                frequency,
                scope,
                risk,
                vec![criterion("The integration branch survives a landing.")],
                vec![step("cargo test -p kanban-domain bug_qualification")],
            )
        };

        assert_eq!(
            full(
                "  ",
                "Re land it.",
                "macOS 26.",
                "Always.",
                "Landings.",
                "Lost reviews."
            )
            .unwrap_err(),
            TicketError::Blank("expected behaviour")
        );
        assert_eq!(
            full(
                "It survives.",
                "   ",
                "macOS 26.",
                "Always.",
                "Landings.",
                "Lost reviews."
            )
            .unwrap_err(),
            TicketError::Blank("reproduction or failing test")
        );
        assert_eq!(
            full(
                "It survives.",
                "Re land it.",
                "  ",
                "Always.",
                "Landings.",
                "Lost reviews."
            )
            .unwrap_err(),
            TicketError::Blank("environment")
        );
        assert_eq!(
            full(
                "It survives.",
                "Re land it.",
                "macOS 26.",
                "\t",
                "Landings.",
                "Lost reviews."
            )
            .unwrap_err(),
            TicketError::Blank("frequency")
        );
        assert_eq!(
            full(
                "It survives.",
                "Re land it.",
                "macOS 26.",
                "Always.",
                "  ",
                "Lost reviews."
            )
            .unwrap_err(),
            TicketError::Blank("affected scope")
        );
        assert_eq!(
            full(
                "It survives.",
                "Re land it.",
                "macOS 26.",
                "Always.",
                "Landings.",
                "   "
            )
            .unwrap_err(),
            TicketError::Blank("risk")
        );

        let missing_criteria = BugQualification::new(
            "It survives.",
            "Re land it.",
            "macOS 26.",
            Severity::Medium,
            "Always.",
            "Landings.",
            "Lost reviews.",
            Vec::new(),
            vec![step("cargo test -p kanban-domain bug_qualification")],
        )
        .unwrap_err();
        assert_eq!(
            missing_criteria,
            TicketError::Blank("Acceptance Criteria claim"),
            "a Bug qualifies against its criteria (DR-TK-09)"
        );

        let missing_steps = BugQualification::new(
            "It survives.",
            "Re land it.",
            "macOS 26.",
            Severity::Medium,
            "Always.",
            "Landings.",
            "Lost reviews.",
            vec![criterion("The integration branch survives a landing.")],
            Vec::new(),
        )
        .unwrap_err();
        assert_eq!(
            missing_steps,
            TicketError::Blank("Verification Steps claim")
        );
    }

    #[test]
    fn severity_takes_the_closed_values_and_qualification_sets_it() {
        assert_eq!(Severity::ALL.len(), 4);
        for severity in Severity::ALL {
            assert_eq!(
                Severity::parse(severity.wire_name()),
                Some(*severity),
                "`{}` must survive the round trip",
                severity.wire_name()
            );
        }
        assert_eq!(Severity::parse("ghost"), None);
        assert_eq!(Severity::parse("urgent"), None, "priority is not severity");

        let mut urgent = captured();
        urgent
            .qualify(qualified(Severity::Critical))
            .expect("the Bug qualifies");
        assert_eq!(
            urgent.bug().expect("the Bug body is reachable").severity(),
            Some(Severity::Critical),
            "qualification sets the severity (DR-LC-13)"
        );

        // A later qualification replaces the whole record, severity
        // included, in one act.
        urgent
            .qualify(qualified(Severity::Low))
            .expect("the Bug qualifies again");
        assert_eq!(
            urgent.bug().expect("the Bug body is reachable").severity(),
            Some(Severity::Low)
        );
    }

    #[test]
    fn only_a_bug_carries_qualification_and_bug_facts() {
        let implementation = Ticket::new(
            TicketId::new(1),
            ProjectId::new(1),
            super::TicketNumber::new(1).expect("the fixture number is positive"),
            super::Priority::Normal,
            TicketBody::implementation(
                Some(SpecId::new(7)),
                SpecNumber::new(1).expect("the fixture number is positive"),
                "Specs approve end to end",
                vec![criterion("Approval freezes content.")],
            )
            .expect("the fixture body validates"),
        );
        let task = Ticket::new(
            TicketId::new(2),
            ProjectId::new(1),
            super::TicketNumber::new(2).expect("the fixture number is positive"),
            super::Priority::Normal,
            TicketBody::task(
                "Archive the old register",
                None,
                Some(TaskSubtype::Operational),
                Some(TaskMode::Human),
                vec![
                    CompletionCriterion::new("The register is archived.")
                        .expect("the outcome binds"),
                ],
                TaskTiming::none(),
            )
            .expect("the fixture body validates"),
        );

        let mut not_a_bug = implementation;
        assert_eq!(
            not_a_bug.qualify(qualified(Severity::Low)).unwrap_err(),
            TicketError::NotABug
        );
        assert_eq!(
            not_a_bug.record_bug_facts(BugFacts::empty()).unwrap_err(),
            TicketError::NotABug
        );
        assert_eq!(not_a_bug.version(), 1, "the refusals changed nothing");

        let mut not_a_bug = task;
        assert_eq!(
            not_a_bug.record_bug_facts(BugFacts::empty()).unwrap_err(),
            TicketError::NotABug
        );
    }

    #[test]
    fn external_references_are_uris_with_optional_labels() {
        let plain = ExternalReference::new(" https://example.invalid/issues/12 ", None)
            .expect("an https URI is a reference");
        assert_eq!(plain.uri(), "https://example.invalid/issues/12");
        assert_eq!(plain.label(), None);

        let labelled =
            ExternalReference::new("mailto:ops@example.invalid", Some("The report".to_owned()))
                .expect("any scheme carrying a URI is a reference");
        assert_eq!(labelled.label(), Some("The report"));

        let custom = ExternalReference::new("tracker:KAN-9000", Some("  ".to_owned()))
            .expect_err("a blank label names nothing");
        assert_eq!(custom, TicketError::Blank("External Reference label"));

        for refused in [
            "",
            "no-scheme-here",
            "https://example.invalid/a b",
            "1https://example.invalid",
            "://missing-scheme",
        ] {
            assert_eq!(
                ExternalReference::new(refused, None).unwrap_err(),
                TicketError::InvalidReference,
                "`{refused}` names no URI"
            );
        }
        let _ = custom_scheme_is_accepted();
    }

    /// A custom scheme stays acceptable: vendor-neutral means any
    /// scheme, not a curated list.
    fn custom_scheme_is_accepted() -> Option<ExternalReference> {
        ExternalReference::new("msteams:+chat", None).ok()
    }

    #[test]
    fn occurrence_snapshots_carry_an_rfc3339_moment_and_an_observation() {
        let snapshot =
            OccurrenceSnapshot::new("2026-09-05T09:41:00+02:00", "Two branches landed at once.")
                .expect("an RFC 3339 moment with an observation is a snapshot");
        assert_eq!(snapshot.observed_at(), "2026-09-05T09:41:00+02:00");
        assert_eq!(snapshot.observation(), "Two branches landed at once.");

        let utc = OccurrenceSnapshot::new("2026-09-05T07:41:00Z", "The log shows the drop.")
            .expect("a UTC moment is RFC 3339 too");
        assert_eq!(utc.observed_at(), "2026-09-05T07:41:00Z");

        assert_eq!(
            OccurrenceSnapshot::new("yesterday", "Seen.").unwrap_err(),
            TicketError::InvalidSnapshot {
                reason: SnapshotError::MalformedTime
            }
        );
        assert_eq!(
            OccurrenceSnapshot::new("2026-13-40T07:41:00Z", "Seen.").unwrap_err(),
            TicketError::InvalidSnapshot {
                reason: SnapshotError::MalformedTime
            }
        );
        assert_eq!(
            OccurrenceSnapshot::new("2026-09-05T07:41:00Z", "   ").unwrap_err(),
            TicketError::InvalidSnapshot {
                reason: SnapshotError::BlankObservation
            }
        );
    }

    #[test]
    fn bug_facts_carry_references_snapshots_and_evidence_items() {
        let facts = BugFacts::new(
            vec![
                ExternalReference::new(
                    "https://example.invalid/issues/12",
                    Some("The report".to_owned()),
                )
                .expect("the reference is a URI"),
            ],
            vec![
                OccurrenceSnapshot::new("2026-09-05T07:41:00Z", "The log shows the drop.")
                    .expect("the snapshot carries its moment"),
            ],
            vec![EvidenceId::new(1), EvidenceId::new(4)],
        )
        .expect("the collections assemble");

        assert_eq!(facts.external_references().len(), 1);
        assert_eq!(
            facts.external_references()[0].uri(),
            "https://example.invalid/issues/12"
        );
        assert_eq!(facts.occurrence_snapshots().len(), 1);
        assert_eq!(
            facts.evidence_items(),
            [EvidenceId::new(1), EvidenceId::new(4)].as_slice()
        );

        assert_eq!(
            BugFacts::new(Vec::new(), Vec::new(), vec![EvidenceId::new(0)]).unwrap_err(),
            TicketError::InvalidEvidenceId,
            "an Evidence Item identity starts at one"
        );
        let empty = BugFacts::new(Vec::new(), Vec::new(), Vec::new())
            .expect("every collection may be empty");
        assert_eq!(empty, BugFacts::empty());
    }

    #[test]
    fn recording_bug_facts_replaces_the_collections_and_bumps_the_version() {
        let mut bug = captured();
        bug.qualify(qualified(Severity::High))
            .expect("the Bug qualifies");
        let version = bug.version();

        let facts = BugFacts::new(
            vec![
                ExternalReference::new("https://example.invalid/issues/12", None)
                    .expect("the reference is a URI"),
            ],
            Vec::new(),
            vec![EvidenceId::new(2)],
        )
        .expect("the collections assemble");
        bug.record_bug_facts(facts)
            .expect("the Bug carries its facts");

        let body = bug.bug().expect("the Bug body is reachable");
        assert_eq!(body.facts().external_references().len(), 1);
        assert_eq!(
            body.facts().evidence_items(),
            [EvidenceId::new(2)].as_slice()
        );
        assert_eq!(bug.version(), version + 1);
        assert_eq!(
            bug.state(),
            TicketState::Draft,
            "carrying facts is not a lifecycle change"
        );
        assert!(
            body.is_qualified(),
            "recording facts leaves the qualification standing"
        );
    }

    #[test]
    fn restore_rehydrates_every_recorded_bug_fact() {
        let qualification = qualified(Severity::Critical);
        let facts = BugFacts::new(
            Vec::new(),
            vec![
                OccurrenceSnapshot::new("2026-09-05T07:41:00Z", "The log shows the drop.")
                    .expect("the snapshot carries its moment"),
            ],
            Vec::new(),
        )
        .expect("the collections assemble");
        let body = BugTicket::restore(
            "Landing drops the integration branch",
            Some(SpecId::new(2)),
            "The integration branch is dropped.",
            "The landing log names the drop.",
            Some(qualification),
            facts,
        );

        assert_eq!(body.spec(), Some(SpecId::new(2)));
        assert!(body.is_qualified());
        assert_eq!(body.severity(), Some(Severity::Critical));
        assert_eq!(body.facts().occurrence_snapshots().len(), 1);
        assert_eq!(
            body.qualification()
                .expect("the qualification exists")
                .verification_steps()
                .len(),
            1
        );
    }
}

#[cfg(test)]
mod task_rules {
    use super::{
        CompletionCriterion, Priority, TaskMode, TaskSubtype, TaskTicket, TaskTiming, Ticket,
        TicketBody, TicketError, TicketId, TicketKind, TicketNumber, TicketState,
    };
    use crate::project::ProjectId;
    use crate::spec::SpecId;

    fn number(value: u64) -> TicketNumber {
        TicketNumber::new(value).expect("the fixture number is positive")
    }

    /// A rule-valid completion criterion.
    fn done(outcome: &str) -> CompletionCriterion {
        CompletionCriterion::new(outcome).expect("the fixture outcome states something")
    }

    /// A Task body with the fields a test varies, otherwise
    /// rule-valid.
    fn task(
        subtype: Option<TaskSubtype>,
        mode: Option<TaskMode>,
        completion: Vec<CompletionCriterion>,
    ) -> Result<TicketBody, TicketError> {
        TicketBody::task(
            "Archive the old register",
            Some(SpecId::new(3)),
            subtype,
            mode,
            completion,
            TaskTiming::none(),
        )
    }

    #[test]
    fn subtypes_and_modes_round_trip_through_their_wire_names() {
        assert_eq!(TaskSubtype::ALL.len(), 7);
        for subtype in TaskSubtype::ALL {
            assert_eq!(
                TaskSubtype::parse(subtype.wire_name()),
                Some(*subtype),
                "`{}` must survive the round trip",
                subtype.wire_name()
            );
        }
        assert_eq!(TaskSubtype::parse("ghost"), None);

        assert_eq!(TaskMode::ALL.len(), 2);
        for mode in TaskMode::ALL {
            assert_eq!(
                TaskMode::parse(mode.wire_name()),
                Some(*mode),
                "`{}` must survive the round trip",
                mode.wire_name()
            );
        }
        assert_eq!(TaskMode::parse("ghost"), None);
    }

    #[test]
    fn a_fresh_task_takes_one_subtype_and_one_mode() {
        let ticket = Ticket::new(
            TicketId::new(4),
            ProjectId::new(1),
            number(6),
            Priority::Normal,
            task(
                Some(TaskSubtype::Migration),
                Some(TaskMode::Agent),
                vec![done("The register archive is restorable.")],
            )
            .expect("the body validates"),
        );

        assert_eq!(ticket.kind(), TicketKind::Task);
        assert_eq!(ticket.state(), TicketState::Draft);
        assert_eq!(ticket.subtype(), Some(TaskSubtype::Migration));
        assert_eq!(ticket.task_mode(), Some(TaskMode::Agent));
        assert_eq!(ticket.title(), Some("Archive the old register"));
        assert_eq!(ticket.spec(), Some(SpecId::new(3)));
        assert_eq!(ticket.completion().len(), 1);
        assert_eq!(
            ticket.completion()[0].outcome(),
            "The register archive is restorable."
        );
        assert_eq!(ticket.scheduled_for(), None);
        assert_eq!(ticket.due(), None);
        assert_eq!(ticket.version(), 1);
    }

    #[test]
    fn a_task_names_one_subtype_and_one_mode_or_is_refused() {
        assert_eq!(
            task(None, Some(TaskMode::Human), vec![done("Done.")]).unwrap_err(),
            TicketError::UnspecifiedSubtype
        );
        assert_eq!(
            task(Some(TaskSubtype::Research), None, vec![done("Done.")]).unwrap_err(),
            TicketError::UnstatedMode
        );
        assert_eq!(
            TicketError::UnspecifiedSubtype.to_string(),
            "a Task Ticket names one subtype of the closed set"
        );
        assert_eq!(
            TicketError::UnstatedMode.to_string(),
            "a Task Ticket names a human or agent mode"
        );
    }

    #[test]
    fn a_task_is_bounded_by_completion_criteria() {
        assert_eq!(
            task(
                Some(TaskSubtype::Operational),
                Some(TaskMode::Human),
                Vec::new()
            )
            .unwrap_err(),
            TicketError::Unbounded
        );
        assert_eq!(
            TicketError::Unbounded.to_string(),
            "a Task Ticket carries completion criteria"
        );
        assert_eq!(
            CompletionCriterion::new("   ").unwrap_err(),
            TicketError::Blank("completion criterion")
        );
    }

    #[test]
    fn a_task_never_claims_user_stories() {
        let ticket = Ticket::new(
            TicketId::new(9),
            ProjectId::new(1),
            number(6),
            Priority::Low,
            task(
                Some(TaskSubtype::Investigative),
                Some(TaskMode::Human),
                vec![
                    done("The cause is named in writing."),
                    done("The follow-up is decided."),
                ],
            )
            .expect("the body validates"),
        );

        // Attaching to a Spec is not claiming it: a Task's
        // criteria state outcomes alone, and the story-linked
        // accessor — the one coverage reads — holds nothing.
        assert_eq!(ticket.spec(), Some(SpecId::new(3)));
        assert!(
            ticket.criteria().is_empty(),
            "a Task claims no User Story through story-linked criteria (DR-TK-07)"
        );
    }

    #[test]
    fn optional_timing_normalises_to_the_stored_shape() {
        let timing = TaskTiming::new(
            Some("2026-09-10T11:00:00+02:00".to_owned()),
            Some("2026-09-30T17:00:00Z".to_owned()),
        )
        .expect("RFC 3339 instants validate");

        assert_eq!(
            timing.scheduled_for(),
            Some("2026-09-10T09:00:00.000Z"),
            "an offset instant stores as UTC"
        );
        assert_eq!(timing.due(), Some("2026-09-30T17:00:00.000Z"));

        let absent = TaskTiming::new(None, None).expect("both timing fields are optional");
        assert_eq!(absent, TaskTiming::none());
    }

    #[test]
    fn malformed_timing_is_refused_with_the_field_named() {
        assert_eq!(
            TaskTiming::new(Some("September".to_owned()), None).unwrap_err(),
            TicketError::MalformedTiming {
                field: "activation",
                value: "September".to_owned(),
            }
        );
        assert_eq!(
            TaskTiming::new(None, Some("2026-09-30".to_owned())).unwrap_err(),
            TicketError::MalformedTiming {
                field: "due date",
                value: "2026-09-30".to_owned(),
            }
        );
        assert_eq!(
            TicketError::MalformedTiming {
                field: "due date",
                value: "2026-09-30".to_owned(),
            }
            .to_string(),
            "a Task due date must be an RFC 3339 instant: `2026-09-30`"
        );
    }

    #[test]
    fn restore_rehydrates_every_recorded_task_fact() {
        let ticket = Ticket::restore(
            TicketId::new(5),
            ProjectId::new(2),
            number(11),
            Priority::Urgent,
            TicketState::Scheduled,
            TicketBody::Task(TaskTicket::restore(
                "Archive the old register",
                None,
                TaskSubtype::Administrative,
                TaskMode::Human,
                vec![done("The archive is restorable.")],
                TaskTiming::new(Some("2026-10-01T00:00:00Z".to_owned()), None)
                    .expect("the fixture timing validates"),
            )),
            None,
            None,
            4,
        );

        assert_eq!(ticket.subtype(), Some(TaskSubtype::Administrative));
        assert_eq!(ticket.task_mode(), Some(TaskMode::Human));
        assert_eq!(
            ticket.completion()[0].outcome(),
            "The archive is restorable."
        );
        assert_eq!(ticket.scheduled_for(), Some("2026-10-01T00:00:00.000Z"));
        assert_eq!(ticket.due(), None);
        assert_eq!(ticket.version(), 4);
    }
}
