//! Workspace entities: registered working copies with observed git
//! state and closed health vocabulary (KAN-S6-US1).

use std::fmt;

use crate::project::ProjectId;

/// The identity of one Workspace. Assigned once by storage and
/// immutable afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkspaceId(u64);

impl WorkspaceId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The closed health vocabulary for a Workspace (DR-LW-04).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceHealth {
    /// Clean, unassigned, present, and not retired.
    Available,
    /// Assigned to an active Lane.
    Assigned,
    /// The working tree carries uncommitted changes.
    Dirty,
    /// The path is absent or not a worktree of the Project repository.
    Missing,
    /// Retired by the operator; the record is preserved.
    Retired,
}

impl WorkspaceHealth {
    /// The wire name every client agrees on.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Assigned => "assigned",
            Self::Dirty => "dirty",
            Self::Missing => "missing",
            Self::Retired => "retired",
        }
    }

    /// Parse a wire name into the closed vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        match wire {
            "available" => Some(Self::Available),
            "assigned" => Some(Self::Assigned),
            "dirty" => Some(Self::Dirty),
            "missing" => Some(Self::Missing),
            "retired" => Some(Self::Retired),
            _ => None,
        }
    }
}

/// Inputs the health rule needs: durable flags plus one observation
/// snapshot. Observation never mutates the repository; it only informs
/// whether the path is present and what git reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceHealthInputs {
    pub retired: bool,
    pub present: bool,
    pub lane_assigned: bool,
    pub working_tree_clean: bool,
}

/// Compute the single health state from durable flags and observation.
/// Retired wins, then missing, then assigned, then dirty, then
/// available.
pub fn compute_health(inputs: WorkspaceHealthInputs) -> WorkspaceHealth {
    if inputs.retired {
        return WorkspaceHealth::Retired;
    }
    if !inputs.present {
        return WorkspaceHealth::Missing;
    }
    if inputs.lane_assigned {
        return WorkspaceHealth::Assigned;
    }
    if !inputs.working_tree_clean {
        return WorkspaceHealth::Dirty;
    }
    WorkspaceHealth::Available
}

/// Why a registration was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceRegistrationError {
    /// The path holds nothing but whitespace.
    BlankPath,
}

impl fmt::Display for WorkspaceRegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankPath => write!(f, "a Workspace path cannot be blank"),
        }
    }
}

impl std::error::Error for WorkspaceRegistrationError {}

/// Why a retirement was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceRetirementError {
    /// The Workspace is already retired; retirement is terminal.
    AlreadyRetired,
}

impl fmt::Display for WorkspaceRetirementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRetired => write!(f, "the Workspace is already retired"),
        }
    }
}

impl std::error::Error for WorkspaceRetirementError {}

/// One validated registration: the Project and filesystem path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRegistration {
    project_id: ProjectId,
    path: String,
    is_seed: bool,
}

impl WorkspaceRegistration {
    /// Validate a registration. Surrounding whitespace is not part of
    /// the path.
    pub fn new(
        project_id: ProjectId,
        path: &str,
        is_seed: bool,
    ) -> Result<Self, WorkspaceRegistrationError> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(WorkspaceRegistrationError::BlankPath);
        }
        Ok(Self {
            project_id,
            path: trimmed.to_owned(),
            is_seed,
        })
    }

    /// The owning Project.
    pub fn project_id(&self) -> ProjectId {
        self.project_id
    }

    /// The registered filesystem path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Whether this Workspace is the Project's Seed.
    pub fn is_seed(&self) -> bool {
        self.is_seed
    }
}

/// The git facts observation reads without mutating the repository.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceObservation {
    repository_identity: Option<String>,
    branch: Option<String>,
    head: Option<String>,
    working_tree_clean: Option<bool>,
    unique_unlanded_commits: Option<bool>,
    lane_assignment: Option<u64>,
}

impl WorkspaceObservation {
    /// An empty snapshot before the first observation.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Apply a fresh git read. Lane assignment comes from durable
    /// storage, not from git; the unlanded guard is `None` when the
    /// observer could not decide it.
    pub fn apply_git_read(
        &mut self,
        repository_identity: Option<String>,
        branch: Option<String>,
        head: Option<String>,
        working_tree_clean: bool,
        unique_unlanded_commits: Option<bool>,
        lane_assignment: Option<u64>,
    ) {
        self.repository_identity = repository_identity;
        self.branch = branch;
        self.head = head;
        self.working_tree_clean = Some(working_tree_clean);
        self.unique_unlanded_commits = unique_unlanded_commits;
        self.lane_assignment = lane_assignment;
    }

    /// Clear git facts when the path is missing.
    pub fn clear_git_read(&mut self, lane_assignment: Option<u64>) {
        self.repository_identity = None;
        self.branch = None;
        self.head = None;
        self.working_tree_clean = None;
        self.unique_unlanded_commits = None;
        self.lane_assignment = lane_assignment;
    }

    /// Whether a git read ever applied on a present path.
    pub fn observed_present(&self) -> bool {
        self.working_tree_clean.is_some()
    }

    /// The observed repository identity, when present.
    pub fn repository_identity(&self) -> Option<&str> {
        self.repository_identity.as_deref()
    }

    /// The checked-out branch, when observed.
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// The HEAD commit, when observed.
    pub fn head(&self) -> Option<&str> {
        self.head.as_deref()
    }

    /// Whether the working tree is clean, when observed.
    pub fn working_tree_clean(&self) -> Option<bool> {
        self.working_tree_clean
    }

    /// Whether the Workspace holds unique unlanded commits, when the
    /// observer could decide it.
    pub fn unique_unlanded_commits(&self) -> Option<bool> {
        self.unique_unlanded_commits
    }

    /// The Lane this Workspace is assigned to, when any.
    pub fn lane_assignment(&self) -> Option<u64> {
        self.lane_assignment
    }
}

/// One Workspace aggregate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    id: WorkspaceId,
    registration: WorkspaceRegistration,
    retired: bool,
    lane_id: Option<u64>,
    health: WorkspaceHealth,
    observation: WorkspaceObservation,
    version: u64,
}

impl Workspace {
    /// A freshly registered Workspace at version 1 with missing
    /// health until the first observation.
    pub fn new(id: WorkspaceId, registration: WorkspaceRegistration) -> Self {
        Self {
            id,
            registration,
            retired: false,
            lane_id: None,
            health: WorkspaceHealth::Missing,
            observation: WorkspaceObservation::empty(),
            version: 1,
        }
    }

    /// Rehydrate a stored Workspace exactly as recorded.
    pub fn restore(
        id: WorkspaceId,
        registration: WorkspaceRegistration,
        retired: bool,
        lane_id: Option<u64>,
        health: WorkspaceHealth,
        observation: WorkspaceObservation,
        version: u64,
    ) -> Self {
        Self {
            id,
            registration,
            retired,
            lane_id,
            health,
            observation,
            version,
        }
    }

    /// The immutable identity.
    pub fn id(&self) -> WorkspaceId {
        self.id
    }

    /// The validated registration.
    pub fn registration(&self) -> &WorkspaceRegistration {
        &self.registration
    }

    /// Whether the operator retired this Workspace.
    pub fn is_retired(&self) -> bool {
        self.retired
    }

    /// The Lane assignment, when any.
    pub fn lane_id(&self) -> Option<u64> {
        self.lane_id
    }

    /// The current health.
    pub fn health(&self) -> WorkspaceHealth {
        self.health
    }

    /// The last observation snapshot.
    pub fn observation(&self) -> &WorkspaceObservation {
        &self.observation
    }

    /// The aggregate version for optimistic checks.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Apply one observation read. Returns the previous and new
    /// health when a transition occurred.
    pub fn observe(
        &mut self,
        present: bool,
        repository_identity: Option<String>,
        branch: Option<String>,
        head: Option<String>,
        working_tree_clean: bool,
        unique_unlanded_commits: Option<bool>,
    ) -> Option<(WorkspaceHealth, WorkspaceHealth)> {
        let lane_assignment = self.lane_id;
        if present {
            self.observation.apply_git_read(
                repository_identity,
                branch,
                head,
                working_tree_clean,
                unique_unlanded_commits,
                lane_assignment,
            );
        } else {
            self.observation.clear_git_read(lane_assignment);
        }
        let previous = self.health;
        let next = compute_health(WorkspaceHealthInputs {
            retired: self.retired,
            present,
            lane_assigned: self.lane_id.is_some(),
            working_tree_clean: if present { working_tree_clean } else { true },
        });
        self.health = next;
        self.version += 1;
        if previous == next {
            None
        } else {
            Some((previous, next))
        }
    }

    /// Evaluate reuse from the recorded facts (DR-LW-06): the
    /// observed tree state, the Lane assignment, retirement, and the
    /// unlanded-commit guard. An undecided guard counts as unlanded
    /// work, so reuse stays refused until git proves the tree landed.
    pub fn reuse_evaluation(&self) -> ReuseEvaluation {
        evaluate_reuse(ReuseInputs {
            retired: self.retired,
            present: self.observation.observed_present(),
            lane_assigned: self.lane_id.is_some(),
            working_tree_clean: self.observation.working_tree_clean().unwrap_or(false),
            unique_unlanded_commits: self.observation.unique_unlanded_commits().unwrap_or(true),
        })
    }

    /// Retire the Workspace: the explicit operator action that ends
    /// reuse while preserving every recorded fact. Retirement is
    /// terminal, so a second retirement is refused rather than
    /// absorbed. Returns the health transition when one occurred.
    pub fn retire(
        &mut self,
    ) -> Result<Option<(WorkspaceHealth, WorkspaceHealth)>, WorkspaceRetirementError> {
        if self.retired {
            return Err(WorkspaceRetirementError::AlreadyRetired);
        }
        self.retired = true;
        let previous = self.health;
        self.health = WorkspaceHealth::Retired;
        self.version += 1;
        if previous == WorkspaceHealth::Retired {
            Ok(None)
        } else {
            Ok(Some((previous, WorkspaceHealth::Retired)))
        }
    }
}

#[cfg(test)]
mod workspace_health {
    use super::{WorkspaceHealth, WorkspaceHealthInputs, compute_health};

    #[test]
    fn retired_wins_over_every_other_signal() {
        for present in [true, false] {
            for lane_assigned in [true, false] {
                for working_tree_clean in [true, false] {
                    assert_eq!(
                        compute_health(WorkspaceHealthInputs {
                            retired: true,
                            present,
                            lane_assigned,
                            working_tree_clean,
                        }),
                        WorkspaceHealth::Retired,
                        "retired must dominate every combination"
                    );
                }
            }
        }
    }

    #[test]
    fn missing_wins_when_the_path_is_not_present() {
        assert_eq!(
            compute_health(WorkspaceHealthInputs {
                retired: false,
                present: false,
                lane_assigned: false,
                working_tree_clean: true,
            }),
            WorkspaceHealth::Missing
        );
        assert_eq!(
            compute_health(WorkspaceHealthInputs {
                retired: false,
                present: false,
                lane_assigned: true,
                working_tree_clean: false,
            }),
            WorkspaceHealth::Missing,
            "a missing path stays missing even when a Lane is recorded"
        );
    }

    #[test]
    fn assigned_wins_over_dirty_and_available() {
        assert_eq!(
            compute_health(WorkspaceHealthInputs {
                retired: false,
                present: true,
                lane_assigned: true,
                working_tree_clean: true,
            }),
            WorkspaceHealth::Assigned
        );
        assert_eq!(
            compute_health(WorkspaceHealthInputs {
                retired: false,
                present: true,
                lane_assigned: true,
                working_tree_clean: false,
            }),
            WorkspaceHealth::Assigned,
            "Lane assignment dominates a dirty tree"
        );
    }

    #[test]
    fn dirty_applies_only_when_unassigned_and_present() {
        assert_eq!(
            compute_health(WorkspaceHealthInputs {
                retired: false,
                present: true,
                lane_assigned: false,
                working_tree_clean: false,
            }),
            WorkspaceHealth::Dirty
        );
        assert_eq!(
            compute_health(WorkspaceHealthInputs {
                retired: false,
                present: true,
                lane_assigned: false,
                working_tree_clean: true,
            }),
            WorkspaceHealth::Available
        );
    }

    #[test]
    fn health_wire_names_round_trip() {
        for health in [
            WorkspaceHealth::Available,
            WorkspaceHealth::Assigned,
            WorkspaceHealth::Dirty,
            WorkspaceHealth::Missing,
            WorkspaceHealth::Retired,
        ] {
            assert_eq!(WorkspaceHealth::parse(health.as_str()), Some(health));
        }
        assert_eq!(WorkspaceHealth::parse("ghost"), None);
    }
}

/// The inputs the reuse rule needs: the durable lifecycle flags, the
/// observed tree state, and the unlanded-commit guard's verdict
/// (DR-LW-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReuseInputs {
    /// Retirement removes a Workspace from reuse for good.
    pub retired: bool,
    /// Whether the last observation found the path present. An
    /// absent or never-observed tree satisfies no condition.
    pub present: bool,
    /// Whether an active Lane assignment holds the Workspace.
    pub lane_assigned: bool,
    /// Whether the observed working tree is clean.
    pub working_tree_clean: bool,
    /// Whether the Workspace holds unique unlanded commits.
    pub unique_unlanded_commits: bool,
}

/// The reuse verdict with every named condition evaluated and
/// reported (DR-LW-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReuseEvaluation {
    reusable: bool,
    clean: bool,
    unassigned: bool,
    free_of_unlanded_commits: bool,
}

impl ReuseEvaluation {
    /// Whether the Workspace may be reused: every condition holds
    /// and the record is present and not retired.
    pub fn reusable(&self) -> bool {
        self.reusable
    }

    /// Whether the working tree is clean on a present path.
    pub fn clean(&self) -> bool {
        self.clean
    }

    /// Whether no Lane assignment holds the Workspace.
    pub fn unassigned(&self) -> bool {
        self.unassigned
    }

    /// Whether the Workspace is free of unique unlanded commits.
    pub fn free_of_unlanded_commits(&self) -> bool {
        self.free_of_unlanded_commits
    }
}

/// Evaluate reuse (DR-LW-06): a Workspace is reusable only when
/// clean, unassigned, and free of unique unlanded commits. Each
/// condition is evaluated whatever the others say, and a retired or
/// missing record is never reusable.
pub fn evaluate_reuse(inputs: ReuseInputs) -> ReuseEvaluation {
    let clean = inputs.present && inputs.working_tree_clean;
    let unassigned = !inputs.lane_assigned;
    let free_of_unlanded_commits = inputs.present && !inputs.unique_unlanded_commits;
    let reusable = clean && unassigned && free_of_unlanded_commits && !inputs.retired;
    ReuseEvaluation {
        reusable,
        clean,
        unassigned,
        free_of_unlanded_commits,
    }
}

#[cfg(test)]
mod reuse_rules {
    use super::{ReuseInputs, evaluate_reuse};

    fn inputs() -> ReuseInputs {
        ReuseInputs {
            retired: false,
            present: true,
            lane_assigned: false,
            working_tree_clean: true,
            unique_unlanded_commits: false,
        }
    }

    #[test]
    fn clean_unassigned_and_landed_is_reusable() {
        let verdict = evaluate_reuse(inputs());

        assert!(verdict.reusable());
        assert!(verdict.clean());
        assert!(verdict.unassigned());
        assert!(verdict.free_of_unlanded_commits());
    }

    #[test]
    fn a_dirty_tree_blocks_reuse_and_only_the_clean_condition() {
        let verdict = evaluate_reuse(ReuseInputs {
            working_tree_clean: false,
            ..inputs()
        });

        assert!(!verdict.reusable());
        assert!(!verdict.clean(), "a dirty tree fails the clean condition");
        assert!(verdict.unassigned(), "the other conditions still report");
        assert!(
            verdict.free_of_unlanded_commits(),
            "the other conditions still report"
        );
    }

    #[test]
    fn a_lane_assignment_blocks_reuse_and_only_the_assignment_condition() {
        let verdict = evaluate_reuse(ReuseInputs {
            lane_assigned: true,
            ..inputs()
        });

        assert!(!verdict.reusable());
        assert!(verdict.clean(), "the other conditions still report");
        assert!(!verdict.unassigned());
        assert!(
            verdict.free_of_unlanded_commits(),
            "the other conditions still report"
        );
    }

    #[test]
    fn unique_unlanded_commits_block_reuse_and_only_their_condition() {
        let verdict = evaluate_reuse(ReuseInputs {
            unique_unlanded_commits: true,
            ..inputs()
        });

        assert!(!verdict.reusable());
        assert!(verdict.clean(), "the other conditions still report");
        assert!(verdict.unassigned(), "the other conditions still report");
        assert!(!verdict.free_of_unlanded_commits());
    }

    #[test]
    fn every_failing_condition_is_reported_together() {
        let verdict = evaluate_reuse(ReuseInputs {
            lane_assigned: true,
            working_tree_clean: false,
            unique_unlanded_commits: true,
            ..inputs()
        });

        assert!(!verdict.reusable());
        assert!(!verdict.clean());
        assert!(!verdict.unassigned());
        assert!(!verdict.free_of_unlanded_commits());
    }

    #[test]
    fn a_retired_workspace_is_never_reusable_whatever_the_conditions() {
        let verdict = evaluate_reuse(ReuseInputs {
            retired: true,
            ..inputs()
        });

        assert!(
            !verdict.reusable(),
            "retirement must dominate every satisfied condition"
        );
    }

    #[test]
    fn a_missing_workspace_satisfies_no_condition() {
        let verdict = evaluate_reuse(ReuseInputs {
            present: false,
            working_tree_clean: true,
            unique_unlanded_commits: false,
            ..inputs()
        });

        assert!(!verdict.reusable());
        assert!(
            !verdict.clean(),
            "an absent tree cannot vouch for cleanliness"
        );
        assert!(verdict.unassigned(), "assignment still reports");
        assert!(
            !verdict.free_of_unlanded_commits(),
            "an absent tree cannot vouch for landed commits"
        );
    }

    #[test]
    fn an_unobserved_workspace_is_not_reusable() {
        use super::super::{Workspace, WorkspaceId, WorkspaceRegistration};
        use crate::project::ProjectId;

        let workspace = Workspace::new(
            WorkspaceId::new(1),
            WorkspaceRegistration::new(ProjectId::new(1), "/workspaces/core", false)
                .expect("the registration validates"),
        );

        let verdict = workspace.reuse_evaluation();

        assert!(
            !verdict.reusable(),
            "reuse stays refused until git proves the tree landed"
        );
        assert!(!verdict.free_of_unlanded_commits());
    }
}

#[cfg(test)]
mod tests {
    use crate::project::ProjectId;

    use super::{
        Workspace, WorkspaceHealth, WorkspaceId, WorkspaceObservation, WorkspaceRegistration,
        WorkspaceRegistrationError, WorkspaceRetirementError,
    };

    fn registration(path: &str) -> WorkspaceRegistration {
        WorkspaceRegistration::new(ProjectId::new(1), path, false)
            .expect("a well-formed registration is accepted")
    }

    #[test]
    fn registration_refuses_a_blank_path() {
        assert_eq!(
            WorkspaceRegistration::new(ProjectId::new(1), " ", false),
            Err(WorkspaceRegistrationError::BlankPath)
        );
    }

    #[test]
    fn registration_trims_surrounding_whitespace() {
        let validated =
            WorkspaceRegistration::new(ProjectId::new(2), " /workspaces/core.seed ", true)
                .expect("the path carries text");

        assert_eq!(validated.path(), "/workspaces/core.seed");
        assert!(validated.is_seed());
    }

    #[test]
    fn a_fresh_workspace_starts_missing_at_version_one() {
        let workspace = Workspace::new(WorkspaceId::new(3), registration("/workspaces/core"));

        assert_eq!(workspace.health(), WorkspaceHealth::Missing);
        assert_eq!(workspace.version(), 1);
        assert_eq!(workspace.observation(), &WorkspaceObservation::empty());
    }

    #[test]
    fn observing_a_clean_workspace_moves_to_available() {
        let mut workspace = Workspace::new(WorkspaceId::new(1), registration("/workspaces/core"));

        let transition = workspace.observe(
            true,
            Some("identity".to_owned()),
            Some("main".to_owned()),
            Some("abc123".to_owned()),
            true,
            Some(false),
        );

        assert_eq!(
            transition,
            Some((WorkspaceHealth::Missing, WorkspaceHealth::Available))
        );
        assert_eq!(workspace.health(), WorkspaceHealth::Available);
        assert_eq!(workspace.observation().branch(), Some("main"));
        assert_eq!(workspace.observation().head(), Some("abc123"));
        assert_eq!(workspace.version(), 2);
    }

    #[test]
    fn observing_a_dirty_workspace_records_dirty_health() {
        let mut workspace = Workspace::new(WorkspaceId::new(1), registration("/workspaces/core"));
        workspace
            .observe(
                true,
                Some("identity".to_owned()),
                Some("feature".to_owned()),
                Some("def456".to_owned()),
                false,
                Some(false),
            )
            .expect("the first observation transitions");

        assert_eq!(workspace.health(), WorkspaceHealth::Dirty);
    }

    #[test]
    fn retiring_moves_health_to_retired_and_preserves_the_record() {
        let mut workspace = Workspace::new(WorkspaceId::new(1), registration("/workspaces/core"));
        workspace
            .observe(
                true,
                Some("identity".to_owned()),
                Some("main".to_owned()),
                Some("abc123".to_owned()),
                true,
                Some(false),
            )
            .expect("the first observation transitions");

        let transition = workspace.retire().expect("an active Workspace retires");

        assert_eq!(
            transition,
            Some((WorkspaceHealth::Available, WorkspaceHealth::Retired))
        );
        assert_eq!(workspace.health(), WorkspaceHealth::Retired);
        assert!(workspace.is_retired());
        assert_eq!(
            workspace.observation().head(),
            Some("abc123"),
            "retirement preserves every observed fact"
        );
        assert_eq!(workspace.version(), 3);
    }

    #[test]
    fn retiring_twice_is_refused_and_changes_nothing() {
        let mut workspace = Workspace::new(WorkspaceId::new(1), registration("/workspaces/core"));
        workspace.retire().expect("the first retirement applies");

        assert_eq!(
            workspace.retire(),
            Err(WorkspaceRetirementError::AlreadyRetired)
        );
        assert_eq!(workspace.health(), WorkspaceHealth::Retired);
        assert_eq!(workspace.version(), 2, "the refusal changed nothing");
    }

    #[test]
    fn observation_after_retirement_keeps_retired_health() {
        let mut workspace = Workspace::new(WorkspaceId::new(1), registration("/workspaces/core"));
        workspace.retire().expect("the retirement applies");

        let transition = workspace.observe(
            true,
            Some("identity".to_owned()),
            Some("main".to_owned()),
            Some("abc123".to_owned()),
            true,
            Some(false),
        );

        assert_eq!(transition, None, "retired dominates every observation");
        assert_eq!(workspace.health(), WorkspaceHealth::Retired);
    }

    #[test]
    fn observing_a_missing_path_clears_git_facts() {
        let mut workspace = Workspace::new(WorkspaceId::new(1), registration("/workspaces/core"));
        workspace
            .observe(
                true,
                Some("identity".to_owned()),
                Some("main".to_owned()),
                Some("abc123".to_owned()),
                true,
                Some(false),
            )
            .expect("the first observation transitions");

        let transition = workspace.observe(false, None, None, None, true, None);

        assert_eq!(
            transition,
            Some((WorkspaceHealth::Available, WorkspaceHealth::Missing))
        );
        assert_eq!(workspace.observation().branch(), None);
        assert_eq!(workspace.observation().unique_unlanded_commits(), None);
        assert_eq!(workspace.health(), WorkspaceHealth::Missing);
    }

    #[test]
    fn the_observation_records_the_unlanded_commit_guard() {
        let mut workspace = Workspace::new(WorkspaceId::new(1), registration("/workspaces/core"));
        workspace
            .observe(
                true,
                Some("identity".to_owned()),
                Some("feature".to_owned()),
                Some("def456".to_owned()),
                true,
                Some(true),
            )
            .expect("the first observation transitions");

        assert_eq!(
            workspace.observation().unique_unlanded_commits(),
            Some(true)
        );
    }

    #[test]
    fn reuse_evaluation_assembles_every_condition_from_stored_facts() {
        let mut reusable = Workspace::new(WorkspaceId::new(1), registration("/workspaces/core"));
        reusable
            .observe(
                true,
                Some("identity".to_owned()),
                Some("main".to_owned()),
                Some("abc123".to_owned()),
                true,
                Some(false),
            )
            .expect("the observation transitions");

        let verdict = reusable.reuse_evaluation();
        assert!(verdict.reusable());
        assert!(verdict.clean());
        assert!(verdict.unassigned());
        assert!(verdict.free_of_unlanded_commits());

        let mut unlanded = Workspace::new(WorkspaceId::new(2), registration("/workspaces/edge"));
        unlanded
            .observe(
                true,
                Some("identity".to_owned()),
                Some("feature".to_owned()),
                Some("def456".to_owned()),
                true,
                Some(true),
            )
            .expect("the observation transitions");
        let verdict = unlanded.reuse_evaluation();
        assert!(!verdict.reusable());
        assert!(verdict.clean(), "the tree itself is clean");
        assert!(verdict.unassigned());
        assert!(!verdict.free_of_unlanded_commits());

        let mut retired = Workspace::new(WorkspaceId::new(3), registration("/workspaces/old"));
        retired
            .observe(
                true,
                Some("identity".to_owned()),
                Some("main".to_owned()),
                Some("abc123".to_owned()),
                true,
                Some(false),
            )
            .expect("the observation transitions");
        retired.retire().expect("the retirement applies");
        let verdict = retired.reuse_evaluation();
        assert!(!verdict.reusable(), "a retired record never reuses");
    }
}
