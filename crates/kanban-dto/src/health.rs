//! Health payload definitions: the one query reporting every
//! component's state (KAN-S13-US5, DR-RB-12).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::HerdrConnectionDiagnostics;

/// Request payload for the `health.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthQuery {}

/// The service component: the core process answering the query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceHealth {
    /// When this core process started serving; its state last
    /// changed by coming into being.
    pub started_at: String,
}

/// The database component: the one authoritative SQLite file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DatabaseHealth {
    /// The connection's journal mode; the product requires `wal`.
    pub journal_mode: String,
    /// The applied schema version after migration.
    pub schema_version: i64,
    /// When the newest timeline row was recorded; absent when
    /// nothing is recorded yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_change_at: Option<String>,
}

/// The scheduler component: the daily backup loop the core owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchedulerHealth {
    /// When the daily backup last succeeded, from the scheduler's
    /// persisted state; absent until one has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_backup_success_at: Option<String>,
}

/// The MCP component: the tool surface the core exposes to adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpHealth {
    /// How many MCP tool definitions the catalogue exposes.
    pub exposed_tools: u32,
}

/// One observed Herdr session, named by the Project it serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HerdrSessionHealth {
    /// The Project whose Herdr binding this session is.
    pub project_id: u64,
    /// The binding's live connection diagnostics, the same shape
    /// `herdr.settings.get` serves.
    pub diagnostics: HerdrConnectionDiagnostics,
}

/// The Herdr component: every session the core is observing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HerdrHealth {
    /// One entry per observed Project, in Project identity order.
    pub sessions: Vec<HerdrSessionHealth>,
}

/// The Workspace census across every Project, one count per health
/// state in the closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceHealthCounts {
    /// Present, clean, and unassigned.
    pub available: u32,
    /// Held by a Lane.
    pub assigned: u32,
    /// Present with an unclean working tree.
    pub dirty: u32,
    /// The path is absent or not a worktree of the repository.
    pub missing: u32,
    /// Retired by the operator; the record is preserved.
    pub retired: u32,
    /// Present, but the last git status read could not complete.
    pub unobserved: u32,
}

/// The Workspace component: the registered working copies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspacesHealth {
    /// The health census across every Project.
    pub by_health: WorkspaceHealthCounts,
    /// When a Workspace last changed on the timeline; absent until
    /// one has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_change_at: Option<String>,
}

/// Response payload for the `health.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthResponse {
    pub connected: bool,
    pub service_version: String,
    pub service: ServiceHealth,
    pub database: DatabaseHealth,
    pub scheduler: SchedulerHealth,
    pub mcp: McpHealth,
    pub herdr: HerdrHealth,
    pub workspaces: WorkspacesHealth,
}
