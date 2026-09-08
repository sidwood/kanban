//! Guarded landing: Ticket Lanes merge into a Spec integration
//! branch; a final integration review lands through the Seed; a
//! standalone Bug may land through the Seed when no active Spec is
//! attached. Every operation is recorded; paths outside this
//! topology are refused.

use std::path::Path;
use std::sync::Arc;

use kanban_domain::{
    LandingKind, LandingRefusal, LandingRequest, ProjectId, SpecExecutionState, SpecId, TicketKind,
    land_lane, land_seed, land_standalone_bug,
};
use kanban_dto::{
    ApiError, LandingBugRequest, LandingLaneRequest, LandingRecord, LandingSeedRequest,
    SpecIntegrationApproveRequest, SpecIntegrationClaimRequest, SpecIntegrationRecord,
    TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, RegistrationError};
use crate::lane::LaneStore;
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;
use crate::spec::SpecStore;
use crate::ticket::TicketStore;
use crate::timeline::TimelineEnvelope;
use crate::workspace::WorkspaceStore;

pub trait GitLanding {
    fn current_branch(&self, path: &str) -> Result<String, ApiError>;
    fn head(&self, path: &str) -> Result<String, ApiError>;
    fn require_clean(&self, path: &str) -> Result<(), ApiError>;
    fn require_base(&self, path: &str, base: &str) -> Result<(), ApiError>;
    fn merge(&self, draft: &LandingDraft) -> Result<String, ApiError>;
}

pub trait LandingStore: Send + Sync {
    fn prepare_landing(
        &self,
        key: &str,
        draft: &LandingDraft,
        envelope: TimelineEnvelope,
    ) -> Result<(), ApiError>;
    fn pending_landing(&self, key: &str) -> Result<LandingDraft, ApiError>;
    fn claim_integration(
        &self,
        record: &SpecIntegrationRecord,
        envelope: TimelineEnvelope,
    ) -> Result<SpecIntegrationRecord, ApiError>;
    fn integration_for(&self, spec_id: u64) -> Result<Option<SpecIntegrationRecord>, ApiError>;
    fn approve_integration(
        &self,
        spec_id: u64,
        tip: &str,
        envelope: TimelineEnvelope,
    ) -> Result<SpecIntegrationRecord, ApiError>;
    fn record_landing(
        &self,
        key: &str,
        record: &LandingDraft,
        landed_tip: &str,
        envelope: TimelineEnvelope,
    ) -> Result<LandingRecord, ApiError>;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LandingDraft {
    pub project_id: u64,
    pub kind: String,
    pub from_path: String,
    pub into_path: String,
    pub from_branch: String,
    pub into_branch: String,
    pub from_tip: String,
    pub into_tip: String,
    pub spec_id: Option<u64>,
    pub ticket_id: Option<u64>,
}

impl Core {
    #[allow(clippy::too_many_arguments)]
    pub fn register_landings(
        &mut self,
        store: Arc<dyn LandingStore>,
        projects: Arc<dyn ProjectStore>,
        specs: Arc<dyn SpecStore>,
        tickets: Arc<dyn TicketStore>,
        workspaces: Arc<dyn WorkspaceStore>,
        lanes: Arc<dyn LaneStore>,
        git: Arc<dyn GitLanding + Send + Sync>,
    ) -> Result<(), RegistrationError> {
        let context = LandingContext {
            store,
            projects,
            specs,
            tickets,
            workspaces,
            lanes,
            git,
        };
        self.register_command(
            "spec.integration.claim",
            Arc::new(ClaimIntegration(context.clone())),
        )?;
        self.register_command(
            "spec.integration.approve",
            Arc::new(ApproveIntegration(context.clone())),
        )?;
        for (name, kind) in [
            ("landing.lane", LandingKind::Lane),
            ("landing.seed", LandingKind::Seed),
            ("landing.bug", LandingKind::StandaloneBug),
        ] {
            self.register_command(
                name,
                Arc::new(LandCommand {
                    context: context.clone(),
                    kind,
                }),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct LandingContext {
    store: Arc<dyn LandingStore>,
    projects: Arc<dyn ProjectStore>,
    specs: Arc<dyn SpecStore>,
    tickets: Arc<dyn TicketStore>,
    workspaces: Arc<dyn WorkspaceStore>,
    lanes: Arc<dyn LaneStore>,
    git: Arc<dyn GitLanding + Send + Sync>,
}

impl LandingContext {
    fn ticket_for_source(
        &self,
        project: u64,
        path: &str,
    ) -> Result<kanban_domain::Ticket, ApiError> {
        for workspace in self.workspaces.list_for_project(ProjectId::new(project))? {
            if !workspace.is_retired() && same_path(workspace.registration().path(), path)? {
                let lane = self
                    .lanes
                    .find_by_workspace(ProjectId::new(project), workspace.id())?
                    .ok_or_else(|| {
                        ApiError::invalid_request("landing requires a registered Ticket Lane")
                    })?;
                let id = lane
                    .ticket_id()
                    .ok_or_else(|| ApiError::invalid_request("the landing Lane holds no Ticket"))?;
                return self
                    .tickets
                    .find(id)?
                    .ok_or_else(|| ApiError::not_found("lane ticket"));
            }
        }
        Err(ApiError::invalid_request(
            "landing requires a registered Ticket Lane Workspace",
        ))
    }
}

fn refuse(error: LandingRefusal) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

fn canonical_path(path: &str) -> Result<String, ApiError> {
    Path::new(path)
        .canonicalize()
        .map_err(|_| ApiError::invalid_request("landing requires an existing Workspace path"))?
        .into_os_string()
        .into_string()
        .map_err(|_| ApiError::invalid_request("landing requires a UTF-8 Workspace path"))
}

fn same_path(left: &str, right: &str) -> Result<bool, ApiError> {
    Ok(canonical_path(left)? == canonical_path(right)?)
}

fn transition(
    project_id: ProjectId,
    entity: TimelineEntityRef,
    action: &str,
    facts: Value,
) -> TimelineEnvelope {
    let mut detail = facts;
    detail
        .as_object_mut()
        .expect("landing facts are an object")
        .insert("action".to_owned(), Value::from(action));
    TimelineEnvelope::project(
        project_id.value(),
        TimelineEventKind::Transition,
        Some(entity),
        detail,
    )
}

struct ClaimIntegration(LandingContext);
impl CommandHandler for ClaimIntegration {
    fn parse(&self, value: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<SpecIntegrationClaimRequest>(value)?;
        ParsedCommand::lift("spec", value)
    }
    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: SpecIntegrationClaimRequest = parse_payload(&command.payload)?;
        Ok(self
            .0
            .specs
            .find(SpecId::new(request.spec_id))?
            .ok_or_else(|| ApiError::not_found("spec"))?
            .version())
    }
    fn apply(&self, command: &ParsedCommand, _: &dyn CommandEffects) -> Result<Value, ApiError> {
        let request: SpecIntegrationClaimRequest = parse_payload(&command.payload)?;
        let spec = self
            .0
            .specs
            .find(SpecId::new(request.spec_id))?
            .ok_or_else(|| ApiError::not_found("spec"))?;
        let workspace = self
            .0
            .workspaces
            .list_for_project(spec.project())?
            .into_iter()
            .find(|workspace| {
                same_path(workspace.registration().path(), &request.workspace_path).unwrap_or(false)
            })
            .ok_or_else(|| {
                ApiError::invalid_request(
                    "integration requires a registered Workspace in the Spec Project",
                )
            })?;
        let project = self
            .0
            .projects
            .find(spec.project())?
            .ok_or_else(|| ApiError::not_found("project"))?;
        if workspace.is_retired()
            || workspace.lane_id().is_some()
            || same_path(
                &request.workspace_path,
                project.registration().seed_workspace(),
            )?
            || request.branch.is_empty()
            || request.branch == project.registration().default_branch()
            || self.0.git.current_branch(&request.workspace_path)? != request.branch
            || self.0.store.integration_for(request.spec_id)?.is_some()
        {
            return Err(ApiError::invalid_request(
                "integration requires its own free Workspace and branch; existing claims cannot be overwritten",
            ));
        }
        self.0.git.require_clean(&request.workspace_path)?;
        let record = SpecIntegrationRecord {
            base_tip: self.0.git.head(&request.workspace_path)?,
            spec_id: request.spec_id,
            branch: request.branch,
            workspace_path: workspace.registration().path().to_owned(),
            workspace_id: Some(workspace.id().value()),
            review_approved: false,
            approved_tip: None,
        };
        let saved = self.0.store.claim_integration(
            &record,
            transition(
                spec.project(),
                TimelineEntityRef {
                    kind: TimelineEntityKind::Spec,
                    id: spec.id().value().to_string(),
                },
                "integration_claimed",
                json!({
                    "branch": record.branch,
                    "workspace_path": record.workspace_path,
                }),
            ),
        )?;
        serde_json::to_value(saved).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

struct ApproveIntegration(LandingContext);
impl CommandHandler for ApproveIntegration {
    fn parse(&self, value: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<SpecIntegrationApproveRequest>(value)?;
        ParsedCommand::lift("spec", value)
    }
    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: SpecIntegrationApproveRequest = parse_payload(&command.payload)?;
        Ok(self
            .0
            .specs
            .find(SpecId::new(request.spec_id))?
            .ok_or_else(|| ApiError::not_found("spec"))?
            .version())
    }
    fn apply(&self, command: &ParsedCommand, _: &dyn CommandEffects) -> Result<Value, ApiError> {
        let request: SpecIntegrationApproveRequest = parse_payload(&command.payload)?;
        let spec = self
            .0
            .specs
            .find(SpecId::new(request.spec_id))?
            .ok_or_else(|| ApiError::not_found("spec"))?;
        let integration = self
            .0
            .store
            .integration_for(request.spec_id)?
            .ok_or_else(|| ApiError::not_found("spec integration"))?;
        if self.0.git.current_branch(&integration.workspace_path)? != integration.branch {
            return Err(refuse(LandingRefusal::UnguardedPath));
        }
        let tip = self.0.git.head(&integration.workspace_path)?;
        if tip != request.reviewed_tip {
            return Err(ApiError::invalid_request(
                "the reviewed tip is not the current integration tip",
            ));
        }
        if request.reviewer.trim().is_empty() || request.evidence.trim().is_empty() {
            return Err(ApiError::invalid_request(
                "integration review requires a reviewer and evidence",
            ));
        }
        self.0.git.require_clean(&integration.workspace_path)?;
        let saved = self.0.store.approve_integration(
            request.spec_id,
            &tip,
            transition(
                spec.project(),
                TimelineEntityRef {
                    kind: TimelineEntityKind::Spec,
                    id: spec.id().value().to_string(),
                },
                "integration_approved",
                json!({ "spec_id": request.spec_id, "approved_tip": tip, "reviewer": request.reviewer, "evidence": request.evidence }),
            ),
        )?;
        serde_json::to_value(saved).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

fn plan_landing(
    context: &LandingContext,
    kind: LandingKind,
    project_id: u64,
    spec_id: Option<u64>,
    ticket_id: Option<u64>,
    from_path: &str,
    into_path: &str,
) -> Result<LandingDraft, ApiError> {
    let project = context
        .projects
        .find(ProjectId::new(project_id))?
        .ok_or_else(|| ApiError::not_found("project"))?;
    if project.is_archived() {
        return Err(ApiError::invalid_request(
            "archived Projects accept no landings",
        ));
    }
    context.git.require_clean(from_path)?;
    context.git.require_clean(into_path)?;
    let from_branch = context.git.current_branch(from_path)?;
    let into_branch = context.git.current_branch(into_path)?;
    let seed = project.registration().seed_workspace();
    let through_seed = same_path(into_path, seed)?;
    if from_branch.is_empty()
        || into_branch.is_empty()
        || (through_seed && into_branch != project.registration().default_branch())
    {
        return Err(ApiError::invalid_request(
            "landing requires attached branches and the Seed default branch",
        ));
    }
    let (integration_branch, spec_active, review_approved) = match spec_id {
        Some(id) if kind == LandingKind::StandaloneBug => {
            let spec = context
                .specs
                .find(SpecId::new(id))?
                .ok_or_else(|| ApiError::not_found("spec"))?;
            let active = !matches!(
                spec.execution(),
                SpecExecutionState::Complete | SpecExecutionState::Cancelled
            );
            (String::new(), active, false)
        }
        Some(spec_id) => {
            let spec = context
                .specs
                .find(SpecId::new(spec_id))?
                .ok_or_else(|| ApiError::not_found("spec"))?;
            let integration = context.store.integration_for(spec_id)?.ok_or_else(|| {
                ApiError::invalid_request("a Spec must own an integration branch")
            })?;
            if spec.project() != project.id() {
                return Err(ApiError::invalid_request(
                    "the integration Spec belongs to another Project",
                ));
            }
            let workspace = integration
                .workspace_id
                .and_then(|id| {
                    context
                        .workspaces
                        .find(kanban_domain::WorkspaceId::new(id))
                        .transpose()
                })
                .transpose()?
                .ok_or_else(|| ApiError::not_found("integration Workspace"))?;
            if workspace.is_retired()
                || workspace.registration().project_id() != project.id()
                || workspace.lane_id().is_some()
                || !same_path(workspace.registration().path(), &integration.workspace_path)?
            {
                return Err(ApiError::invalid_request(
                    "the owned integration Workspace is no longer available",
                ));
            }
            let integration_path = match kind {
                LandingKind::Lane => into_path,
                LandingKind::Seed => from_path,
                LandingKind::StandaloneBug => &integration.workspace_path,
            };
            if !same_path(integration_path, &integration.workspace_path)? {
                return Err(refuse(LandingRefusal::UnguardedPath));
            }
            if kind == LandingKind::Lane {
                context.git.require_base(from_path, &integration.base_tip)?;
            }
            (
                integration.branch,
                !matches!(
                    spec.execution(),
                    SpecExecutionState::Complete | SpecExecutionState::Cancelled
                ),
                integration.review_approved
                    && integration.approved_tip.as_deref()
                        == Some(context.git.head(from_path)?.as_str()),
            )
        }
        None => (String::new(), false, false),
    };
    let request = LandingRequest {
        kind,
        from_branch: from_branch.clone(),
        into_branch: into_branch.clone(),
        integration_branch,
        through_seed,
        spec_active,
        integration_review_approved: review_approved,
    };
    match kind {
        LandingKind::Lane => land_lane(&request).map_err(refuse)?,
        LandingKind::Seed => land_seed(&request).map_err(refuse)?,
        LandingKind::StandaloneBug => land_standalone_bug(&request).map_err(refuse)?,
    }
    let wire = match kind {
        LandingKind::Lane => "lane",
        LandingKind::Seed => "seed",
        LandingKind::StandaloneBug => "standalone_bug",
    };
    Ok(LandingDraft {
        project_id,
        kind: wire.to_owned(),
        from_path: canonical_path(from_path)?,
        into_path: canonical_path(into_path)?,
        from_branch,
        into_branch,
        from_tip: context.git.head(from_path)?,
        into_tip: context.git.head(into_path)?,
        spec_id,
        ticket_id,
    })
}

struct LandCommand {
    context: LandingContext,
    kind: LandingKind,
}

impl LandCommand {
    fn plan(&self, command: &ParsedCommand) -> Result<LandingDraft, ApiError> {
        let (project, spec, ticket, from, into) = match self.kind {
            LandingKind::Lane => {
                let r: LandingLaneRequest = parse_payload(&command.payload)?;
                let ticket = self.context.ticket_for_source(r.project_id, &r.from_path)?;
                if ticket.project().value() != r.project_id
                    || ticket.spec().map(|id| id.value()) != Some(r.spec_id)
                {
                    return Err(ApiError::invalid_request(
                        "the Ticket Lane must belong to the landing Spec and Project",
                    ));
                }
                (
                    r.project_id,
                    Some(r.spec_id),
                    Some(ticket.id().value()),
                    r.from_path,
                    r.into_path,
                )
            }
            LandingKind::Seed => {
                let r: LandingSeedRequest = parse_payload(&command.payload)?;
                (
                    r.project_id,
                    Some(r.spec_id),
                    None,
                    r.from_path,
                    r.into_path,
                )
            }
            LandingKind::StandaloneBug => {
                let r: LandingBugRequest = parse_payload(&command.payload)?;
                let ticket = self.context.ticket_for_source(r.project_id, &r.from_path)?;
                if ticket.kind() != TicketKind::Bug
                    || ticket.id().value() != r.ticket_id
                    || ticket.project().value() != r.project_id
                {
                    return Err(ApiError::invalid_request(
                        "the source Lane must hold this Bug in its Project",
                    ));
                }
                (
                    r.project_id,
                    ticket.spec().map(|id| id.value()),
                    Some(r.ticket_id),
                    r.from_path,
                    r.into_path,
                )
            }
        };
        plan_landing(
            &self.context,
            self.kind,
            project,
            spec,
            ticket,
            &from,
            &into,
        )
    }
}

fn landing_event(draft: &LandingDraft, key: &str, action: &str) -> TimelineEnvelope {
    transition(
        ProjectId::new(draft.project_id),
        TimelineEntityRef {
            kind: TimelineEntityKind::Project,
            id: draft.project_id.to_string(),
        },
        action,
        json!({"intent_key": key, "landing": draft}),
    )
}

impl CommandHandler for LandCommand {
    fn parse(&self, value: &Value) -> Result<ParsedCommand, ApiError> {
        match self.kind {
            LandingKind::Lane => {
                parse_payload::<LandingLaneRequest>(value)?;
            }
            LandingKind::Seed => {
                parse_payload::<LandingSeedRequest>(value)?;
            }
            LandingKind::StandaloneBug => {
                parse_payload::<LandingBugRequest>(value)?;
            }
        }
        ParsedCommand::lift("landing", value)
    }
    fn current_version(&self, _: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }
    fn prepare(&self, command: &ParsedCommand) -> Result<(), ApiError> {
        let draft = self.plan(command)?;
        self.context.store.prepare_landing(
            &command.idempotency_key,
            &draft,
            landing_event(&draft, &command.idempotency_key, "landing_started"),
        )
    }
    fn apply(&self, command: &ParsedCommand, _: &dyn CommandEffects) -> Result<Value, ApiError> {
        let draft = self
            .context
            .store
            .pending_landing(&command.idempotency_key)?;
        if draft != self.plan(command)? {
            return Err(ApiError::invalid_request(
                "landing inputs changed after intent; recovery is required",
            ));
        }
        let landed_tip = self.context.git.merge(&draft)?;
        let envelope = transition(
            ProjectId::new(draft.project_id),
            TimelineEntityRef {
                kind: TimelineEntityKind::Project,
                id: draft.project_id.to_string(),
            },
            "landed",
            json!({"intent_key": command.idempotency_key, "landing": draft, "landed_tip": landed_tip}),
        );
        let record = self.context.store.record_landing(
            &command.idempotency_key,
            &draft,
            &landed_tip,
            envelope,
        )?;
        serde_json::to_value(record).map_err(|error| ApiError::internal(&error.to_string()))
    }
}
