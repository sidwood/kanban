//! Linear issue persistence port (operator MCP and UI adapters).

use kanban_domain::{IssueKey, WorkflowStateType};
use kanban_dto::{ApiError, IssueRelationRecord};

use crate::TimelineEnvelope;

/// One workflow state row exposed to the application layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueWorkflowState {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub state_type: WorkflowStateType,
    pub legacy_state: String,
}

/// Project label metadata for operator lookups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueLabelRow {
    pub name: String,
    pub color: String,
    pub group: Option<String>,
    pub group_single_select: bool,
    pub retired: bool,
}

/// Project milestone metadata for operator lookups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueMilestoneRow {
    pub id: u64,
    pub name: String,
    pub target_date: Option<String>,
    pub issues_total: u32,
    pub issues_done: u32,
}

/// Issue-specific columns on the ticket row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueTicketFields {
    pub description: String,
    pub linear_priority: u8,
    pub due_date: String,
    pub milestone_id: Option<i64>,
    pub updated_at: String,
    pub created_at: String,
    pub version: u64,
    pub parent_ticket_id: Option<i64>,
    pub duplicate_of_ticket_id: Option<i64>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub canceled_at: Option<String>,
    pub assignee: String,
}

/// Loads and mutates Linear-shaped issue fields beside the Ticket port.
pub trait IssueStore: Send + Sync {
    fn set_description(&self, ticket_id: i64, description: &str) -> Result<(), ApiError>;
    fn patch_description(&self, ticket_id: i64, patch: &str) -> Result<String, ApiError>;
    fn set_linear_priority(&self, ticket_id: i64, priority: u8) -> Result<(), ApiError>;
    fn set_due_date(&self, ticket_id: i64, due_date: Option<&str>) -> Result<(), ApiError>;
    fn set_milestone(&self, ticket_id: i64, milestone_id: Option<i64>) -> Result<(), ApiError>;
    fn apply_free_state(
        &self,
        ticket_id: i64,
        project_id: i64,
        to_state: &IssueWorkflowState,
        envelope: TimelineEnvelope,
    ) -> Result<(), ApiError>;
    /// Record a new issue's first state: its workflow row, one open
    /// history row, and the entry stamps, without a transition.
    fn record_initial_state(&self, ticket_id: i64, state_name: &str) -> Result<(), ApiError>;
    fn list_labels(&self, ticket_id: i64) -> Result<Vec<String>, ApiError>;
    fn add_label(&self, project_id: i64, ticket_id: i64, name: &str) -> Result<(), ApiError>;
    fn remove_label(&self, project_id: i64, ticket_id: i64, name: &str) -> Result<(), ApiError>;
    fn load_issue_key(&self, ticket_id: i64) -> Result<IssueKey, ApiError>;
    fn workflow_state_for_ticket(
        &self,
        ticket_id: i64,
    ) -> Result<Option<IssueWorkflowState>, ApiError>;
    fn resolve_identifier(&self, identifier: &str) -> Result<Option<(IssueKey, i64)>, ApiError>;
    fn list_workflow_states(&self, project_id: i64) -> Result<Vec<IssueWorkflowState>, ApiError>;
    fn list_project_labels(&self, project_id: i64) -> Result<Vec<IssueLabelRow>, ApiError>;
    fn list_project_milestones(&self, project_id: i64) -> Result<Vec<IssueMilestoneRow>, ApiError>;
    fn resolve_state_by_name(
        &self,
        project_id: i64,
        name: &str,
    ) -> Result<IssueWorkflowState, ApiError>;
    fn issue_fields(&self, ticket_id: i64) -> Result<IssueTicketFields, ApiError>;
    fn list_relations(&self, ticket_id: i64) -> Result<Vec<IssueRelationRecord>, ApiError>;
    fn list_state_history(
        &self,
        ticket_id: i64,
    ) -> Result<Vec<kanban_dto::IssueStateHistoryRecord>, ApiError>;
    fn set_title(&self, ticket_id: i64, title: &str) -> Result<(), ApiError>;
    fn bump_version(&self, ticket_id: i64) -> Result<u64, ApiError>;
    fn set_parent(&self, ticket_id: i64, parent_id: Option<i64>) -> Result<(), ApiError>;
    fn list_children(&self, ticket_id: i64) -> Result<Vec<kanban_dto::IssueChildRecord>, ApiError>;
    fn add_relation(
        &self,
        from_ticket_id: i64,
        to_identifier: &str,
        kind: &str,
    ) -> Result<(), ApiError>;
    fn remove_relation(
        &self,
        from_ticket_id: i64,
        to_identifier: &str,
        kind: &str,
    ) -> Result<(), ApiError>;
    fn set_duplicate_of(
        &self,
        ticket_id: i64,
        target_identifier: Option<&str>,
    ) -> Result<(), ApiError>;
    fn set_sub_issue_sort(&self, ticket_id: i64, sort: f64) -> Result<(), ApiError>;
    fn append_sub_issue_sort(&self, ticket_id: i64, parent_id: i64) -> Result<(), ApiError>;
    fn list_duplicate_identifiers(&self, ticket_id: i64) -> Result<Vec<String>, ApiError>;
    fn sub_issue_progress(&self, ticket_id: i64) -> Result<(u32, u32), ApiError>;
    fn add_blocked_by(&self, ticket_id: i64, blocker_identifier: &str) -> Result<(), ApiError>;
    fn remove_blocked_by(&self, ticket_id: i64, blocker_identifier: &str) -> Result<(), ApiError>;
    fn sync_description_mentions(&self, ticket_id: i64, description: &str) -> Result<(), ApiError>;
    fn set_auto_close_sub_issues(&self, project_id: i64, enabled: bool) -> Result<(), ApiError>;
    fn auto_close_sub_issues_enabled(&self, project_id: i64) -> Result<bool, ApiError>;
    fn create_milestone(
        &self,
        project_id: i64,
        name: &str,
        target_date: Option<&str>,
    ) -> Result<IssueMilestoneRow, ApiError>;
    fn create_label(
        &self,
        project_id: i64,
        name: &str,
        color: &str,
        group: Option<&str>,
        group_single_select: bool,
    ) -> Result<IssueLabelRow, ApiError>;
    fn retire_label(&self, project_id: i64, name: &str) -> Result<IssueLabelRow, ApiError>;
    fn set_assignee(&self, ticket_id: i64, assignee: Option<&str>) -> Result<(), ApiError>;
}
