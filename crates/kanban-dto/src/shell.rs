//! Shell preference payload definitions (KAN-S5-US1, KAN-S5-US3): the
//! wire form of how the operator keeps the surface arranged — the
//! navigation rail's collapse and the board columns collapsed to
//! their rail, per scope — and the operations that read and replace
//! it. The arrangement is per-operator data in the authoritative
//! store rather than browser state, so it survives a reload, a new
//! window, and a cleared browser origin. A Saved View owns which
//! columns are hidden and which groups are open (DR-BP-05); these are
//! the decisions no view owns, and one update replaces the whole
//! record so writing one preference never drops the other.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::mutation::MutationContext;
use crate::view::ViewScope;

/// One column a board can show: a fixed group while its axis reads as
/// a single column, or one of the states the two multi-state groups
/// open into. Collapse addresses exactly these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BoardColumn {
    /// The Draft group.
    Draft,
    /// The Backlog group, aggregated.
    Backlog,
    /// Backlog opened: work the operator set aside.
    Parked,
    /// Backlog opened: work a dependency or blocker holds.
    Blocked,
    /// Backlog opened: work waiting on a date.
    Scheduled,
    /// Backlog opened: work anything may claim.
    Ready,
    /// The Current group.
    Current,
    /// The Review group.
    Review,
    /// The Staged group, aggregated.
    Staged,
    /// Staged opened: work a review approved.
    Approved,
    /// Staged opened: work on its way to the default branch.
    Landing,
    /// The Done group.
    Done,
}

/// The columns one scope keeps collapsed, in board order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScopedCollapsedColumns {
    /// The scope the collapse belongs to: the whole board, or one
    /// Project's.
    pub scope: ViewScope,
    /// The collapsed columns, named once each in board order.
    pub columns: Vec<BoardColumn>,
}

/// Request payload for the `shell.preferences` query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShellPreferencesQuery {}

/// The operator's shell arrangement, whole. A shell nobody has
/// rearranged answers the everyday arrangement at version 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShellPreferencesRecord {
    /// Whether the navigation rail stands open with its labels.
    pub rail_open: bool,
    /// Every scope that keeps columns collapsed, and which.
    pub collapsed_columns: Vec<ScopedCollapsedColumns>,
    /// The record's optimistic version.
    pub version: u64,
}

/// Request payload for the `shell.preferences.update` command: the
/// whole arrangement, replaced wholesale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShellPreferencesUpdateRequest {
    /// The optimistic version and idempotency key of the write.
    pub mutation: MutationContext,
    /// Whether the navigation rail stands open with its labels.
    pub rail_open: bool,
    /// Every scope that keeps columns collapsed, and which. A scope
    /// left out keeps nothing collapsed.
    #[serde(default)]
    pub collapsed_columns: Vec<ScopedCollapsedColumns>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        BoardColumn, ScopedCollapsedColumns, ShellPreferencesRecord, ShellPreferencesUpdateRequest,
    };
    use crate::mutation::MutationContext;
    use crate::schema_definitions;
    use crate::view::ViewScope;

    #[test]
    fn the_record_round_trips_every_scope_it_carries() {
        let record = ShellPreferencesRecord {
            rail_open: false,
            collapsed_columns: vec![
                ScopedCollapsedColumns {
                    scope: ViewScope::Global,
                    columns: vec![BoardColumn::Review],
                },
                ScopedCollapsedColumns {
                    scope: ViewScope::Project(2),
                    columns: vec![BoardColumn::Backlog, BoardColumn::Ready],
                },
            ],
            version: 4,
        };

        let encoded = serde_json::to_value(&record).expect("the record encodes");
        assert_eq!(
            encoded,
            json!({
                "rail_open": false,
                "collapsed_columns": [
                    { "scope": "global", "columns": ["review"] },
                    { "scope": { "project": 2 }, "columns": ["backlog", "ready"] },
                ],
                "version": 4,
            })
        );
        let decoded: ShellPreferencesRecord =
            serde_json::from_value(encoded).expect("the record decodes");
        assert_eq!(decoded, record);
    }

    #[test]
    fn an_update_carrying_no_collapse_replaces_the_arrangement_with_none() {
        let request: ShellPreferencesUpdateRequest = serde_json::from_value(json!({
            "mutation": { "optimistic_version": 2, "idempotency_key": "key-rail" },
            "rail_open": true,
        }))
        .expect("the request decodes");

        assert_eq!(
            request,
            ShellPreferencesUpdateRequest {
                mutation: MutationContext {
                    optimistic_version: 2,
                    idempotency_key: "key-rail".to_owned(),
                },
                rail_open: true,
                collapsed_columns: Vec::new(),
            }
        );
    }

    #[test]
    fn an_unknown_column_is_outside_the_closed_vocabulary() {
        let refused = serde_json::from_value::<ScopedCollapsedColumns>(json!({
            "scope": "global",
            "columns": ["prototype"],
        }));

        assert!(refused.is_err(), "the column vocabulary is closed");
    }

    #[test]
    fn every_shell_schema_is_registered_and_closed() {
        let registered = schema_definitions();
        for name in [
            "BoardColumn",
            "ScopedCollapsedColumns",
            "ShellPreferencesQuery",
            "ShellPreferencesRecord",
            "ShellPreferencesUpdateRequest",
        ] {
            let schema = registered
                .iter()
                .find(|(registered_name, _)| *registered_name == name)
                .map(|(_, schema)| schema)
                .unwrap_or_else(|| panic!("{name} is registered"));
            let encoded = serde_json::to_string(schema).expect("the schema serialises");
            assert!(
                encoded.contains("\"additionalProperties\":false") || encoded.contains("\"enum\":"),
                "{name} should reject unknown fields or close its vocabulary"
            );
        }
    }
}
