//! Operator Linear issue commands (MCP parity).

use std::sync::Arc;

use kanban_domain::{ProjectId, Ticket, TicketId};
use kanban_dto::{
    ApiError, IssueCreateLabelRequest, IssueCreateMilestoneRequest, IssueGetQuery,
    IssueLabelRecord, IssueLabelsQuery, IssueLabelsResponse, IssueListQuery, IssueListResponse,
    IssueMilestoneRecord, IssueMilestonesQuery, IssueMilestonesResponse, IssueProjectRecord,
    IssueProjectSettingsResponse, IssueProjectsResponse, IssueRecord, IssueSaveRequest,
    IssueSaveResponse, IssueSubIssueProgress, IssueUpdateProjectSettingsRequest,
    IssueWorkflowStateRecord, IssueWorkflowStatesQuery, IssueWorkflowStatesResponse,
    TaskMode as WireMode, TaskSubtype as WireSubtype, TicketCreateRequest, TicketKind,
    TicketPriority,
};
use serde_json::{Value, json};

use crate::comment::CommentStore;
use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::issue::IssueStore;
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;
use crate::spec::SpecStore;
use crate::ticket::{TicketStore, create_ticket, transition};

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
        parse_payload::<IssueSaveRequest>(payload)?;
        ParsedCommand::lift("issue", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: IssueSaveRequest = parse_payload(&command.payload)?;
        if let Some(identifier) = request.identifier.as_deref() {
            let (_, ticket_id) = self
                .0
                .issues
                .resolve_identifier(identifier)?
                .ok_or_else(|| ApiError::not_found(identifier))?;
            return Ok(self
                .0
                .tickets
                .find(TicketId::new(ticket_id as u64))?
                .ok_or_else(|| ApiError::not_found(identifier))?
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
        let request: IssueGetQuery = parse_payload(payload)?;
        let record = load_issue_record(&self.0, &request.identifier)?;
        serde_json::to_value(record).map_err(|_| ApiError::internal("issue response invalid"))
    }
}

struct ListIssues(IssueContext);

impl QueryHandler for ListIssues {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let request: IssueListQuery = parse_payload(payload)?;
        let response = list_issues(&self.0, &request)?;
        serde_json::to_value(response).map_err(|_| ApiError::internal("issue list invalid"))
    }
}

struct ListWorkflowStates(IssueContext);

impl QueryHandler for ListWorkflowStates {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let request: IssueWorkflowStatesQuery = parse_payload(payload)?;
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
        let request: IssueLabelsQuery = parse_payload(payload)?;
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
        let request: IssueMilestonesQuery = parse_payload(payload)?;
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
        parse_payload::<IssueCreateMilestoneRequest>(payload)?;
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
        parse_payload::<IssueCreateLabelRequest>(payload)?;
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

struct UpdateProjectSettings(IssueContext);

impl CommandHandler for UpdateProjectSettings {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<IssueUpdateProjectSettingsRequest>(payload)?;
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
    let mut project_id = request
        .project_id
        .ok_or_else(|| ApiError::invalid_request("project_id is required when creating"))?;
    if let Some(parent_ref) = request.parent_id.as_ref().and_then(|value| value.as_deref()) {
        let parent_id = resolve_parent_reference(ctx, parent_ref)?;
        let parent = ctx
            .tickets
            .find(TicketId::new(parent_id))?
            .ok_or_else(|| ApiError::not_found("parent issue"))?;
        let parent_project = parent.project().value();
        if request.project_id.is_some() && project_id != parent_project {
            return Err(ApiError::invalid_request(
                "sub-issues must belong to the parent's project",
            ));
        }
        project_id = parent_project;
    }
    let title = request
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .ok_or_else(|| ApiError::invalid_request("title is required when creating"))?;
    let create = TicketCreateRequest {
        mutation: request.mutation.clone(),
        project_id,
        kind: TicketKind::Task,
        priority: linear_to_wire_priority(request.priority.unwrap_or(3)),
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
    if let Some(priority) = request.priority {
        ctx.issues.set_linear_priority(ticket_id as i64, priority)?;
    }
    if let Some(due) = request.due_date.as_ref() {
        ctx.issues.set_due_date(ticket_id as i64, due.as_deref())?;
    }
    if let Some(state) = request.state.as_deref() {
        apply_named_state(ctx, ticket_id as i64, project_id, state, effects)?;
    }
    if let Some(milestone) = request.milestone_id.as_ref() {
        ctx.issues
            .set_milestone(ticket_id as i64, milestone.map(|value| value as i64))?;
    } else if let Some(parent_ref) = request.parent_id.as_ref().and_then(|value| value.as_deref()) {
        let parent_id = resolve_parent_reference(ctx, parent_ref)?;
        let fields = ctx.issues.issue_fields(parent_id as i64)?;
        if let Some(milestone_id) = fields.milestone_id {
            ctx.issues
                .set_milestone(ticket_id as i64, Some(milestone_id))?;
        }
    }
    apply_label_changes(ctx, project_id as i64, ticket_id as i64, request)?;
    apply_issue_extensions(ctx, ticket_id as i64, request)?;
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
        .ok_or_else(|| ApiError::not_found(identifier))?;
    let project_id = ctx
        .tickets
        .find(TicketId::new(ticket_id as u64))?
        .ok_or_else(|| ApiError::not_found(identifier))?
        .project()
        .value();
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
    if let Some(title) = request.title.as_deref().filter(|value| !value.is_empty()) {
        ctx.issues.set_title(ticket_id, title)?;
    }
    apply_label_changes(ctx, project_id as i64, ticket_id, request)?;
    apply_issue_extensions(ctx, ticket_id, request)?;
    finish_save_response(ctx, ticket_id as u64)
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
        ctx.issues.set_parent(ticket_id, parent_id)?;
    }
    if let Some(duplicate) = request.duplicate_of.as_ref() {
        ctx.issues
            .set_duplicate_of(ticket_id, duplicate.as_deref())?;
    }
    if let Some(sort) = request.sort_order {
        ctx.issues.set_sub_issue_sort(ticket_id, sort)?;
    }
    if let Some(add) = request.add_blocks.as_ref() {
        for target in add {
            ctx.issues.add_relation(ticket_id, target, "blocks")?;
        }
    }
    if let Some(remove) = request.remove_blocks.as_ref() {
        for target in remove {
            ctx.issues.remove_relation(ticket_id, target, "blocks")?;
        }
    }
    if let Some(add) = request.add_related.as_ref() {
        for target in add {
            ctx.issues.add_relation(ticket_id, target, "related")?;
        }
    }
    if let Some(remove) = request.remove_related.as_ref() {
        for target in remove {
            ctx.issues.remove_relation(ticket_id, target, "related")?;
        }
    }
    if let Some(add) = request.add_blocked_by.as_ref() {
        for target in add {
            ctx.issues.add_blocked_by(ticket_id, target)?;
        }
    }
    if let Some(remove) = request.remove_blocked_by.as_ref() {
        for target in remove {
            ctx.issues.remove_blocked_by(ticket_id, target)?;
        }
    }
    Ok(())
}

fn resolve_parent_reference(ctx: &IssueContext, reference: &str) -> Result<u64, ApiError> {
    if let Ok(id) = reference.parse::<u64>() {
        return Ok(id);
    }
    let (_, ticket_id) = ctx
        .issues
        .resolve_identifier(reference)?
        .ok_or_else(|| ApiError::not_found(reference))?;
    Ok(ticket_id as u64)
}

fn apply_label_changes(
    ctx: &IssueContext,
    project_id: i64,
    ticket_id: i64,
    request: &IssueSaveRequest,
) -> Result<(), ApiError> {
    if let Some(add) = request.add_labels.as_ref() {
        for label in add {
            ctx.issues.add_label(project_id, ticket_id, label)?;
        }
    }
    if let Some(remove) = request.remove_labels.as_ref() {
        for label in remove {
            ctx.issues.remove_label(project_id, ticket_id, label)?;
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
        .ok_or_else(|| ApiError::not_found(identifier))?;
    let ticket = ctx
        .tickets
        .find(TicketId::new(ticket_id as u64))?
        .ok_or_else(|| ApiError::not_found(identifier))?;
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
        due_date: (fields.due_date != "2099-12-31").then_some(fields.due_date),
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
    })
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
    let project_ids: Vec<u64> = match request.team_id.or(request.project_id) {
        Some(project_id) => vec![project_id],
        None => ctx
            .projects
            .list()?
            .into_iter()
            .map(|project| project.id().value())
            .collect(),
    };

    let mut matches = Vec::new();
    for project_id in project_ids {
        for ticket in ctx.tickets.list(ProjectId::new(project_id))? {
            let ticket_id = ticket.id().value();
            if let Some(state) = request.state.as_deref() {
                let workflow = ctx.issues.workflow_state_for_ticket(ticket_id as i64)?;
                if workflow.as_ref().is_none_or(|value| value.name != state) {
                    continue;
                }
            }
            if let Some(label) = request.label.as_deref() {
                let labels = ctx.issues.list_labels(ticket_id as i64)?;
                if !labels.iter().any(|held| held == label) {
                    continue;
                }
            }
            if let Some(parent_id) = request.parent_id {
                let fields = ctx.issues.issue_fields(ticket_id as i64)?;
                if fields.parent_ticket_id != Some(parent_id as i64) {
                    continue;
                }
            }
            if let Some(milestone_id) = request.milestone_id {
                let fields = ctx.issues.issue_fields(ticket_id as i64)?;
                if fields.milestone_id != Some(milestone_id as i64) {
                    continue;
                }
            }
            if request.assignee.is_some() {
                continue;
            }
            let record =
                load_issue_record(ctx, &ctx.issues.load_issue_key(ticket_id as i64)?.display())?;
            if let Some(since) = request.updated_since.as_deref()
                && record.updated_at.as_str() < since
            {
                continue;
            }
            if let Some(query) = request.query.as_deref() {
                let needle = query.to_lowercase();
                let haystack = format!(
                    "{} {} {}",
                    record.identifier, record.title, record.description
                )
                .to_lowercase();
                if !haystack.contains(&needle) {
                    continue;
                }
            }
            matches.push(record);
        }
    }
    matches.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| right.ticket_id.cmp(&left.ticket_id))
    });
    let start = if let Some(raw) = request.cursor.as_deref() {
        let cursor_ticket = raw
            .parse::<u64>()
            .map_err(|_| ApiError::invalid_request("cursor must be a numeric ticket id"))?;
        matches
            .iter()
            .position(|record| record.ticket_id == cursor_ticket)
            .map(|index| index + 1)
            .ok_or_else(|| ApiError::invalid_request("cursor does not match any listed issue"))?
    } else {
        0
    };
    let page: Vec<IssueRecord> = matches.into_iter().skip(start).collect();
    let next_cursor = if page.len() > limit as usize {
        Some(
            page[(limit as usize).saturating_sub(1)]
                .ticket_id
                .to_string(),
        )
    } else {
        None
    };
    let issues = page.into_iter().take(limit as usize).collect();
    Ok(IssueListResponse {
        issues,
        next_cursor,
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

fn linear_to_wire_priority(value: u8) -> TicketPriority {
    match value {
        1 => TicketPriority::Urgent,
        2 => TicketPriority::High,
        4 => TicketPriority::Low,
        _ => TicketPriority::Normal,
    }
}
