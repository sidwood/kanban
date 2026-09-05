//! The Spec entity: the lightweight PRD for one behaviour area
//! (CONTEXT.md). A Spec belongs to its Project and, once planned, to
//! one Plan at a time (DR-PS-06); it carries the nine PRD sections
//! (DR-PS-07) in content versions that move draft, approved,
//! superseded (DR-PS-08). Approved versions are immutable: a material
//! change mints a new version instead of editing one, and supersession
//! is always an explicit act that keeps the superseded version
//! queryable for the Tickets pinned to it (DR-PS-09, DR-PS-10,
//! DR-PS-11). Spec execution — a Spec's progress through its Plan —
//! is tracked separately from content: unplanned, planned, blocked,
//! ready, active, integration review, complete, cancelled (DR-PS-12).

use std::fmt;

use crate::plan::{PlanId, SpecNumber};
use crate::project::ProjectId;

/// The identity of one Spec. Assigned once by storage and immutable
/// afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpecId(u64);

impl SpecId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SpecId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a Spec refused an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecError {
    /// A Spec is named; the name section cannot be empty.
    Unnamed,
    /// Only a draft version can be approved.
    ApproveRequiresDraft,
    /// Another version is still approved; supersede it first, because
    /// supersession is explicit (DR-PS-11).
    ApproveBlockedByApproved {
        /// The version that must be superseded first.
        approved: u64,
    },
    /// The version named for supersession does not exist.
    SupersedeUnknown {
        /// The refused version number.
        version: u64,
    },
    /// A superseded version is terminal and cannot move again.
    SupersedeRefused {
        /// The refused version number.
        version: u64,
    },
    /// Only an unplanned Spec joins a Plan.
    RequiresUnplanned,
    /// The Spec already belongs to a Plan; one Plan at a time
    /// (DR-PS-06).
    AlreadyPlanned {
        /// The Plan the Spec already belongs to.
        plan: PlanId,
    },
    /// The Spec does not belong to the named Plan, so it holds no
    /// binding there to leave.
    NotBoundTo {
        /// The Plan the caller named.
        plan: PlanId,
    },
    /// The execution move is not in the closed transition set.
    IllegalExecutionMove {
        /// The state the Spec is in.
        from: SpecExecutionState,
        /// The state the command named.
        to: SpecExecutionState,
    },
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unnamed => write!(f, "a Spec needs a name"),
            Self::ApproveRequiresDraft => {
                write!(f, "only a draft content version can be approved")
            }
            Self::ApproveBlockedByApproved { approved } => write!(
                f,
                "version {approved} is still approved; supersede it before approving another"
            ),
            Self::SupersedeUnknown { version } => {
                write!(f, "version {version} does not exist on this Spec")
            }
            Self::SupersedeRefused { version } => {
                write!(f, "version {version} is already superseded")
            }
            Self::RequiresUnplanned => write!(f, "only an unplanned Spec joins a Plan"),
            Self::AlreadyPlanned { plan } => {
                write!(f, "the Spec already belongs to Plan {plan}")
            }
            Self::NotBoundTo { plan } => {
                write!(f, "the Spec does not belong to Plan {plan}")
            }
            Self::IllegalExecutionMove { from, to } => write!(
                f,
                "execution cannot move from {} to {}",
                from.wire_name(),
                to.wire_name()
            ),
        }
    }
}

impl std::error::Error for SpecError {}

/// The lightweight PRD one Spec carries: the nine sections of
/// CONTEXT.md (DR-PS-07). Every section is free text; the name alone
/// is required, because a draft starts rough and the rest fills in as
/// thinking does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecContent {
    name: String,
    short_description: String,
    problem_statement: String,
    solution: String,
    user_stories: String,
    implementation_decisions: String,
    testing_decisions: String,
    out_of_scope: String,
    further_notes: String,
}

impl SpecContent {
    /// Assemble the nine sections, refusing an empty name. The arity
    /// is the PRD's, fixed by CONTEXT.md (DR-PS-07), not a design
    /// choice to refactor away.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: impl Into<String>,
        short_description: impl Into<String>,
        problem_statement: impl Into<String>,
        solution: impl Into<String>,
        user_stories: impl Into<String>,
        implementation_decisions: impl Into<String>,
        testing_decisions: impl Into<String>,
        out_of_scope: impl Into<String>,
        further_notes: impl Into<String>,
    ) -> Result<Self, SpecError> {
        let content = Self {
            name: name.into(),
            short_description: short_description.into(),
            problem_statement: problem_statement.into(),
            solution: solution.into(),
            user_stories: user_stories.into(),
            implementation_decisions: implementation_decisions.into(),
            testing_decisions: testing_decisions.into(),
            out_of_scope: out_of_scope.into(),
            further_notes: further_notes.into(),
        };
        content.validate()?;
        Ok(content)
    }

    /// Rehydrate stored content exactly as it was recorded. The arity
    /// is the PRD's, fixed by CONTEXT.md (DR-PS-07).
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        name: impl Into<String>,
        short_description: impl Into<String>,
        problem_statement: impl Into<String>,
        solution: impl Into<String>,
        user_stories: impl Into<String>,
        implementation_decisions: impl Into<String>,
        testing_decisions: impl Into<String>,
        out_of_scope: impl Into<String>,
        further_notes: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            short_description: short_description.into(),
            problem_statement: problem_statement.into(),
            solution: solution.into(),
            user_stories: user_stories.into(),
            implementation_decisions: implementation_decisions.into(),
            testing_decisions: testing_decisions.into(),
            out_of_scope: out_of_scope.into(),
            further_notes: further_notes.into(),
        }
    }

    /// The section headings, in PRD order. Content section values map
    /// onto these; the order is the editorial order of CONTEXT.md.
    pub const SECTIONS: &'static [&'static str] = &[
        "name",
        "short_description",
        "problem_statement",
        "solution",
        "user_stories",
        "implementation_decisions",
        "testing_decisions",
        "out_of_scope",
        "further_notes",
    ];

    /// Refuse content that names no Spec.
    fn validate(&self) -> Result<(), SpecError> {
        if self.name.trim().is_empty() {
            return Err(SpecError::Unnamed);
        }
        Ok(())
    }

    /// The Spec's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The one-line description.
    pub fn short_description(&self) -> &str {
        &self.short_description
    }

    /// The problem being solved.
    pub fn problem_statement(&self) -> &str {
        &self.problem_statement
    }

    /// The chosen solution.
    pub fn solution(&self) -> &str {
        &self.solution
    }

    /// The behaviour claims, one `US` bullet per line.
    pub fn user_stories(&self) -> &str {
        &self.user_stories
    }

    /// The settled implementation decisions.
    pub fn implementation_decisions(&self) -> &str {
        &self.implementation_decisions
    }

    /// The settled testing decisions.
    pub fn testing_decisions(&self) -> &str {
        &self.testing_decisions
    }

    /// What this Spec deliberately does not deliver.
    pub fn out_of_scope(&self) -> &str {
        &self.out_of_scope
    }

    /// Anything else worth keeping beside the PRD.
    pub fn further_notes(&self) -> &str {
        &self.further_notes
    }
}

/// The closed lifecycle vocabulary for one content version (DR-PS-08).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecContentState {
    /// Editable working content.
    Draft,
    /// Immutable, operative content (DR-PS-09).
    Approved,
    /// Explicitly retired; immutable forever, and still queryable for
    /// the Tickets pinned to it (DR-PS-11).
    Superseded,
}

impl SpecContentState {
    /// Whether this state accepts no further movement. Superseded is
    /// the one terminal content state.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Superseded)
    }

    /// The stored and wire name of this state.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Approved => "approved",
            Self::Superseded => "superseded",
        }
    }

    /// The state a stored row names, or `None` outside the
    /// vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        match stored {
            "draft" => Some(Self::Draft),
            "approved" => Some(Self::Approved),
            "superseded" => Some(Self::Superseded),
            _ => None,
        }
    }
}

/// One content version of one Spec: the PRD exactly as it stood when
/// the version was minted or last drafted. Approved and superseded
/// versions never change again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecVersion {
    number: u64,
    state: SpecContentState,
    content: SpecContent,
}

impl SpecVersion {
    /// Rehydrate a stored version exactly as it was recorded.
    pub fn new(number: u64, state: SpecContentState, content: SpecContent) -> Self {
        Self {
            number,
            state,
            content,
        }
    }

    /// The version's number; the first content is one and every
    /// material change mints the next.
    pub fn number(&self) -> u64 {
        self.number
    }

    /// The version's lifecycle state.
    pub fn state(&self) -> SpecContentState {
        self.state
    }

    /// The PRD this version carries.
    pub fn content(&self) -> &SpecContent {
        &self.content
    }
}

/// The closed Spec execution vocabulary (DR-PS-12): a Spec's progress
/// through its Plan, tracked separately from every content version so
/// progress never rewrites documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecExecutionState {
    /// Authored, belonging to no Plan yet.
    Unplanned,
    /// Belonging to one Plan, not yet free to execute.
    Planned,
    /// Waiting on something outside the Spec.
    Blocked,
    /// Free to execute when capacity allows.
    Ready,
    /// Executing through its Ticket graph.
    Active,
    /// The final Spec integration review before landing.
    IntegrationReview,
    /// Terminal: landed in full.
    Complete,
    /// Terminal: will not execute.
    Cancelled,
}

impl SpecExecutionState {
    /// Every execution state, in vocabulary order.
    pub const ALL: &'static [Self] = &[
        Self::Unplanned,
        Self::Planned,
        Self::Blocked,
        Self::Ready,
        Self::Active,
        Self::IntegrationReview,
        Self::Complete,
        Self::Cancelled,
    ];

    /// Whether this state accepts no further execution movement.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Cancelled)
    }

    /// Whether one Spec execution may move directly from `from` to
    /// `to`. The closed set: the forward path through the Plan with
    /// `blocked` recoverable in both directions, and `cancelled`
    /// reachable from every open state. Moving into `planned` is not
    /// a free move at all — joining a Plan is what plans a Spec.
    pub fn can_move(from: Self, to: Self) -> bool {
        if from.is_terminal() || to == Self::Planned {
            return false;
        }
        match from {
            Self::Unplanned => matches!(to, Self::Cancelled),
            Self::Planned => matches!(to, Self::Blocked | Self::Ready | Self::Cancelled),
            Self::Blocked => matches!(to, Self::Ready | Self::Cancelled),
            Self::Ready => matches!(to, Self::Blocked | Self::Active | Self::Cancelled),
            Self::Active => matches!(to, Self::IntegrationReview | Self::Cancelled),
            Self::IntegrationReview => matches!(to, Self::Complete | Self::Cancelled),
            Self::Complete | Self::Cancelled => false,
        }
    }

    /// The stored and wire name of this state.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Unplanned => "unplanned",
            Self::Planned => "planned",
            Self::Blocked => "blocked",
            Self::Ready => "ready",
            Self::Active => "active",
            Self::IntegrationReview => "integration_review",
            Self::Complete => "complete",
            Self::Cancelled => "cancelled",
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

/// What one content update did: edited the working draft in place, or
/// minted a new version because the current content was no draft to
/// edit (DR-PS-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentChange {
    /// The working draft's content was replaced.
    Edited {
        /// The draft version edited.
        number: u64,
    },
    /// A new draft version was minted beside the frozen content.
    Minted {
        /// The freshly minted version.
        number: u64,
    },
}

impl ContentChange {
    /// The version the change touched.
    pub fn number(self) -> u64 {
        match self {
            Self::Edited { number } | Self::Minted { number } => number,
        }
    }
}

/// One Spec aggregate: the Project it belongs to, the number that
/// Project minted for it, every content version, and the execution
/// tracked separately. The version counts applied changes: creation
/// lands at 1 and every legal edit or transition bumps it, so a
/// stored version is all a caller needs for optimistic checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    id: SpecId,
    project: ProjectId,
    number: SpecNumber,
    versions: Vec<SpecVersion>,
    execution: SpecExecutionState,
    plan: Option<PlanId>,
    version: u64,
}

impl Spec {
    /// A fresh Spec: version one of its content in draft, execution
    /// unplanned, belonging to no Plan.
    pub fn new(
        id: SpecId,
        project: ProjectId,
        number: SpecNumber,
        content: SpecContent,
    ) -> Result<Self, SpecError> {
        content.validate()?;
        Ok(Self {
            id,
            project,
            number,
            versions: vec![SpecVersion::new(1, SpecContentState::Draft, content)],
            execution: SpecExecutionState::Unplanned,
            plan: None,
            version: 1,
        })
    }

    /// Rehydrate a stored Spec exactly as it was recorded.
    pub fn restore(
        id: SpecId,
        project: ProjectId,
        number: SpecNumber,
        versions: Vec<SpecVersion>,
        execution: SpecExecutionState,
        plan: Option<PlanId>,
        version: u64,
    ) -> Self {
        Self {
            id,
            project,
            number,
            versions,
            execution,
            plan,
            version,
        }
    }

    /// The immutable identity.
    pub fn id(&self) -> SpecId {
        self.id
    }

    /// The Project this Spec belongs to.
    pub fn project(&self) -> ProjectId {
        self.project
    }

    /// The number this Project minted for this Spec, rendered with
    /// the Project's code as `CORE-S1`.
    pub fn number(&self) -> SpecNumber {
        self.number
    }

    /// Every content version, oldest first; each stays queryable
    /// beside its replacements.
    pub fn versions(&self) -> &[SpecVersion] {
        &self.versions
    }

    /// The working content: the newest version, whatever its state.
    /// A draft here is editable; anything else needs a new version.
    pub fn current_version(&self) -> Option<&SpecVersion> {
        self.versions.last()
    }

    /// One version by number — the lookup a Ticket's pin resolves
    /// through, superseded versions included (DR-PS-11).
    pub fn pinned_version(&self, number: u64) -> Option<&SpecVersion> {
        self.versions.iter().find(|held| held.number() == number)
    }

    /// The version an approval would land on next: the newest one,
    /// when it is still a draft.
    pub fn draft_version(&self) -> Option<&SpecVersion> {
        match self.versions.last() {
            Some(version) if version.state() == SpecContentState::Draft => Some(version),
            _ => None,
        }
    }

    /// The still-approved version, when one is operative.
    pub fn approved_version(&self) -> Option<&SpecVersion> {
        self.versions
            .iter()
            .find(|version| version.state() == SpecContentState::Approved)
    }

    /// The Spec's name, from the current content.
    pub fn name(&self) -> &str {
        self.versions
            .last()
            .map(|version| version.content().name())
            .unwrap_or_default()
    }

    /// The number the next minted version takes. Versions are minted
    /// monotonically and never reused, so this is one past the
    /// highest number ever minted — never one past the count, which a
    /// gap in the recorded numbers would send backwards into a
    /// collision.
    pub fn next_version_number(&self) -> u64 {
        self.versions
            .iter()
            .map(|version| version.number())
            .max()
            .map_or(1, |highest| highest + 1)
    }

    /// The execution state, tracked separately from content.
    pub fn execution(&self) -> SpecExecutionState {
        self.execution
    }

    /// The Plan this Spec belongs to, once planned (DR-PS-06).
    pub fn plan(&self) -> Option<PlanId> {
        self.plan
    }

    /// The number of applied changes, for optimistic version checks.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Replace the working content. While the newest version is a
    /// draft this edits it in place; once content has moved on —
    /// approved or superseded — the material change mints a new draft
    /// version beside the frozen ones (DR-PS-10).
    pub fn update_content(&mut self, content: SpecContent) -> Result<ContentChange, SpecError> {
        content.validate()?;
        let change = match self.versions.last() {
            Some(latest) if latest.state() == SpecContentState::Draft => {
                let number = latest.number();
                let held = self
                    .versions
                    .last_mut()
                    .expect("the newest version is present");
                held.content = content;
                ContentChange::Edited { number }
            }
            _ => {
                let number = self.next_version_number();
                self.versions
                    .push(SpecVersion::new(number, SpecContentState::Draft, content));
                ContentChange::Minted { number }
            }
        };
        self.applied();
        Ok(change)
    }

    /// Approve the working draft into immutable, operative content
    /// (DR-PS-09). Refused while another version is still approved:
    /// retiring operative content is the explicit supersession the
    /// operator performs first.
    pub fn approve_version(&mut self) -> Result<u64, SpecError> {
        let number = match self.draft_version() {
            Some(draft) => draft.number(),
            None => return Err(SpecError::ApproveRequiresDraft),
        };
        if let Some(approved) = self.approved_version() {
            return Err(SpecError::ApproveBlockedByApproved {
                approved: approved.number(),
            });
        }
        let held = self
            .versions
            .iter_mut()
            .find(|version| version.number() == number)
            .expect("the draft version is present");
        held.state = SpecContentState::Approved;
        self.applied();
        Ok(number)
    }

    /// Supersede one version explicitly (DR-PS-11). The superseded
    /// version keeps its content unchanged and stays queryable for
    /// the Tickets pinned to it.
    pub fn supersede_version(&mut self, number: u64) -> Result<(), SpecError> {
        let held = self
            .versions
            .iter_mut()
            .find(|version| version.number() == number)
            .ok_or(SpecError::SupersedeUnknown { version: number })?;
        if held.state.is_terminal() {
            return Err(SpecError::SupersedeRefused { version: number });
        }
        held.state = SpecContentState::Superseded;
        self.applied();
        Ok(())
    }

    /// Join a Plan: the one move into `planned`, and the moment the
    /// Spec starts belonging to exactly one Plan (DR-PS-06).
    pub fn assign_to_plan(&mut self, plan: PlanId) -> Result<(), SpecError> {
        if let Some(existing) = self.plan {
            return Err(SpecError::AlreadyPlanned { plan: existing });
        }
        if self.execution != SpecExecutionState::Unplanned {
            return Err(SpecError::RequiresUnplanned);
        }
        self.plan = Some(plan);
        self.execution = SpecExecutionState::Planned;
        self.applied();
        Ok(())
    }

    /// Leave `plan`: the binding clears and a Spec still on the
    /// execution track restarts at unplanned, free to join another
    /// Plan later (DR-PS-06). Complete and cancelled execution stays
    /// as it ended — those states are terminal, so unbinding cannot
    /// restart the track, and the preserved state keeps the next join
    /// refused. Refused when the Spec does not belong to `plan` — a
    /// binding to another Plan is not this call's to clear.
    /// This is unbinding, not an execution move: the vocabulary's
    /// terminal states govern moves alone, and the record of the
    /// attempt stays on the timeline.
    pub fn leave_plan(&mut self, plan: PlanId) -> Result<(), SpecError> {
        if self.plan != Some(plan) {
            return Err(SpecError::NotBoundTo { plan });
        }
        self.plan = None;
        if !self.execution.is_terminal() {
            self.execution = SpecExecutionState::Unplanned;
        }
        self.applied();
        Ok(())
    }

    /// Move the execution state along the closed transition set,
    /// independently of every content version. Moving into `planned`
    /// belongs to joining a Plan alone.
    pub fn transition_execution(&mut self, to: SpecExecutionState) -> Result<(), SpecError> {
        if !SpecExecutionState::can_move(self.execution, to) {
            return Err(SpecError::IllegalExecutionMove {
                from: self.execution,
                to,
            });
        }
        self.execution = to;
        self.applied();
        Ok(())
    }

    /// Count one applied change.
    fn applied(&mut self) {
        self.version += 1;
    }
}

#[cfg(test)]
mod spec_content {
    use super::{SpecContent, SpecError};

    #[test]
    fn content_without_a_name_is_refused() {
        assert_eq!(
            SpecContent::new(
                "   ",
                "short",
                "problem",
                "solution",
                "stories",
                "implementation",
                "testing",
                "out of scope",
                "notes",
            ),
            Err(SpecError::Unnamed)
        );
        assert_eq!(
            SpecContent::new("", "", "", "", "", "", "", "", ""),
            Err(SpecError::Unnamed)
        );
    }

    #[test]
    fn content_carries_the_nine_prd_sections() {
        let content = SpecContent::new(
            "Plans and specifications",
            "Versioned Plan graphs of Specs",
            "Planning must survive change without losing truth.",
            "A Plan is a versioned, ordered dependency graph of Specs.",
            "KAN-S3-US1: As an operator, I want to compose a Plan.",
            "Display order is a per-Plan sequence.",
            "Domain tests prove lifecycle legality.",
            "Ticket graph proposal and approval mechanics.",
            "This Spec is the model for the product's own planning.",
        )
        .expect("named content validates");

        assert_eq!(content.name(), "Plans and specifications");
        assert_eq!(
            content.short_description(),
            "Versioned Plan graphs of Specs"
        );
        assert_eq!(
            content.problem_statement(),
            "Planning must survive change without losing truth."
        );
        assert_eq!(
            content.solution(),
            "A Plan is a versioned, ordered dependency graph of Specs."
        );
        assert_eq!(
            content.user_stories(),
            "KAN-S3-US1: As an operator, I want to compose a Plan."
        );
        assert_eq!(
            content.implementation_decisions(),
            "Display order is a per-Plan sequence."
        );
        assert_eq!(
            content.testing_decisions(),
            "Domain tests prove lifecycle legality."
        );
        assert_eq!(
            content.out_of_scope(),
            "Ticket graph proposal and approval mechanics."
        );
        assert_eq!(
            content.further_notes(),
            "This Spec is the model for the product's own planning."
        );
        assert_eq!(
            SpecContent::SECTIONS.len(),
            9,
            "the PRD carries exactly the nine CONTEXT.md sections"
        );
    }
}

#[cfg(test)]
mod spec_versions {
    use super::{
        ContentChange, Spec, SpecContent, SpecContentState, SpecError, SpecId, SpecVersion,
    };
    use crate::plan::{PlanId, SpecNumber};
    use crate::project::ProjectId;

    fn content(name: &str) -> SpecContent {
        SpecContent::new(
            name,
            "short",
            "problem",
            "solution",
            "stories",
            "implementation",
            "testing",
            "out of scope",
            "notes",
        )
        .expect("the fixture content validates")
    }

    fn renamed(name: &str) -> SpecContent {
        SpecContent::new(
            name,
            "a changed short description",
            "problem",
            "solution",
            "stories",
            "implementation",
            "testing",
            "out of scope",
            "notes",
        )
        .expect("the fixture content validates")
    }

    fn spec() -> Spec {
        Spec::new(
            SpecId::new(3),
            ProjectId::new(7),
            SpecNumber::new(1).expect("the fixture number is positive"),
            content("Registration"),
        )
        .expect("the fixture content validates")
    }

    #[test]
    fn a_fresh_spec_holds_version_one_in_draft() {
        let spec = spec();

        assert_eq!(spec.id(), SpecId::new(3));
        assert_eq!(spec.project(), ProjectId::new(7));
        assert_eq!(
            spec.number(),
            SpecNumber::new(1).expect("the fixture number is positive")
        );
        assert_eq!(spec.versions().len(), 1);
        assert_eq!(spec.current_version().unwrap().number(), 1);
        assert_eq!(
            spec.current_version().unwrap().state(),
            SpecContentState::Draft
        );
        assert_eq!(spec.name(), "Registration");
        assert_eq!(spec.next_version_number(), 2);
        assert_eq!(spec.version(), 1);
    }

    #[test]
    fn unnamed_content_is_refused_at_creation() {
        // restore() rehydrates stored bytes without validating, which
        // is the only way to hold unnamed content long enough to ask
        // the aggregate to refuse it.
        let unnamed = SpecContent::restore(
            "  ",
            "short",
            "problem",
            "solution",
            "stories",
            "implementation",
            "testing",
            "out of scope",
            "notes",
        );

        let error = Spec::new(
            SpecId::new(1),
            ProjectId::new(7),
            SpecNumber::new(1).expect("the fixture number is positive"),
            unnamed,
        )
        .unwrap_err();

        assert_eq!(error, SpecError::Unnamed);
    }

    #[test]
    fn updating_a_draft_edits_it_in_place() {
        let mut spec = spec();
        let version = spec.version();

        let change = spec
            .update_content(renamed("Registration"))
            .expect("the draft edits");

        assert_eq!(change, ContentChange::Edited { number: 1 });
        assert_eq!(spec.versions().len(), 1, "no second version minted");
        assert_eq!(
            spec.current_version()
                .unwrap()
                .content()
                .short_description(),
            "a changed short description"
        );
        assert_eq!(spec.version(), version + 1);
    }

    #[test]
    fn approving_freezes_the_draft() {
        let mut spec = spec();

        let approved = spec.approve_version().expect("the draft approves");

        assert_eq!(approved, 1);
        assert_eq!(
            spec.current_version().unwrap().state(),
            SpecContentState::Approved
        );
        assert_eq!(
            spec.draft_version(),
            None,
            "approved content is no draft to edit"
        );
    }

    #[test]
    fn approving_without_a_draft_is_refused() {
        let mut spec = spec();
        spec.approve_version().expect("the draft approves");
        let version = spec.version();

        assert_eq!(
            spec.approve_version().unwrap_err(),
            SpecError::ApproveRequiresDraft
        );
        assert_eq!(spec.version(), version, "the refusal changed nothing");
    }

    #[test]
    fn a_material_change_after_approval_mints_a_new_version() {
        let mut spec = spec();
        spec.approve_version().expect("the draft approves");
        let frozen = spec.current_version().unwrap().clone();
        let version = spec.version();

        let change = spec
            .update_content(renamed("Registration"))
            .expect("the material change mints");

        assert_eq!(change, ContentChange::Minted { number: 2 });
        assert_eq!(spec.version(), version + 1);
        assert_eq!(spec.versions().len(), 2);
        assert_eq!(
            spec.versions()[0],
            frozen,
            "the approved version is untouched"
        );
        assert_eq!(spec.versions()[0].state(), SpecContentState::Approved);
        assert_eq!(spec.versions()[1].state(), SpecContentState::Draft);
        assert_eq!(spec.next_version_number(), 3);
    }

    #[test]
    fn approving_a_second_version_requires_explicit_supersession_first() {
        let mut spec = spec();
        spec.approve_version().expect("the first version approves");
        spec.update_content(renamed("Registration"))
            .expect("the material change mints a draft");

        let error = spec.approve_version().unwrap_err();

        assert_eq!(
            error,
            SpecError::ApproveBlockedByApproved { approved: 1 },
            "supersession is explicit, never implicit"
        );

        spec.supersede_version(1)
            .expect("the approved version supersedes explicitly");
        let approved = spec.approve_version().expect("the draft approves now");

        assert_eq!(approved, 2);
        assert_eq!(spec.versions()[0].state(), SpecContentState::Superseded);
        assert_eq!(spec.versions()[1].state(), SpecContentState::Approved);
    }

    #[test]
    fn superseding_keeps_the_pinned_version_queryable_and_unchanged() {
        let mut spec = spec();
        spec.approve_version().expect("the draft approves");
        let pinned_content = spec.pinned_version(1).unwrap().content().clone();

        spec.supersede_version(1)
            .expect("the approved version supersedes");

        let pinned = spec
            .pinned_version(1)
            .expect("the superseded version stays queryable");
        assert_eq!(pinned.state(), SpecContentState::Superseded);
        assert_eq!(
            pinned.content(),
            &pinned_content,
            "a Ticket's pinned content never moves"
        );
        assert!(pinned.state().is_terminal());
    }

    #[test]
    fn superseding_an_unknown_or_terminal_version_is_refused() {
        let mut spec = spec();

        assert_eq!(
            spec.supersede_version(9).unwrap_err(),
            SpecError::SupersedeUnknown { version: 9 }
        );

        spec.approve_version().expect("the draft approves");
        spec.supersede_version(1).expect("the version supersedes");
        let version = spec.version();

        assert_eq!(
            spec.supersede_version(1).unwrap_err(),
            SpecError::SupersedeRefused { version: 1 }
        );
        assert_eq!(spec.version(), version, "the refusal changed nothing");
    }

    #[test]
    fn a_gap_in_version_numbers_mints_one_past_the_highest() {
        // Versions one and three with no two: a count would mint
        // three again and collide with the recorded version.
        let mut spec = Spec::restore(
            SpecId::new(1),
            ProjectId::new(7),
            SpecNumber::new(1).expect("the fixture number is positive"),
            vec![
                SpecVersion::new(1, SpecContentState::Superseded, content("Registration")),
                SpecVersion::new(3, SpecContentState::Approved, content("Registration")),
            ],
            super::SpecExecutionState::Unplanned,
            None,
            9,
        );

        assert_eq!(spec.next_version_number(), 4);
        assert_eq!(
            spec.update_content(renamed("Registration"))
                .expect("the material change mints"),
            ContentChange::Minted { number: 4 },
            "the minted number never collides with a recorded one"
        );
    }

    #[test]
    fn restore_rehydrates_every_recorded_fact() {
        let spec = Spec::restore(
            SpecId::new(5),
            ProjectId::new(2),
            SpecNumber::new(4).expect("the fixture number is positive"),
            vec![
                SpecVersion::new(1, SpecContentState::Superseded, content("Registration")),
                SpecVersion::new(2, SpecContentState::Approved, content("Registration")),
                SpecVersion::new(3, SpecContentState::Draft, content("Registration again")),
            ],
            super::SpecExecutionState::Active,
            Some(PlanId::new(6)),
            12,
        );

        assert_eq!(spec.id().value(), 5);
        assert_eq!(spec.project().value(), 2);
        assert_eq!(spec.number().value(), 4);
        assert_eq!(spec.versions().len(), 3);
        assert_eq!(
            spec.approved_version().unwrap().number(),
            2,
            "the approved version sits in the middle of the record"
        );
        assert_eq!(spec.draft_version().unwrap().number(), 3);
        assert_eq!(spec.name(), "Registration again");
        assert_eq!(spec.execution(), super::SpecExecutionState::Active);
        assert_eq!(spec.plan(), Some(PlanId::new(6)));
        assert_eq!(spec.version(), 12);
        assert_eq!(spec.next_version_number(), 4);
    }
}

#[cfg(test)]
mod spec_execution {
    use super::{Spec, SpecContent, SpecContentState, SpecError, SpecExecutionState, SpecId};
    use crate::plan::{PlanId, SpecNumber};
    use crate::project::ProjectId;

    fn content(name: &str) -> SpecContent {
        SpecContent::new(
            name,
            "short",
            "problem",
            "solution",
            "stories",
            "implementation",
            "testing",
            "out of scope",
            "notes",
        )
        .expect("the fixture content validates")
    }

    fn spec() -> Spec {
        Spec::new(
            SpecId::new(1),
            ProjectId::new(7),
            SpecNumber::new(2).expect("the fixture number is positive"),
            content("Registration"),
        )
        .expect("the fixture content validates")
    }

    /// A spec planned onto Plan 1.
    fn planned() -> Spec {
        let mut spec = spec();
        spec.assign_to_plan(PlanId::new(1))
            .expect("the fixture joins its Plan");
        spec
    }

    #[test]
    fn a_fresh_spec_is_unplanned_and_belongs_to_no_plan() {
        let spec = spec();

        assert_eq!(spec.execution(), SpecExecutionState::Unplanned);
        assert_eq!(spec.plan(), None);
        assert!(!spec.execution().is_terminal());
    }

    #[test]
    fn joining_a_plan_plans_the_spec() {
        let mut spec = spec();
        let version = spec.version();

        spec.assign_to_plan(PlanId::new(4))
            .expect("an unplanned Spec joins a Plan");

        assert_eq!(spec.execution(), SpecExecutionState::Planned);
        assert_eq!(spec.plan(), Some(PlanId::new(4)));
        assert_eq!(spec.version(), version + 1);
    }

    #[test]
    fn joining_a_second_plan_is_refused() {
        let mut spec = planned();
        let version = spec.version();

        assert_eq!(
            spec.assign_to_plan(PlanId::new(2)).unwrap_err(),
            SpecError::AlreadyPlanned {
                plan: PlanId::new(1)
            },
            "a Spec belongs to one Plan at a time"
        );
        assert_eq!(spec.plan(), Some(PlanId::new(1)));
        assert_eq!(spec.version(), version, "the refusal changed nothing");
    }

    #[test]
    fn leaving_a_plan_frees_the_spec_to_join_another() {
        let mut spec = planned();
        spec.transition_execution(SpecExecutionState::Blocked)
            .expect("the fixture work blocks");
        let version = spec.version();

        spec.leave_plan(PlanId::new(1))
            .expect("a bound Spec leaves its Plan");

        assert_eq!(spec.plan(), None);
        assert_eq!(
            spec.execution(),
            SpecExecutionState::Unplanned,
            "leaving restarts the execution track from wherever it stood"
        );
        assert_eq!(spec.version(), version + 1);

        spec.assign_to_plan(PlanId::new(2))
            .expect("the freed Spec joins another Plan");
        assert_eq!(spec.plan(), Some(PlanId::new(2)));
        assert_eq!(spec.execution(), SpecExecutionState::Planned);
    }

    #[test]
    fn leaving_a_plan_the_spec_does_not_hold_is_refused() {
        let mut spec = planned();
        let version = spec.version();

        assert_eq!(
            spec.leave_plan(PlanId::new(9)).unwrap_err(),
            SpecError::NotBoundTo {
                plan: PlanId::new(9)
            },
            "a binding to another Plan is not this call's to clear"
        );
        assert_eq!(spec.plan(), Some(PlanId::new(1)));
        assert_eq!(spec.version(), version, "the refusal changed nothing");
    }

    #[test]
    fn execution_walks_the_full_path_to_complete() {
        let mut spec = planned();

        spec.transition_execution(SpecExecutionState::Ready)
            .expect("planned work becomes ready");
        spec.transition_execution(SpecExecutionState::Active)
            .expect("ready work activates");
        spec.transition_execution(SpecExecutionState::IntegrationReview)
            .expect("active work reaches integration review");
        spec.transition_execution(SpecExecutionState::Complete)
            .expect("reviewed work completes");

        assert_eq!(spec.execution(), SpecExecutionState::Complete);
        assert!(spec.execution().is_terminal());
        assert_eq!(
            spec.transition_execution(SpecExecutionState::Active)
                .unwrap_err(),
            SpecError::IllegalExecutionMove {
                from: SpecExecutionState::Complete,
                to: SpecExecutionState::Active,
            },
            "terminal execution accepts no further movement"
        );
    }

    #[test]
    fn blocked_is_recoverable() {
        let mut spec = planned();

        spec.transition_execution(SpecExecutionState::Blocked)
            .expect("planned work blocks");
        spec.transition_execution(SpecExecutionState::Ready)
            .expect("a cleared blocker frees the work");

        let mut from_ready = spec.clone();
        from_ready
            .transition_execution(SpecExecutionState::Blocked)
            .expect("ready work blocks too");
        from_ready
            .transition_execution(SpecExecutionState::Cancelled)
            .expect("blocked work may be cancelled");
    }

    #[test]
    fn every_open_state_may_cancel() {
        for walk in [
            vec![],
            vec![SpecExecutionState::Planned],
            vec![SpecExecutionState::Planned, SpecExecutionState::Blocked],
            vec![SpecExecutionState::Planned, SpecExecutionState::Ready],
            vec![
                SpecExecutionState::Planned,
                SpecExecutionState::Ready,
                SpecExecutionState::Active,
            ],
            vec![
                SpecExecutionState::Planned,
                SpecExecutionState::Ready,
                SpecExecutionState::Active,
                SpecExecutionState::IntegrationReview,
            ],
        ] {
            let mut spec = spec();
            for step in walk {
                if step == SpecExecutionState::Planned {
                    spec.assign_to_plan(PlanId::new(1))
                        .expect("the fixture joins its Plan");
                } else {
                    spec.transition_execution(step)
                        .expect("the fixture walks the legal path");
                }
            }

            spec.transition_execution(SpecExecutionState::Cancelled)
                .expect("every open state may cancel");

            assert_eq!(spec.execution(), SpecExecutionState::Cancelled);
            assert!(spec.execution().is_terminal());
        }
    }

    #[test]
    fn the_transition_set_is_closed() {
        // No free move enters planned: joining a Plan plans a Spec.
        assert!(!SpecExecutionState::can_move(
            SpecExecutionState::Unplanned,
            SpecExecutionState::Planned
        ));
        assert!(!SpecExecutionState::can_move(
            SpecExecutionState::Blocked,
            SpecExecutionState::Planned
        ));
        // Unplanned work may only leave by cancellation.
        assert!(!SpecExecutionState::can_move(
            SpecExecutionState::Unplanned,
            SpecExecutionState::Ready
        ));
        // The forward path never skips a stage.
        assert!(!SpecExecutionState::can_move(
            SpecExecutionState::Planned,
            SpecExecutionState::Active
        ));
        assert!(!SpecExecutionState::can_move(
            SpecExecutionState::Ready,
            SpecExecutionState::IntegrationReview
        ));
        assert!(!SpecExecutionState::can_move(
            SpecExecutionState::Active,
            SpecExecutionState::Complete
        ));
        // Terminal states never move.
        assert!(!SpecExecutionState::can_move(
            SpecExecutionState::Complete,
            SpecExecutionState::Cancelled
        ));
        assert!(!SpecExecutionState::can_move(
            SpecExecutionState::Cancelled,
            SpecExecutionState::Cancelled
        ));
        // Every state's wire name round trips.
        for state in SpecExecutionState::ALL {
            assert_eq!(SpecExecutionState::parse(state.wire_name()), Some(*state));
        }
        assert_eq!(SpecExecutionState::parse("ghost"), None);
        for state in [SpecContentState::Draft, SpecContentState::Approved] {
            assert!(!state.is_terminal());
        }
        assert!(SpecContentState::Superseded.is_terminal());
    }

    #[test]
    fn execution_moves_never_touch_content_versions() {
        let mut spec = planned();
        spec.approve_version().expect("the draft approves");
        let versions = spec.versions().to_vec();

        spec.transition_execution(SpecExecutionState::Blocked)
            .expect("planned work blocks");
        spec.transition_execution(SpecExecutionState::Ready)
            .expect("the blocker clears");

        assert_eq!(
            spec.versions(),
            versions.as_slice(),
            "progress never rewrites documents"
        );
        assert_eq!(
            spec.current_version().unwrap().state(),
            SpecContentState::Approved
        );
    }

    #[test]
    fn content_moves_never_touch_execution() {
        let mut spec = planned();
        spec.approve_version().expect("the draft approves");
        spec.supersede_version(1)
            .expect("the approved version supersedes");

        assert_eq!(
            spec.execution(),
            SpecExecutionState::Planned,
            "documents moving never moves progress"
        );
        assert_eq!(spec.plan(), Some(PlanId::new(1)));

        spec.update_content(content("Registration"))
            .expect("content keeps moving after execution settles");
        assert_eq!(spec.execution(), SpecExecutionState::Planned);
    }
}
