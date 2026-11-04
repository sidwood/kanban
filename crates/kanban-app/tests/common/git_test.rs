//! Git subprocess defaults for integration tests without a global identity.

use std::path::Path;
use std::process::Command;

pub fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_COUNT", "3")
        .env("GIT_CONFIG_KEY_0", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_0", "master")
        .env("GIT_CONFIG_KEY_1", "user.email")
        .env("GIT_CONFIG_VALUE_1", "test@example.com")
        .env("GIT_CONFIG_KEY_2", "user.name")
        .env("GIT_CONFIG_VALUE_2", "Test");
    command
}

pub fn git(dir: &Path, args: &[&str]) {
    let status = git_command(dir, args).status().expect("git runs");
    assert!(status.success(), "git {:?} in {}", args, dir.display());
}
