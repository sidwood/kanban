//! Operator Linear issue commands (MCP parity).

use std::sync::Arc;

use kanban_domain::{ProjectId, Ticket, TicketId};
use kanban_dto::LiveEventName;
use kanban_dto::{
    ApiError, IssueCreateLabelRequest, IssueCreateMilestoneRequest, IssueGetQuery,
    IssueLabelRecord, IssueLabelsQuery, IssueLabelsResponse, IssueListQuery, IssueListResponse,
    IssueMilestoneRecord, IssueMilestonesQuery, IssueMilestonesResponse, IssueProjectRecord,
    IssueProjectSettingsResponse, IssueProjectsResponse, IssueRecord, IssueRetireLabelRequest,
    IssueSaveRequest, IssueSaveResponse, IssueSubIssueProgress, IssueUpdateProjectSettingsRequest,
    IssueWorkflowStateRecord, IssueWorkflowStatesQuery, IssueWorkflowStatesResponse,
    TaskMode as WireMode, TaskSubtype as WireSubtype, TicketCreateRequest, TicketKind,
    TicketPriority,
};
use serde_json::{Value, json};

use crate::comment::CommentStore;
use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::events::emit_catalogued;
use crate::issue::{IssuePageQuery, IssueStore};
use crate::mutation::{
    CommandEffects, CommandHandler, ParsedCommand, parse_issue_payload, parse_payload,
};
use crate::project::{ProjectStore, resolve_project};
use crate::spec::SpecStore;
use crate::ticket::{TicketStore, create_ticket, record_of, transition};

/// MCP operations granted to installation-authenticated operator clients.
pub const OPERATOR_MCP_OPERATIONS: &[&str] = &[
    "issue.get",
    "issue.list",
    "issue.list_labels",
    "issue.list_milestones",
    "issue.list_projects",
    "issue.list_workflow_states",
    "issue.save",
    "issue.create_milestone",
    "issue.create_label",
    "issue.retire_label",
    "issue.update_project_settings",
    "comment.create",
    "comment.edit",
    "comment.list",
    "comment.revisions",
    "comment.resolve",
];

struct IssueContext {
    tickets: Arc<dyn TicketStore>,
    projects: Arc<dyn ProjectStore>,
    specs: Arc<dyn SpecStore>,
    issues: Arc<dyn IssueStore>,
}

impl Core {
    pub fn register_issue_operator(
        &mut self,
        tickets: Arc<dyn TicketStore>,
        projects: Arc<dyn ProjectStore>,
        specs: Arc<dyn SpecStore>,
        issues: Arc<dyn IssueStore>,
        comments: Arc<dyn CommentStore>,
    ) -> Result<(), RegistrationError> {
        let _comments = comments;
        self.register_command(
            "issue.save",
            Arc::new(SaveIssue(IssueContext {
                tickets: tickets.clone(),
                projects: projects.clone(),
                specs: specs.clone(),
                issues: issues.clone(),
            })),
        )?;
        self.register_query(
            "issue.get",
            Arc::new(GetIssue(IssueContext {
                tickets: tickets.clone(),
                projects: projects.clone(),
                specs: specs.clone(),
                issues: issues.clone(),
            })),
        )?;
        self.register_query(
            "issue.list",
            Arc::new(ListIssues(IssueContext {
                tickets: tickets.clone(),
                projects: projects.clone(),
                specs: specs.clone(),
                issues: issues.clone(),
            })),
        )?;
        self.register_query(
            "issue.list_workflow_states",
            Arc::new(ListWorkflowStates(IssueContext {
                tickets: tickets.clone(),
                projects: projects.clone(),
                specs: specs.clone(),
                issues: issues.clone(),
            })),
        )?;
        self.register_query(
            "issue.list_labels",
            Arc::new(ListLabels(IssueContext {
                tickets: tickets.clone(),
                projects: projects.clone(),
                specs: specs.clone(),
                issues: issues.clone(),
            })),
        )?;
        self.register_query(
            "issue.list_milestones",
            Arc::new(ListMilestones(IssueContext {
                tickets: tickets.clone(),
                projects: projects.clone(),
                specs: specs.clone(),
                issues: issues.clone(),
            })),
        )?;
        self.register_query(
            "issue.list_projects",
            Arc::new(ListProjects(IssueContext {
                tickets: tickets.clone(),
                projects: projects.clone(),
                specs: specs.clone(),
                issues: issues.clone(),
            })),
        )?;
        self.register_command(
            "issue.create_milestone",
            Arc::new(CreateMilestone(IssueContext {
                tickets: tickets.clone(),
                projects: projects.clone(),
                specs: specs.clone(),
                issues: issues.clone(),
            })),
        )?;
        self.register_command(
            "issue.create_label",
            Arc::new(CreateLabel(IssueContext {
                tickets: tickets.clone(),
                projects: projects.clone(),
                specs: specs.clone(),
                issues: issues.clone(),
            })),
        )?;
        self.register_command(
            "issue.retire_label",
            Arc::new(RetireLabel(IssueContext {
                tickets: tickets.clone(),
                projects: projects.clone(),
                specs: specs.clone(),
                issues: issues.clone(),
            })),
        )?;
        self.register_command(
            "issue.update_project_settings",
            Arc::new(UpdateProjectSettings(IssueContext {
                tickets,
                projects,
                specs,
                issues,
            })),
        )?;
        Ok(())
    }
}

struct SaveIssue(IssueContext);

impl CommandHandler for SaveIssue {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_issue_payload::<IssueSaveRequest>(payload)?;
        ParsedCommand::lift("issue", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: IssueSaveRequest = parse_payload(&command.payload)?;
        if let Some(identifier) = request.identifier.as_deref() {
            let (_, ticket_id) = self
                .0
                .issues
                .resolve_identifier(identifier)?
                .ok_or_else(|| ApiError::not_found_field("identifier", identifier))?;
            return Ok(self
                .0
                .tickets
                .find(TicketId::new(ticket_id as u64))?
                .ok_or_else(|| ApiError::not_found_field("identifier", identifier))?
                .version());
        }
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: IssueSaveRequest = parse_payload(&command.payload)?;
        if request.identifier.is_some() {
            update_issue(&self.0, &request, effects)
        } else {
            create_issue(&self.0, &request, effects)
        }
    }
}

struct GetIssue(IssueContext);

impl QueryHandler for GetIssue {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let request: IssueGetQuery = parse_issue_payload(payload)?;
        let record = load_issue_record(&self.0, &request.identifier)?;
        serde_json::to_value(record).map_err(|_| ApiError::internal("issue response invalid"))
    }
}

struct ListIssues(IssueContext);

impl QueryHandler for ListIssues {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let request: IssueListQuery = parse_issue_payload(payload)?;
        let response = list_issues(&self.0, &request)?;
        serde_json::to_value(response).map_err(|_| ApiError::internal("issue list invalid"))
    }
}

struct ListWorkflowStates(IssueContext);

impl QueryHandler for ListWorkflowStates {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let request: IssueWorkflowStatesQuery = parse_issue_payload(payload)?;
        resolve_project(self.0.projects.as_ref(), request.project_id)?;
        let states = self
            .0
            .issues
            .list_workflow_states(request.project_id as i64)?;
        let response = IssueWorkflowStatesResponse {
            states: states
                .into_iter()
                .map(|state| IssueWorkflowStateRecord {
                    name: state.name,
                    state_type: state.state_type.wire_name().to_owned(),
                })
                .collect(),
        };
        serde_json::to_value(response).map_err(|_| ApiError::internal("states response invalid"))
    }
}

struct ListLabels(IssueContext);

impl QueryHandler for ListLabels {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let request: IssueLabelsQuery = parse_issue_payload(payload)?;
        resolve_project(self.0.projects.as_ref(), request.project_id)?;
        let labels = self
            .0
            .issues
            .list_project_labels(request.project_id as i64)?
            .into_iter()
            .map(|label| IssueLabelRecord {
                name: label.name,
                color: label.color,
                group: label.group,
                group_single_select: label.group_single_select,
                retired: label.retired,
            })
            .collect();
        serde_json::to_value(IssueLabelsResponse { labels })
            .map_err(|_| ApiError::internal("labels response invalid"))
    }
}

struct ListMilestones(IssueContext);

impl QueryHandler for ListMilestones {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let request: IssueMilestonesQuery = parse_issue_payload(payload)?;
        resolve_project(self.0.projects.as_ref(), request.project_id)?;
        let milestones = self
            .0
            .issues
            .list_project_milestones(request.project_id as i64)?
            .into_iter()
            .map(|milestone| IssueMilestoneRecord {
                id: milestone.id,
                name: milestone.name,
                target_date: milestone.target_date,
                issues_total: milestone.issues_total,
                issues_done: milestone.issues_done,
            })
            .collect();
        serde_json::to_value(IssueMilestonesResponse { milestones })
            .map_err(|_| ApiError::internal("milestones response invalid"))
    }
}

struct ListProjects(IssueContext);

impl QueryHandler for ListProjects {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let _payload = payload;
        let projects = self
            .0
            .projects
            .list()?
            .into_iter()
            .map(|project| IssueProjectRecord {
                id: project.id().value(),
                code: project.code().as_str().to_owned(),
                name: project.registration().name().to_owned(),
            })
            .collect();
        serde_json::to_value(IssueProjectsResponse { projects })
            .map_err(|_| ApiError::internal("projects response invalid"))
    }
}

struct CreateMilestone(IssueContext);

impl CommandHandler for CreateMilestone {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_issue_payload::<IssueCreateMilestoneRequest>(payload)?;
        ParsedCommand::lift("issue", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: IssueCreateMilestoneRequest = parse_payload(&command.payload)?;
        resolve_issue_project(self.0.projects.as_ref(), request.project_id)?;
        refuse_blank_name(&request.name)?;
        let row = self.0.issues.create_milestone(
            request.project_id as i64,
            &request.name,
            request.target_date.as_deref(),
        )?;
        serde_json::to_value(IssueMilestoneRecord {
            id: row.id,
            name: row.name,
            target_date: row.target_date,
            issues_total: row.issues_total,
            issues_done: row.issues_done,
        })
        .map_err(|_| ApiError::internal("milestone response invalid"))
    }
}

struct CreateLabel(IssueContext);

impl CommandHandler for CreateLabel {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_issue_payload::<IssueCreateLabelRequest>(payload)?;
        ParsedCommand::lift("issue", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: IssueCreateLabelRequest = parse_payload(&command.payload)?;
        resolve_issue_project(self.0.projects.as_ref(), request.project_id)?;
        refuse_blank_name(&request.name)?;
        if request
            .group
            .as_deref()
            .is_some_and(|group| group.trim().is_empty())
        {
            return Err(ApiError::invalid_request_field(
                "group",
                "group must not be blank",
            ));
        }
        validate_label_colour(&request.color)?;
        let row = self.0.issues.create_label(
            request.project_id as i64,
            &request.name,
            &request.color,
            request.group.as_deref(),
            request.group_single_select,
        )?;
        serde_json::to_value(IssueLabelRecord {
            name: row.name,
            color: row.color,
            group: row.group,
            group_single_select: row.group_single_select,
            retired: row.retired,
        })
        .map_err(|_| ApiError::internal("label response invalid"))
    }
}

struct RetireLabel(IssueContext);

impl CommandHandler for RetireLabel {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_issue_payload::<IssueRetireLabelRequest>(payload)?;
        ParsedCommand::lift("issue", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: IssueRetireLabelRequest = parse_payload(&command.payload)?;
        resolve_issue_project(self.0.projects.as_ref(), request.project_id)?;
        let row = self
            .0
            .issues
            .retire_label(request.project_id as i64, &request.name)?;
        serde_json::to_value(IssueLabelRecord {
            name: row.name,
            color: row.color,
            group: row.group,
            group_single_select: row.group_single_select,
            retired: row.retired,
        })
        .map_err(|_| ApiError::internal("label response invalid"))
    }
}

struct UpdateProjectSettings(IssueContext);

impl CommandHandler for UpdateProjectSettings {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_issue_payload::<IssueUpdateProjectSettingsRequest>(payload)?;
        ParsedCommand::lift("issue", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: IssueUpdateProjectSettingsRequest = parse_payload(&command.payload)?;
        resolve_issue_project(self.0.projects.as_ref(), request.project_id)?;
        if let Some(enabled) = request.auto_close_sub_issues {
            self.0
                .issues
                .set_auto_close_sub_issues(request.project_id as i64, enabled)?;
        }
        let enabled = self
            .0
            .issues
            .auto_close_sub_issues_enabled(request.project_id as i64)?;
        let response = IssueProjectSettingsResponse {
            project_id: request.project_id,
            auto_close_sub_issues: enabled,
        };
        serde_json::to_value(response).map_err(|_| ApiError::internal("settings response invalid"))
    }
}

fn create_issue(
    ctx: &IssueContext,
    request: &IssueSaveRequest,
    effects: &dyn CommandEffects,
) -> Result<Value, ApiError> {
    let mut project_id = if let Some(id) = request.project_id {
        id
    } else if let Some(parent_ref) = request
        .parent_id
        .as_ref()
        .and_then(|value| value.as_deref())
    {
        let parent_id = resolve_parent_reference(ctx, parent_ref)?;
        ctx.tickets
            .find(TicketId::new(parent_id))?
            .ok_or_else(|| ApiError::not_found_field("parent_id", "parent issue"))?
            .project()
            .value()
    } else {
        return Err(ApiError::invalid_request_field(
            "project_id",
            "project_id is required when creating without parent_id",
        ));
    };
    if let Some(parent_ref) = request
        .parent_id
        .as_ref()
        .and_then(|value| value.as_deref())
    {
        let parent_id = resolve_parent_reference(ctx, parent_ref)?;
        let parent = ctx
            .tickets
            .find(TicketId::new(parent_id))?
            .ok_or_else(|| ApiError::not_found_field("parent_id", "parent issue"))?;
        let parent_project = parent.project().value();
        if request.project_id.is_some() && project_id != parent_project {
            return Err(ApiError::invalid_request_field(
                "project_id",
                "sub-issues must belong to the parent's project",
            ));
        }
        project_id = parent_project;
    }
    let title = request.title.as_deref().ok_or_else(|| {
        ApiError::invalid_request_field("title", "title is required when creating")
    })?;
    if title.trim().is_empty() {
        return Err(ApiError::invalid_request_field(
            "title",
            "a Ticket title cannot be blank",
        ));
    }
    if resolve_project(ctx.projects.as_ref(), project_id)?.is_archived() {
        return Err(ApiError::invalid_request_field(
            "project_id",
            "archived is terminal; the Project accepts no further changes",
        ));
    }
    let linear_priority = request.priority.unwrap_or(0);
    let create = TicketCreateRequest {
        mutation: request.mutation.clone(),
        project_id,
        kind: TicketKind::Task,
        priority: linear_to_wire_priority(linear_priority),
        spec_id: None,
        title: Some(title.to_owned()),
        actual_behaviour: None,
        reporter_evidence: None,
        slice: None,
        criteria: None,
        subtype: Some(WireSubtype::Operational),
        mode: Some(WireMode::Human),
        completion: Some(vec!["Issue tracked".into()]),
        scheduled_for: None,
        due: None,
    };
    let value = create_ticket(
        &create,
        ctx.projects.as_ref(),
        ctx.tickets.as_ref(),
        ctx.specs.as_ref(),
        effects,
    )?;
    let ticket_id = value["id"]
        .as_u64()
        .ok_or_else(|| ApiError::internal("ticket id missing"))?;
    if let Some(description) = request.description.as_deref() {
        ctx.issues.set_description(ticket_id as i64, description)?;
    }
    if request.priority.is_some() {
        ctx.issues
            .set_linear_priority(ticket_id as i64, linear_priority)?;
    } else {
        ctx.issues
            .set_initial_linear_priority(ticket_id as i64, 0)?;
    }
    if let Some(due) = request.due_date.as_ref() {
        ctx.issues.set_due_date(ticket_id as i64, due.as_deref())?;
    }
    let initial_state = request.state.as_deref().unwrap_or("Draft");
    ctx.issues
        .record_initial_state(ticket_id as i64, initial_state)?;
    if let Some(milestone) = request.milestone_id.as_ref() {
        ctx.issues
            .set_milestone(ticket_id as i64, milestone.map(|value| value as i64))?;
    } else if let Some(parent_ref) = request
        .parent_id
        .as_ref()
        .and_then(|value| value.as_deref())
    {
        let parent_id = resolve_parent_reference(ctx, parent_ref)?;
        let fields = ctx.issues.issue_fields(parent_id as i64)?;
        if let Some(milestone_id) = fields.milestone_id {
            ctx.issues
                .set_milestone(ticket_id as i64, Some(milestone_id))?;
        }
    }
    apply_label_changes(ctx, project_id as i64, ticket_id as i64, request)?;
    apply_issue_extensions(ctx, ticket_id as i64, request)?;
    if let Some(assignee) = request.assignee.as_ref() {
        ctx.issues
            .set_assignee(ticket_id as i64, assignee.as_deref())?;
    }
    finish_save_response(ctx, ticket_id as u64)
}

fn update_issue(
    ctx: &IssueContext,
    request: &IssueSaveRequest,
    effects: &dyn CommandEffects,
) -> Result<Value, ApiError> {
    let identifier = request
        .identifier
        .as_deref()
        .ok_or_else(|| ApiError::invalid_request("identifier is required when updating"))?;
    let (_, ticket_id) = ctx
        .issues
        .resolve_identifier(identifier)?
        .ok_or_else(|| ApiError::not_found_field("identifier", identifier))?;
    let ticket = ctx
        .tickets
        .find(TicketId::new(ticket_id as u64))?
        .ok_or_else(|| ApiError::not_found_field("identifier", identifier))?;
    let project_id = ticket.project().value();
    if request.project_id.is_some() {
        return Err(ApiError::invalid_request_field(
            "project_id",
            "moving issues between projects is not supported; project_id is only read when creating",
        ));
    }
    let project = ctx
        .projects
        .find(ticket.project())?
        .ok_or_else(|| ApiError::not_found(identifier))?;
    let before_state = ticket.state();
    let before_version = ticket.version();
    let project_code = project.code().clone();
    if project.is_archived() {
        return Err(ApiError::invalid_request_field(
            "project_id",
            "archived is terminal; the Project accepts no further changes",
        ));
    }
    if let Some(description) = request.description.as_deref() {
        ctx.issues.set_description(ticket_id, description)?;
    } else if let Some(patch) = request.description_patch.as_deref() {
        ctx.issues.patch_description(ticket_id, patch)?;
    }
    if let Some(priority) = request.priority {
        ctx.issues.set_linear_priority(ticket_id, priority)?;
    }
    if let Some(due) = request.due_date.as_ref() {
        ctx.issues.set_due_date(ticket_id, due.as_deref())?;
    }
    if let Some(milestone) = request.milestone_id.as_ref() {
        ctx.issues
            .set_milestone(ticket_id, milestone.map(|value| value as i64))?;
    }
    if let Some(state) = request.state.as_deref() {
        apply_named_state(ctx, ticket_id, project_id, state, effects)?;
    }
    if let Some(title) = request.title.as_deref() {
        if title.trim().is_empty() {
            return Err(ApiError::invalid_request_field(
                "title",
                "title must not be blank",
            ));
        }
        ctx.issues.set_title(ticket_id, title)?;
    }
    apply_label_changes(ctx, project_id as i64, ticket_id, request)?;
    apply_issue_extensions(ctx, ticket_id, request)?;
    if let Some(assignee) = request.assignee.as_ref() {
        ctx.issues.set_assignee(ticket_id, assignee.as_deref())?;
    }
    let after = ctx
        .tickets
        .find(TicketId::new(ticket_id as u64))?
        .ok_or_else(|| ApiError::not_found(identifier))?;
    announce_saved_issue(
        effects,
        before_state,
        before_version,
        &after,
        &project_code,
        request,
    );
    finish_save_response(ctx, ticket_id as u64)
}

fn announce_saved_issue(
    effects: &dyn CommandEffects,
    before_state: kanban_domain::TicketState,
    before_version: u64,
    after: &Ticket,
    code: &kanban_domain::ProjectCode,
    request: &IssueSaveRequest,
) {
    if after.version() == before_version && after.state() == before_state {
        return;
    }
    let state_changed = after.state() != before_state;
    if state_changed {
        emit_catalogued(
            effects,
            LiveEventName::TicketStateChanged,
            &record_of(after, code),
        );
    }
    let other_fields = request.title.is_some()
        || request.description.is_some()
        || request.description_patch.is_some()
        || request.priority.is_some()
        || request.due_date.is_some()
        || request.milestone_id.is_some()
        || request.add_labels.is_some()
        || request.remove_labels.is_some()
        || request.parent_id.is_some()
        || request.duplicate_of.is_some()
        || request.sort_order.is_some()
        || request.add_blocks.is_some()
        || request.remove_blocks.is_some()
        || request.add_related.is_some()
        || request.remove_related.is_some()
        || request.add_blocked_by.is_some()
        || request.remove_blocked_by.is_some()
        || request.assignee.is_some();
    if after.version() != before_version && (!state_changed || other_fields) {
        emit_catalogued(
            effects,
            LiveEventName::TicketEdited,
            &record_of(after, code),
        );
    }
}

fn resolve_listed_project(
    ctx: &IssueContext,
    project_id: u64,
    field: &str,
) -> Result<(), ApiError> {
    resolve_project(ctx.projects.as_ref(), project_id).map_err(|error| {
        if error.code == kanban_dto::ErrorCode::NotFound {
            ApiError::not_found_field(field, &format!("project {project_id}"))
        } else {
            error
        }
    })?;
    Ok(())
}
fn apply_issue_extensions(
    ctx: &IssueContext,
    ticket_id: i64,
    request: &IssueSaveRequest,
) -> Result<(), ApiError> {
    if let Some(parent) = request.parent_id.as_ref() {
        let parent_id = match parent {
            None => None,
            Some(reference) => Some(resolve_parent_reference(ctx, reference)? as i64),
        };
        let before = ctx.issues.issue_fields(ticket_id)?.parent_ticket_id;
        ctx.issues.set_parent(ticket_id, parent_id)?;
        if request.sort_order.is_none()
            && before != parent_id
            && let Some(parent_id) = parent_id
        {
            ctx.issues.append_sub_issue_sort(ticket_id, parent_id)?;
        }
    }
    if let Some(duplicate) = request.duplicate_of.as_ref() {
        ctx.issues
            .set_duplicate_of(ticket_id, duplicate.as_deref())?;
    }
    if let Some(sort) = request.sort_order {
        ctx.issues.set_sub_issue_sort(ticket_id, sort)?;
    }
    if let Some(add) = request.add_blocks.as_ref() {
        refuse_self_references(ctx, ticket_id, add, "add_blocks", "block")?;
        for target in add {
            name_relation_refusal(
                ctx.issues.add_relation(ticket_id, target, "blocks"),
                "add_blocks",
            )?;
        }
    }
    if let Some(remove) = request.remove_blocks.as_ref() {
        for target in remove {
            name_relation_refusal(
                ctx.issues.remove_relation(ticket_id, target, "blocks"),
                "remove_blocks",
            )?;
        }
    }
    if let Some(add) = request.add_related.as_ref() {
        refuse_self_references(ctx, ticket_id, add, "add_related", "relate to")?;
        for target in add {
            name_relation_refusal(
                ctx.issues.add_relation(ticket_id, target, "related"),
                "add_related",
            )?;
        }
    }
    if let Some(remove) = request.remove_related.as_ref() {
        for target in remove {
            name_relation_refusal(
                ctx.issues.remove_relation(ticket_id, target, "related"),
                "remove_related",
            )?;
        }
    }
    if let Some(add) = request.add_blocked_by.as_ref() {
        for target in add {
            name_relation_refusal(
                ctx.issues.add_blocked_by(ticket_id, target),
                "add_blocked_by",
            )?;
        }
    }
    if let Some(remove) = request.remove_blocked_by.as_ref() {
        for target in remove {
            name_relation_refusal(
                ctx.issues.remove_blocked_by(ticket_id, target),
                "remove_blocked_by",
            )?;
        }
    }
    Ok(())
}

/// A relation list's `not_found` refusal names the list it came from,
/// so the operator hears which key was unresolvable.
fn name_relation_refusal(
    outcome: Result<(), ApiError>,
    field: &'static str,
) -> Result<(), ApiError> {
    match outcome {
        Err(error) if error.code == kanban_dto::ErrorCode::NotFound => {
            Err(ApiError::not_found_field(field, &error.message))
        }
        Err(error)
            if error.code == kanban_dto::ErrorCode::InvalidRequest && error.field.is_none() =>
        {
            Err(ApiError::invalid_request_field(field, &error.message))
        }
        other => other,
    }
}

fn resolve_parent_reference(ctx: &IssueContext, reference: &str) -> Result<u64, ApiError> {
    if let Ok(id) = reference.parse::<u64>() {
        if ctx.tickets.find(TicketId::new(id))?.is_none() {
            return Err(ApiError::not_found_field("parent_id", reference));
        }
        return Ok(id);
    }
    let (_, ticket_id) = ctx
        .issues
        .resolve_identifier(reference)?
        .ok_or_else(|| ApiError::not_found_field("parent_id", reference))?;
    Ok(ticket_id as u64)
}

/// The project's stored spelling of one label name. An exact
/// spelling always wins; only when the project holds no exact
/// spelling does a case-insensitive match resolve, and then only
/// when exactly one held spelling folds to the name — two held
/// spellings of one name are ambiguous, so the caller's field
/// refuses the request. `None` when no label of the project
/// carries the name in any case.
fn canonical_label_name(
    ctx: &IssueContext,
    project_id: i64,
    name: &str,
    field: &'static str,
) -> Result<Option<String>, ApiError> {
    let held: Vec<String> = ctx
        .issues
        .list_project_labels(project_id)?
        .into_iter()
        .map(|label| label.name)
        .collect();
    if let Some(exact) = held.iter().find(|stored| stored.as_str() == name) {
        return Ok(Some(exact.clone()));
    }
    let mut folded = held
        .into_iter()
        .filter(|stored| equals_ignoring_case(stored, name));
    let Some(unique) = folded.next() else {
        return Ok(None);
    };
    if let Some(second) = folded.next() {
        return Err(ApiError::invalid_request_field(
            field,
            &format!(
                "labels `{unique}` and `{second}` both match `{name}`; \
                 use an exact spelling"
            ),
        ));
    }
    Ok(Some(unique))
}

fn apply_label_changes(
    ctx: &IssueContext,
    project_id: i64,
    ticket_id: i64,
    request: &IssueSaveRequest,
) -> Result<(), ApiError> {
    if let Some(add) = request.add_labels.as_ref() {
        let mut canonical = Vec::with_capacity(add.len());
        for label in add {
            canonical.push(
                canonical_label_name(ctx, project_id, label, "add_labels")?.ok_or_else(|| {
                    ApiError::invalid_request_field(
                        "add_labels",
                        &format!("unknown label `{label}`"),
                    )
                })?,
            );
        }
        ctx.issues.validate_label_adds(project_id, &canonical)?;
        for label in canonical {
            ctx.issues.add_label(project_id, ticket_id, &label)?;
        }
    }
    if let Some(remove) = request.remove_labels.as_ref() {
        for label in remove {
            let canonical = canonical_label_name(ctx, project_id, label, "remove_labels")?
                .ok_or_else(|| {
                    ApiError::not_found_field("remove_labels", &format!("unknown label `{label}`"))
                })?;
            ctx.issues.remove_label(project_id, ticket_id, &canonical)?;
        }
    }
    Ok(())
}

fn apply_named_state(
    ctx: &IssueContext,
    ticket_id: i64,
    project_id: u64,
    state_name: &str,
    _effects: &dyn CommandEffects,
) -> Result<(), ApiError> {
    let to = ctx
        .issues
        .resolve_state_by_name(project_id as i64, state_name)?;
    if ctx
        .issues
        .workflow_state_for_ticket(ticket_id)?
        .is_some_and(|current| current.name == to.name)
    {
        return Ok(());
    }
    let envelope = transition(
        ProjectId::new(project_id),
        TicketId::new(ticket_id as u64),
        "state.changed",
        json!({ "state": state_name }),
    );
    ctx.issues
        .apply_free_state(ticket_id, project_id as i64, &to, envelope)
}

fn finish_save_response(ctx: &IssueContext, ticket_id: u64) -> Result<Value, ApiError> {
    let key = ctx.issues.load_issue_key(ticket_id as i64)?;
    let response = IssueSaveResponse {
        identifier: key.display(),
        url: key.deep_link(),
        ticket_id,
    };
    serde_json::to_value(response).map_err(|_| ApiError::internal("save response invalid"))
}

fn load_issue_record(ctx: &IssueContext, identifier: &str) -> Result<IssueRecord, ApiError> {
    let (_, ticket_id) = ctx
        .issues
        .resolve_identifier(identifier)?
        .ok_or_else(|| ApiError::not_found_field("identifier", identifier))?;
    let ticket = ctx
        .tickets
        .find(TicketId::new(ticket_id as u64))?
        .ok_or_else(|| ApiError::not_found_field("identifier", identifier))?;
    let key = ctx.issues.load_issue_key(ticket_id)?;
    let state = ctx
        .issues
        .workflow_state_for_ticket(ticket_id)?
        .ok_or_else(|| ApiError::internal("workflow state missing"))?;
    let fields = ctx.issues.issue_fields(ticket_id)?;
    let labels = ctx.issues.list_labels(ticket_id)?;
    let relations = ctx.issues.list_relations(ticket_id)?;
    let state_history = ctx.issues.list_state_history(ticket_id)?;
    let parent = if let Some(parent_id) = fields.parent_ticket_id {
        Some(ctx.issues.load_issue_key(parent_id)?.display())
    } else {
        None
    };
    let children = ctx.issues.list_children(ticket_id)?;
    let duplicate_of = if let Some(target_id) = fields.duplicate_of_ticket_id {
        Some(ctx.issues.load_issue_key(target_id)?.display())
    } else {
        None
    };
    let duplicates = ctx.issues.list_duplicate_identifiers(ticket_id)?;
    let (sub_done, sub_total) = ctx.issues.sub_issue_progress(ticket_id)?;
    Ok(IssueRecord {
        identifier: key.display(),
        url: key.deep_link(),
        ticket_id: ticket_id as u64,
        project_id: ticket.project().value(),
        title: ticket_title(&ticket),
        description: fields.description,
        state: state.name,
        state_type: state.state_type.wire_name().to_owned(),
        priority: fields.linear_priority,
        due_date: fields.due_date,
        milestone_id: fields.milestone_id.map(|value| value as u64),
        labels,
        relations,
        state_history,
        version: fields.version,
        created_at: fields.created_at,
        updated_at: fields.updated_at,
        started_at: fields.started_at,
        completed_at: fields.completed_at,
        canceled_at: fields.canceled_at,
        parent,
        sub_issues: (sub_total > 0).then_some(IssueSubIssueProgress {
            done: sub_done,
            total: sub_total,
        }),
        duplicate_of,
        duplicates: (!duplicates.is_empty()).then_some(duplicates),
        children: (!children.is_empty()).then_some(children),
        assignee: (!fields.assignee.is_empty()).then_some(fields.assignee),
    })
}

fn equals_ignoring_case(left: &str, right: &str) -> bool {
    focaccia::unicode_full_case_eq(left, right)
}

fn list_issues(
    ctx: &IssueContext,
    request: &IssueListQuery,
) -> Result<IssueListResponse, ApiError> {
    let limit = if request.limit == 0 {
        50
    } else {
        request.limit.clamp(1, 100)
    };
    let window = request
        .updated_since
        .as_deref()
        .map(start_of_window)
        .transpose()?;
    let project_ids: Vec<u64> = match (request.team_id, request.project_id) {
        (Some(team_id), _) => {
            resolve_listed_project(ctx, team_id, "team_id")?;
            vec![team_id]
        }
        (None, Some(project_id)) => {
            resolve_listed_project(ctx, project_id, "project_id")?;
            vec![project_id]
        }
        (None, None) => ctx
            .projects
            .list()?
            .into_iter()
            .map(|project| project.id().value())
            .collect(),
    };
    let parent_ticket_id = request
        .parent_id
        .as_deref()
        .map(|parent| resolve_parent_filter(ctx, parent))
        .transpose()?;
    let cursor = request
        .cursor
        .as_deref()
        .map(|raw| {
            raw.parse::<u64>().map_err(|_| {
                ApiError::invalid_request_field("cursor", "cursor must be a numeric ticket id")
            })
        })
        .transpose()?;
    let page = ctx.issues.page_issue_ids(&IssuePageQuery {
        project_ids: &project_ids,
        state: request.state.as_deref(),
        label: request.label.as_deref(),
        parent_ticket_id,
        milestone_id: request.milestone_id.map(|value| value as i64),
        assignee: request.assignee.as_deref(),
        updated_since_floor: window.as_ref().map(|bound| bound.floor.as_str()),
        updated_since_holds_at_floor: window.as_ref().is_some_and(|bound| bound.holds_at_floor),
        query: request.query.as_deref(),
        cursor,
        limit,
    })?;
    let issues = page
        .ticket_ids
        .iter()
        .map(|ticket_id| load_issue_record_by_id(ctx, *ticket_id))
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = page
        .more
        .then(|| issues.last().map(|record| record.ticket_id.to_string()))
        .flatten();
    Ok(IssueListResponse {
        issues,
        next_cursor,
    })
}

fn load_issue_record_by_id(ctx: &IssueContext, ticket_id: u64) -> Result<IssueRecord, ApiError> {
    let identifier = ctx.issues.load_issue_key(ticket_id as i64)?.display();
    load_issue_record(ctx, &identifier)
}

fn start_of_window(raw: &str) -> Result<WindowBound, ApiError> {
    if let Ok(instant) =
        time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339)
    {
        return Ok(WindowBound::at(instant));
    }
    if let Some(instant) = window_from_duration(raw) {
        return Ok(WindowBound::at(instant));
    }
    let bytes = raw.as_bytes();
    let padded_shape = bytes.len() == 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..].iter().all(u8::is_ascii_digit);
    if padded_shape {
        let date = time::Date::parse(
            raw,
            &time::macros::format_description!("[year]-[month]-[day]"),
        )
        .map_err(|_| window_refused())?;
        return Ok(WindowBound::at(date.midnight().assume_utc()));
    }
    Err(window_refused())
}

struct WindowBound {
    floor: String,
    holds_at_floor: bool,
}

impl WindowBound {
    fn at(instant: time::OffsetDateTime) -> Self {
        let nanoseconds = instant.time().nanosecond();
        let holds_at_floor = nanoseconds.is_multiple_of(1_000_000);
        Self {
            floor: window_stamp(instant),
            holds_at_floor,
        }
    }
}

fn window_stamp(instant: time::OffsetDateTime) -> String {
    let shape = time::format_description::parse_borrowed::<2>(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z",
    )
    .expect("the window stamp shape is a valid format");
    instant
        .replace_time(instant.time().truncate_to_millisecond())
        .to_offset(time::UtcOffset::UTC)
        .format(&shape)
        .expect("an instant formats as a window stamp")
}

fn window_from_duration(raw: &str) -> Option<time::OffsetDateTime> {
    let span = parse_iso_8601_duration(raw)?;
    shift_by_span(time::OffsetDateTime::now_utc(), &span)
}

#[derive(Debug, Default, PartialEq)]
struct IsoDuration {
    negative: bool,
    months: i128,
    span: time::SignedDuration,
}

fn parse_iso_8601_duration(raw: &str) -> Option<IsoDuration> {
    let (negative, body) = match raw.as_bytes().first() {
        Some(b'-') => (true, &raw[1..]),
        Some(b'+') => (false, &raw[1..]),
        _ => (false, raw),
    };
    let body = body.strip_prefix('P')?;
    let (date_half, time_half) = match body.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (body, None),
    };
    let mut months = 0i128;
    let mut seconds = 0f64;
    let mut components = 0usize;
    let halves = [(date_half, false), (time_half.unwrap_or(""), true)];
    for (half, is_time) in halves {
        let mut seen: Vec<char> = Vec::new();
        for (number, designator) in duration_components(half)? {
            match (designator, is_time) {
                ('Y', false) => {
                    months = months.checked_add(i128::from(whole(number)?).checked_mul(12)?)?
                }
                ('M', false) => months = months.checked_add(i128::from(whole(number)?))?,
                ('W', false) => seconds += number * 7.0 * 86_400.0,
                ('D', false) => seconds += number * 86_400.0,
                ('H', true) => seconds += number * 3_600.0,
                ('M', true) => seconds += number * 60.0,
                ('S', true) => seconds += number,
                _ => return None,
            }
            if seen.contains(&designator) {
                return None;
            }
            seen.push(designator);
            components += 1;
        }
    }
    if components == 0 {
        return None;
    }
    Some(IsoDuration {
        negative,
        months,
        span: time::SignedDuration::checked_seconds_f64(seconds)?,
    })
}

fn whole(number: f64) -> Option<i64> {
    if number.fract() != 0.0 || !number.is_finite() || number >= 9.007_199_254_740_992e15 {
        return None;
    }
    Some(number as i64)
}

fn duration_components(half: &str) -> Option<Vec<(f64, char)>> {
    let mut components = Vec::new();
    let mut rest = half;
    while !rest.is_empty() {
        let digits_end = rest
            .find(|character: char| !(character.is_ascii_digit() || character == '.'))
            .unwrap_or(rest.len());
        let (digits, remainder) = rest.split_at(digits_end);
        let number: f64 = digits.parse().ok()?;
        let designator = remainder.chars().next()?.to_ascii_uppercase();
        rest = &remainder[designator.len_utf8()..];
        components.push((number, designator));
    }
    Some(components)
}

fn shift_by_span(
    instant: time::OffsetDateTime,
    span: &IsoDuration,
) -> Option<time::OffsetDateTime> {
    let mut date = instant.date();
    if span.months != 0 {
        let month_index = i128::from(date.year()) * 12 + i128::from(u8::from(date.month())) - 1;
        let shifted = month_index.checked_add(if span.negative {
            -span.months
        } else {
            span.months
        })?;
        let year = i32::try_from(shifted.div_euclid(12)).ok()?;
        let month = u8::try_from(shifted.rem_euclid(12) + 1).ok()?;
        let month = time::Month::try_from(month).ok()?;
        let day = date.day().min(month.length(year));
        date = time::Date::from_calendar_date(year, month, day).ok()?;
    }
    let calendar = date
        .with_time(instant.time())
        .assume_utc()
        .to_offset(instant.offset());
    let exact = if span.negative {
        span.span.checked_neg()?
    } else {
        span.span
    };
    calendar.checked_add(exact)
}

fn window_refused() -> ApiError {
    ApiError::invalid_request_field(
        "updated_since",
        "updated_since must be an RFC 3339 timestamp, a YYYY-MM-DD date, \
         or an ISO-8601 duration",
    )
}

fn resolve_parent_filter(ctx: &IssueContext, raw: &str) -> Result<i64, ApiError> {
    if raw.contains('-') {
        let (_, ticket_id) = ctx
            .issues
            .resolve_identifier(raw)?
            .ok_or_else(|| ApiError::not_found_field("parent_id", raw))?;
        return Ok(ticket_id);
    }
    raw.parse::<i64>().map_err(|_| {
        ApiError::invalid_request_field(
            "parent_id",
            "parent_id must be a ticket id or an issue key",
        )
    })
}

fn ticket_title(ticket: &Ticket) -> String {
    match ticket.body() {
        kanban_domain::TicketBody::Task(task) => task.title().to_owned(),
        kanban_domain::TicketBody::Bug(bug) => bug.title().to_owned(),
        kanban_domain::TicketBody::Implementation(implementation) => {
            implementation.slice().to_owned()
        }
    }
}

/// Resolve the project a milestone or label lands on, naming the
/// offending field when it does not exist.
fn resolve_issue_project(projects: &dyn ProjectStore, project_id: u64) -> Result<(), ApiError> {
    resolve_project(projects, project_id)
        .map_err(|error| {
            if error.code == kanban_dto::ErrorCode::NotFound {
                return ApiError::not_found_field("project_id", &format!("project {project_id}"));
            }
            error
        })
        .map(|_| ())
}

/// A milestone or label name must carry visible characters.
fn refuse_blank_name(name: &str) -> Result<(), ApiError> {
    if name.trim().is_empty() {
        return Err(ApiError::invalid_request_field(
            "name",
            "name must not be blank",
        ));
    }
    Ok(())
}

/// A label colour is exactly `#` followed by six hex digits.
fn validate_label_colour(color: &str) -> Result<(), ApiError> {
    let shape = ApiError::invalid_request_field("color", "color must be #RRGGBB");
    let Some(digits) = color.strip_prefix('#') else {
        return Err(shape);
    };
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(shape);
    }
    Ok(())
}

/// An issue cannot name itself in a relation it adds.
fn refuse_self_references(
    ctx: &IssueContext,
    ticket_id: i64,
    targets: &[String],
    field: &'static str,
    verb: &str,
) -> Result<(), ApiError> {
    for target in targets {
        let referenced = match resolve_parent_reference(ctx, target) {
            Ok(value) => value,
            Err(error) if error.code == kanban_dto::ErrorCode::NotFound => {
                return Err(ApiError::not_found_field(field, &error.message));
            }
            Err(error) => return Err(error),
        };
        if referenced as i64 == ticket_id {
            return Err(ApiError::invalid_request_field(
                field,
                &format!("an issue cannot {verb} itself"),
            ));
        }
    }
    Ok(())
}

fn linear_to_wire_priority(value: u8) -> TicketPriority {
    match value {
        1 => TicketPriority::Urgent,
        2 => TicketPriority::High,
        4 => TicketPriority::Low,
        _ => TicketPriority::Normal,
    }
}
