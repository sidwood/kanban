//! Read-only git observation for registered Workspaces.

use std::path::Path;
use std::process::Command;

use kanban_app::{WorkspaceGitObserver, WorkspaceGitSnapshot};

/// Observe Workspaces through the local `git` binary without mutating
/// clones (KAN-S6-US1).
#[derive(Debug, Default)]
pub struct LocalWorkspaceGitObserver;

impl WorkspaceGitObserver for LocalWorkspaceGitObserver {
    fn observe(&self, workspace_path: &str, repository_path: &str) -> WorkspaceGitSnapshot {
        let workspace = Path::new(workspace_path);
        if !workspace.exists() {
            return WorkspaceGitSnapshot {
                present: false,
                ..WorkspaceGitSnapshot::default()
            };
        }
        let expected = match repository_identity(repository_path) {
            Some(identity) => identity,
            None => {
                return WorkspaceGitSnapshot {
                    present: false,
                    ..WorkspaceGitSnapshot::default()
                };
            }
        };
        let actual = match repository_identity(workspace_path) {
            Some(identity) => identity,
            None => {
                return WorkspaceGitSnapshot {
                    present: false,
                    ..WorkspaceGitSnapshot::default()
                };
            }
        };
        if expected != actual {
            return WorkspaceGitSnapshot {
                present: false,
                ..WorkspaceGitSnapshot::default()
            };
        }
        let branch = git_output(workspace_path, &["rev-parse", "--abbrev-ref", "HEAD"]);
        let head = git_output(workspace_path, &["rev-parse", "HEAD"]);
        let working_tree_clean = Command::new("git")
            .args(["-C", workspace_path, "status", "--porcelain"])
            .output()
            .map(|output| output.stdout.is_empty())
            .unwrap_or(false);
        WorkspaceGitSnapshot {
            present: true,
            repository_identity: Some(actual),
            branch,
            head,
            working_tree_clean,
        }
    }
}

fn git_output(path: &str, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(path);
    command.args(args);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn repository_identity(path: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", path, "rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let relative = String::from_utf8(output.stdout).ok()?;
    let relative = relative.trim();
    Path::new(path)
        .join(relative)
        .canonicalize()
        .ok()?
        .to_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Command;

    use tempfile::TempDir;

    use super::LocalWorkspaceGitObserver;
    use kanban_app::WorkspaceGitObserver;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {:?} in {}", args, dir.display());
    }

    fn init_repo(dir: &Path) -> String {
        git(dir, &["init"]);
        git(dir, &["config", "user.email", "test@example.com"]);
        git(dir, &["config", "user.name", "Test"]);
        fs::write(dir.join("README.md"), "seed\n").expect("the seed file is written");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-m", "initial"]);
        dir.to_str().expect("the path is UTF-8").to_owned()
    }

    #[test]
    fn branch_clone_observation_is_present() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = init_repo(dir.path());
        git(
            Path::new(&repository),
            &["remote", "add", "origin", "https://example.com/kanban.git"],
        );
        let workspace = dir.path().join("clone");
        git(
            dir.path(),
            &["clone", "--local", &repository, workspace.to_str().unwrap()],
        );
        git(
            &workspace,
            &[
                "config",
                "bc.source",
                &Path::new(&repository)
                    .canonicalize()
                    .expect("the repository resolves")
                    .to_str()
                    .expect("the path is UTF-8"),
            ],
        );

        let snapshot = LocalWorkspaceGitObserver
            .observe(workspace.to_str().expect("the path is UTF-8"), &repository);

        assert!(
            snapshot.present,
            "sanctioned branch clones must be observed"
        );
        assert_eq!(snapshot.branch, Some("main".to_owned()));
        assert!(snapshot.head.is_some());
        assert!(snapshot.working_tree_clean);
    }

    #[test]
    fn failed_git_status_is_not_reported_as_clean() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = init_repo(dir.path());
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).expect("the wrapper directory is created");
        let real_git = Command::new("which")
            .arg("git")
            .output()
            .expect("git is on PATH")
            .stdout;
        let real_git = String::from_utf8(real_git)
            .expect("which output is UTF-8")
            .trim()
            .to_owned();
        fs::write(
            bin.join("git"),
            format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"-C\" ] && [ \"$3\" = \"status\" ] && [ \"$4\" = \"--porcelain\" ]; then\n\
                   exit 1\n\
                 fi\n\
                 exec \"{real_git}\" \"$@\"\n"
            ),
        )
        .expect("the wrapper is written");
        let mut permissions = fs::metadata(bin.join("git"))
            .expect("the wrapper exists")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(bin.join("git"), permissions).expect("the wrapper is executable");

        let path = std::env::var("PATH").expect("PATH is set");
        // SAFETY: the test owns the environment for this process only.
        unsafe {
            std::env::set_var("PATH", format!("{}:{}", bin.display(), path));
        }

        let snapshot = LocalWorkspaceGitObserver.observe(&repository, &repository);

        assert!(snapshot.present);
        assert!(
            !snapshot.working_tree_clean,
            "a failed git status must not report a clean tree"
        );
    }
}
