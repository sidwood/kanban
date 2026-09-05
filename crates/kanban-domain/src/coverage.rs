//! Story coverage: the rules that make a Ticket graph executable
//! (CONTEXT.md). A User Story is the atomic unit of coverage; every
//! Acceptance Criterion links to one or more stories (DR-PS-13), every
//! story a Spec version claims must be covered before the Ticket graph
//! for that version becomes executable (DR-PS-14), and technical
//! commands stay Verification Steps, never criteria (DR-PS-15). These
//! are the rules the graph approval gate enforces (KAN-S4, T23); this
//! module owns them, not the approval mechanics.

use std::fmt;

use crate::plan::SpecNumber;
use crate::project::ProjectCode;

/// One User Story: the behaviour claim a `US` bullet of a Spec version
/// makes, named by its Spec's minted number and the claim's ordinal,
/// for example the `3-US6` of `CORE-S3-US6` (CONTEXT.md). Stories are
/// the atomic unit of coverage, so this is an identity, not the claim
/// text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UserStoryRef {
    spec: SpecNumber,
    story: u64,
}

/// Why a User Story identity was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoryRefError {
    /// Story ordinals start at one; zero names no story.
    Zero,
    /// The text is not a story id: neither `CODE-S<n>-US<m>` nor
    /// `S<n>-US<m>` in full.
    Malformed,
    /// The full id names another Project's story; a criterion or a
    /// Spec section of this Project claims its own.
    ForeignProject,
}

impl fmt::Display for StoryRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => write!(f, "a User Story ordinal starts at one"),
            Self::Malformed => {
                write!(f, "a User Story is named like `CORE-S3-US6` or `S3-US6`")
            }
            Self::ForeignProject => write!(f, "the story names another Project"),
        }
    }
}

impl std::error::Error for StoryRefError {}

impl UserStoryRef {
    /// Wrap a story identity, refusing a zero ordinal.
    pub fn new(spec: SpecNumber, story: u64) -> Result<Self, StoryRefError> {
        if story == 0 {
            return Err(StoryRefError::Zero);
        }
        Ok(Self { spec, story })
    }

    /// Parse one story identity from its token, against the Project
    /// the claim belongs to. The full form `CORE-S3-US6` must carry
    /// this Project's code; the bare form `S3-US6` names this
    /// Project's own Spec by number, the form a Spec's story bullets
    /// use.
    pub fn parse(token: &str, code: &ProjectCode) -> Result<Self, StoryRefError> {
        let token = token.trim();
        let parts: Vec<&str> = token.split('-').collect();
        let (named_code, spec, story) = match parts.as_slice() {
            [named_code, spec, story] => (Some(*named_code), *spec, *story),
            [spec, story] => (None, *spec, *story),
            _ => return Err(StoryRefError::Malformed),
        };
        if let Some(named) = named_code {
            if !is_project_code_shape(named) {
                return Err(StoryRefError::Malformed);
            }
            if named != code.as_str() {
                return Err(StoryRefError::ForeignProject);
            }
        }
        let spec = parse_ordinal(spec, 'S')?;
        let story = parse_ordinal(story, 'U')?;
        Ok(Self {
            spec: SpecNumber::new(spec).map_err(|_| StoryRefError::Zero)?,
            story,
        })
    }

    /// The Spec whose story this is.
    pub fn spec(self) -> SpecNumber {
        self.spec
    }

    /// The story's ordinal within its Spec.
    pub fn story(self) -> u64 {
        self.story
    }

    /// The code-less stored and wire name, for example `S3-US6`.
    pub fn wire_name(self) -> String {
        format!("S{}-US{}", self.spec.value(), self.story)
    }

    /// The rendered identity with the Project's code, for example
    /// `CORE-S3-US6` (DR-PH-06).
    pub fn render(self, code: &ProjectCode) -> String {
        format!("{}-{}", code, self.wire_name())
    }
}

/// Whether `text` matches the Project code shape `[A-Z][A-Z0-9]{1,7}`
/// in full, the reserved product code included.
fn is_project_code_shape(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_ascii_uppercase() => {
            let rest = chars.as_str();
            (1..=7).contains(&rest.len())
                && rest
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        }
        _ => false,
    }
}

/// Read `S3` as 3 or `US6` as 6, refusing anything but digits without
/// a leading zero behind the marker.
fn parse_ordinal(text: &str, marker: char) -> Result<u64, StoryRefError> {
    let digits = match marker {
        'U' => text.strip_prefix("US"),
        _ => text.strip_prefix(marker),
    }
    .ok_or(StoryRefError::Malformed)?;
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return Err(StoryRefError::Malformed);
    }
    let value: u64 = digits.parse().expect("the digits are validated");
    if value == 0 {
        return Err(StoryRefError::Zero);
    }
    Ok(value)
}

/// Why an Acceptance Criterion was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriterionError {
    /// A criterion states an observable outcome; whitespace states
    /// nothing.
    NoOutcome,
    /// A criterion links to one or more User Stories (DR-PS-13); an
    /// unlinked criterion ships nothing owned.
    Unlinked,
    /// The outcome is a technical command, which belongs to a
    /// Verification Step instead (DR-PS-15).
    TechnicalCommand,
}

impl fmt::Display for CriterionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOutcome => {
                write!(f, "an Acceptance Criterion states an observable outcome")
            }
            Self::Unlinked => {
                write!(
                    f,
                    "an Acceptance Criterion links to one or more User Stories"
                )
            }
            Self::TechnicalCommand => write!(
                f,
                "technical commands are Verification Steps, never Acceptance Criteria"
            ),
        }
    }
}

impl std::error::Error for CriterionError {}

/// One Acceptance Criterion: an observable outcome linked to the User
/// Stories it delivers (CONTEXT.md). The identity a Ticket mints for
/// its criteria — the `KAN-T<n>-AC<k>` of the planning corpus —
/// belongs to the Ticket aggregate (KAN-S4); this value carries the
/// rule-bearing content alone. Creation and edit enforce the same
/// links, so a criterion never exists unlinked (DR-PS-13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceCriterion {
    outcome: String,
    stories: Vec<UserStoryRef>,
}

impl AcceptanceCriterion {
    /// Assemble a criterion, refusing an outcome that states nothing,
    /// a link list that names no story, and a technical command
    /// (DR-PS-13, DR-PS-15).
    pub fn new(
        outcome: impl Into<String>,
        stories: Vec<UserStoryRef>,
    ) -> Result<Self, CriterionError> {
        let criterion = Self {
            outcome: outcome.into(),
            stories,
        };
        criterion.validate()?;
        Ok(criterion)
    }

    /// Replace the outcome and the links under the creation rules.
    /// A refusal leaves the criterion exactly as it stood.
    pub fn edit(
        &mut self,
        outcome: impl Into<String>,
        stories: Vec<UserStoryRef>,
    ) -> Result<(), CriterionError> {
        let replacement = Self::new(outcome, stories)?;
        self.outcome = replacement.outcome;
        self.stories = replacement.stories;
        Ok(())
    }

    /// Refuse what the rules refuse.
    fn validate(&self) -> Result<(), CriterionError> {
        if self.outcome.trim().is_empty() {
            return Err(CriterionError::NoOutcome);
        }
        if self.stories.is_empty() {
            return Err(CriterionError::Unlinked);
        }
        if is_technical_command(&self.outcome) {
            return Err(CriterionError::TechnicalCommand);
        }
        Ok(())
    }

    /// The observable outcome.
    pub fn outcome(&self) -> &str {
        &self.outcome
    }

    /// The linked User Stories, in the order they were linked.
    pub fn stories(&self) -> &[UserStoryRef] {
        &self.stories
    }
}

/// The leading tokens that mark text as a technical command, the
/// closed vocabulary of DR-PS-15. A criterion may mention any of
/// these in prose; only text shaped as the command itself — the token
/// leading the text — is refused.
const COMMAND_LEADS: &[&str] = &[
    "bash", "brew", "cargo", "curl", "docker", "echo", "git", "just", "make", "npm", "npx", "pnpm",
    "python", "python3", "rustup", "sh", "zsh",
];

/// Whether text is a technical command: a shell prompt, a code span,
/// or a command-leading token with its arguments. Prose that merely
/// mentions a command stays prose.
fn is_technical_command(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.starts_with('$') {
        return true;
    }
    if trimmed.len() >= 2 && trimmed.starts_with('`') && trimmed.ends_with('`') {
        return true;
    }
    COMMAND_LEADS.iter().any(|lead| {
        trimmed
            .strip_prefix(lead)
            .is_some_and(|rest| rest.starts_with(char::is_whitespace))
    })
}

/// Why a Verification Step was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationStepError {
    /// A step carries its command; whitespace carries nothing.
    Blank,
}

impl fmt::Display for VerificationStepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blank => write!(f, "a Verification Step carries its command"),
        }
    }
}

impl std::error::Error for VerificationStepError {}

/// One Verification Step: the command or scripted procedure that
/// demonstrates a criterion (CONTEXT.md). Commands live here, in
/// Tickets, and never as criteria (DR-PS-15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationStep {
    command: String,
}

impl VerificationStep {
    /// Store one command or scripted procedure, refusing whitespace.
    pub fn new(command: impl Into<String>) -> Result<Self, VerificationStepError> {
        let step = Self {
            command: command.into(),
        };
        if step.command.trim().is_empty() {
            return Err(VerificationStepError::Blank);
        }
        Ok(step)
    }

    /// The command as it runs.
    pub fn command(&self) -> &str {
        &self.command
    }
}

/// Why a story scope was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeError {
    /// The Spec version's story section names no User Story, so no
    /// Ticket graph could ever cover it; an empty scope admits no
    /// coverage.
    NoStories,
}

impl fmt::Display for ScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoStories => {
                write!(f, "the Spec version claims no User Stories to cover")
            }
        }
    }
}

impl std::error::Error for ScopeError {}

/// The User Stories one Spec version claims: every story its `US`
/// bullets name, in first-appearance order. This is the scope a
/// Ticket graph for that version must cover before it becomes
/// executable (DR-PS-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoryScope {
    spec: SpecNumber,
    stories: Vec<UserStoryRef>,
}

impl StoryScope {
    /// Collect the stories a Spec version's story section claims.
    /// Each `US` bullet names its story with the full identity —
    /// `CORE-S3-US6` — or the bare `S3-US6` of the Project's own
    /// Spec; lines naming another Spec or another Project claim
    /// nothing here. A section naming no story is refused: a scope
    /// of nothing can never prove coverage.
    pub fn extract(
        code: &ProjectCode,
        spec: SpecNumber,
        user_stories: &str,
    ) -> Result<Self, ScopeError> {
        let mut stories: Vec<UserStoryRef> = Vec::new();
        for line in user_stories.lines() {
            let Some(found) = line
                .trim()
                .strip_prefix(['-', '*', '+'])
                .map(str::trim_start)
                .unwrap_or(line.trim())
                .split_whitespace()
                .next()
                .map(|token| token.trim_end_matches(':'))
            else {
                continue;
            };
            let Ok(story) = UserStoryRef::parse(found, code) else {
                continue;
            };
            if story.spec() == spec && !stories.contains(&story) {
                stories.push(story);
            }
        }
        if stories.is_empty() {
            return Err(ScopeError::NoStories);
        }
        Ok(Self { spec, stories })
    }

    /// The Spec this scope belongs to.
    pub fn spec(&self) -> SpecNumber {
        self.spec
    }

    /// Every claimed story, in first-appearance order.
    pub fn stories(&self) -> &[UserStoryRef] {
        &self.stories
    }

    /// Whether one story belongs to this scope.
    pub fn contains(&self, story: UserStoryRef) -> bool {
        self.stories.contains(&story)
    }

    /// The stories no criterion claims, in scope order: the coverage
    /// gaps a Ticket graph must close.
    pub fn uncovered(&self, criteria: &[AcceptanceCriterion]) -> Vec<UserStoryRef> {
        self.stories
            .iter()
            .copied()
            .filter(|story| {
                !criteria
                    .iter()
                    .any(|criterion| criterion.stories().contains(story))
            })
            .collect()
    }
}

/// Why a Ticket graph cannot become executable (DR-PS-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableRefusal {
    uncovered: Vec<UserStoryRef>,
}

impl ExecutableRefusal {
    /// Every story left uncovered, in scope order.
    pub fn uncovered(&self) -> &[UserStoryRef] {
        &self.uncovered
    }
}

impl fmt::Display for ExecutableRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<String> = self
            .uncovered
            .iter()
            .map(|story| story.wire_name())
            .collect();
        write!(
            f,
            "the Ticket graph cannot become executable while User Stories {} stay uncovered",
            names.join(", ")
        )
    }
}

impl std::error::Error for ExecutableRefusal {}

/// The executable gate (DR-PS-14): a Ticket graph cannot become
/// executable while any User Story in scope is uncovered. Graph
/// approval consumes this gate (KAN-S4, T23); the criteria arrive
/// already rule-valid, created and edited through
/// [`AcceptanceCriterion`].
pub fn enforce_executable(
    scope: &StoryScope,
    criteria: &[AcceptanceCriterion],
) -> Result<(), ExecutableRefusal> {
    let uncovered = scope.uncovered(criteria);
    if uncovered.is_empty() {
        return Ok(());
    }
    Err(ExecutableRefusal { uncovered })
}

#[cfg(test)]
mod story_refs {
    use super::{StoryRefError, UserStoryRef};
    use crate::plan::SpecNumber;
    use crate::project::ProjectCode;

    fn code() -> ProjectCode {
        ProjectCode::new("CORE").expect("the fixture code is well formed")
    }

    fn spec(number: u64) -> SpecNumber {
        SpecNumber::new(number).expect("the fixture number is positive")
    }

    #[test]
    fn full_and_bare_ids_name_the_same_story() {
        assert_eq!(
            UserStoryRef::parse("CORE-S3-US6", &code()).expect("the full id parses"),
            UserStoryRef::parse("S3-US6", &code()).expect("the bare id parses")
        );
        assert_eq!(
            UserStoryRef::parse("CORE-S3-US6", &code()).expect("the full id parses"),
            UserStoryRef::new(spec(3), 6).expect("the ordinal is positive")
        );
        assert_eq!(
            UserStoryRef::parse("  CORE-S3-US6  ", &code()).expect("padding is trimmed"),
            UserStoryRef::new(spec(3), 6).expect("the ordinal is positive")
        );
    }

    #[test]
    fn ids_render_with_and_without_the_project_code() {
        let story = UserStoryRef::new(spec(3), 6).expect("the ordinal is positive");

        assert_eq!(story.wire_name(), "S3-US6");
        assert_eq!(story.render(&code()), "CORE-S3-US6");
        assert_eq!(story.spec(), spec(3));
        assert_eq!(story.story(), 6);
    }

    #[test]
    fn malformed_ids_are_refused() {
        for token in [
            "",
            "CORE-S3",
            "S3",
            "S3-US",
            "US6",
            "core-s3-us6",
            "CORE-s3-US6",
            "CORE-S03-US6",
            "CORE-S3-US06",
            "CORE-S3-US6:",
            "CORE-S3-US6 extra",
            "CORE-S3-US6-US9",
            "CORE$-S3-US6",
        ] {
            assert_eq!(
                UserStoryRef::parse(token, &code()).unwrap_err(),
                StoryRefError::Malformed,
                "`{token}` names no User Story"
            );
        }
    }

    #[test]
    fn zero_ordinals_name_no_story() {
        assert_eq!(
            UserStoryRef::parse("CORE-S3-US0", &code()).unwrap_err(),
            StoryRefError::Zero
        );
        assert_eq!(
            UserStoryRef::parse("S0-US1", &code()).unwrap_err(),
            StoryRefError::Zero
        );
        assert_eq!(
            UserStoryRef::new(spec(3), 0).unwrap_err(),
            StoryRefError::Zero
        );
    }

    #[test]
    fn a_full_id_of_another_project_is_refused() {
        assert_eq!(
            UserStoryRef::parse("EDGE-S3-US6", &code()).unwrap_err(),
            StoryRefError::ForeignProject
        );
    }
}

#[cfg(test)]
mod story_scope {
    use super::{ScopeError, StoryScope, UserStoryRef};
    use crate::plan::SpecNumber;
    use crate::project::ProjectCode;

    fn code() -> ProjectCode {
        ProjectCode::new("CORE").expect("the fixture code is well formed")
    }

    fn spec(number: u64) -> SpecNumber {
        SpecNumber::new(number).expect("the fixture number is positive")
    }

    fn story(spec_number: u64, ordinal: u64) -> UserStoryRef {
        UserStoryRef::new(spec(spec_number), ordinal).expect("the ordinal is positive")
    }

    #[test]
    fn extraction_collects_the_bullet_ids_in_first_appearance_order() {
        let section = "\
- CORE-S1-US2: As an operator, I want every criterion linked, so that
  nothing ships unowned.
- CORE-S1-US1: As an operator, I want coverage enforced, so that the
  graph is provably executable.
";

        let scope =
            StoryScope::extract(&code(), spec(1), section).expect("the section claims its stories");

        assert_eq!(scope.spec(), spec(1));
        assert_eq!(
            scope.stories(),
            [story(1, 2), story(1, 1)].as_slice(),
            "first appearance orders the scope"
        );
    }

    #[test]
    fn extraction_accepts_bare_ids_and_dedupes_repeats() {
        let section = "\
S1-US3: bare ids name the Project's own Spec.
- S1-US3: a repeat names the same story once.
";

        let scope =
            StoryScope::extract(&code(), spec(1), section).expect("the section claims its stories");

        assert_eq!(scope.stories(), [story(1, 3)].as_slice());
        assert!(scope.contains(story(1, 3)));
        assert!(!scope.contains(story(1, 4)));
    }

    #[test]
    fn extraction_ignores_foreign_spec_and_project_lines() {
        let section = "\
- CORE-S9-US1: another Spec's story stays out of this scope.
- EDGE-S1-US1: another Project's story stays out too.
- CORE-S1-US1: this one claims.
";

        let scope =
            StoryScope::extract(&code(), spec(1), section).expect("the section claims a story");

        assert_eq!(scope.stories(), [story(1, 1)].as_slice());
    }

    #[test]
    fn a_section_claiming_no_story_is_refused() {
        for section in [
            "",
            "As an operator, I want prose alone, with no id anywhere.\n",
            "- EDGE-S1-US1: every line names another Project.\n",
        ] {
            assert_eq!(
                StoryScope::extract(&code(), spec(1), section).unwrap_err(),
                ScopeError::NoStories,
                "an empty scope admits no coverage"
            );
        }
    }
}

#[cfg(test)]
mod criteria {
    use super::{AcceptanceCriterion, CriterionError, UserStoryRef, VerificationStep};
    use crate::plan::SpecNumber;

    fn story(ordinal: u64) -> UserStoryRef {
        UserStoryRef::new(
            SpecNumber::new(1).expect("the fixture number is positive"),
            ordinal,
        )
        .expect("the ordinal is positive")
    }

    fn linked(ordinal: u64, outcome: &str) -> AcceptanceCriterion {
        AcceptanceCriterion::new(outcome, vec![story(ordinal)]).expect("the criterion links")
    }

    #[test]
    fn a_criterion_links_one_or_more_stories_at_creation() {
        let criterion = AcceptanceCriterion::new(
            "Every criterion links to one or more User Stories.",
            vec![story(6), story(7)],
        )
        .expect("several stories may share one criterion");

        assert_eq!(
            criterion.outcome(),
            "Every criterion links to one or more User Stories."
        );
        assert_eq!(criterion.stories(), [story(6), story(7)].as_slice());

        assert_eq!(
            AcceptanceCriterion::new("An unlinked outcome.", Vec::new()).unwrap_err(),
            CriterionError::Unlinked,
            "an unlinked criterion ships nothing owned (DR-PS-13)"
        );
    }

    #[test]
    fn a_criterion_links_one_or_more_stories_at_edit() {
        let mut criterion = linked(6, "Every criterion links to stories.");

        criterion
            .edit("Relinked to a later story.", vec![story(9)])
            .expect("the edit relinks");

        assert_eq!(criterion.outcome(), "Relinked to a later story.");
        assert_eq!(criterion.stories(), [story(9)].as_slice());

        let error = criterion
            .edit("Reworded without links.", Vec::new())
            .unwrap_err();

        assert_eq!(
            error,
            CriterionError::Unlinked,
            "an edit may not strip every link (DR-PS-13)"
        );
        assert_eq!(criterion.outcome(), "Relinked to a later story.");
        assert_eq!(
            criterion.stories(),
            [story(9)].as_slice(),
            "the refusal changed nothing"
        );
    }

    #[test]
    fn a_criterion_states_an_outcome() {
        for outcome in ["", "   "] {
            assert_eq!(
                AcceptanceCriterion::new(outcome, vec![story(6)]).unwrap_err(),
                CriterionError::NoOutcome
            );
        }

        let mut criterion = linked(6, "An outcome stands here.");
        assert_eq!(
            criterion.edit("  ", vec![story(6)]).unwrap_err(),
            CriterionError::NoOutcome
        );
        assert_eq!(
            criterion.outcome(),
            "An outcome stands here.",
            "the refusal changed nothing"
        );
    }

    #[test]
    fn technical_commands_are_refused_as_criteria() {
        for outcome in [
            "cargo test -p kanban-domain coverage",
            "pnpm --filter desktop test",
            "git diff --check",
            "`cargo fmt --all --check`",
            "$ git status --porcelain",
            "just verify-contracts",
        ] {
            assert_eq!(
                AcceptanceCriterion::new(outcome, vec![story(6)]).unwrap_err(),
                CriterionError::TechnicalCommand,
                "`{outcome}` is a Verification Step, never a criterion (DR-PS-15)"
            );
        }

        let mut criterion = linked(6, "The suite passes on the approval tip.");
        assert_eq!(
            criterion
                .edit("python3 scripts/check_planning.py", vec![story(6)])
                .unwrap_err(),
            CriterionError::TechnicalCommand
        );
        assert_eq!(
            criterion.outcome(),
            "The suite passes on the approval tip.",
            "the refusal changed nothing"
        );
    }

    #[test]
    fn prose_mentioning_a_command_stays_a_criterion() {
        for outcome in [
            "The suite `cargo test -p x` passes on the approval tip.",
            "Run cargo test locally before review.",
            "Echoes of a command word are still prose.",
        ] {
            AcceptanceCriterion::new(outcome, vec![story(6)])
                .expect("only command-shaped text is refused");
        }
    }

    #[test]
    fn verification_steps_store_their_commands() {
        let step = VerificationStep::new("cargo test -p kanban-domain coverage")
            .expect("a command is stored as a step");

        assert_eq!(step.command(), "cargo test -p kanban-domain coverage");

        let scripted = VerificationStep::new("scripts/check_planning.py --strict")
            .expect("a scripted procedure is a step too");
        assert_eq!(scripted.command(), "scripts/check_planning.py --strict");

        assert_eq!(
            VerificationStep::new("   ").unwrap_err(),
            super::VerificationStepError::Blank,
            "a step carries its command"
        );
    }
}

#[cfg(test)]
mod executable_gate {
    use super::{AcceptanceCriterion, UserStoryRef, enforce_executable};
    use crate::plan::SpecNumber;
    use crate::project::ProjectCode;

    fn code() -> ProjectCode {
        ProjectCode::new("CORE").expect("the fixture code is well formed")
    }

    fn spec(number: u64) -> SpecNumber {
        SpecNumber::new(number).expect("the fixture number is positive")
    }

    fn story(spec_number: u64, ordinal: u64) -> UserStoryRef {
        UserStoryRef::new(spec(spec_number), ordinal).expect("the ordinal is positive")
    }

    /// A scope of three stories on Spec 1, as a section claims them.
    fn scope() -> super::StoryScope {
        let section = "\
- CORE-S1-US1: compose.
- CORE-S1-US2: cover.
- CORE-S1-US3: gate.
";
        super::StoryScope::extract(&code(), spec(1), section)
            .expect("the fixture section claims its stories")
    }

    fn criterion(spec_number: u64, ordinal: u64, outcome: &str) -> AcceptanceCriterion {
        AcceptanceCriterion::new(outcome, vec![story(spec_number, ordinal)])
            .expect("the fixture criterion links")
    }

    #[test]
    fn the_gate_refuses_while_any_story_stays_uncovered() {
        let scope = scope();
        let criteria = [criterion(1, 1, "Composition is reviewable.")];

        let refusal = enforce_executable(&scope, &criteria).unwrap_err();

        assert_eq!(
            refusal.uncovered(),
            [story(1, 2), story(1, 3)].as_slice(),
            "every gap is listed in scope order (DR-PS-14)"
        );
        assert_eq!(
            refusal.to_string(),
            "the Ticket graph cannot become executable while User Stories \
             S1-US2, S1-US3 stay uncovered"
        );
    }

    #[test]
    fn the_gate_passes_when_every_story_is_claimed() {
        let scope = scope();
        let criteria = [
            criterion(1, 1, "Composition is reviewable."),
            criterion(1, 2, "Coverage is enforced."),
            criterion(1, 3, "The gate refuses gaps."),
        ];

        enforce_executable(&scope, &criteria).expect("every story is claimed (DR-PS-14)");
    }

    #[test]
    fn overlapping_claims_cover_their_stories_together() {
        let scope = scope();
        let shared = AcceptanceCriterion::new(
            "One outcome serves two stories.",
            vec![story(1, 1), story(1, 2)],
        )
        .expect("the criterion links twice");

        enforce_executable(&scope, &[shared, criterion(1, 3, "The gate holds.")])
            .expect("a story needs one claim, not one criterion");
    }

    #[test]
    fn a_criterion_of_another_spec_claims_nothing_here() {
        let scope = scope();
        let criteria = [
            criterion(1, 1, "Composition is reviewable."),
            criterion(9, 2, "A foreign story is well linked, just not here."),
        ];

        let refusal = enforce_executable(&scope, &criteria).unwrap_err();

        assert_eq!(
            refusal.uncovered(),
            [story(1, 2), story(1, 3)].as_slice(),
            "claims outside the scope cover nothing in it"
        );
    }

    #[test]
    fn a_graph_without_criteria_refuses_every_story() {
        let refusal = enforce_executable(&scope(), &[]).unwrap_err();

        assert_eq!(
            refusal.uncovered(),
            [story(1, 1), story(1, 2), story(1, 3)].as_slice()
        );
    }
}
