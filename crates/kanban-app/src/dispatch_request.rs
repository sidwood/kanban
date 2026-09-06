//! Dispatch Request commands and queries: create a durable request,
//! claim it atomically, and list the queue in deterministic order
//! (KAN-S9-US1, DR-EP-08, DR-HB-14, DR-HB-16). Capacity evaluation
//! is KAN-T37's; the claim writes the decision inside one storage
//! transaction so concurrent claimants see exactly one winner.
//! Creating a request wakes the Project Coordinator after the write
//! commits and never launches an implementation agent.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use kanban_domain::{
    ActiveRun, CapacityInputs, ClaimDecision, DispatchRequest, DispatchRequestId, DispatchStatus,
    GlobalCapacity, Lane, Priority, Project, ProjectCapacity, ProjectId, Ticket, TicketId,
    compute_readiness, decide_claim, evaluate_capacity, refuse_duplicate_open, sort_queue,
};
use kanban_dto::{
    ApiError, DispatchClaimRequest, DispatchClaimResponse, DispatchQueueQuery,
    DispatchQueueResponse, DispatchRequestCreateRequest, DispatchRequestRecord,
    DispatchStatus as WireStatus, LiveEventName, TicketPriority, TimelineEntityKind,
    TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::capacity::CapacityStore;
use crate::dependency::DependencyStore;
use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::events::emit_catalogued;
use crate::lane::LaneStore;
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::profile::ProfileStore;
use crate::project::ProjectStore;
use crate::ticket::TicketStore;
use crate::timeline::TimelineEnvelope;

/// The facts the Coordinator wake needs after a Dispatch Request is
/// durably queued. The service sends them over the Herdr session
/// socket; tests record them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorWakeRequest {
    /// The Project whose Coordinator to wake.
    pub project_id: u64,
    /// The Dispatch Request that just entered the queue.
    pub dispatch_request_id: u64,
    /// The Project's Seed Workspace, the product workspace the
    /// session maps to.
    pub seed_workspace: String,
    /// The target Herdr workspace inside that session.
    pub herdr_workspace: String,
    /// The named session, when the Project selected one.
    pub herdr_session: Option<String>,
}

/// Wakes the Project Coordinator after a Dispatch Request commits.
/// Implementations talk to Herdr; they must not launch implementation
/// agents.
pub trait CoordinatorWake: Send + Sync {
    /// Wake the Coordinator for `request`. Failures stay with the
    /// implementation: the Dispatch Request is already durable.
    fn wake(&self, request: CoordinatorWakeRequest);
}

/// A wake port that records nothing, for cores that do not exercise
/// the Coordinator path.
#[derive(Debug, Default)]
pub struct NoopCoordinatorWake;

impl CoordinatorWake for NoopCoordinatorWake {
    fn wake(&self, _request: CoordinatorWakeRequest) {}
}

/// The capacity numbers and occupied Lane count one claim reads.
/// Claimed Dispatch Requests themselves are the active runs; the
/// store loads those inside the same transaction. The Lane count
/// excludes the Lane holding the candidate's own Ticket: the
/// evaluation adds the candidate once, so a Ticket already seated in
/// a Lane is never counted twice.
#[derive(Debug, Clone)]
pub struct ClaimContext {
    /// The global capacity defaults.
    pub defaults: GlobalCapacity,
    /// The candidate Project's caps, when it imposes any.
    pub project_caps: Option<ProjectCapacity>,
    /// The candidate Project's count of Lanes holding a Ticket other
    /// than the candidate's own.
    pub active_lanes: u64,
}

/// The snapshotted facts one enqueue writes: the Ticket, its
/// priority and readiness, and the profile families it will draw.
#[derive(Debug, Clone)]
pub struct DispatchEnqueue {
    /// The Project the request belongs to.
    pub project: ProjectId,
    /// The Ticket the request dispatches.
    pub ticket: TicketId,
    /// The priority snapshotted at enqueue.
    pub priority: Priority,
    /// Whether the Ticket was ready at enqueue.
    pub ready: bool,
    /// The snapshotted harness family.
    pub harness: String,
    /// The snapshotted model family.
    pub model: String,
    /// The snapshotted usage pool.
    pub usage_pool: String,
    /// When the request entered the queue, as unix seconds.
    pub created_at: u64,
}

/// The storage port Dispatch Request operations call through.
/// `try_claim` is the atomic claim: status, capacity, and the row
/// change share one write so concurrent claimants cannot both win.
pub trait DispatchStore: Send + Sync {
    /// Insert a fresh queued request. Storage assigns the identity
    /// and asks `envelope` for the timeline row that identity belongs
    /// in. A Ticket that already has an open request is refused.
    fn enqueue(
        &self,
        draft: &DispatchEnqueue,
        envelope: &dyn Fn(DispatchRequestId) -> TimelineEnvelope,
    ) -> Result<DispatchRequest, ApiError>;
    /// Load one request, if it exists.
    fn find(&self, id: DispatchRequestId) -> Result<Option<DispatchRequest>, ApiError>;
    /// The open request for `ticket`, if it has one.
    fn open_for_ticket(&self, ticket: TicketId) -> Result<Option<DispatchStatus>, ApiError>;
    /// Claim `id` inside one transaction: reload, evaluate capacity
    /// against currently claimed requests, and persist a win.
    fn try_claim(
        &self,
        id: DispatchRequestId,
        context: &ClaimContext,
        envelope: TimelineEnvelope,
    ) -> Result<(DispatchRequest, ClaimDecision), ApiError>;
    /// Every queued request of one Project, unsorted; the application
    /// layer applies the domain order.
    fn list_queued(&self, project: ProjectId) -> Result<Vec<DispatchRequest>, ApiError>;
}

/// Shared ports the dispatch operations call through.
struct DispatchContext {
    requests: Arc<dyn DispatchStore>,
    tickets: Arc<dyn TicketStore>,
    profiles: Arc<dyn ProfileStore>,
    projects: Arc<dyn ProjectStore>,
    capacity: Arc<dyn CapacityStore>,
    lanes: Arc<dyn LaneStore>,
    dependencies: Arc<dyn DependencyStore>,
    wake: Arc<dyn CoordinatorWake>,
}

impl Clone for DispatchContext {
    fn clone(&self) -> Self {
        Self {
            requests: self.requests.clone(),
            tickets: self.tickets.clone(),
            profiles: self.profiles.clone(),
            projects: self.projects.clone(),
            capacity: self.capacity.clone(),
            lanes: self.lanes.clone(),
            dependencies: self.dependencies.clone(),
            wake: self.wake.clone(),
        }
    }
}

impl Core {
    /// Register the Dispatch Request operations.
    #[allow(clippy::too_many_arguments)]
    pub fn register_dispatch(
        &mut self,
        requests: Arc<dyn DispatchStore>,
        tickets: Arc<dyn TicketStore>,
        profiles: Arc<dyn ProfileStore>,
        projects: Arc<dyn ProjectStore>,
        capacity: Arc<dyn CapacityStore>,
        lanes: Arc<dyn LaneStore>,
        dependencies: Arc<dyn DependencyStore>,
        wake: Arc<dyn CoordinatorWake>,
    ) -> Result<(), RegistrationError> {
        let context = DispatchContext {
            requests,
            tickets,
            profiles,
            projects,
            capacity,
            lanes,
            dependencies,
            wake,
        };
        self.register_command(
            "dispatch.request",
            Arc::new(CreateDispatchRequest(context.clone())),
        )?;
        self.register_command(
            "dispatch.claim",
            Arc::new(ClaimDispatchRequest(context.clone())),
        )?;
        self.register_query(
            "dispatch.queue",
            Arc::new(ListDispatchQueue {
                requests: context.requests,
                projects: context.projects,
            }),
        )?;
        Ok(())
    }
}

/// Serves `dispatch.request`.
struct CreateDispatchRequest(DispatchContext);

impl CommandHandler for CreateDispatchRequest {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<DispatchRequestCreateRequest>(payload)?;
        ParsedCommand::lift("dispatch", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: DispatchRequestCreateRequest = parse_payload(&command.payload)?;
        let (project, ticket) = self.open(request.ticket_id)?;
        let profile_name = ticket.profile().ok_or_else(|| {
            ApiError::invalid_request("a Dispatch Request requires an assigned Execution Profile")
        })?;
        let profile =
            self.0.profiles.find(profile_name)?.ok_or_else(|| {
                ApiError::not_found(&format!("profile {}", profile_name.as_str()))
            })?;
        refuse_duplicate_open(ticket.id(), self.0.requests.open_for_ticket(ticket.id())?)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let ready = self.ticket_is_ready(&ticket)?;
        let created_at = unix_now();
        let queued = self.0.requests.enqueue(
            &DispatchEnqueue {
                project: project.id(),
                ticket: ticket.id(),
                priority: ticket.priority(),
                ready,
                harness: profile.harness().to_owned(),
                model: profile.model().to_owned(),
                usage_pool: profile.usage_pool().to_owned(),
                created_at,
            },
            &|id| {
                transition(
                    project.id(),
                    ticket.id(),
                    id,
                    "requested",
                    json!({
                        "ticket_id": ticket.id().value(),
                        "status": DispatchStatus::Queued.wire_name(),
                    }),
                )
            },
        )?;
        let record = encode_record(&queued);
        emit_catalogued(effects, LiveEventName::DispatchRequested, &record);
        let wake = CoordinatorWakeRequest {
            project_id: project.id().value(),
            dispatch_request_id: queued.id().value(),
            seed_workspace: project.registration().seed_workspace().to_owned(),
            herdr_workspace: project.registration().herdr_workspace().to_owned(),
            herdr_session: project.registration().herdr_session().map(str::to_owned),
        };
        let wake_port = self.0.wake.clone();
        effects.after_commit(Box::new(move || wake_port.wake(wake)));
        serde_json::to_value(record).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

impl CreateDispatchRequest {
    /// The Ticket a Dispatch Request addresses with its Project,
    /// refusing an unknown Ticket, the terminal Ticket states, and
    /// the terminal archived-Project state — the same open guards the
    /// lifecycle and dependency commands apply.
    fn open(&self, ticket_id: u64) -> Result<(Project, Ticket), ApiError> {
        let ticket = self.ticket(ticket_id)?;
        let project = self.project(ticket.project())?;
        if project.is_archived() {
            return Err(ApiError::invalid_request(
                "archived is terminal; the Project accepts no further changes",
            ));
        }
        if ticket.state().is_terminal() {
            return Err(ApiError::invalid_request(
                "cancelled and superseded are terminal; the Ticket accepts no further changes",
            ));
        }
        Ok((project, ticket))
    }

    fn ticket(&self, ticket_id: u64) -> Result<Ticket, ApiError> {
        self.0
            .tickets
            .find(TicketId::new(ticket_id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {ticket_id}")))
    }

    fn project(&self, project_id: ProjectId) -> Result<Project, ApiError> {
        self.0
            .projects
            .find(project_id)?
            .ok_or_else(|| ApiError::not_found(&format!("project {}", project_id.value())))
    }

    fn ticket_is_ready(&self, ticket: &Ticket) -> Result<bool, ApiError> {
        use kanban_domain::{DependencyState, ReadinessInputs, TicketDependencyGraph};

        let graph = TicketDependencyGraph::restore(self.0.dependencies.list_dependencies()?);
        let mut states = Vec::new();
        for edge in graph.required_by(ticket.id()) {
            let blocking = self.0.tickets.find(edge.from())?.ok_or_else(|| {
                ApiError::internal(&format!(
                    "dependency {} names no stored Ticket",
                    edge.from().value()
                ))
            })?;
            states.push(DependencyState {
                dependency: edge,
                state: blocking.state(),
            });
        }
        let blockers = self.0.dependencies.blockers_of(ticket.id())?;
        Ok(compute_readiness(ReadinessInputs {
            dependencies: &states,
            blockers: &blockers,
        })
        .is_ready())
    }
}

/// Serves `dispatch.claim`.
struct ClaimDispatchRequest(DispatchContext);

impl CommandHandler for ClaimDispatchRequest {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<DispatchClaimRequest>(payload)?;
        ParsedCommand::lift("dispatch", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: DispatchClaimRequest = parse_payload(&command.payload)?;
        Ok(self.request(request.dispatch_request_id)?.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: DispatchClaimRequest = parse_payload(&command.payload)?;
        let queued = self.request(request.dispatch_request_id)?;
        let defaults = self.0.capacity.global_defaults()?;
        let caps = self.0.capacity.project_caps(queued.project().value())?;
        let project_caps = project_caps_of(&caps);
        let lanes = self.0.lanes.list_for_project(queued.project())?;
        let active_lanes = active_lane_count(&lanes, queued.ticket());
        let (updated, decision) = self.0.requests.try_claim(
            queued.id(),
            &ClaimContext {
                defaults: GlobalCapacity::restore(
                    defaults.max_active_per_harness,
                    defaults.max_active_per_model,
                    defaults.max_active_per_usage_pool,
                ),
                project_caps,
                active_lanes,
            },
            transition(
                queued.project(),
                queued.ticket(),
                queued.id(),
                "claimed",
                json!({
                    "ticket_id": queued.ticket().value(),
                    "status": DispatchStatus::Claimed.wire_name(),
                }),
            ),
        )?;
        let record = encode_record(&updated);
        let (claimed, capacity_refusal) = match &decision {
            ClaimDecision::Claim => {
                emit_catalogued(effects, LiveEventName::DispatchClaimed, &record);
                (true, None)
            }
            ClaimDecision::AlreadyClaimed => {
                return Err(ApiError::invalid_request(
                    "the Dispatch Request is already claimed",
                ));
            }
            ClaimDecision::RemainQueued(refusal) => (false, Some(refusal.to_string())),
        };
        let response = DispatchClaimResponse {
            request: record,
            claimed,
            capacity_refusal,
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

impl ClaimDispatchRequest {
    fn request(&self, id: u64) -> Result<DispatchRequest, ApiError> {
        self.0
            .requests
            .find(DispatchRequestId::new(id))?
            .ok_or_else(|| ApiError::not_found(&format!("dispatch request {id}")))
    }
}

/// Serves `dispatch.queue`.
struct ListDispatchQueue {
    requests: Arc<dyn DispatchStore>,
    projects: Arc<dyn ProjectStore>,
}

impl QueryHandler for ListDispatchQueue {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: DispatchQueueQuery = parse_payload(payload)?;
        self.projects
            .find(ProjectId::new(query.project_id))?
            .ok_or_else(|| ApiError::not_found(&format!("project {}", query.project_id)))?;
        let mut requests = self
            .requests
            .list_queued(ProjectId::new(query.project_id))?;
        sort_queue(&mut requests);
        let response = DispatchQueueResponse {
            project_id: query.project_id,
            requests: requests.iter().map(encode_record).collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// The timeline row for one Dispatch Request change: on the Project's
/// timeline, about the request, with `action` naming the change.
fn transition(
    project: ProjectId,
    ticket: TicketId,
    request: DispatchRequestId,
    action: &str,
    facts: Value,
) -> TimelineEnvelope {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("dispatch transition facts are a JSON object");
    object.insert("action".to_owned(), Value::from(action));
    object.insert("dispatch_request_id".to_owned(), json!(request.value()));
    TimelineEnvelope::project(
        project.value(),
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Ticket,
            id: ticket.value().to_string(),
        }),
        detail,
    )
}

fn encode_record(request: &DispatchRequest) -> DispatchRequestRecord {
    DispatchRequestRecord {
        id: request.id().value(),
        project_id: request.project().value(),
        ticket_id: request.ticket().value(),
        status: match request.status() {
            DispatchStatus::Queued => WireStatus::Queued,
            DispatchStatus::Claimed => WireStatus::Claimed,
        },
        priority: match request.priority() {
            Priority::Urgent => TicketPriority::Urgent,
            Priority::High => TicketPriority::High,
            Priority::Normal => TicketPriority::Normal,
            Priority::Low => TicketPriority::Low,
        },
        ready: request.ready(),
        harness: request.harness().to_owned(),
        model: request.model().to_owned(),
        usage_pool: request.usage_pool().to_owned(),
        created_at: request.created_at(),
        version: request.version(),
    }
}

fn project_caps_of(caps: &kanban_dto::CapacityProjectCaps) -> Option<ProjectCapacity> {
    let restored = ProjectCapacity::restore(
        caps.max_active_per_harness,
        caps.max_active_per_model,
        caps.max_active_per_usage_pool,
        caps.max_active_lanes,
    );
    if restored.is_unset() {
        None
    } else {
        Some(restored)
    }
}

/// The Project's active Lanes other than the candidate's own: the
/// evaluation adds the candidate once, so the Lane already holding
/// its Ticket must not be counted here.
fn active_lane_count(lanes: &[Lane], candidate: TicketId) -> u64 {
    lanes
        .iter()
        .filter(|lane| lane.ticket_id().is_some_and(|held| held != candidate))
        .count() as u64
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Evaluate one claim against currently claimed runs. Shared by the
/// SQLite and in-memory stores so the decision stays in domain code.
pub fn evaluate_dispatch_claim(
    candidate: &DispatchRequest,
    claimed: &[DispatchRequest],
    context: &ClaimContext,
) -> ClaimDecision {
    let occupied: Vec<ActiveRun<'_>> = claimed
        .iter()
        .map(|run| ActiveRun {
            project: run.project(),
            harness: run.harness(),
            model: run.model(),
            usage_pool: run.usage_pool(),
        })
        .collect();
    let capacity = evaluate_capacity(&CapacityInputs {
        candidate: ActiveRun {
            project: candidate.project(),
            harness: candidate.harness(),
            model: candidate.model(),
            usage_pool: candidate.usage_pool(),
        },
        active: &occupied,
        active_lanes: context.active_lanes,
        defaults: context.defaults,
        project_caps: context.project_caps,
    });
    decide_claim(candidate.status(), capacity)
}
