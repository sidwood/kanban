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
        if repository_identity(repository_path).is_none() {
            return WorkspaceGitSnapshot {
                present: false,
                ..WorkspaceGitSnapshot::default()
            };
        }
        let identity = match repository_identity(workspace_path) {
            Some(identity) => identity,
            None => {
                return WorkspaceGitSnapshot {
                    present: false,
                    ..WorkspaceGitSnapshot::default()
                };
            }
        };
        if !belongs_to_repository(workspace_path, repository_path) {
            return WorkspaceGitSnapshot {
                present: false,
                ..WorkspaceGitSnapshot::default()
            };
        }
        let branch = git_output(workspace_path, &["rev-parse", "--abbrev-ref", "HEAD"]);
        let head = git_output(workspace_path, &["rev-parse", "HEAD"]);
        WorkspaceGitSnapshot {
            present: true,
            repository_identity: Some(identity),
            branch,
            head,
            working_tree_clean: working_tree_clean(workspace_path),
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

fn canonical_path(path: &str) -> Option<String> {
    Path::new(path)
        .canonicalize()
        .ok()?
        .to_str()
        .map(str::to_owned)
}

fn belongs_to_repository(workspace_path: &str, repository_path: &str) -> bool {
    match (
        repository_identity(workspace_path),
        repository_identity(repository_path),
    ) {
        (Some(actual), Some(expected)) if actual == expected => return true,
        _ => {}
    }
    let repository = match canonical_path(repository_path) {
        Some(path) => path,
        None => return false,
    };
    let mut current = workspace_path.to_owned();
    for _ in 0..10 {
        if canonical_path(&current).as_deref() == Some(repository.as_str()) {
            return true;
        }
        current = match git_output(&current, &["config", "--local", "--get", "bc.source"]) {
            Some(source) => source,
            None => break,
        };
    }
    match (origin_url(workspace_path), origin_url(repository_path)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn origin_url(path: &str) -> Option<String> {
    git_output(path, &["remote", "get-url", "origin"])
}

fn working_tree_clean(workspace_path: &str) -> bool {
    Command::new("git")
        .args([
            "--no-optional-locks",
            "-C",
            workspace_path,
            "status",
            "--porcelain",
        ])
        .output()
        .map(|output| output.status.success() && output.stdout.is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::fs;
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
                Path::new(&repository)
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
    fn mtime_dirty_observation_preserves_index_without_lock_contention() {
        use std::io::Write;
        use std::os::unix::fs::MetadataExt;
        use std::time::Duration;

        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = init_repo(dir.path());
        let readme = dir.path().join("README.md");
        let index_path = dir.path().join(".git").join("index");

        std::thread::sleep(Duration::from_secs(2));
        let status = Command::new("touch")
            .arg(&readme)
            .status()
            .expect("touch runs");
        assert!(status.success(), "touch must succeed");

        let index_before = fs::read(&index_path).expect("the index is readable");
        let meta_before = fs::metadata(&index_path).expect("the index metadata is readable");

        let snapshot = LocalWorkspaceGitObserver.observe(&repository, &repository);

        assert!(
            snapshot.present,
            "mtime-dirty workspaces must still be observed"
        );
        let index_after = fs::read(&index_path).expect("the index stays readable");
        assert_eq!(
            index_before, index_after,
            "observation must not rewrite index bytes for mtime-dirty tracked files"
        );
        let meta_after = fs::metadata(&index_path).expect("the index metadata stays readable");
        assert_eq!(
            meta_before.mtime(),
            meta_after.mtime(),
            "observation must not rewrite index metadata for mtime-dirty tracked files"
        );
        assert_eq!(meta_before.len(), meta_after.len());

        let lock_path = dir.path().join(".git").join("index.lock");
        let mut lock_file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&lock_path)
            .expect("index.lock can be held for the observation");
        writeln!(lock_file, "held by test").expect("the lock file is written");

        let snapshot_with_lock = LocalWorkspaceGitObserver.observe(&repository, &repository);

        assert!(
            snapshot_with_lock.present,
            "observation must not contend on index.lock for mtime-dirty tracked files"
        );

        drop(lock_file);
        fs::remove_file(lock_path).expect("the held lock file is removed");
    }

    #[test]
    fn sibling_git_status_reads_survive_concurrent_path_poisoning() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let barrier = Arc::new(Barrier::new(2));

        let poisoner = thread::spawn({
            let barrier = barrier.clone();
            move || {
                barrier.wait();
                let status = Command::new(env!("CARGO"))
                    .args([
                        "test",
                        "-p",
                        "kanban-service",
                        "--test",
                        "git_observer_failed_status",
                    ])
                    .status()
                    .expect("the isolated failure-mode probe runs");
                assert!(
                    status.success(),
                    "the isolated failure-mode probe must pass in its own process"
                );
            }
        });

        let sibling = thread::spawn({
            let barrier = barrier.clone();
            move || {
                barrier.wait();
                let dir = TempDir::new().expect("a scratch directory is available");
                let repository = init_repo(dir.path());
                let snapshot = LocalWorkspaceGitObserver.observe(&repository, &repository);
                assert!(snapshot.present);
                assert!(
                    snapshot.working_tree_clean,
                    "sibling git status reads must stay clean when PATH is not theirs to poison"
                );
            }
        });

        poisoner
            .join()
            .expect("the failure-mode probe thread completes");
        sibling.join().expect("the sibling git thread completes");
    }
}
