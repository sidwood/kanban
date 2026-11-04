//! Git subprocess defaults so local operations succeed without a global identity.

use std::process::Command;

/// Apply a fixed author identity for non-interactive git operations.
pub fn configure_git_command(command: &mut Command) {
    command
        .env("GIT_CONFIG_COUNT", "3")
        .env("GIT_CONFIG_KEY_0", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_0", "master")
        .env("GIT_CONFIG_KEY_1", "user.email")
        .env("GIT_CONFIG_VALUE_1", "kanban@localhost")
        .env("GIT_CONFIG_KEY_2", "user.name")
        .env("GIT_CONFIG_VALUE_2", "Kanban");
}
