//! Isolated failure-mode proof for git status errors (KAN-T31).
//!
//! The wrapper poisons `PATH` for this process only so sibling unit tests
//! that spawn git in parallel never inherit the mutation.

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
    // SAFETY: this binary is the only test in its process.
    unsafe {
        std::env::set_var("PATH", format!("{}:{}", bin.display(), path));
    }

    let snapshot = LocalWorkspaceGitObserver.observe(&repository, &repository);

    assert!(snapshot.present, "the repository must be observed");
    assert!(
        !snapshot.working_tree_clean,
        "a failed git status must not report a clean tree"
    );
}
