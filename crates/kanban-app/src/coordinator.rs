//! The Coordinator execution loop (KAN-S9-US2, DR-HB-15): claim a
//! Dispatch Request, select capacity, prepare a Workspace under the
//! reuse rules, launch the implementer through Herdr, and acknowledge
//! the run. Kanban wakes the Coordinator; only the Coordinator loop
//! prompts an implementation agent (DR-HB-16).

use std::sync::Arc;

use kanban_domain::{
    DispatchRequestId, DispatchStatus, LaneId, ProjectId, TicketId, WorkspaceId, execution_branch,
    execution_workspace_path, select_reusable_workspace,
};
use kanban_dto::{
    ApiError, TimelineEntityKind, TimelineEntityRef, TimelineEventKind, WorkspaceRecord,
};
use serde_json::{Value, json};

use crate::clone::CloneGuardStore;
use crate::dispatch::Core;
use crate::dispatch_request::DispatchStore;
use crate::lane::LaneStore;
use crate::mutation::parse_payload;
use crate::ticket::TicketStore;
use crate::timeline::TimelineEnvelope;
use crate::workspace::WorkspaceStore;

/// The closed vocabulary of Coordinator loop steps recorded on the
/// timeline with run and role correlation (KAN-T46-AC2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinatorStep {
    /// The Ticket was seated in a Lane before the claim.
    SeatLane,
    /// The Dispatch Request was claimed and capacity selected.
    Claim,
    /// A Workspace was prepared under the reuse rules.
    PrepareWorkspace,
    /// The prepared Workspace was assigned to the Lane.
    AssignWorkspace,
    /// The implementer was launched through Herdr.
    Launch,
    /// The run was acknowledged with profile snapshots.
    Acknowledge,
}

impl CoordinatorStep {
    fn wire_name(self) -> &'static str {
        match self {
            Self::SeatLane => "seat_lane",
            Self::Claim => "claim",
            Self::PrepareWorkspace => "prepare_workspace",
            Self::AssignWorkspace => "assign_workspace",
            Self::Launch => "launch",
            Self::Acknowledge => "acknowledge",
        }
    }
}

/// What one implementer launch asks Herdr for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplementerLaunch {
    /// The Dispatch Request executing.
    pub dispatch_request_id: u64,
    /// The Ticket the run serves.
    pub ticket_id: u64,
    /// The Lane the run executes in.
    pub lane_id: u64,
    /// The run, once acknowledged.
    pub run_id: Option<u64>,
    /// The operator message delivered to the implementer tab.
    pub message: String,
}

/// Launches implementation agents through Herdr. Only the Coordinator
/// loop uses this port; Kanban never prompts implementers elsewhere
/// (DR-HB-16).
pub trait CoordinatorHerdr: Send + Sync {
    /// Prompt the implementer role tab. Returns whether Herdr accepted
    /// the launch.
    fn launch_implementer(&self, request: ImplementerLaunch) -> Result<bool, ApiError>;
}

/// A Herdr port that records nothing, for cores that do not exercise
/// the Coordinator loop.
#[derive(Debug, Default)]
pub struct NoopCoordinatorHerdr;

impl CoordinatorHerdr for NoopCoordinatorHerdr {
    fn launch_implementer(&self, _request: ImplementerLaunch) -> Result<bool, ApiError> {
        Ok(false)
    }
}

/// What one Coordinator loop execution needs to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorLoopRequest {
    /// The Project whose Coordinator is executing.
    pub project_id: u64,
    /// The Dispatch Request the wake named.
    pub dispatch_request_id: u64,
}

/// What one Coordinator loop execution produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorLoopOutcome {
    /// The Lane the Ticket executed in.
    pub lane_id: u64,
    /// The Workspace prepared for execution.
    pub workspace_id: u64,
    /// The acknowledged run.
    pub run_id: u64,
    /// Whether the Dispatch Request was claimed in this pass.
    pub claimed: bool,
}

/// The Coordinator execution loop the service drives after a wake
/// (DR-HB-15). It calls the same commands the Coordinator would over
/// MCP, records each step on the timeline, and launches the
/// implementer only through the Herdr port.
pub struct CoordinatorLoop {
    core: Arc<Core>,
    timeline: Arc<dyn CloneGuardStore>,
    herdr: Arc<dyn CoordinatorHerdr>,
    tickets: Arc<dyn TicketStore>,
    lanes: Arc<dyn LaneStore>,
    workspaces: Arc<dyn WorkspaceStore>,
    dispatch: Arc<dyn DispatchStore>,
}

impl CoordinatorLoop {
    /// Wire the loop over the stores and ports the serving core shares.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        core: Arc<Core>,
        timeline: Arc<dyn CloneGuardStore>,
        herdr: Arc<dyn CoordinatorHerdr>,
        tickets: Arc<dyn TicketStore>,
        lanes: Arc<dyn LaneStore>,
        workspaces: Arc<dyn WorkspaceStore>,
        dispatch: Arc<dyn DispatchStore>,
    ) -> Self {
        Self {
            core,
            timeline,
            herdr,
            tickets,
            lanes,
            workspaces,
            dispatch,
        }
    }

    /// Execute one wake-prepare-launch-acknowledge pass for `request`.
    pub fn execute(
        &self,
        request: CoordinatorLoopRequest,
    ) -> Result<CoordinatorLoopOutcome, ApiError> {
        let project = ProjectId::new(request.project_id);
        let dispatch_request_id = request.dispatch_request_id;
        let queued = self
            .dispatch
            .find(DispatchRequestId::new(dispatch_request_id))?
            .ok_or_else(|| {
                ApiError::not_found(&format!("dispatch request {dispatch_request_id}"))
            })?;
        if queued.status() != DispatchStatus::Queued {
            return Err(ApiError::invalid_request(
                "the Coordinator loop expects a queued Dispatch Request",
            ));
        }

        let ticket_id = queued.ticket().value();
        let lane_id = self.seat_lane(project, queued.ticket(), dispatch_request_id)?;
        self.record_step(
            project,
            CoordinatorStep::SeatLane,
            ticket_id,
            dispatch_request_id,
            None,
            json!({
                "ticket_id": ticket_id,
                "lane_id": lane_id.value(),
            }),
        )?;

        let claimed = self.claim_dispatch(dispatch_request_id, queued.version())?;
        if !claimed.claimed {
            return Err(ApiError::invalid_request(
                "the Dispatch Request stayed queued for capacity",
            ));
        }
        self.record_step(
            project,
            CoordinatorStep::Claim,
            ticket_id,
            dispatch_request_id,
            None,
            json!({
                "ticket_id": ticket_id,
                "lane_id": lane_id.value(),
            }),
        )?;

        let ticket = self
            .tickets
            .find(queued.ticket())?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {ticket_id}")))?;
        let workspace_id = self.prepare_workspace(project, &ticket, dispatch_request_id)?;
        self.assign_workspace(
            project,
            lane_id,
            workspace_id,
            dispatch_request_id,
            ticket_id,
        )?;

        let launched = self.herdr.launch_implementer(ImplementerLaunch {
            dispatch_request_id,
            ticket_id,
            lane_id: lane_id.value(),
            run_id: None,
            message: format!("execute ticket {ticket_id}"),
        })?;
        if !launched {
            return Err(ApiError::invalid_request(
                "Herdr refused the implementer launch",
            ));
        }
        self.record_step(
            project,
            CoordinatorStep::Launch,
            ticket_id,
            dispatch_request_id,
            None,
            json!({
                "ticket_id": ticket_id,
                "lane_id": lane_id.value(),
                "workspace_id": workspace_id.value(),
                "role": "implementer",
            }),
        )?;

        let run = self.acknowledge_run(dispatch_request_id, claimed.version)?;
        let run_id = run["id"].as_u64().expect("the run has an identity");
        self.record_step(
            project,
            CoordinatorStep::Acknowledge,
            ticket_id,
            dispatch_request_id,
            Some(run_id),
            json!({
                "ticket_id": ticket_id,
                "lane_id": lane_id.value(),
                "workspace_id": workspace_id.value(),
            }),
        )?;

        Ok(CoordinatorLoopOutcome {
            lane_id: lane_id.value(),
            workspace_id: workspace_id.value(),
            run_id,
            claimed: true,
        })
    }

    fn seat_lane(
        &self,
        project: ProjectId,
        ticket: TicketId,
        dispatch_request_id: u64,
    ) -> Result<LaneId, ApiError> {
        let lanes = self.lanes.list_for_project(project)?;
        if let Some(lane) = lanes.iter().find(|lane| lane.ticket_id() == Some(ticket)) {
            return Ok(lane.id());
        }
        let created = self.core.command(
            "lane.create",
            &json!({
                "mutation": {
                    "optimistic_version": 0,
                    "idempotency_key": format!("coordinator-lane-{dispatch_request_id}"),
                },
                "project_id": project.value(),
            }),
        )?;
        let lane_id = LaneId::new(created["id"].as_u64().expect("the Lane has an identity"));
        self.core.command(
            "lane.ticket.assign",
            &json!({
                "mutation": {
                    "optimistic_version": 1,
                    "idempotency_key": format!("coordinator-seat-{dispatch_request_id}"),
                },
                "lane_id": lane_id.value(),
                "ticket_id": ticket.value(),
            }),
        )?;
        Ok(lane_id)
    }

    fn claim_dispatch(
        &self,
        dispatch_request_id: u64,
        version: u64,
    ) -> Result<ClaimedDispatch, ApiError> {
        let response = self.core.command(
            "dispatch.claim",
            &json!({
                "mutation": {
                    "optimistic_version": version,
                    "idempotency_key": format!("coordinator-claim-{dispatch_request_id}"),
                },
                "dispatch_request_id": dispatch_request_id,
            }),
        )?;
        Ok(ClaimedDispatch {
            claimed: response["claimed"] == json!(true),
            version: response["request"]["version"]
                .as_u64()
                .expect("the request carries a version"),
        })
    }

    fn prepare_workspace(
        &self,
        project: ProjectId,
        ticket: &kanban_domain::Ticket,
        dispatch_request_id: u64,
    ) -> Result<WorkspaceId, ApiError> {
        let ticket_id = ticket.id().value();
        let ticket_number = ticket.number().value();
        let branch = execution_branch(ticket_number);
        let path = execution_workspace_path(ticket_number);
        let listed = self.workspaces.list_for_project(project)?;
        if let Some(selected) = select_reusable_workspace(&listed, ticket_number) {
            let workspace = listed
                .iter()
                .find(|workspace| workspace.id() == selected)
                .expect("the selected Workspace is listed");
            let observed = self.core.command(
                "workspace.observe",
                &json!({
                    "mutation": {
                        "optimistic_version": workspace.version(),
                        "idempotency_key": format!("coordinator-observe-reuse-{dispatch_request_id}"),
                    },
                    "workspace_id": selected.value(),
                }),
            )?;
            let record: WorkspaceRecord = parse_payload(&observed)?;
            if !record.reuse.reusable {
                return Err(ApiError::invalid_request(
                    "the prepared Workspace is not reusable under the reuse rules",
                ));
            }
            if record.observation.branch.as_deref() != Some(branch.as_str()) {
                return Err(ApiError::invalid_request(
                    "the reused Workspace checkout must match the execution branch",
                ));
            }
            self.record_step(
                project,
                CoordinatorStep::PrepareWorkspace,
                ticket_id,
                dispatch_request_id,
                None,
                json!({
                    "workspace_id": selected.value(),
                    "reused": true,
                    "ticket_id": ticket_id,
                    "path": path,
                    "branch": branch,
                }),
            )?;
            return Ok(selected);
        }

        let created = self.core.command(
            "clone.create",
            &json!({
                "mutation": {
                    "optimistic_version": 0,
                    "idempotency_key": format!("coordinator-clone-{dispatch_request_id}"),
                },
                "project_id": project.value(),
                "path": path,
                "branch": branch,
            }),
        )?;
        let created: kanban_dto::CloneCreatedRecord = parse_payload(&created)?;
        let workspace_id = WorkspaceId::new(created.workspace_id);
        let workspace = self
            .workspaces
            .find(workspace_id)?
            .ok_or_else(|| ApiError::internal("the created clone has no adopted Workspace"))?;
        let observed = self.core.command(
            "workspace.observe",
            &json!({
                "mutation": {
                    "optimistic_version": workspace.version(),
                    "idempotency_key": format!("coordinator-observe-{dispatch_request_id}"),
                },
                "workspace_id": workspace_id.value(),
            }),
        )?;
        let record: WorkspaceRecord = parse_payload(&observed)?;
        if !record.reuse.reusable {
            return Err(ApiError::invalid_request(
                "the prepared Workspace is not reusable under the reuse rules",
            ));
        }
        if record.observation.branch.as_deref() != Some(branch.as_str()) {
            return Err(ApiError::invalid_request(
                "the created Workspace checkout must match the execution branch",
            ));
        }
        self.record_step(
            project,
            CoordinatorStep::PrepareWorkspace,
            ticket_id,
            dispatch_request_id,
            None,
            json!({
                "workspace_id": workspace_id.value(),
                "reused": false,
                "ticket_id": ticket_id,
                "path": path,
                "branch": branch,
            }),
        )?;
        Ok(workspace_id)
    }

    fn assign_workspace(
        &self,
        project: ProjectId,
        lane: LaneId,
        workspace: WorkspaceId,
        dispatch_request_id: u64,
        ticket_id: u64,
    ) -> Result<(), ApiError> {
        let lane_record = self
            .lanes
            .find(lane)?
            .ok_or_else(|| ApiError::not_found(&format!("lane {}", lane.value())))?;
        self.core.command(
            "lane.workspace.assign",
            &json!({
                "mutation": {
                    "optimistic_version": lane_record.version(),
                    "idempotency_key": format!("coordinator-assign-{dispatch_request_id}"),
                },
                "lane_id": lane.value(),
                "workspace_id": workspace.value(),
            }),
        )?;
        self.record_step(
            project,
            CoordinatorStep::AssignWorkspace,
            ticket_id,
            dispatch_request_id,
            None,
            json!({
                "lane_id": lane.value(),
                "workspace_id": workspace.value(),
            }),
        )?;
        Ok(())
    }

    fn acknowledge_run(&self, dispatch_request_id: u64, version: u64) -> Result<Value, ApiError> {
        self.core.command(
            "run.acknowledge",
            &json!({
                "mutation": {
                    "optimistic_version": version,
                    "idempotency_key": format!("coordinator-ack-{dispatch_request_id}"),
                },
                "dispatch_request_id": dispatch_request_id,
            }),
        )
    }

    fn record_step(
        &self,
        project: ProjectId,
        step: CoordinatorStep,
        ticket_id: u64,
        dispatch_request_id: u64,
        run_id: Option<u64>,
        facts: Value,
    ) -> Result<(), ApiError> {
        let mut detail = facts;
        let object = detail
            .as_object_mut()
            .expect("coordinator step facts are a JSON object");
        object.insert("action".to_owned(), json!("coordinator_step"));
        object.insert("step".to_owned(), json!(step.wire_name()));
        if !object.contains_key("role") {
            object.insert("role".to_owned(), json!("coordinator"));
        }
        object.insert("dispatch_request_id".to_owned(), json!(dispatch_request_id));
        if let Some(run_id) = run_id {
            object.insert("run_id".to_owned(), json!(run_id));
        }
        let entity = match run_id {
            Some(run_id) => Some(TimelineEntityRef {
                kind: TimelineEntityKind::Run,
                id: run_id.to_string(),
            }),
            None => Some(TimelineEntityRef {
                kind: TimelineEntityKind::Ticket,
                id: ticket_id.to_string(),
            }),
        };
        self.timeline.append(TimelineEnvelope::project(
            project.value(),
            TimelineEventKind::Run,
            entity,
            detail,
        ))
    }
}

struct ClaimedDispatch {
    claimed: bool,
    version: u64,
}
