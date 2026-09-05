//! Isolated failure-mode proof for git status errors (KAN-T31).
//!
//! The wrapper poisons `PATH` for this process only so sibling unit tests
//! that spawn git in parallel never inherit the mutation. The fake git
//! binary lives outside the observed repository (KAN-T96's deferred
//! finding): the tree stays genuinely clean, so the proof fails if the
//! interception ever stops working, and the failed status is reported
//! as an absent verdict — never as a dirty worktree (KAN-T99).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use kanban_app::WorkspaceGitObserver;
use kanban_service::git_observer::LocalWorkspaceGitObserver;
use tempfile::TempDir;

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

fn main() {
    let dir = TempDir::new().expect("a scratch directory is available");
    // The repository and the fake binary live in sibling directories:
    // the wrapper is never an untracked file inside the observed tree,
    // so a clean verdict from the real git stays possible.
    let repo_dir = dir.path().join("repo");
    fs::create_dir_all(&repo_dir).expect("the repository directory is created");
    let repository = init_repo(&repo_dir);
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
             if [ \"$1\" = \"--no-optional-locks\" ] && [ \"$2\" = \"-C\" ] && [ \"$4\" = \"status\" ] && [ \"$5\" = \"--porcelain\" ]; then\n\
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

    // Control: with the real git still resolving first, the tree is
    // genuinely clean. If this ever fails, the fixture itself dirtied
    // the repository and the proof below would be vacuous.
    let control = Command::new(&real_git)
        .args(["-C", &repository, "status", "--porcelain"])
        .output()
        .expect("the control status runs");
    assert!(
        control.status.success() && control.stdout.is_empty(),
        "the observed repository must be genuinely clean before interception"
    );

    let path = std::env::var("PATH").expect("PATH is set");
    // SAFETY: this binary is the only test in its process.
    unsafe {
        std::env::set_var("PATH", format!("{}:{}", bin.display(), path));
    }

    let snapshot = LocalWorkspaceGitObserver.observe(&repository, &repository);

    assert!(snapshot.present, "the repository must be observed");
    assert_eq!(
        snapshot.working_tree_clean, None,
        "a failed git status is an observation failure, not a dirty verdict"
    );
    assert_ne!(
        snapshot.working_tree_clean,
        Some(false),
        "an observation failure must never pose as uncommitted work"
    );
}
