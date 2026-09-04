//! Prove the shell exposes exactly the catalogue operations through
//! typed Tauri commands and no extra handlers.

use std::collections::BTreeSet;

use kanban_app::exposed_operations;

#[test]
fn operation_catalogue_matches_registered_tauri_commands() {
    let expected: BTreeSet<_> = exposed_operations()
        .iter()
        .map(|operation| operation.mcp_tool_name)
        .collect();
    let actual: BTreeSet<_> = kanban_desktop_lib::REGISTERED_TAURI_COMMANDS
        .iter()
        .copied()
        .collect();

    assert_eq!(
        expected, actual,
        "every catalogue operation must have one typed Tauri exposure and no extra exposure"
    );
}
