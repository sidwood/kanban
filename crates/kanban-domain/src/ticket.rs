//! The Ticket entity: one independently grabbable unit of work
//! (CONTEXT.md). Every Ticket belongs to exactly one Project (DR-TK-01)
//! and is named by the number that Project minted for it. Kinds carry
//! different obligations: an Implementation Ticket is a small vertical
//! slice delivering the behaviour of exactly one Spec, named end to end
//! by its slice description and claiming that Spec's User Stories
//! through story-linked criteria (DR-TK-02, DR-TK-04); a Bug records
//! incorrect behaviour and may attach to one Spec or stand alone
//! (DR-TK-03), qualifying in a later slice; a Task is bounded
//! non-story work with the same optional attachment (DR-TK-06). Every
//! kind carries the closed priority vocabulary urgent, high, normal,
//! low (DR-LC-12) and starts its lifecycle in draft (DR-LC-01). The
//! lifecycle's transitions and readiness rules land in KAN-T21,
//! dependencies in KAN-T20, and graph approval pinning in KAN-T23;
//! this module owns the shape a Ticket is created with.

use std::fmt;

use crate::coverage::{AcceptanceCriterion, UserStoryRef};
use crate::plan::SpecNumber;
use crate::project::ProjectId;
use crate::spec::SpecId;

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
}

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
}

/// One Bug Ticket's creation schema (DR-TK-03): a title and an
/// optional Spec attachment. Quick capture fields and qualification
/// land in KAN-T18.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BugTicket {
    title: String,
    spec: Option<SpecId>,
}

impl BugTicket {
    /// Rehydrate a stored body exactly as it was recorded.
    pub fn restore(title: impl Into<String>, spec: Option<SpecId>) -> Self {
        Self {
            title: title.into(),
            spec,
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
}

/// One Task Ticket's creation schema (DR-TK-06): a title and an
/// optional Spec attachment. Subtypes, modes, completion criteria,
/// and schedule fields land in KAN-T19.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskTicket {
    title: String,
    spec: Option<SpecId>,
}

impl TaskTicket {
    /// Rehydrate a stored body exactly as it was recorded.
    pub fn restore(title: impl Into<String>, spec: Option<SpecId>) -> Self {
        Self {
            title: title.into(),
            spec,
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
}

/// The kind-specific schema one Ticket carries (KAN-S4-US1): each kind
/// holds exactly its own obligations, so a field that names no kind —
/// a Bug's qualification evidence, a Task's completion criteria —
/// arrives with the slice that owns it, never here early.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketBody {
    /// A vertical slice of exactly one Spec.
    Implementation(ImplementationTicket),
    /// Incorrect behaviour, qualification pending.
    Bug(BugTicket),
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

    /// Assemble a Bug body, refusing a blank title.
    pub fn bug(title: impl Into<String>, spec: Option<SpecId>) -> Result<Self, TicketError> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(TicketError::Blank("title"));
        }
        Ok(Self::Bug(BugTicket { title, spec }))
    }

    /// Assemble a Task body, refusing a blank title.
    pub fn task(title: impl Into<String>, spec: Option<SpecId>) -> Result<Self, TicketError> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(TicketError::Blank("title"));
        }
        Ok(Self::Task(TaskTicket { title, spec }))
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
/// Project minted for it, its priority, its lifecycle state, and the
/// kind-specific body. The version counts applied changes: creation
/// lands at 1 and every later legal change bumps it, so a stored
/// version is all a caller needs for optimistic checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    id: TicketId,
    project: ProjectId,
    number: TicketNumber,
    priority: Priority,
    state: TicketState,
    body: TicketBody,
    version: u64,
}

impl Ticket {
    /// A fresh Ticket: created into draft, at version 1, carrying its
    /// kind's schema. The body's own constructors hold the
    /// kind-specific rules; a body rehydrated from storage passes
    /// through unchanged.
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
            version: 1,
        }
    }

    /// Rehydrate a stored Ticket exactly as it was recorded.
    pub fn restore(
        id: TicketId,
        project: ProjectId,
        number: TicketNumber,
        priority: Priority,
        state: TicketState,
        body: TicketBody,
        version: u64,
    ) -> Self {
        Self {
            id,
            project,
            number,
            priority,
            state,
            body,
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
    /// other kind.
    pub fn criteria(&self) -> &[AcceptanceCriterion] {
        match &self.body {
            TicketBody::Implementation(implementation) => implementation.criteria(),
            _ => &[],
        }
    }

    /// The number of applied changes, for optimistic version checks.
    pub fn version(&self) -> u64 {
        self.version
    }
}

#[cfg(test)]
mod ticket_kinds {
    use super::{
        BugTicket, Priority, TaskTicket, Ticket, TicketBody, TicketError, TicketId, TicketKind,
        TicketNumber, TicketState,
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
        let standing = TicketBody::bug("Landing drops the integration branch", None)
            .expect("a Bug may stand alone");
        let attached = TicketBody::task("Archive the old register", Some(SpecId::new(3)))
            .expect("a Task may attach to one Spec");

        assert_eq!(
            standing,
            TicketBody::Bug(BugTicket::restore(
                "Landing drops the integration branch",
                None
            ))
        );
        assert_eq!(
            attached,
            TicketBody::Task(TaskTicket::restore(
                "Archive the old register",
                Some(SpecId::new(3))
            ))
        );
        assert_eq!(standing.kind(), TicketKind::Bug);
        assert_eq!(attached.kind(), TicketKind::Task);
    }

    #[test]
    fn a_blank_title_is_refused() {
        assert_eq!(
            TicketBody::bug("  ", None).unwrap_err(),
            TicketError::Blank("title")
        );
        assert_eq!(
            TicketBody::task("\t", None).unwrap_err(),
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
        assert_eq!(ticket.version(), 7);
    }
}
