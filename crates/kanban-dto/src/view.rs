//! Saved View payload definitions (DR-BP-05, DR-BP-06): the wire
//! form of one named operator perspective and the operations that
//! read and edit it. A view carries its whole owned set together —
//! the ten-axis filter, the expanded groups, the hidden columns, the
//! mode, the Done placement, and the sorting key — so a record
//! round-trips every property it owns and an update replaces the set
//! whole: writing one property never drops the others. The list query
//! answers every scope's views with the generated defaults
//! materialised (DR-BP-06), so a client never renders a scope
//! without its default.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::board::{BoardFilter, BoardGroup};
use crate::mutation::MutationContext;

/// The scope a view belongs to: the whole board, or one Project's
/// work. The global scope holds exactly one default view; every
/// Project holds exactly one of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewScope {
    /// Every Project's work on one surface.
    Global,
    /// One Project's work, named by its numeric identity.
    Project(u64),
}

impl ViewScope {
    /// The Project identity this scope names, or `None` when the
    /// scope is global.
    pub fn project_id(&self) -> Option<u64> {
        match self {
            Self::Global => None,
            Self::Project(project_id) => Some(*project_id),
        }
    }
}

/// The mode a view restores: the grouped columns or the register
/// tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    /// Grouped columns.
    Board,
    /// One table per visible column.
    Register,
}

/// Where Done work sits under a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DonePlacement {
    /// Done stands with the other groups.
    Column,
    /// Done sits below the board as a table.
    Table,
}

/// The deterministic order a view reads cards in (DR-LC-11): which
/// key leads. Both keys are closed and deterministic; no view orders
/// cards by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewSorting {
    /// Priority leads, readiness beneath.
    Priority,
    /// Readiness leads, priority beneath.
    Readiness,
}

/// One named operator perspective, whole: the name, the scope, every
/// property the view owns, whether it is its scope's generated
/// default, and the version that guards edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SavedViewRecord {
    /// The view's storage-assigned identity.
    pub id: u64,
    /// The perspective the operator reads.
    pub name: String,
    /// The scope the view belongs to.
    pub scope: ViewScope,
    /// The owned filter: the ten axes the view selects by.
    #[serde(default)]
    pub filter: BoardFilter,
    /// The owned expanded groups: the groups the view opens into
    /// their states, in the fixed group order.
    pub expanded_groups: Vec<BoardGroup>,
    /// The owned hidden columns: the groups the view keeps off the
    /// board, in the fixed group order.
    pub hidden_columns: Vec<BoardGroup>,
    /// The owned mode: board or register.
    pub mode: ViewMode,
    /// The owned Done placement: column or table.
    pub done_placement: DonePlacement,
    /// The owned sorting key.
    pub sorting: ViewSorting,
    /// Whether this view is its scope's generated default.
    pub is_default: bool,
    /// The number of applied changes, for optimistic version checks.
    pub version: u64,
}

/// Request payload for the `view.list` query: every scope's views,
/// generated defaults included.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewListQuery {}

/// Response payload for the `view.list` query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewListResponse {
    /// Every view of every scope, each scope's default first, the
    /// global scope before the Projects.
    pub views: Vec<SavedViewRecord>,
}

/// Request payload for the `view.create` command. The owned set
/// arrives whole; an axis the filter leaves absent constrains
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewCreateRequest {
    /// The mutation context.
    pub mutation: MutationContext,
    /// The scope the view belongs to.
    pub scope: ViewScope,
    /// The perspective's name, unique within its scope.
    pub name: String,
    /// The owned filter.
    #[serde(default)]
    pub filter: BoardFilter,
    /// The owned expanded groups.
    #[serde(default)]
    pub expanded_groups: Vec<BoardGroup>,
    /// The owned hidden columns.
    #[serde(default)]
    pub hidden_columns: Vec<BoardGroup>,
    /// The owned mode.
    pub mode: ViewMode,
    /// The owned Done placement.
    pub done_placement: DonePlacement,
    /// The owned sorting key.
    pub sorting: ViewSorting,
}

/// Request payload for the `view.update` command: the whole owned
/// set replaced at once, guarded by the view's optimistic version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewUpdateRequest {
    /// The mutation context.
    pub mutation: MutationContext,
    /// The view being updated.
    pub view_id: u64,
    /// The owned filter.
    #[serde(default)]
    pub filter: BoardFilter,
    /// The owned expanded groups.
    #[serde(default)]
    pub expanded_groups: Vec<BoardGroup>,
    /// The owned hidden columns.
    #[serde(default)]
    pub hidden_columns: Vec<BoardGroup>,
    /// The owned mode.
    pub mode: ViewMode,
    /// The owned Done placement.
    pub done_placement: DonePlacement,
    /// The owned sorting key.
    pub sorting: ViewSorting,
}

/// Request payload for the `view.rename` command: the name changes,
/// nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewRenameRequest {
    /// The mutation context.
    pub mutation: MutationContext,
    /// The view being renamed.
    pub view_id: u64,
    /// The perspective's new name, unique within its scope.
    pub name: String,
}

/// Request payload for the `view.remove` command. Removing a scope's
/// default is legal: the next read generates it again (DR-BP-06).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewRemoveRequest {
    /// The mutation context.
    pub mutation: MutationContext,
    /// The view being removed.
    pub view_id: u64,
}

/// Response payload for the `view.remove` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewRemovedRecord {
    /// The view that was removed.
    pub view_id: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        DonePlacement, SavedViewRecord, ViewCreateRequest, ViewListQuery, ViewListResponse,
        ViewMode, ViewRemoveRequest, ViewRemovedRecord, ViewRenameRequest, ViewScope, ViewSorting,
        ViewUpdateRequest,
    };
    use crate::board::{BoardFilter, BoardGroup};
    use crate::mutation::MutationContext;
    use crate::schema_definitions;
    use crate::ticket::{TicketKind, TicketPriority, TicketState};

    fn mutation(version: u64) -> MutationContext {
        MutationContext {
            optimistic_version: version,
            idempotency_key: "key-1".to_owned(),
        }
    }

    /// One record owning every property, varied away from every
    /// default so a round trip proves each one.
    fn record() -> SavedViewRecord {
        SavedViewRecord {
            id: 5,
            name: "Review queue".to_owned(),
            scope: ViewScope::Project(2),
            filter: BoardFilter {
                projects: vec![2],
                kinds: vec![TicketKind::Implementation],
                states: vec![TicketState::InReview],
                priorities: vec![TicketPriority::Urgent],
                ..BoardFilter::default()
            },
            expanded_groups: vec![BoardGroup::Backlog, BoardGroup::Staged],
            hidden_columns: vec![BoardGroup::Draft, BoardGroup::Done],
            mode: ViewMode::Register,
            done_placement: DonePlacement::Table,
            sorting: ViewSorting::Readiness,
            is_default: false,
            version: 4,
        }
    }

    #[test]
    fn a_record_round_trips_every_property_it_owns() {
        let encoded = serde_json::to_value(record()).expect("the record encodes");
        assert_eq!(
            encoded,
            json!({
                "id": 5,
                "name": "Review queue",
                "scope": { "project": 2 },
                "filter": {
                    "projects": [2],
                    "kinds": ["implementation"],
                    "states": ["in_review"],
                    "priorities": ["urgent"],
                },
                "expanded_groups": ["backlog", "staged"],
                "hidden_columns": ["draft", "done"],
                "mode": "register",
                "done_placement": "table",
                "sorting": "readiness",
                "is_default": false,
                "version": 4,
            })
        );
        let decoded: SavedViewRecord = serde_json::from_value(encoded).expect("the record decodes");
        assert_eq!(decoded, record());
    }

    #[test]
    fn the_generated_defaults_round_trip_too() {
        let global = SavedViewRecord {
            id: 1,
            name: "All work".to_owned(),
            scope: ViewScope::Global,
            filter: BoardFilter::default(),
            expanded_groups: vec![],
            hidden_columns: vec![BoardGroup::Draft],
            mode: ViewMode::Board,
            done_placement: DonePlacement::Column,
            sorting: ViewSorting::Priority,
            is_default: true,
            version: 1,
        };
        let encoded = serde_json::to_value(&global).expect("the default encodes");
        assert_eq!(encoded["scope"], json!("global"));
        assert_eq!(encoded["filter"], json!({}));
        let decoded: SavedViewRecord =
            serde_json::from_value(encoded).expect("the default decodes");
        assert_eq!(decoded, global);
    }

    #[test]
    fn the_list_query_carries_nothing_and_answers_with_views() {
        let encoded = serde_json::to_value(ViewListQuery::default()).expect("the query encodes");
        assert_eq!(encoded, json!({}));
        let decoded: ViewListQuery = serde_json::from_value(json!({})).expect("it decodes");
        assert_eq!(decoded, ViewListQuery::default());

        let response = ViewListResponse {
            views: vec![record()],
        };
        let encoded = serde_json::to_value(&response).expect("the response encodes");
        assert_eq!(encoded["views"].as_array().map(Vec::len), Some(1));
        let decoded: ViewListResponse =
            serde_json::from_value(encoded).expect("the response decodes");
        assert_eq!(decoded, response);
    }

    #[test]
    fn a_create_request_round_trips_and_omitted_sets_start_empty() {
        let request = ViewCreateRequest {
            mutation: mutation(0),
            scope: ViewScope::Global,
            name: "Deep work".to_owned(),
            filter: BoardFilter::default(),
            expanded_groups: vec![BoardGroup::Backlog],
            hidden_columns: vec![],
            mode: ViewMode::Board,
            done_placement: DonePlacement::Column,
            sorting: ViewSorting::Priority,
        };
        let encoded = serde_json::to_value(&request).expect("the request encodes");
        assert_eq!(
            encoded,
            json!({
                "mutation": {
                    "optimistic_version": 0,
                    "idempotency_key": "key-1",
                },
                "scope": "global",
                "name": "Deep work",
                "filter": {},
                "expanded_groups": ["backlog"],
                "hidden_columns": [],
                "mode": "board",
                "done_placement": "column",
                "sorting": "priority",
            })
        );
        let decoded: ViewCreateRequest =
            serde_json::from_value(encoded).expect("the request decodes");
        assert_eq!(decoded, request);

        let bare: ViewCreateRequest = serde_json::from_value(json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": "key-2" },
            "scope": "global",
            "name": "Everything",
            "mode": "board",
            "done_placement": "column",
            "sorting": "priority",
        }))
        .expect("the owned sets may arrive absent");
        assert_eq!(bare.filter, BoardFilter::default());
        assert!(bare.expanded_groups.is_empty());
        assert!(bare.hidden_columns.is_empty());
    }

    #[test]
    fn an_update_request_replaces_the_whole_owned_set() {
        let request = ViewUpdateRequest {
            mutation: mutation(4),
            view_id: 5,
            filter: BoardFilter {
                attention: vec![crate::board::AttentionState::StaleRun],
                ..BoardFilter::default()
            },
            expanded_groups: vec![BoardGroup::Staged],
            hidden_columns: vec![BoardGroup::Draft],
            mode: ViewMode::Register,
            done_placement: DonePlacement::Table,
            sorting: ViewSorting::Readiness,
        };
        let encoded = serde_json::to_value(&request).expect("the request encodes");
        assert_eq!(encoded["view_id"], json!(5));
        assert_eq!(encoded["filter"]["attention"], json!(["stale_run"]));
        let decoded: ViewUpdateRequest =
            serde_json::from_value(encoded).expect("the request decodes");
        assert_eq!(decoded, request);
    }

    #[test]
    fn rename_and_remove_carry_their_own_narrow_payloads() {
        let rename = ViewRenameRequest {
            mutation: mutation(2),
            view_id: 5,
            name: "Deep work".to_owned(),
        };
        let encoded = serde_json::to_value(&rename).expect("the rename encodes");
        assert_eq!(
            encoded,
            json!({
                "mutation": { "optimistic_version": 2, "idempotency_key": "key-1" },
                "view_id": 5,
                "name": "Deep work",
            })
        );
        let decoded: ViewRenameRequest =
            serde_json::from_value(encoded).expect("the rename decodes");
        assert_eq!(decoded, rename);

        let remove = ViewRemoveRequest {
            mutation: mutation(3),
            view_id: 5,
        };
        let encoded = serde_json::to_value(&remove).expect("the remove encodes");
        assert_eq!(encoded["view_id"], json!(5));
        let removed = ViewRemovedRecord { view_id: 5 };
        assert_eq!(
            serde_json::to_value(removed).expect("the record encodes"),
            json!({ "view_id": 5 })
        );
    }

    #[test]
    fn view_payloads_reject_unknown_fields_and_values() {
        let surprise = serde_json::from_value::<ViewCreateRequest>(json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
            "scope": "global",
            "name": "Everything",
            "mode": "board",
            "done_placement": "column",
            "sorting": "priority",
            "sort": "manual",
        }))
        .expect_err("a field outside the owned set is refused");
        assert!(surprise.to_string().contains("unknown field"));

        let wandering = serde_json::from_value::<SavedViewRecord>(json!({
            "id": 1,
            "name": "Everything",
            "scope": "global",
            "expanded_groups": [],
            "hidden_columns": [],
            "mode": "board",
            "done_placement": "column",
            "sorting": "manual",
            "is_default": true,
            "version": 1,
        }))
        .expect_err("values outside the vocabularies are refused");
        assert!(wandering.to_string().contains("unknown variant"));

        let query = serde_json::from_value::<ViewListQuery>(json!({ "scope": "global" }))
            .expect_err("the list query carries nothing");
        assert!(query.to_string().contains("unknown field"));
    }

    #[test]
    fn view_payloads_are_in_the_schema_registry() {
        let names: Vec<_> = schema_definitions()
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        for expected in [
            "DonePlacement",
            "SavedViewRecord",
            "ViewCreateRequest",
            "ViewListQuery",
            "ViewListResponse",
            "ViewMode",
            "ViewRemoveRequest",
            "ViewRemovedRecord",
            "ViewRenameRequest",
            "ViewScope",
            "ViewSorting",
            "ViewUpdateRequest",
        ] {
            assert!(names.contains(&expected), "{expected} must be registered");
        }
    }
}
