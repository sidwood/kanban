//! The Project entity: the unit of work ownership (CONTEXT.md). A
//! Project anchors all of one repository's work: exactly one target
//! Git repository, one Seed Workspace, one default branch, and one
//! required target Herdr workspace, with an optional Herdr session
//! whose absence selects Herdr's default session, optionally under one
//! Initiative. It mints its own Plan, Spec, and Ticket numbers through
//! independent monotonic counters, carries an immutable globally
//! unique code, and is archived rather than deleted.

use std::fmt;
use std::path::Path;

use crate::herdr::HerdrSession;
use crate::herdr::herdr_session_name;
use crate::initiative::InitiativeId;

/// The identity of one Project. Assigned once by storage and
/// immutable afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectId(u64);

impl ProjectId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a Project code was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeError {
    /// The text does not match `[A-Z][A-Z0-9]{1,7}` in full.
    Malformed,
    /// `KAN` names this product; it is never a Project code.
    Reserved,
}

impl fmt::Display for CodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => {
                write!(f, "a Project code must match [A-Z][A-Z0-9]{{1,7}} in full")
            }
            Self::Reserved => write!(f, "KAN is reserved for this product"),
        }
    }
}

impl std::error::Error for CodeError {}

/// An immutable, globally unique Project code matching
/// `[A-Z][A-Z0-9]{1,7}` in full. Minted once at registration and
/// never changed; `KAN` is reserved for this product.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectCode(String);

impl ProjectCode {
    /// Accept only text matching the code pattern in full, except
    /// the reserved product code.
    pub fn new(raw: &str) -> Result<Self, CodeError> {
        let mut chars = raw.chars();
        let well_formed = match chars.next() {
            Some(first) if first.is_ascii_uppercase() => {
                let rest = chars.as_str();
                (1..=7).contains(&rest.len())
                    && rest
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            }
            _ => false,
        };
        if !well_formed {
            return Err(CodeError::Malformed);
        }
        if raw == "KAN" {
            return Err(CodeError::Reserved);
        }
        Ok(Self(raw.to_owned()))
    }

    /// The code text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The kind of number a Project mints: one independent counter per
/// kind (CONTEXT.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberKind {
    Plan,
    Spec,
    Ticket,
}

impl NumberKind {
    /// The letter a minted number renders with.
    pub fn prefix(self) -> char {
        match self {
            Self::Plan => 'P',
            Self::Spec => 'S',
            Self::Ticket => 'T',
        }
    }

    /// The rendered identifier of one minted number, for example
    /// `CORE-P1` (DR-PH-06).
    pub fn render(self, code: &ProjectCode, number: u64) -> String {
        format!("{}-{}{}", code, self.prefix(), number)
    }
}

/// The Plan, Spec, and Ticket counters of one Project: independent,
/// monotonic per kind, gap tolerant, and never reusing a number. A
/// counter holds the last minted number, so zero means nothing has
/// been minted yet and the first number is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectCounters {
    plan: u64,
    spec: u64,
    ticket: u64,
}

impl ProjectCounters {
    /// Counters for a fresh Project: nothing minted yet.
    pub fn zeroed() -> Self {
        Self {
            plan: 0,
            spec: 0,
            ticket: 0,
        }
    }

    /// Rehydrate stored counters exactly as they were recorded. Gaps
    /// are valid and are preserved: a stored 7 with nothing at 3
    /// stays a gap, and the next number is 8.
    pub fn restore(plan: u64, spec: u64, ticket: u64) -> Self {
        Self { plan, spec, ticket }
    }

    /// The last number minted for `kind`; zero when nothing has been
    /// minted.
    pub fn last(self, kind: NumberKind) -> u64 {
        match kind {
            NumberKind::Plan => self.plan,
            NumberKind::Spec => self.spec,
            NumberKind::Ticket => self.ticket,
        }
    }

    /// Mint the next number for `kind`, moving that counter only.
    /// Numbers are never reused: the counter only ever moves forward.
    pub fn next(&mut self, kind: NumberKind) -> u64 {
        let slot = match kind {
            NumberKind::Plan => &mut self.plan,
            NumberKind::Spec => &mut self.spec,
            NumberKind::Ticket => &mut self.ticket,
        };
        *slot += 1;
        *slot
    }
}

/// Why a registration was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationError {
    /// The code is malformed or reserved.
    Code(CodeError),
    /// A text field holds nothing but whitespace. The value names
    /// the field.
    Blank(&'static str),
    /// The Herdr session name is not one safe path segment.
    InvalidHerdrSession,
    /// The target repository path is relative rather than absolute.
    RelativeRepository,
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(cause) => write!(f, "{cause}"),
            Self::Blank(field) => write!(f, "a Project {field} cannot be blank"),
            Self::InvalidHerdrSession => {
                write!(f, "a Herdr session name must be one safe path segment")
            }
            Self::RelativeRepository => {
                write!(f, "a Project target repository must be an absolute path")
            }
        }
    }
}

impl std::error::Error for RegistrationError {}

/// One validated registration: the anchors a Project owns exactly
/// one of, plus its optional Initiative and optional Herdr session.
/// Whitespace is not part of any field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRegistration {
    code: ProjectCode,
    name: String,
    repository: String,
    seed_workspace: String,
    default_branch: String,
    herdr_workspace: String,
    herdr_session: Option<String>,
    initiative: Option<InitiativeId>,
}

impl ProjectRegistration {
    /// Validate a registration: the code pattern and reservation, and
    /// that every anchor carries text. A blank Herdr session name is
    /// absence, and absence selects Herdr's default session.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        code: &str,
        name: &str,
        repository: &str,
        seed_workspace: &str,
        default_branch: &str,
        herdr_workspace: &str,
        herdr_session: Option<&str>,
        initiative: Option<InitiativeId>,
    ) -> Result<Self, RegistrationError> {
        Ok(Self {
            code: ProjectCode::new(code).map_err(RegistrationError::Code)?,
            name: anchored("name", name)?,
            repository: absolute_repository_anchor(repository)?,
            seed_workspace: anchored("Seed Workspace", seed_workspace)?,
            default_branch: anchored("default branch", default_branch)?,
            herdr_workspace: anchored("target Herdr workspace", herdr_workspace)?,
            herdr_session: optional_herdr_session(herdr_session)?,
            initiative,
        })
    }

    /// The immutable, globally unique code.
    pub fn code(&self) -> &ProjectCode {
        &self.code
    }

    /// The display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The one target Git repository.
    pub fn repository(&self) -> &str {
        &self.repository
    }

    /// The one Seed Workspace.
    pub fn seed_workspace(&self) -> &str {
        &self.seed_workspace
    }

    /// The one default branch.
    pub fn default_branch(&self) -> &str {
        &self.default_branch
    }

    /// The one required target Herdr workspace.
    pub fn herdr_workspace(&self) -> &str {
        &self.herdr_workspace
    }

    /// The named Herdr session, if the Project selected one; absence
    /// selects Herdr's default session (DR-PH-07).
    pub fn herdr_session(&self) -> Option<&str> {
        self.herdr_session.as_deref()
    }

    /// The session this Project's Herdr observation resolves inside:
    /// the named session, or Herdr's default when none is named
    /// (DR-HB-19).
    pub fn effective_herdr_session(&self) -> HerdrSession {
        self.herdr_session
            .clone()
            .map_or(HerdrSession::Default, HerdrSession::Named)
    }

    /// The Initiative the Project sits under, if any.
    pub fn initiative(&self) -> Option<InitiativeId> {
        self.initiative
    }
}

/// The named session the text selects, when it names one: a blank
/// name is absence, not a refusal.
fn optional_herdr_session(raw: Option<&str>) -> Result<Option<String>, RegistrationError> {
    match raw {
        Some(name) if name.trim().is_empty() => Ok(None),
        Some(name) => Ok(Some(herdr_session_name(name)?)),
        None => Ok(None),
    }
}

/// Accept a repository anchor that carries text and names an
/// absolute path; surrounding whitespace is not part of the field.
fn absolute_repository_anchor(raw: &str) -> Result<String, RegistrationError> {
    let trimmed = anchored("target repository", raw)?;
    if !Path::new(&trimmed).is_absolute() {
        return Err(RegistrationError::RelativeRepository);
    }
    Ok(trimmed)
}

/// Accept a field that carries at least one non-whitespace
/// character; surrounding whitespace is not part of the field.
pub(crate) fn anchored(field: &'static str, raw: &str) -> Result<String, RegistrationError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(RegistrationError::Blank(field));
    }
    Ok(trimmed.to_owned())
}

/// The closed lifecycle vocabulary for a Project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectState {
    /// Registered and holding its anchors.
    Active,
    /// Terminal: every recorded fact is preserved and no further
    /// change is legal.
    Archived,
}

/// Why a transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectError {
    /// The Project is already archived.
    AlreadyArchived,
    /// Archived is terminal: the Project accepts no further changes.
    ArchivedIsTerminal,
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyArchived => write!(f, "the Project is already archived"),
            Self::ArchivedIsTerminal => {
                write!(
                    f,
                    "archived is terminal; the Project accepts no further changes"
                )
            }
        }
    }
}

impl std::error::Error for ProjectError {}

/// One Project aggregate. The version counts applied changes:
/// registration lands at 1 and every legal transition bumps it, so a
/// stored version is all a caller needs for optimistic checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    id: ProjectId,
    registration: ProjectRegistration,
    state: ProjectState,
    counters: ProjectCounters,
    version: u64,
}

impl Project {
    /// A fresh Project: active, at version 1, nothing minted yet.
    pub fn new(id: ProjectId, registration: ProjectRegistration) -> Self {
        Self {
            id,
            registration,
            state: ProjectState::Active,
            counters: ProjectCounters::zeroed(),
            version: 1,
        }
    }

    /// Rehydrate a stored Project exactly as it was recorded.
    pub fn restore(
        id: ProjectId,
        registration: ProjectRegistration,
        state: ProjectState,
        counters: ProjectCounters,
        version: u64,
    ) -> Self {
        Self {
            id,
            registration,
            state,
            counters,
            version,
        }
    }

    /// The immutable identity.
    pub fn id(&self) -> ProjectId {
        self.id
    }

    /// The validated registration this Project was created from.
    pub fn registration(&self) -> &ProjectRegistration {
        &self.registration
    }

    /// The immutable code.
    pub fn code(&self) -> &ProjectCode {
        self.registration.code()
    }

    /// The lifecycle state.
    pub fn state(&self) -> ProjectState {
        self.state
    }

    /// Whether the Project is archived.
    pub fn is_archived(&self) -> bool {
        self.state == ProjectState::Archived
    }

    /// The Project's counters, preserved through every state.
    pub fn counters(&self) -> ProjectCounters {
        self.counters
    }

    /// The number of applied changes, for optimistic version checks.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Mint the next Plan, Spec, or Ticket number. Minting is an
    /// applied change: the counter and the aggregate version move
    /// together, so a writer holding a stale version can never save a
    /// counter that rewinds a minted number. Archived is terminal, so
    /// an archived Project refuses every mint.
    pub fn mint(&mut self, kind: NumberKind) -> Result<u64, ProjectError> {
        if self.state == ProjectState::Archived {
            return Err(ProjectError::ArchivedIsTerminal);
        }
        let number = self.counters.next(kind);
        self.version += 1;
        Ok(number)
    }

    /// Archive an active Project. Archived is terminal, so a second
    /// archive is refused rather than absorbed.
    pub fn archive(&mut self) -> Result<(), ProjectError> {
        if self.state == ProjectState::Archived {
            return Err(ProjectError::AlreadyArchived);
        }
        self.state = ProjectState::Archived;
        self.version += 1;
        Ok(())
    }
}

#[cfg(test)]
mod project_code {
    use super::{CodeError, NumberKind, ProjectCode};

    fn code(raw: &str) -> ProjectCode {
        ProjectCode::new(raw).expect("a well-formed code is accepted")
    }

    #[test]
    fn a_code_matching_the_pattern_in_full_is_accepted() {
        for well_formed in ["AB", "A1", "CORE", "A2345678", "ABCDEFGH"] {
            assert!(
                ProjectCode::new(well_formed).is_ok(),
                "{well_formed} matches [A-Z][A-Z0-9]{{1,7}}"
            );
        }
    }

    #[test]
    fn a_code_outside_the_pattern_is_refused() {
        for malformed in [
            "",
            "A",
            "aB",
            "abc",
            "1AB",
            "A-B",
            "A B",
            "ABCDEFGHI",
            "ÄB",
            "CO RE",
        ] {
            assert_eq!(
                ProjectCode::new(malformed),
                Err(CodeError::Malformed),
                "{malformed:?} must not pass as a code"
            );
        }
    }

    #[test]
    fn the_product_code_is_reserved() {
        // `KAN` is well-formed, so the refusal proves the reservation,
        // not the pattern.
        assert_eq!(ProjectCode::new("KAN"), Err(CodeError::Reserved));
    }

    #[test]
    fn identical_text_mints_one_code() {
        assert_eq!(code("CORE"), code("CORE"));
    }

    #[test]
    fn distinct_text_mints_distinct_codes() {
        let mut seen = std::collections::HashSet::new();
        assert!(seen.insert(code("CORE")));
        assert!(
            seen.insert(code("WAVE")),
            "distinct codes must stay distinct"
        );
        assert_eq!(seen.len(), 2, "two codes mint two identities");
    }

    #[test]
    fn a_minted_number_renders_with_its_kind_prefix() {
        let code = code("CORE");

        assert_eq!(NumberKind::Plan.render(&code, 1), "CORE-P1");
        assert_eq!(NumberKind::Spec.render(&code, 4), "CORE-S4");
        assert_eq!(NumberKind::Ticket.render(&code, 23), "CORE-T23");
    }
}

#[cfg(test)]
mod counters {
    use super::{NumberKind, ProjectCounters};

    #[test]
    fn counters_mint_from_one() {
        let mut counters = ProjectCounters::zeroed();

        assert_eq!(counters.last(NumberKind::Plan), 0, "nothing minted yet");
        assert_eq!(counters.next(NumberKind::Plan), 1);
        assert_eq!(counters.next(NumberKind::Spec), 1);
        assert_eq!(counters.next(NumberKind::Ticket), 1);
    }

    #[test]
    fn counters_are_independent_per_kind() {
        let mut counters = ProjectCounters::zeroed();
        counters.next(NumberKind::Plan);
        counters.next(NumberKind::Plan);

        assert_eq!(counters.last(NumberKind::Plan), 2);
        assert_eq!(
            counters.last(NumberKind::Spec),
            0,
            "minting a Plan number must not move the Spec counter"
        );
        assert_eq!(
            counters.last(NumberKind::Ticket),
            0,
            "minting a Plan number must not move the Ticket counter"
        );
    }

    #[test]
    fn counters_are_monotonic_and_never_reuse_a_number() {
        let mut counters = ProjectCounters::zeroed();

        let mut minted = 0;
        for _ in 0..3 {
            let number = counters.next(NumberKind::Ticket);
            assert!(
                number > minted,
                "every minted number must exceed the previous one"
            );
            minted = number;
        }
        assert_eq!(minted, 3);
        assert_eq!(counters.next(NumberKind::Ticket), 4, "numbers never rewind");
    }

    #[test]
    fn counters_tolerate_gaps_in_stored_values() {
        // Gaps are valid: a stored 7 with nothing minted at 3 stays a
        // gap, and minting continues past it rather than refilling it.
        let mut counters = ProjectCounters::restore(4, 0, 7);

        assert_eq!(counters.last(NumberKind::Plan), 4);
        assert_eq!(counters.last(NumberKind::Spec), 0);
        assert_eq!(counters.last(NumberKind::Ticket), 7);

        assert_eq!(counters.next(NumberKind::Plan), 5);
        assert_eq!(counters.next(NumberKind::Spec), 1);
        assert_eq!(counters.next(NumberKind::Ticket), 8);
    }

    #[test]
    fn restore_preserves_every_stored_value() {
        let stored = ProjectCounters::restore(9, 1, 12);

        assert_eq!(
            stored,
            ProjectCounters::restore(9, 1, 12),
            "rehydrated counters compare by their stored values"
        );
    }
}

#[cfg(test)]
mod project_registration {
    use crate::herdr::HerdrSession;
    use crate::initiative::InitiativeId;

    use super::{CodeError, ProjectRegistration, RegistrationError};

    /// A registration attempt with the standard anchors, so a test
    /// can vary the code alone.
    fn registering(code: &str) -> Result<ProjectRegistration, RegistrationError> {
        ProjectRegistration::new(
            code,
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            None,
        )
    }

    fn registration(code: &str) -> ProjectRegistration {
        registering(code).expect("a well-formed registration is accepted")
    }

    /// A registration attempt with every anchor carried, so a test
    /// can blank one field at a time.
    fn attempt(
        name: &str,
        repository: &str,
        seed_workspace: &str,
        default_branch: &str,
        herdr_workspace: &str,
    ) -> Result<ProjectRegistration, RegistrationError> {
        ProjectRegistration::new(
            "CORE",
            name,
            repository,
            seed_workspace,
            default_branch,
            herdr_workspace,
            Some("kanban-main"),
            None,
        )
    }

    #[test]
    fn a_registration_refuses_a_relative_repository_anchor() {
        for relative in ["repo", "./repo", "../repo", "workspaces/kanban"] {
            let outcome = attempt(
                "Control plane",
                relative,
                "/workspaces/kanban.seed",
                "main",
                "kanban.seed",
            );
            assert_eq!(
                outcome,
                Err(RegistrationError::RelativeRepository),
                "{relative:?} must not be stored as a repository anchor"
            );
        }
    }

    #[test]
    fn a_registration_refuses_a_blank_anchor() {
        let carried = [
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
        ];
        for blanked in 0..carried.len() {
            let mut fields = carried;
            fields[blanked] = " ";
            let outcome = attempt(fields[0], fields[1], fields[2], fields[3], fields[4]);
            assert!(
                matches!(outcome, Err(RegistrationError::Blank(_))),
                "anchor {blanked} must carry text"
            );
        }
    }

    #[test]
    fn a_registration_trims_surrounding_whitespace() {
        let validated = ProjectRegistration::new(
            "CORE",
            "  Control plane  ",
            " /repositories/kanban ",
            "/workspaces/kanban.seed",
            " main ",
            " kanban.seed ",
            Some(" kanban-main "),
            None,
        )
        .expect("the anchors carry text");

        assert_eq!(validated.name(), "Control plane");
        assert_eq!(validated.repository(), "/repositories/kanban");
        assert_eq!(validated.default_branch(), "main");
        assert_eq!(validated.herdr_workspace(), "kanban.seed");
        assert_eq!(validated.herdr_session(), Some("kanban-main"));
    }

    #[test]
    fn a_registration_stores_one_required_target_herdr_workspace() {
        let validated = registration("CORE");

        assert_eq!(validated.herdr_workspace(), "kanban.seed");
    }

    #[test]
    fn a_registration_without_a_session_selects_the_default_session() {
        let validated = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            None,
            None,
        )
        .expect("an absent session is a valid registration");

        assert_eq!(validated.herdr_session(), None);
        assert_eq!(
            validated.effective_herdr_session(),
            HerdrSession::Default,
            "absence selects Herdr's default session (DR-PH-07)"
        );
    }

    #[test]
    fn a_registration_with_a_named_session_selects_that_session() {
        let validated = registration("CORE");

        assert_eq!(validated.herdr_session(), Some("kanban-main"));
        assert_eq!(
            validated.effective_herdr_session(),
            HerdrSession::Named("kanban-main".to_owned())
        );
    }

    #[test]
    fn a_blank_session_name_is_absence_not_a_refusal() {
        let validated = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("  "),
            None,
        )
        .expect("a blank session name selects the default session");

        assert_eq!(validated.herdr_session(), None);
        assert_eq!(validated.effective_herdr_session(), HerdrSession::Default);
    }

    #[test]
    fn a_registration_refuses_an_unsafe_herdr_session_name() {
        for session in [
            "/absolute",
            "foo/bar",
            "..",
            "../escape",
            "still/../escape",
            ".",
        ] {
            let outcome = ProjectRegistration::new(
                "CORE",
                "Control plane",
                "/repositories/kanban",
                "/workspaces/kanban.seed",
                "main",
                "kanban.seed",
                Some(session),
                None,
            );
            assert_eq!(
                outcome,
                Err(RegistrationError::InvalidHerdrSession),
                "session `{session}` must be refused"
            );
        }
    }

    #[test]
    fn a_registration_accepts_a_single_safe_herdr_session_segment() {
        let validated = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            None,
        )
        .expect("a single safe segment is accepted");

        assert_eq!(validated.herdr_session(), Some("kanban-main"));
    }

    #[test]
    fn a_registration_refuses_a_malformed_code() {
        assert_eq!(
            registering("core"),
            Err(RegistrationError::Code(CodeError::Malformed))
        );
    }

    #[test]
    fn a_registration_refuses_the_reserved_code() {
        assert_eq!(
            registering("KAN"),
            Err(RegistrationError::Code(CodeError::Reserved))
        );
    }

    #[test]
    fn a_registration_carries_its_optional_initiative() {
        let under_initiative = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            Some(InitiativeId::new(4)),
        )
        .expect("a well-formed registration is accepted");

        assert_eq!(under_initiative.initiative(), Some(InitiativeId::new(4)));
        assert_eq!(registration("CORE").initiative(), None);
    }
}

#[cfg(test)]
mod project_lifecycle {
    use crate::initiative::InitiativeId;

    use super::{
        NumberKind, Project, ProjectCounters, ProjectError, ProjectId, ProjectRegistration,
        ProjectState,
    };

    fn registration(code: &str) -> ProjectRegistration {
        ProjectRegistration::new(
            code,
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            None,
        )
        .expect("a well-formed registration is accepted")
    }

    #[test]
    fn a_fresh_project_is_active_at_version_one_with_zeroed_counters() {
        let project = Project::new(ProjectId::new(7), registration("CORE"));

        assert_eq!(project.id(), ProjectId::new(7));
        assert_eq!(project.code().as_str(), "CORE");
        assert_eq!(project.state(), ProjectState::Active);
        assert!(!project.is_archived());
        assert_eq!(project.counters(), ProjectCounters::zeroed());
        assert_eq!(project.version(), 1);
    }

    #[test]
    fn archiving_moves_to_the_terminal_state_and_bumps_the_version() {
        let mut project = Project::new(ProjectId::new(1), registration("CORE"));

        project.archive().expect("active archives");

        assert_eq!(project.state(), ProjectState::Archived);
        assert!(project.is_archived());
        assert_eq!(project.version(), 2);
        assert_eq!(
            project.code().as_str(),
            "CORE",
            "archiving preserves the code"
        );
        assert_eq!(
            project.counters(),
            ProjectCounters::zeroed(),
            "archiving preserves the counters"
        );
    }

    #[test]
    fn archiving_twice_is_refused() {
        let mut project = Project::new(ProjectId::new(1), registration("CORE"));
        project.archive().expect("the first archive applies");

        assert_eq!(project.archive(), Err(ProjectError::AlreadyArchived));
        assert_eq!(project.version(), 2, "the refusal changed nothing");
    }

    #[test]
    fn minting_through_the_aggregate_stays_monotonic_across_restore() {
        let mut project = Project::new(ProjectId::new(1), registration("CORE"));

        assert_eq!(project.mint(NumberKind::Plan).expect("active mints"), 1);
        assert_eq!(project.mint(NumberKind::Spec).expect("active mints"), 1);
        assert_eq!(project.mint(NumberKind::Ticket).expect("active mints"), 1);

        let mut reloaded = Project::restore(
            project.id(),
            project.registration().clone(),
            project.state(),
            project.counters(),
            project.version(),
        );

        assert_eq!(
            reloaded.mint(NumberKind::Plan).expect("active mints"),
            2,
            "the Plan counter resumes past the last minted number"
        );
        assert_eq!(
            reloaded.mint(NumberKind::Spec).expect("active mints"),
            2,
            "the Spec counter resumes past the last minted number"
        );
        assert_eq!(
            reloaded.mint(NumberKind::Ticket).expect("active mints"),
            2,
            "the Ticket counter resumes past the last minted number"
        );
    }

    #[test]
    fn minting_on_an_archived_project_is_refused() {
        let mut project = Project::new(ProjectId::new(1), registration("CORE"));
        project.archive().expect("active archives");

        assert_eq!(
            project.mint(NumberKind::Plan),
            Err(ProjectError::ArchivedIsTerminal)
        );
        assert_eq!(project.version(), 2, "the refusal changed nothing");
    }

    #[test]
    fn restore_rehydrates_every_recorded_fact() {
        let project = Project::restore(
            ProjectId::new(9),
            ProjectRegistration::new(
                "WAVE",
                "Wave pool",
                "/repositories/wave",
                "/workspaces/wave.seed",
                "trunk",
                "wave.seed",
                Some("wave-main"),
                Some(InitiativeId::new(2)),
            )
            .expect("a well-formed registration is accepted"),
            ProjectState::Archived,
            ProjectCounters::restore(3, 0, 11),
            5,
        );

        assert_eq!(project.id().value(), 9);
        assert_eq!(project.code().as_str(), "WAVE");
        assert_eq!(project.registration().name(), "Wave pool");
        assert_eq!(project.registration().repository(), "/repositories/wave");
        assert_eq!(
            project.registration().seed_workspace(),
            "/workspaces/wave.seed"
        );
        assert_eq!(project.registration().default_branch(), "trunk");
        assert_eq!(project.registration().herdr_workspace(), "wave.seed");
        assert_eq!(project.registration().herdr_session(), Some("wave-main"));
        assert_eq!(
            project.registration().initiative(),
            Some(InitiativeId::new(2))
        );
        assert!(project.is_archived());
        assert_eq!(project.counters().last(NumberKind::Ticket), 11);
        assert_eq!(project.version(), 5);
    }

    #[test]
    fn minting_a_number_moves_one_counter_and_the_version() {
        let mut project = Project::new(ProjectId::new(1), registration("CORE"));

        assert_eq!(project.mint(NumberKind::Plan).expect("active mints"), 1);
        assert_eq!(project.mint(NumberKind::Plan).expect("active mints"), 2);
        assert_eq!(project.mint(NumberKind::Spec).expect("active mints"), 1);

        assert_eq!(project.counters().last(NumberKind::Plan), 2);
        assert_eq!(project.counters().last(NumberKind::Spec), 1);
        assert_eq!(
            project.counters().last(NumberKind::Ticket),
            0,
            "minting other kinds must not move the Ticket counter"
        );
        assert_eq!(
            project.version(),
            4,
            "every mint is an applied change, so a stale writer can never rewind a minted number"
        );
    }

    #[test]
    fn ids_order_by_their_value() {
        let mut ids = [ProjectId::new(3), ProjectId::new(1), ProjectId::new(2)];
        ids.sort();

        assert_eq!(
            ids.map(|id| id.value()),
            [1, 2, 3],
            "projects list in a stable order"
        );
    }
}
