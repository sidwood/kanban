//! Guarded branch-clone rules (KAN-S6-US4). Creating and removing
//! branch clones goes through the fleet's `git bc-add` family — the
//! only sanctioned clone mechanism (DR-LW-09) — and Kanban refuses
//! conflicting paths, branches, and Lane assignments before anything
//! is invoked (DR-LW-10). The rules here are pure: they decide, from
//! registered Workspace facts and whatever filesystem facts a caller
//! hands them as values, whether a guarded command may proceed, and
//! they name the conflict when it may not.

use std::fmt;

use crate::workspace::Workspace;

/// The facts the clone guard reads from one registered Workspace: the
/// identity, the registered path, the Seed marker, retirement, the
/// last observed branch, and any Lane assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCloneFacts {
    pub workspace_id: u64,
    pub path: String,
    pub is_seed: bool,
    pub retired: bool,
    pub branch: Option<String>,
    pub lane_assignment: Option<u64>,
}

impl WorkspaceCloneFacts {
    /// Read the guard-relevant facts off one Workspace aggregate.
    pub fn from_workspace(workspace: &Workspace) -> Self {
        Self {
            workspace_id: workspace.id().value(),
            path: workspace.registration().path().to_owned(),
            is_seed: workspace.registration().is_seed(),
            retired: workspace.is_retired(),
            branch: workspace.observation().branch().map(str::to_owned),
            lane_assignment: workspace.lane_id(),
        }
    }
}

/// Why a guarded clone command was refused. Every variant names the
/// conflict it refused (DR-LW-10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneConflict {
    /// The target path is the Project's Seed Workspace path, which a
    /// branch clone never occupies.
    SeedPath { path: String },
    /// The target path is already a registered Workspace.
    PathTaken { path: String, workspace_id: u64 },
    /// The target path already exists on the filesystem and no
    /// registered Workspace records it, so a create would record a
    /// clone Kanban did not place there.
    TargetExists { path: String },
    /// Another Workspace has the requested branch checked out.
    BranchCheckedOut {
        branch: String,
        workspace_id: u64,
        path: String,
    },
    /// The Workspace is claimed by an active Lane.
    LaneAssigned {
        workspace_id: u64,
        path: String,
        lane_id: u64,
    },
    /// The Workspace is the Seed, which is never a branch clone.
    SeedWorkspace { workspace_id: u64, path: String },
}

impl CloneConflict {
    /// The stable reason code the timeline records beside the facts.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::SeedPath { .. } => "seed_path",
            Self::PathTaken { .. } => "path_taken",
            Self::TargetExists { .. } => "target_exists",
            Self::BranchCheckedOut { .. } => "branch_checked_out",
            Self::LaneAssigned { .. } => "lane_assigned",
            Self::SeedWorkspace { .. } => "seed_workspace",
        }
    }
}

impl fmt::Display for CloneConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SeedPath { path } => {
                write!(
                    f,
                    "the path `{path}` is the Seed Workspace path and never holds a branch clone"
                )
            }
            Self::PathTaken { path, workspace_id } => {
                write!(
                    f,
                    "the clone path `{path}` is already registered as Workspace {workspace_id}"
                )
            }
            Self::TargetExists { path } => {
                write!(
                    f,
                    "the clone target `{path}` already exists and is registered to no Workspace"
                )
            }
            Self::BranchCheckedOut {
                branch,
                workspace_id,
                path,
            } => write!(
                f,
                "the branch `{branch}` is already checked out by Workspace {workspace_id} at `{path}`"
            ),
            Self::LaneAssigned {
                workspace_id,
                path,
                lane_id,
            } => write!(
                f,
                "Workspace {workspace_id} at `{path}` is assigned to Lane {lane_id}; release the Lane before removing its clone"
            ),
            Self::SeedWorkspace { workspace_id, path } => write!(
                f,
                "Workspace {workspace_id} at `{path}` is the Seed Workspace, which is never a branch clone"
            ),
        }
    }
}

impl std::error::Error for CloneConflict {}

/// Why a guarded clone request could not even be examined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneTargetError {
    /// The path holds nothing but whitespace.
    BlankPath,
    /// The branch holds nothing but whitespace.
    BlankBranch,
}

impl fmt::Display for CloneTargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankPath => write!(f, "a clone path cannot be blank"),
            Self::BlankBranch => write!(f, "a clone branch cannot be blank"),
        }
    }
}

impl std::error::Error for CloneTargetError {}

/// Validate a guarded create's target. Surrounding whitespace is not
/// part of either value.
pub fn validate_clone_target(
    path: &str,
    branch: &str,
) -> Result<(String, String), CloneTargetError> {
    let trimmed_path = path.trim();
    if trimmed_path.is_empty() {
        return Err(CloneTargetError::BlankPath);
    }
    let trimmed_branch = branch.trim();
    if trimmed_branch.is_empty() {
        return Err(CloneTargetError::BlankBranch);
    }
    Ok((trimmed_path.to_owned(), trimmed_branch.to_owned()))
}

/// The form a clone path is judged by: not its spelling but the
/// directory it names. A trailing separator, a doubled separator, and
/// `.` or `..` segments are spelling, and none of them may steer a
/// guarded request past a refusal. The form is lexical — the guard is
/// pure and reads no filesystem — so `..` pops toward the anchor and
/// stops there.
fn normalised_path_form(path: &str) -> String {
    let trimmed = path.trim();
    let anchored = trimmed.starts_with('/');
    let mut segments: Vec<&str> = Vec::new();
    for segment in trimmed.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    let joined = segments.join("/");
    if anchored {
        format!("/{joined}")
    } else {
        joined
    }
}

/// Decide whether a guarded create may proceed (DR-LW-10). The path is
/// judged first — against the Project's declared Seed path, then every
/// registered Workspace, then the filesystem itself: `target_exists`
/// arrives as a fact because the guard is pure and reads no disk, and
/// a target that already exists records a creation Kanban did not
/// perform — the registered conflicts are still judged before it so a
/// known holder keeps being named. The branch is judged last, against
/// every non-retired Workspace's observed checkout: a retired record
/// keeps history, not a live execution slot, so it blocks no branch.
/// Paths are judged by their normal form on both sides, so an
/// equivalent spelling of a refused path is refused too. The first
/// conflict wins and names itself.
pub fn clone_create_conflict(
    path: &str,
    branch: &str,
    seed_path: &str,
    workspaces: &[WorkspaceCloneFacts],
    target_exists: bool,
) -> Option<CloneConflict> {
    let requested = normalised_path_form(path);
    if requested == normalised_path_form(seed_path)
        || workspaces
            .iter()
            .any(|facts| facts.is_seed && normalised_path_form(&facts.path) == requested)
    {
        return Some(CloneConflict::SeedPath {
            path: path.to_owned(),
        });
    }
    if let Some(holder) = workspaces
        .iter()
        .find(|facts| normalised_path_form(&facts.path) == requested)
    {
        return Some(CloneConflict::PathTaken {
            path: path.to_owned(),
            workspace_id: holder.workspace_id,
        });
    }
    if target_exists {
        return Some(CloneConflict::TargetExists {
            path: path.to_owned(),
        });
    }
    let holder = workspaces
        .iter()
        .find(|facts| !facts.retired && facts.branch.as_deref() == Some(branch.trim()))?;
    Some(CloneConflict::BranchCheckedOut {
        branch: branch.trim().to_owned(),
        workspace_id: holder.workspace_id,
        path: holder.path.clone(),
    })
}

/// Decide whether a guarded remove of `target` may proceed (DR-LW-10).
/// The Seed is never a branch clone, and a Workspace an active Lane
/// claims is never removed underneath it. A retired record blocks
/// nothing: removal is the operator's explicit action and the record
/// survives it (DR-LW-11).
pub fn clone_remove_conflict(target: &WorkspaceCloneFacts) -> Option<CloneConflict> {
    if target.is_seed {
        return Some(CloneConflict::SeedWorkspace {
            workspace_id: target.workspace_id,
            path: target.path.clone(),
        });
    }
    if let Some(lane_id) = target.lane_assignment {
        return Some(CloneConflict::LaneAssigned {
            workspace_id: target.workspace_id,
            path: target.path.clone(),
            lane_id,
        });
    }
    None
}

#[cfg(test)]
mod clone_guard_rules {
    use super::{
        CloneConflict, CloneTargetError, WorkspaceCloneFacts, clone_create_conflict,
        clone_remove_conflict, validate_clone_target,
    };

    fn facts(workspace_id: u64, path: &str) -> WorkspaceCloneFacts {
        WorkspaceCloneFacts {
            workspace_id,
            path: path.to_owned(),
            is_seed: false,
            retired: false,
            branch: None,
            lane_assignment: None,
        }
    }

    fn seed(workspace_id: u64, path: &str) -> WorkspaceCloneFacts {
        WorkspaceCloneFacts {
            is_seed: true,
            ..facts(workspace_id, path)
        }
    }

    fn on_branch(mut workspace: WorkspaceCloneFacts, branch: &str) -> WorkspaceCloneFacts {
        workspace.branch = Some(branch.to_owned());
        workspace
    }

    fn claimed_by(mut workspace: WorkspaceCloneFacts, lane_id: u64) -> WorkspaceCloneFacts {
        workspace.lane_assignment = Some(lane_id);
        workspace
    }

    fn retired(mut workspace: WorkspaceCloneFacts) -> WorkspaceCloneFacts {
        workspace.retired = true;
        workspace
    }

    const SEED_PATH: &str = "/workspaces/kanban.seed";

    /// Spellings that name the Seed path without spelling it plainly:
    /// a trailing separator, a doubled separator, a `.` segment, a
    /// `..` segment, and a `..` that runs past the anchor.
    const EQUIVALENT_SEED_FORMS: [&str; 5] = [
        "/workspaces/kanban.seed/",
        "/workspaces//kanban.seed",
        "/workspaces/./kanban.seed",
        "/workspaces/clone/../kanban.seed",
        "/../workspaces/kanban.seed",
    ];

    /// The same spellings of one registered clone path.
    const EQUIVALENT_CLONE_FORMS: [&str; 5] = [
        "/workspaces/kanban.fleet-t31/",
        "/workspaces//kanban.fleet-t31",
        "/workspaces/./kanban.fleet-t31",
        "/workspaces/clone/../kanban.fleet-t31",
        "/../workspaces/kanban.fleet-t31",
    ];

    #[test]
    fn a_clean_target_conflicts_with_nothing() {
        let registered = vec![seed(1, SEED_PATH), facts(2, "/workspaces/kanban.other")];

        assert_eq!(
            clone_create_conflict(
                "/workspaces/kanban.fleet-t34",
                "fleet/kan-t34",
                SEED_PATH,
                &registered,
                false,
            ),
            None,
            "a fresh path on a fresh branch is free"
        );
    }

    #[test]
    fn equivalent_spellings_of_a_free_path_conflict_with_nothing() {
        let registered = vec![seed(1, SEED_PATH), facts(2, "/workspaces/kanban.fleet-t31")];

        for path in [
            "/workspaces/kanban.fleet-t34/",
            "/workspaces//kanban.fleet-t34",
            "/workspaces/./kanban.fleet-t34",
        ] {
            assert_eq!(
                clone_create_conflict(path, "fleet/kan-t34", SEED_PATH, &registered, false),
                None,
                "`{path}` names no registered path, whatever its spelling"
            );
        }
    }

    #[test]
    fn the_declared_seed_path_is_refused_even_before_registration() {
        let conflict = clone_create_conflict(SEED_PATH, "fleet/kan-t34", SEED_PATH, &[], false)
            .expect("the Seed path never holds a branch clone");

        assert_eq!(
            conflict,
            CloneConflict::SeedPath {
                path: SEED_PATH.to_owned(),
            }
        );
        assert_eq!(conflict.reason(), "seed_path");
        assert!(conflict.to_string().contains(SEED_PATH));
    }

    #[test]
    fn a_registered_seed_workspace_path_is_the_seed_conflict() {
        let registered = vec![seed(1, SEED_PATH)];

        let conflict =
            clone_create_conflict(SEED_PATH, "fleet/kan-t34", SEED_PATH, &registered, false)
                .expect("the Seed is named as itself, not as a taken path");

        assert_eq!(
            conflict,
            CloneConflict::SeedPath {
                path: SEED_PATH.to_owned()
            }
        );
    }

    #[test]
    fn equivalent_spellings_of_the_declared_seed_path_are_refused() {
        for path in EQUIVALENT_SEED_FORMS {
            assert_eq!(
                clone_create_conflict(path, "fleet/kan-t34", SEED_PATH, &[], false),
                Some(CloneConflict::SeedPath {
                    path: path.to_owned(),
                }),
                "`{path}` names the declared Seed path `{SEED_PATH}`"
            );
        }
    }

    #[test]
    fn equivalent_spellings_of_a_registered_seed_workspace_are_refused() {
        let registered = vec![seed(1, SEED_PATH), facts(2, "/workspaces/kanban.other")];

        for path in EQUIVALENT_SEED_FORMS {
            assert_eq!(
                clone_create_conflict(
                    path,
                    "fleet/kan-t34",
                    "/declared/kanban.seed",
                    &registered,
                    false
                ),
                Some(CloneConflict::SeedPath {
                    path: path.to_owned(),
                }),
                "`{path}` names registered Seed Workspace 1"
            );
        }
    }

    #[test]
    fn a_path_another_workspace_holds_is_refused_and_named() {
        let registered = vec![facts(3, "/workspaces/kanban.fleet-t34")];

        let conflict = clone_create_conflict(
            "/workspaces/kanban.fleet-t34",
            "fleet/kan-t35",
            SEED_PATH,
            &registered,
            false,
        )
        .expect("a registered path is a conflict");

        assert_eq!(
            conflict,
            CloneConflict::PathTaken {
                path: "/workspaces/kanban.fleet-t34".to_owned(),
                workspace_id: 3,
            }
        );
        assert_eq!(conflict.reason(), "path_taken");
        assert!(
            conflict
                .to_string()
                .contains("already registered as Workspace 3"),
            "the refusal names the holder: {conflict}"
        );
    }

    #[test]
    fn a_retired_workspace_still_holds_its_registered_path() {
        let registered = vec![retired(facts(4, "/workspaces/kanban.old"))];

        let conflict = clone_create_conflict(
            "/workspaces/kanban.old",
            "fleet/kan-t34",
            SEED_PATH,
            &registered,
            false,
        )
        .expect("the preserved record still owns its path");

        assert_eq!(
            conflict,
            CloneConflict::PathTaken {
                path: "/workspaces/kanban.old".to_owned(),
                workspace_id: 4,
            }
        );
    }

    #[test]
    fn equivalent_spellings_of_a_registered_path_are_refused_and_named() {
        let registered = vec![facts(5, "/workspaces/kanban.fleet-t31")];

        for path in EQUIVALENT_CLONE_FORMS {
            assert_eq!(
                clone_create_conflict(path, "fleet/kan-t34", SEED_PATH, &registered, false),
                Some(CloneConflict::PathTaken {
                    path: path.to_owned(),
                    workspace_id: 5,
                }),
                "`{path}` names the path registered by Workspace 5"
            );
        }
    }

    #[test]
    fn non_canonical_registered_paths_are_matched_by_their_plain_form() {
        let registered = vec![
            seed(1, "/workspaces/kanban.seed/"),
            facts(2, "/workspaces//kanban.old"),
        ];

        let seed_conflict = clone_create_conflict(
            "/workspaces/kanban.seed",
            "fleet/kan-t34",
            "/declared/kanban.seed",
            &registered,
            false,
        )
        .expect("the registered Seed is judged by its normal form");
        assert_eq!(seed_conflict.reason(), "seed_path");

        let conflict = clone_create_conflict(
            "/workspaces/kanban.old",
            "fleet/kan-t35",
            SEED_PATH,
            &registered,
            false,
        )
        .expect("the registered path is judged by its normal form");
        assert_eq!(
            conflict,
            CloneConflict::PathTaken {
                path: "/workspaces/kanban.old".to_owned(),
                workspace_id: 2,
            }
        );
    }

    #[test]
    fn a_branch_another_workspace_has_checked_out_is_refused_and_named() {
        let registered = vec![
            facts(1, SEED_PATH),
            on_branch(facts(2, "/workspaces/kanban.fleet-t31"), "fleet/kan-t31"),
        ];

        let conflict = clone_create_conflict(
            "/workspaces/kanban.fleet-t34",
            "fleet/kan-t31",
            SEED_PATH,
            &registered,
            false,
        )
        .expect("a checked-out branch is a conflict");

        assert_eq!(
            conflict,
            CloneConflict::BranchCheckedOut {
                branch: "fleet/kan-t31".to_owned(),
                workspace_id: 2,
                path: "/workspaces/kanban.fleet-t31".to_owned(),
            }
        );
        assert_eq!(conflict.reason(), "branch_checked_out");
        assert!(
            conflict.to_string().contains("already checked out"),
            "the refusal names the branch and its holder: {conflict}"
        );
    }

    #[test]
    fn a_retired_workspace_blocks_no_branch() {
        let registered = vec![
            seed(1, SEED_PATH),
            retired(on_branch(
                facts(2, "/workspaces/kanban.gone"),
                "fleet/kan-t30",
            )),
        ];

        assert_eq!(
            clone_create_conflict(
                "/workspaces/kanban.fleet-t34",
                "fleet/kan-t30",
                SEED_PATH,
                &registered,
                false,
            ),
            None,
            "a retired record is history, not a live checkout"
        );
    }

    #[test]
    fn an_unobserved_workspace_blocks_no_branch() {
        let registered = vec![seed(1, SEED_PATH), facts(2, "/workspaces/kanban.blind")];

        assert_eq!(
            clone_create_conflict(
                "/workspaces/kanban.fleet-t34",
                "fleet/kan-t34",
                SEED_PATH,
                &registered,
                false,
            ),
            None,
            "a Workspace with no observed checkout holds no branch"
        );
    }

    #[test]
    fn an_existing_target_no_workspace_records_is_refused_and_named() {
        let conflict = clone_create_conflict(
            "/workspaces/kanban.fleet-t34",
            "fleet/kan-t34",
            SEED_PATH,
            &[],
            true,
        )
        .expect("an occupied target conflicts even when no Workspace records it");

        assert_eq!(
            conflict,
            CloneConflict::TargetExists {
                path: "/workspaces/kanban.fleet-t34".to_owned(),
            }
        );
        assert_eq!(conflict.reason(), "target_exists");
        assert!(
            conflict
                .to_string()
                .contains("/workspaces/kanban.fleet-t34"),
            "the refusal names the target: {conflict}"
        );
        assert!(
            conflict.to_string().contains("registered to no Workspace"),
            "the refusal names the rule: {conflict}"
        );
    }

    #[test]
    fn a_registered_holder_wins_over_a_bare_existing_target() {
        let registered = vec![facts(2, "/workspaces/kanban.fleet-t34")];

        let conflict = clone_create_conflict(
            "/workspaces/kanban.fleet-t34",
            "fleet/kan-t34",
            SEED_PATH,
            &registered,
            true,
        )
        .expect("both conflict; the holder is named, not the bare existence");

        assert_eq!(conflict.reason(), "path_taken");
    }

    #[test]
    fn the_declared_seed_path_wins_over_an_existing_target() {
        let conflict = clone_create_conflict(SEED_PATH, "fleet/kan-t34", SEED_PATH, &[], true)
            .expect("the Seed rule is named, not the bare existence");

        assert_eq!(conflict.reason(), "seed_path");
    }

    #[test]
    fn an_existing_target_wins_over_a_branch_conflict() {
        let registered = vec![
            seed(1, SEED_PATH),
            on_branch(facts(2, "/workspaces/kanban.fleet-t31"), "fleet/kan-t31"),
        ];

        let conflict = clone_create_conflict(
            "/workspaces/kanban.fleet-t34",
            "fleet/kan-t31",
            SEED_PATH,
            &registered,
            true,
        )
        .expect("both conflict; the occupied path is judged first");

        assert_eq!(conflict.reason(), "target_exists");
    }

    #[test]
    fn the_path_conflict_wins_over_the_branch_conflict() {
        let registered = vec![
            on_branch(facts(2, "/workspaces/kanban.fleet-t31"), "fleet/kan-t31"),
            facts(3, "/workspaces/kanban.shared"),
        ];

        let conflict = clone_create_conflict(
            "/workspaces/kanban.shared",
            "fleet/kan-t31",
            SEED_PATH,
            &registered,
            false,
        )
        .expect("both conflict; the path is judged first");

        assert_eq!(conflict.reason(), "path_taken");
    }

    #[test]
    fn removing_a_clone_a_lane_claims_is_refused_and_named() {
        let target = claimed_by(facts(2, "/workspaces/kanban.fleet-t31"), 7);

        let conflict =
            clone_remove_conflict(&target).expect("a claimed Workspace is never removed");

        assert_eq!(
            conflict,
            CloneConflict::LaneAssigned {
                workspace_id: 2,
                path: "/workspaces/kanban.fleet-t31".to_owned(),
                lane_id: 7,
            }
        );
        assert_eq!(conflict.reason(), "lane_assigned");
        assert!(
            conflict.to_string().contains("assigned to Lane 7"),
            "the refusal names the Lane: {conflict}"
        );
    }

    #[test]
    fn removing_the_seed_workspace_is_refused_and_named() {
        let target = seed(1, SEED_PATH);

        let conflict = clone_remove_conflict(&target).expect("the Seed is never a branch clone");

        assert_eq!(
            conflict,
            CloneConflict::SeedWorkspace {
                workspace_id: 1,
                path: SEED_PATH.to_owned(),
            }
        );
        assert_eq!(conflict.reason(), "seed_workspace");
        assert!(
            conflict.to_string().contains("Seed Workspace"),
            "the refusal names the rule: {conflict}"
        );
    }

    #[test]
    fn removing_an_unclaimed_non_seed_clone_conflicts_with_nothing() {
        assert_eq!(
            clone_remove_conflict(&facts(2, "/workspaces/kanban.fleet-t31")),
            None
        );
    }

    #[test]
    fn a_retired_workspace_still_cannot_be_the_seed_or_lane_claimed() {
        assert_eq!(
            clone_remove_conflict(&retired(seed(1, SEED_PATH))).map(|c| c.reason()),
            Some("seed_workspace")
        );
        assert_eq!(
            clone_remove_conflict(&retired(claimed_by(facts(2, "/w"), 3))).map(|c| c.reason()),
            Some("lane_assigned")
        );
    }

    #[test]
    fn a_retired_unclaimed_workspace_may_have_its_clone_removed() {
        assert_eq!(
            clone_remove_conflict(&retired(facts(2, "/workspaces/kanban.done"))),
            None,
            "retirement preserves the record, not the clone"
        );
    }

    #[test]
    fn the_seed_conflict_wins_over_the_lane_conflict() {
        let target = claimed_by(seed(1, SEED_PATH), 4);

        let conflict = clone_remove_conflict(&target).expect("both conflict; the Seed wins");

        assert_eq!(conflict.reason(), "seed_workspace");
    }

    #[test]
    fn blank_targets_are_refused_before_any_conflict_is_read() {
        assert_eq!(
            validate_clone_target("   ", "fleet/kan-t34"),
            Err(CloneTargetError::BlankPath)
        );
        assert_eq!(
            validate_clone_target("/workspaces/kanban.fleet-t34", " "),
            Err(CloneTargetError::BlankBranch)
        );
    }

    #[test]
    fn targets_trim_surrounding_whitespace() {
        let (path, branch) =
            validate_clone_target(" /workspaces/kanban.fleet-t34 ", " fleet/kan-t34 ")
                .expect("a well-formed target validates");

        assert_eq!(path, "/workspaces/kanban.fleet-t34");
        assert_eq!(branch, "fleet/kan-t34");
    }
}
