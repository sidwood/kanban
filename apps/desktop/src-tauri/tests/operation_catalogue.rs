//! Prove the shell exposes exactly the catalogue operations through
//! typed Tauri commands: every operation is registered exactly once,
//! and the only other handler reachable is the shell's own, named
//! below.

use std::collections::{BTreeMap, BTreeSet};

use kanban_app::exposed_operations;

/// Occurrences of each name, so a repeated registration cannot hide
/// inside set equality.
fn occurrence_counts(names: impl Iterator<Item = &'static str>) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for name in names {
        *counts.entry(name).or_insert(0) += 1;
    }
    counts
}

#[test]
fn operation_catalogue_matches_registered_tauri_commands() {
    let expected = occurrence_counts(
        exposed_operations()
            .iter()
            .map(|operation| operation.mcp_tool_name),
    );
    for (name, count) in &expected {
        assert_eq!(*count, 1, "the catalogue must list {name} exactly once");
    }
    let actual = occurrence_counts(
        kanban_desktop_lib::REGISTERED_TAURI_COMMANDS
            .iter()
            .copied(),
    );
    for (name, count) in &actual {
        assert_eq!(
            *count, 1,
            "the shell must register {name} exactly once, not {count} times"
        );
    }

    assert_eq!(
        expected, actual,
        "every catalogue operation must have one typed Tauri exposure and no extra exposure"
    );
}

#[test]
fn the_shell_adds_only_its_issue_link_command() {
    assert_eq!(
        kanban_desktop_lib::SHELL_TAURI_COMMANDS,
        &["issue_link_listen"],
        "a shell command beyond the catalogue must be named here deliberately"
    );
    let catalogue: BTreeSet<&str> = exposed_operations()
        .iter()
        .map(|operation| operation.mcp_tool_name)
        .collect();
    for command in kanban_desktop_lib::SHELL_TAURI_COMMANDS {
        assert!(
            !catalogue.contains(command),
            "the shell command {command} must not shadow a catalogue operation"
        );
    }
}
