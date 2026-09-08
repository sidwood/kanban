//! Local Git adapter for guarded landing operations.

use kanban_app::GitLanding;
use kanban_app::landing::LandingDraft;
use kanban_dto::ApiError;

pub struct LocalGitLanding;

impl GitLanding for LocalGitLanding {
    fn require_base(&self, path: &str, base: &str) -> Result<(), ApiError> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["merge-base", "--is-ancestor", base, "HEAD"])
            .output()
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        if !output.status.success() {
            return Err(ApiError::invalid_request(
                "Ticket Lane does not contain its Spec integration base",
            ));
        }
        Ok(())
    }

    fn require_clean(&self, path: &str) -> Result<(), ApiError> {
        if !git_output(path, &["status", "--porcelain=v1", "--untracked-files=all"])?.is_empty() {
            return Err(ApiError::invalid_request(
                "landing requires clean source and target Workspaces",
            ));
        }
        Ok(())
    }

    fn head(&self, path: &str) -> Result<String, ApiError> {
        git_output(path, &["rev-parse", "--verify", "HEAD"])
    }

    fn current_branch(&self, path: &str) -> Result<String, ApiError> {
        git_output(path, &["branch", "--show-current"])
    }

    fn merge(&self, draft: &LandingDraft) -> Result<String, ApiError> {
        git_output(
            &draft.into_path,
            &[
                "fetch",
                "--no-tags",
                "--no-recurse-submodules",
                "--",
                &draft.from_path,
                &draft.from_tip,
            ],
        )?;
        if self.head(&draft.into_path)? != draft.into_tip
            || self.current_branch(&draft.into_path)? != draft.into_branch
            || self.head(&draft.from_path)? != draft.from_tip
            || self.current_branch(&draft.from_path)? != draft.from_branch
        {
            return Err(ApiError::invalid_request(
                "landing inputs changed before merge; explicit recovery is required",
            ));
        }
        self.require_clean(&draft.into_path)?;
        self.require_clean(&draft.from_path)?;
        git_output(
            &draft.into_path,
            &["merge", "--no-ff", "--no-edit", "--", &draft.from_tip],
        )?;
        self.head(&draft.into_path)
    }
}

fn git_output(path: &str, args: &[&str]) -> Result<String, ApiError> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|error| ApiError::internal(&error.to_string()))?;
    if !output.status.success() {
        return Err(ApiError::internal(
            "Git landing operation failed; inspect the Workspace before recovery",
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
