//! Git subprocess defaults for integration tests without a global identity.

use std::path::Path;
use std::process::Command;

fn apply_test_git_env(command: &mut Command) {
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_0", "master");
}

pub fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    apply_test_git_env(&mut command);
    command.args(args).current_dir(dir);
    command
}

pub fn git(dir: &Path, args: &[&str]) {
    let status = git_command(dir, args).status().expect("git runs");
    assert!(status.success(), "git {:?} in {}", args, dir.display());
}

pub fn git_clone(from: &Path, to: &Path, user_name: &str, user_email: &str) {
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_0", "master")
        .args([
            "clone",
            "-c",
            &format!("user.name={user_name}"),
            "-c",
            &format!("user.email={user_email}"),
            from.to_str().expect("utf-8 path"),
            to.to_str().expect("utf-8 path"),
        ])
        .status()
        .expect("git clone runs");
    assert!(
        status.success(),
        "git clone {} -> {}",
        from.display(),
        to.display()
    );
}

pub fn git_clone_local(from: &str, to: &str, user_name: &str, user_email: &str) {
    let status = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_0", "master")
        .args([
            "clone",
            "-c",
            &format!("user.name={user_name}"),
            "-c",
            &format!("user.email={user_email}"),
            "--local",
            from,
            to,
        ])
        .status()
        .expect("git clone runs");
    assert!(status.success(), "git clone --local {from} -> {to}");
}

pub fn init_repo(dir: &Path, user_name: &str, user_email: &str) {
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.email", user_email]);
    git(dir, &["config", "user.name", user_name]);
}
