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
