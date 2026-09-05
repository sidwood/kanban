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
            unique_unlanded_commits: unique_unlanded_commits(workspace_path, repository_path),
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

/// Membership is proven only by a shared Git common directory (the
/// Seed and its linked worktrees) or by the explicit, transitive
/// `bc.source` chain of sanctioned branch clones. Matching origin
/// URLs never establish membership (KAN-T97).
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
    false
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

/// Decide whether the Workspace holds commits the repository's HEAD
/// lacks (DR-LW-06): unique unlanded work that would be lost with the
/// clone. Work can sit on any local branch or a detached HEAD, not
/// only the checked-out branch, so the count walks local branches and
/// HEAD. Remote-tracking and tool-managed refs (refs/remotes,
/// refs/codex checkpoints) are deliberately out of scope: they name
/// commits the Workspace never authored, and counting them would
/// leave a clean zero-work Workspace permanently non-reusable.
/// Landed means reachable from the repository HEAD, so the count
/// runs in the Workspace with the repository's object store
/// attached read-only as an alternate — a hardlinked local clone
/// does not carry commits the repository gained after the clone.
/// The read never mutates either path; an undecidable answer is
/// `None`, which reuse evaluation treats as unlanded.
fn unique_unlanded_commits(workspace_path: &str, repository_path: &str) -> Option<bool> {
    let repository_head = git_output(repository_path, &["rev-parse", "HEAD"])?;
    let repository_objects = repository_objects(repository_path)?;
    let output = Command::new("git")
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &repository_objects)
        .args([
            "--no-optional-locks",
            "-C",
            workspace_path,
            "rev-list",
            "--count",
            "--branches",
            "HEAD",
            "--not",
            &repository_head,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let count = String::from_utf8(output.stdout).ok()?;
    let count: u64 = count.trim().parse().ok()?;
    Some(count > 0)
}

/// The repository's object directory, addressed through its common
/// git directory so worktree checkouts resolve too.
fn repository_objects(repository_path: &str) -> Option<String> {
    Some(format!("{}/objects", repository_identity(repository_path)?))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
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

    /// Cargo builds the `harness = false` failure-mode probe as a test
    /// binary beside this suite before any test runs, so running it
    /// directly keeps Cargo and its shared build lock out of the test
    /// suite (KAN-T96-AC2). The newest matching binary is the current
    /// build; older hashes linger from earlier builds.
    fn prebuilt_failed_status_probe() -> PathBuf {
        let deps = std::env::current_exe()
            .expect("the test binary path is available")
            .parent()
            .expect("the test binary lives inside a directory")
            .to_path_buf();
        let mut probes: Vec<_> = fs::read_dir(&deps)
            .expect("the test binary directory is readable")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("git_observer_failed_status-") && !name.ends_with(".d")
                    })
            })
            .filter(|path| path.is_file())
            .collect();
        probes.sort_by_key(|path| fs::metadata(path).and_then(|meta| meta.modified()).ok());
        probes
            .pop()
            .expect("cargo builds the failure-mode probe beside the suite")
    }

    fn init_repo_at(parent: &Path, name: &str) -> String {
        let dir = parent.join(name);
        fs::create_dir_all(&dir).expect("the repository directory is created");
        init_repo(&dir)
    }

    fn set_bc_source(workspace: &Path, source: &str) {
        git(
            workspace,
            &[
                "config",
                "bc.source",
                Path::new(source)
                    .canonicalize()
                    .expect("the source resolves")
                    .to_str()
                    .expect("the path is UTF-8"),
            ],
        );
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
        set_bc_source(&workspace, &repository);

        let snapshot = LocalWorkspaceGitObserver
            .observe(workspace.to_str().expect("the path is UTF-8"), &repository);

        assert!(
            snapshot.present,
            "sanctioned branch clones must be observed"
        );
        assert_eq!(snapshot.branch, Some("main".to_owned()));
        assert!(snapshot.head.is_some());
        assert!(snapshot.working_tree_clean);
        assert_eq!(
            snapshot.unique_unlanded_commits,
            Some(false),
            "a clone at the seed head holds nothing unlanded"
        );
    }

    #[test]
    fn local_commits_on_a_clone_report_as_unlanded() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = init_repo(dir.path());
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
        fs::write(workspace.join("work.md"), "local change\n")
            .expect("the local change is written");
        git(&workspace, &["add", "."]);
        git(&workspace, &["commit", "-m", "local work"]);

        let snapshot = LocalWorkspaceGitObserver
            .observe(workspace.to_str().expect("the path is UTF-8"), &repository);

        assert_eq!(
            snapshot.unique_unlanded_commits,
            Some(true),
            "commits the seed lacks must report as unlanded"
        );
    }

    #[test]
    fn landed_work_stops_reporting_unlanded_once_the_seed_advances() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = init_repo(dir.path());
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
        fs::write(workspace.join("work.md"), "local change\n")
            .expect("the local change is written");
        git(&workspace, &["add", "."]);
        git(&workspace, &["commit", "-m", "local work"]);
        // Land the clone's branch through the seed with a merge commit
        // that exists only in the seed's object store.
        git(
            Path::new(&repository),
            &[
                "fetch",
                workspace.to_str().expect("the path is UTF-8"),
                "main",
            ],
        );
        git(Path::new(&repository), &["merge", "--no-ff", "FETCH_HEAD"]);

        let snapshot = LocalWorkspaceGitObserver
            .observe(workspace.to_str().expect("the path is UTF-8"), &repository);

        assert_eq!(
            snapshot.unique_unlanded_commits,
            Some(false),
            "work the seed merged is landed even when the merge commit exists only there"
        );
    }

    #[test]
    fn unique_work_on_another_ref_reports_unlanded_at_equal_head() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = init_repo(dir.path());
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
        // Work left on a side branch is lost with the clone even while
        // HEAD matches the seed.
        git(&workspace, &["switch", "-c", "topic"]);
        fs::write(workspace.join("topic.md"), "side branch\n")
            .expect("the side branch change is written");
        git(&workspace, &["add", "."]);
        git(&workspace, &["commit", "-m", "side work"]);
        git(&workspace, &["switch", "-"]);

        let snapshot = LocalWorkspaceGitObserver
            .observe(workspace.to_str().expect("the path is UTF-8"), &repository);

        assert_eq!(
            snapshot.unique_unlanded_commits,
            Some(true),
            "commits on a non-HEAD ref must report as unlanded even at equal HEAD"
        );
    }

    #[test]
    fn unique_work_on_another_ref_reports_unlanded_at_divergent_head() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = init_repo(dir.path());
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
        git(&workspace, &["switch", "-c", "topic"]);
        fs::write(workspace.join("topic.md"), "side branch\n")
            .expect("the side branch change is written");
        git(&workspace, &["add", "."]);
        git(&workspace, &["commit", "-m", "side work"]);
        git(&workspace, &["switch", "-"]);
        // The seed advances independently of the side branch work.
        fs::write(Path::new(&repository).join("advance.md"), "seed work\n")
            .expect("the seed change is written");
        git(Path::new(&repository), &["add", "."]);
        git(Path::new(&repository), &["commit", "-m", "seed advance"]);

        let snapshot = LocalWorkspaceGitObserver
            .observe(workspace.to_str().expect("the path is UTF-8"), &repository);

        assert_eq!(
            snapshot.unique_unlanded_commits,
            Some(true),
            "commits on a non-HEAD ref must report as unlanded even at divergent HEAD"
        );
    }

    #[test]
    fn remote_divergence_does_not_report_unlanded() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = init_repo(dir.path());
        let upstream = dir.path().join("upstream");
        git(
            dir.path(),
            &["clone", "--local", &repository, upstream.to_str().unwrap()],
        );
        git(&upstream, &["switch", "-c", "feature"]);
        fs::write(upstream.join("feature.md"), "remote work\n")
            .expect("the remote change is written");
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-m", "remote work"]);
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
        // The clone's origin holds a branch the seed never landed; the
        // fetch leaves that work only on a remote-tracking ref.
        git(
            &workspace,
            &["remote", "set-url", "origin", upstream.to_str().unwrap()],
        );
        git(&workspace, &["fetch", "origin"]);

        let snapshot = LocalWorkspaceGitObserver
            .observe(workspace.to_str().expect("the path is UTF-8"), &repository);

        assert_eq!(
            snapshot.unique_unlanded_commits,
            Some(false),
            "remote-tracking refs must not make a zero-work clone non-reusable"
        );
    }

    #[test]
    fn codex_checkpoint_refs_do_not_report_unlanded() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = init_repo(dir.path());
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
        // A tool checkpoints a commit under refs/codex and rewinds the
        // branch; the checkpoint names work no local branch holds.
        fs::write(workspace.join("checkpoint.md"), "checkpoint\n")
            .expect("the checkpoint change is written");
        git(&workspace, &["add", "."]);
        git(&workspace, &["commit", "-m", "checkpoint work"]);
        git(
            &workspace,
            &["update-ref", "refs/codex/checkpoints/run-1", "HEAD"],
        );
        git(&workspace, &["reset", "--hard", "origin/main"]);

        let snapshot = LocalWorkspaceGitObserver
            .observe(workspace.to_str().expect("the path is UTF-8"), &repository);

        assert_eq!(
            snapshot.unique_unlanded_commits,
            Some(false),
            "tool-managed refs must not make a zero-work clone non-reusable"
        );
    }

    #[test]
    fn detached_head_work_reports_unlanded() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = init_repo(dir.path());
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
        // Work left on a detached HEAD is lost with the clone even
        // though no branch names it.
        git(&workspace, &["switch", "--detach"]);
        fs::write(workspace.join("detached.md"), "detached work\n")
            .expect("the detached change is written");
        git(&workspace, &["add", "."]);
        git(&workspace, &["commit", "-m", "detached work"]);

        let snapshot = LocalWorkspaceGitObserver
            .observe(workspace.to_str().expect("the path is UTF-8"), &repository);

        assert_eq!(
            snapshot.unique_unlanded_commits,
            Some(true),
            "commits only reachable from a detached HEAD must report as unlanded"
        );
    }

    #[test]
    fn worktree_observation_shares_the_common_directory() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = init_repo(dir.path());
        let worktree = dir.path().join("worktree");
        git(
            Path::new(&repository),
            &[
                "worktree",
                "add",
                "-b",
                "feature",
                worktree.to_str().expect("the path is UTF-8"),
            ],
        );

        let snapshot = LocalWorkspaceGitObserver
            .observe(worktree.to_str().expect("the path is UTF-8"), &repository);

        assert!(
            snapshot.present,
            "linked worktrees share the repository's common Git directory"
        );
        assert_eq!(snapshot.branch, Some("feature".to_owned()));
        assert!(snapshot.working_tree_clean);
    }

    #[test]
    fn transitive_bc_source_membership_observes_nested_clones() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = init_repo(dir.path());
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        git(
            Path::new(&repository),
            &["clone", "--local", &repository, first.to_str().unwrap()],
        );
        git(
            &first,
            &[
                "clone",
                "--local",
                first.to_str().unwrap(),
                second.to_str().unwrap(),
            ],
        );
        set_bc_source(&first, &repository);
        set_bc_source(&second, first.to_str().expect("the path is UTF-8"));

        let snapshot = LocalWorkspaceGitObserver
            .observe(second.to_str().expect("the path is UTF-8"), &repository);

        assert!(
            snapshot.present,
            "the transitive bc.source chain must establish membership"
        );
        assert_eq!(snapshot.branch, Some("main".to_owned()));
        assert!(snapshot.head.is_some());
    }

    #[test]
    fn unrelated_same_origin_clone_is_refused() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let origin = init_repo_at(dir.path(), "origin");
        let repository = dir.path().join("repository");
        let unrelated = dir.path().join("unrelated");
        git(
            Path::new(&origin),
            &["clone", "--local", &origin, repository.to_str().unwrap()],
        );
        git(
            Path::new(&origin),
            &["clone", "--local", &origin, unrelated.to_str().unwrap()],
        );

        let snapshot = LocalWorkspaceGitObserver.observe(
            unrelated.to_str().expect("the path is UTF-8"),
            repository.to_str().expect("the path is UTF-8"),
        );

        assert!(
            !snapshot.present,
            "origin URL equality alone must not establish membership"
        );
        assert_eq!(snapshot.repository_identity, None);
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
            "mtime-dirty workspaces must stay observable under a held index.lock"
        );
        assert!(
            snapshot_with_lock.working_tree_clean,
            "git status must complete and report a clean tree under a held \
             index.lock for mtime-dirty tracked files"
        );

        drop(lock_file);
        fs::remove_file(lock_path).expect("the held lock file is removed");
    }

    #[test]
    fn sibling_git_status_reads_survive_concurrent_path_poisoning() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let barrier = Arc::new(Barrier::new(2));
        let probe = prebuilt_failed_status_probe();

        let poisoner = thread::spawn({
            let barrier = barrier.clone();
            move || {
                barrier.wait();
                let status = Command::new(&probe)
                    .status()
                    .expect("the prebuilt failure-mode probe runs");
                assert!(
                    status.success(),
                    "the prebuilt failure-mode probe must pass in its own process"
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
