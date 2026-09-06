//! The fleet branch-clone skill, wrapped (KAN-S6-US4). The `git
//! bc-add` family is the only sanctioned clone mechanism; this
//! adapter invokes it and nothing else, because the guards — the
//! conflict refusal, the ordering, the timeline rows — all live in
//! the application layer, which calls this port only after every
//! precondition has held.

use std::process::Command;

use kanban_app::FleetCloneTool;
use kanban_dto::ApiError;

/// Run the fleet's `git bc-add` and `git bc-rm` skills locally.
#[derive(Debug, Default)]
pub struct LocalFleetCloneTool;

impl LocalFleetCloneTool {
    /// `git bc-add <source> <branch> <target>`: the target is always
    /// explicit, so the guarded path is the path that lands.
    fn add_arguments(source: &str, branch: &str, target: &str) -> Vec<String> {
        vec![source.to_owned(), branch.to_owned(), target.to_owned()]
    }

    /// `git bc-rm <clone-dir> -y`: the confirmation prompt is skipped
    /// because the guarded command already carries the operator's
    /// authority, while `-f` is never passed — the fleet's own
    /// refusal rules against base clones, dirty trees, and unique
    /// commits stay armed.
    fn remove_arguments(target: &str) -> Vec<String> {
        vec![target.to_owned(), "-y".to_owned()]
    }
}

impl FleetCloneTool for LocalFleetCloneTool {
    fn add_clone(&self, source: &str, path: &str, branch: &str) -> Result<(), ApiError> {
        run_skill("bc-add", Self::add_arguments(source, branch, path))
    }

    fn remove_clone(&self, path: &str) -> Result<(), ApiError> {
        run_skill("bc-rm", Self::remove_arguments(path))
    }
}

/// Run one fleet skill and report its refusal or failure.
fn run_skill(skill: &str, arguments: Vec<String>) -> Result<(), ApiError> {
    let output = Command::new("git")
        .arg(skill)
        .args(&arguments)
        .output()
        .map_err(|source| {
            ApiError::internal(&format!(
                "the fleet clone skill `git {skill}` could not run: {source}"
            ))
        })?;
    if output.status.success() {
        return Ok(());
    }
    let refusal = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(ApiError::internal(&format!(
        "the fleet clone skill `git {skill}` refused: {refusal}"
    )))
}

#[cfg(test)]
mod tests {
    use super::LocalFleetCloneTool;

    /// The argument lists are asserted without running the skill: it
    /// lives in the Operator's dotfiles, not in a clean checkout, so
    /// the wrapper's contract is the arguments and never a live
    /// invocation here.
    #[test]
    fn add_names_the_source_the_branch_and_the_explicit_target() {
        assert_eq!(
            LocalFleetCloneTool::add_arguments(
                "/workspaces/kanban.seed",
                "fleet/kan-t34",
                "/workspaces/kanban.fleet-kan-t34",
            ),
            vec![
                "/workspaces/kanban.seed".to_owned(),
                "fleet/kan-t34".to_owned(),
                "/workspaces/kanban.fleet-kan-t34".to_owned(),
            ],
            "the guarded path is the target git bc-add lands"
        );
    }

    #[test]
    fn remove_skips_the_prompt_but_never_forces() {
        let arguments = LocalFleetCloneTool::remove_arguments("/workspaces/kanban.fleet-t34");

        assert_eq!(
            arguments,
            vec!["/workspaces/kanban.fleet-t34".to_owned(), "-y".to_owned()]
        );
        assert!(
            !arguments
                .iter()
                .any(|argument| argument == "-f" || argument == "--force"),
            "the fleet's own refusal rules must stay armed"
        );
    }
}
