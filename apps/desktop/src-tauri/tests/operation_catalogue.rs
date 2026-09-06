//! Prove the shell exposes exactly the catalogue operations through
//! typed Tauri commands: every operation is registered exactly once
//! and no extra handler is reachable.

use std::collections::BTreeMap;

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
