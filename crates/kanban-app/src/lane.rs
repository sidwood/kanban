//! Lane commands and queries: create durable execution slots, claim
//! and release Workspaces and Tickets under the assignment
//! constraints, and read Lanes back per Project (KAN-S6-US2). A Lane
//! holds at most one active Ticket (DR-LW-02); a non-seed Workspace
//! belongs to at most one active Lane (DR-LW-03); the Seed Workspace
//! is never an execution Lane, and every attempt to make it one is
//! refused and recorded on the timeline (DR-LW-07). An applied
//! Workspace claim mirrors onto the Workspace record — health and
//! lane pointer — inside the same write, so observation, reuse, and
//! the panel always agree with the Lane.

use std::sync::Arc;

use kanban_domain::{
    Lane, LaneError, LaneId, ProjectId, TicketId, Workspace, WorkspaceHealth, WorkspaceId,
    workspace_lane_conflict,
};
use kanban_dto::{
    ApiError, LaneCreateRequest, LaneListQuery, LaneListResponse, LaneRecord,
    LaneTicketAssignRequest, LaneTicketReleaseRequest, LaneWorkspaceAssignRequest,
    LaneWorkspaceReleaseRequest, LiveEventName, TimelineEntityKind, TimelineEntityRef,
    TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::events::{EventSink, emit_catalogued};
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;
use crate::ticket::TicketStore;
use crate::timeline::TimelineEnvelope;
use crate::workspace::WorkspaceStore;

/// The storage port Lane commands call through. Every write lands the
/// Lane row, the Workspace mirror when the claim moved, and the
/// timeline rows inside one span, so no crash boundary can split an
/// assignment.
pub trait LaneStore: Send + Sync {
    /// Insert a fresh Lane. Storage assigns the identity and asks
    /// `envelope` for the timeline row that identity belongs in.
    fn create(
        &self,
        project_id: ProjectId,
        envelope: &dyn Fn(LaneId) -> TimelineEnvelope,
    ) -> Result<Lane, ApiError>;

    /// Load one Lane, if it exists.
    fn find(&self, id: LaneId) -> Result<Option<Lane>, ApiError>;

    /// Every Lane of one Project, in id order.
    fn list_for_project(&self, project_id: ProjectId) -> Result<Vec<Lane>, ApiError>;

    /// The Lane of `project_id` currently claiming `workspace_id`, if
    /// any (DR-LW-03).
    fn find_by_workspace(
        &self,
        project_id: ProjectId,
        workspace_id: WorkspaceId,
    ) -> Result<Option<Lane>, ApiError>;

    /// Persist a Lane-only change with its timeline row in one write.
    /// The optimistic guard is the Lane version.
    fn save(&self, lane: &Lane, envelope: TimelineEnvelope) -> Result<(), ApiError>;

    /// Persist a Lane change and the Workspace claim mirror with both
    /// timeline rows in one write. The optimistic guard is the Lane
    /// version; the mirror follows the claim it was handed.
    fn save_with_workspace(
        &self,
        lane: &Lane,
        lane_envelope: TimelineEnvelope,
        workspace: &Workspace,
        workspace_envelope: TimelineEnvelope,
    ) -> Result<(), ApiError>;

    /// Append one timeline row without changing any aggregate: the
    /// durable record that a Seed assignment was attempted and
    /// refused (DR-LW-07). The write opens its own span, so it must
    /// be called with no enclosing command span — inside the refused
    /// command's span it would roll back with the rejection it
    /// records.
    fn record_refusal(&self, envelope: TimelineEnvelope) -> Result<(), ApiError>;
}

/// One Lane transition row: on the Project's timeline, about the
/// Lane, with `action` naming the change.
fn lane_transition(
    project_id: ProjectId,
    lane: LaneId,
    action: &str,
    facts: Value,
) -> TimelineEnvelope {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("Lane transition facts are a JSON object");
    object.insert("action".to_owned(), Value::from(action));
    object.insert("id".to_owned(), Value::from(lane.value()));
    TimelineEnvelope::project(
        project_id.value(),
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Lane,
            id: lane.value().to_string(),
        }),
        detail,
    )
}

/// The Workspace-side row an applied claim or release appends: a
/// health transition when the state moved, a plain note otherwise.
fn workspace_transition(
    project_id: ProjectId,
    workspace: &Workspace,
    action: &str,
    health_change: Option<(WorkspaceHealth, WorkspaceHealth)>,
    mut facts: Value,
) -> TimelineEnvelope {
    let object = facts
        .as_object_mut()
        .expect("Workspace claim facts are a JSON object");
    object.insert("action".to_owned(), Value::from(action));
    object.insert("id".to_owned(), Value::from(workspace.id().value()));
    if let Some((from, to)) = health_change {
        object.insert("from".to_owned(), Value::from(from.as_str()));
        object.insert("to".to_owned(), Value::from(to.as_str()));
    }
    TimelineEnvelope::project(
        project_id.value(),
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Workspace,
            id: workspace.id().value().to_string(),
        }),
        facts,
    )
}

fn record_of(lane: &Lane) -> LaneRecord {
    LaneRecord {
        id: lane.id().value(),
        project_id: lane.project().value(),
        workspace_id: lane.workspace_id().map(WorkspaceId::value),
        ticket_id: lane.ticket_id().map(TicketId::value),
        version: lane.version(),
    }
}

fn encode_record(lane: &Lane) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(lane)).map_err(|error| ApiError::internal(&error.to_string()))
}

fn announce(events: &dyn EventSink, event: LiveEventName, lane: &Lane) {
    emit_catalogued(events, event, &record_of(lane));
}

/// Report a refused domain rule as the stable invalid-request code.
fn refuse(error: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

/// The stores every Lane command reads and writes through.
#[derive(Clone)]
struct LaneContext {
    lanes: Arc<dyn LaneStore>,
    projects: Arc<dyn ProjectStore>,
    workspaces: Arc<dyn WorkspaceStore>,
    tickets: Arc<dyn TicketStore>,
}

impl Core {
    /// Register the Lane operations against `lanes`, resolving
    /// Projects through `projects`, Workspace claims through
    /// `workspaces`, and Ticket assignments through `tickets`.
    pub fn register_lanes(
        &mut self,
        lanes: Arc<dyn LaneStore>,
        projects: Arc<dyn ProjectStore>,
        workspaces: Arc<dyn WorkspaceStore>,
        tickets: Arc<dyn TicketStore>,
    ) -> Result<(), RegistrationError> {
        let context = LaneContext {
            lanes,
            projects,
            workspaces,
            tickets,
        };
        self.register_command("lane.create", Arc::new(CreateLane(context.clone())))?;
        self.register_command(
            "lane.workspace.assign",
            Arc::new(AssignLaneWorkspace(context.clone())),
        )?;
        self.register_command(
            "lane.workspace.release",
            Arc::new(ReleaseLaneWorkspace(context.clone())),
        )?;
        self.register_command(
            "lane.ticket.assign",
            Arc::new(AssignLaneTicket(context.clone())),
        )?;
        self.register_command(
            "lane.ticket.release",
            Arc::new(ReleaseLaneTicket(context.clone())),
        )?;
        self.register_query(
            "lane.list",
            Arc::new(ListLanes {
                lanes: context.lanes,
            }),
        )?;
        Ok(())
    }
}

fn load_project(context: &LaneContext, id: ProjectId) -> Result<kanban_domain::Project, ApiError> {
    context
        .projects
        .find(id)?
        .ok_or_else(|| ApiError::not_found(&format!("project {}", id.value())))
}

fn load_lane(context: &LaneContext, id: u64) -> Result<Lane, ApiError> {
    context
        .lanes
        .find(LaneId::new(id))?
        .ok_or_else(|| ApiError::not_found(&format!("lane {id}")))
}

/// Serves `lane.create`.
struct CreateLane(LaneContext);

impl CommandHandler for CreateLane {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<LaneCreateRequest>(payload)?;
        ParsedCommand::lift("lane", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        // A fresh aggregate is created at version 0.
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        events: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: LaneCreateRequest = parse_payload(&command.payload)?;
        let project_id = ProjectId::new(request.project_id);
        let project = load_project(&self.0, project_id)?;
        if project.is_archived() {
            return Err(refuse("archived Projects cannot create Lanes"));
        }
        let lane = self.0.lanes.create(project_id, &|id| {
            lane_transition(
                project_id,
                id,
                "created",
                json!({ "project_id": project_id.value() }),
            )
        })?;
        announce(events, LiveEventName::LaneCreated, &lane);
        encode_record(&lane)
    }
}

/// Serves `lane.workspace.assign`.
struct AssignLaneWorkspace(LaneContext);

impl CommandHandler for AssignLaneWorkspace {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<LaneWorkspaceAssignRequest>(payload)?;
        ParsedCommand::lift("lane", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: LaneWorkspaceAssignRequest = parse_payload(&command.payload)?;
        Ok(load_lane(&self.0, request.lane_id)?.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        events: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: LaneWorkspaceAssignRequest = parse_payload(&command.payload)?;
        let mut lane = load_lane(&self.0, request.lane_id)?;
        let project = load_project(&self.0, lane.project())?;
        if project.is_archived() {
            return Err(refuse("archived Projects accept no further changes"));
        }
        let mut workspace = self
            .0
            .workspaces
            .find(WorkspaceId::new(request.workspace_id))?
            .ok_or_else(|| ApiError::not_found(&format!("workspace {}", request.workspace_id)))?;
        if workspace.registration().project_id() != lane.project() {
            return Err(refuse("the Workspace belongs to another Project"));
        }
        let lane_before = lane.version();
        if let Err(error) = lane.assign_workspace(&workspace) {
            if matches!(error, LaneError::SeedWorkspace { .. }) {
                // The refusal outlives the failed command: the
                // timeline row is the durable record (DR-LW-07). It
                // cannot ride this command's mutation span, which is
                // about to roll the whole rejection back, so it lands
                // after the discard instead — alone, in its own write.
                let envelope = lane_transition(
                    lane.project(),
                    lane.id(),
                    "seed_assignment_refused",
                    json!({
                        "workspace_id": workspace.id().value(),
                        "path": workspace.registration().path(),
                        "reason": "seed",
                    }),
                );
                let lanes = self.0.lanes.clone();
                events.after_discard(Box::new(move || {
                    if let Err(error) = lanes.record_refusal(envelope) {
                        eprintln!(
                            "kanban: the Seed refusal could not be recorded: {}",
                            error.message
                        );
                    }
                }));
            }
            return Err(refuse(error));
        }
        if let Some(holder) = self
            .0
            .lanes
            .find_by_workspace(lane.project(), workspace.id())?
        {
            if let Some(conflict) = workspace_lane_conflict(Some(holder.id()), lane.id()) {
                return Err(refuse(format!(
                    "Workspace {} already belongs to Lane {conflict}",
                    workspace.id().value()
                )));
            }
        }
        if lane.version() == lane_before && workspace.lane_id() == Some(lane.id().value()) {
            // Claiming what is already claimed is the same state.
            return encode_record(&lane);
        }
        let health_change = workspace.assign_lane(lane.id().value());
        let lane_envelope = lane_transition(
            lane.project(),
            lane.id(),
            "workspace_assigned",
            json!({
                "workspace_id": workspace.id().value(),
                "path": workspace.registration().path(),
            }),
        );
        let workspace_envelope = workspace_transition(
            lane.project(),
            &workspace,
            "lane_assigned",
            health_change,
            json!({
                "path": workspace.registration().path(),
                "health": workspace.health().as_str(),
                "lane_assignment": lane.id().value(),
            }),
        );
        self.0
            .lanes
            .save_with_workspace(&lane, lane_envelope, &workspace, workspace_envelope)?;
        announce(events, LiveEventName::LaneWorkspaceAssigned, &lane);
        encode_record(&lane)
    }
}

/// Serves `lane.workspace.release`.
struct ReleaseLaneWorkspace(LaneContext);

impl CommandHandler for ReleaseLaneWorkspace {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<LaneWorkspaceReleaseRequest>(payload)?;
        ParsedCommand::lift("lane", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: LaneWorkspaceReleaseRequest = parse_payload(&command.payload)?;
        Ok(load_lane(&self.0, request.lane_id)?.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        events: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: LaneWorkspaceReleaseRequest = parse_payload(&command.payload)?;
        let mut lane = load_lane(&self.0, request.lane_id)?;
        let held = lane
            .workspace_id()
            .ok_or_else(|| refuse(LaneError::LaneHoldsNoWorkspace))?;
        lane.release_workspace()
            .map_err(|error| refuse(error.to_string()))?;
        let mut workspace = self
            .0
            .workspaces
            .find(held)?
            .ok_or_else(|| ApiError::not_found(&format!("workspace {}", held.value())))?;
        let health_change = workspace.release_lane();
        let lane_envelope = lane_transition(
            lane.project(),
            lane.id(),
            "workspace_released",
            json!({ "workspace_id": held.value() }),
        );
        let workspace_envelope = workspace_transition(
            lane.project(),
            &workspace,
            "lane_released",
            health_change,
            json!({
                "path": workspace.registration().path(),
                "health": workspace.health().as_str(),
                "lane_assignment": serde_json::Value::Null,
            }),
        );
        self.0
            .lanes
            .save_with_workspace(&lane, lane_envelope, &workspace, workspace_envelope)?;
        announce(events, LiveEventName::LaneWorkspaceReleased, &lane);
        encode_record(&lane)
    }
}

/// Serves `lane.ticket.assign`.
struct AssignLaneTicket(LaneContext);

impl CommandHandler for AssignLaneTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<LaneTicketAssignRequest>(payload)?;
        ParsedCommand::lift("lane", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: LaneTicketAssignRequest = parse_payload(&command.payload)?;
        Ok(load_lane(&self.0, request.lane_id)?.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        events: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: LaneTicketAssignRequest = parse_payload(&command.payload)?;
        let mut lane = load_lane(&self.0, request.lane_id)?;
        let project = load_project(&self.0, lane.project())?;
        if project.is_archived() {
            return Err(refuse("archived Projects accept no further changes"));
        }
        let ticket = self
            .0
            .tickets
            .find(TicketId::new(request.ticket_id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {}", request.ticket_id)))?;
        if ticket.project() != lane.project() {
            return Err(refuse("the Ticket belongs to another Project"));
        }
        let lane_before = lane.version();
        lane.assign_ticket(ticket.id())
            .map_err(|error| refuse(error.to_string()))?;
        if lane.version() == lane_before {
            // Holding the Ticket already held is the same state.
            return encode_record(&lane);
        }
        let envelope = lane_transition(
            lane.project(),
            lane.id(),
            "ticket_assigned",
            json!({ "ticket_id": ticket.id().value() }),
        );
        self.0.lanes.save(&lane, envelope)?;
        announce(events, LiveEventName::LaneTicketAssigned, &lane);
        encode_record(&lane)
    }
}

/// Serves `lane.ticket.release`.
struct ReleaseLaneTicket(LaneContext);

impl CommandHandler for ReleaseLaneTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<LaneTicketReleaseRequest>(payload)?;
        ParsedCommand::lift("lane", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: LaneTicketReleaseRequest = parse_payload(&command.payload)?;
        Ok(load_lane(&self.0, request.lane_id)?.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        events: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: LaneTicketReleaseRequest = parse_payload(&command.payload)?;
        let mut lane = load_lane(&self.0, request.lane_id)?;
        let held = lane
            .ticket_id()
            .ok_or_else(|| refuse(LaneError::LaneHoldsNoTicket))?;
        lane.release_ticket()
            .map_err(|error| refuse(error.to_string()))?;
        let envelope = lane_transition(
            lane.project(),
            lane.id(),
            "ticket_released",
            json!({ "ticket_id": held.value() }),
        );
        self.0.lanes.save(&lane, envelope)?;
        announce(events, LiveEventName::LaneTicketReleased, &lane);
        encode_record(&lane)
    }
}

/// Serves `lane.list`.
struct ListLanes {
    lanes: Arc<dyn LaneStore>,
}

impl QueryHandler for ListLanes {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: LaneListQuery = parse_payload(payload)?;
        // An unknown Project holds no Lanes, exactly as the Ticket
        // list reports it: empty, not a refusal.
        let response = LaneListResponse {
            lanes: self
                .lanes
                .list_for_project(ProjectId::new(query.project_id))?
                .iter()
                .map(record_of)
                .collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex};

    use kanban_domain::{Lane, LaneId, ProjectId, WorkspaceId};
    use kanban_dto::ApiError;

    use super::LaneStore;
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::NoopEventSink;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::plan::testing::MemoryProjects;
    use crate::spec::testing::MemorySpecs;
    use crate::ticket::testing::{MemoryTicketEvidence, MemoryTickets};
    use crate::timeline::TimelineEnvelope;
    use crate::workspace::WorkspaceStore;
    use crate::workspace::testing::MemoryWorkspaceStore;

    /// An in-memory Lane store: rows by id, the timeline envelopes it
    /// was asked to land, and the refusals it recorded. Workspace
    /// mirrors land in the shared workspace store, exactly as the
    /// SQLite store lands them in the same span.
    pub(crate) struct MemoryLaneStore {
        state: Mutex<MemoryLaneState>,
        workspaces: Arc<MemoryWorkspaceStore>,
    }

    #[derive(Default)]
    struct MemoryLaneState {
        lanes: Vec<Lane>,
        next_id: u64,
        timeline: Vec<TimelineEnvelope>,
    }

    impl MemoryLaneStore {
        /// A lane store sharing the Workspace rows the harness owns.
        pub(crate) fn sharing(workspaces: Arc<MemoryWorkspaceStore>) -> Self {
            Self {
                state: Mutex::new(MemoryLaneState::default()),
                workspaces,
            }
        }

        /// The stored rows and timeline envelopes, for assertions.
        pub(crate) fn snapshot(&self) -> (Vec<Lane>, Vec<TimelineEnvelope>) {
            let state = self.state.lock().expect("the memory lane lock is sound");
            (state.lanes.clone(), state.timeline.clone())
        }
    }

    impl LaneStore for MemoryLaneStore {
        fn create(
            &self,
            project_id: ProjectId,
            envelope: &dyn Fn(LaneId) -> TimelineEnvelope,
        ) -> Result<Lane, ApiError> {
            let mut state = self.state.lock().expect("the memory lane lock is sound");
            state.next_id += 1;
            let id = LaneId::new(state.next_id);
            let lane = Lane::new(id, project_id);
            state.lanes.push(lane.clone());
            state.timeline.push(envelope(id));
            Ok(lane)
        }

        fn find(&self, id: LaneId) -> Result<Option<Lane>, ApiError> {
            let state = self.state.lock().expect("the memory lane lock is sound");
            Ok(state.lanes.iter().find(|row| row.id() == id).cloned())
        }

        fn list_for_project(&self, project_id: ProjectId) -> Result<Vec<Lane>, ApiError> {
            let state = self.state.lock().expect("the memory lane lock is sound");
            Ok(state
                .lanes
                .iter()
                .filter(|row| row.project() == project_id)
                .cloned()
                .collect())
        }

        fn find_by_workspace(
            &self,
            project_id: ProjectId,
            workspace_id: WorkspaceId,
        ) -> Result<Option<Lane>, ApiError> {
            let state = self.state.lock().expect("the memory lane lock is sound");
            Ok(state
                .lanes
                .iter()
                .find(|row| row.project() == project_id && row.workspace_id() == Some(workspace_id))
                .cloned())
        }

        fn save(&self, lane: &Lane, envelope: TimelineEnvelope) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory lane lock is sound");
            Self::replace(&mut state, lane)?;
            state.timeline.push(envelope);
            Ok(())
        }

        fn save_with_workspace(
            &self,
            lane: &Lane,
            lane_envelope: TimelineEnvelope,
            workspace: &kanban_domain::Workspace,
            workspace_envelope: TimelineEnvelope,
        ) -> Result<(), ApiError> {
            self.workspaces
                .save(workspace, workspace_envelope)
                .expect("the shared workspace row accepts the mirror");
            let mut state = self.state.lock().expect("the memory lane lock is sound");
            Self::replace(&mut state, lane)?;
            state.timeline.push(lane_envelope);
            Ok(())
        }

        fn record_refusal(&self, envelope: TimelineEnvelope) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory lane lock is sound");
            state.timeline.push(envelope);
            Ok(())
        }
    }

    impl MemoryLaneStore {
        fn replace(state: &mut MemoryLaneState, lane: &Lane) -> Result<(), ApiError> {
            let id = lane.id();
            let row = state
                .lanes
                .iter_mut()
                .find(|row| row.id() == id)
                .ok_or_else(|| ApiError::not_found(&format!("lane {}", id.value())))?;
            *row = lane.clone();
            Ok(())
        }
    }

    /// A core with the Workspace, Ticket, and Lane operations wired
    /// to in-memory stores over one active Project.
    pub(crate) struct LaneHarness {
        pub(crate) projects: Arc<MemoryProjects>,
        pub(crate) workspaces: Arc<MemoryWorkspaceStore>,
        pub(crate) lanes: Arc<MemoryLaneStore>,
        pub(crate) core: Core,
    }

    /// A harness whose git observer the test chooses.
    pub(crate) fn lane_harness_with_observer(
        observer: Arc<dyn crate::workspace::WorkspaceGitObserver>,
    ) -> LaneHarness {
        let projects = Arc::new(MemoryProjects::default());
        projects.seed(crate::plan::testing::active_project(
            1,
            "CORE",
            kanban_domain::ProjectCounters::zeroed(),
        ));
        let specs = Arc::new(MemorySpecs::sharing(projects.clone()));
        let tickets = Arc::new(MemoryTickets::sharing(projects.clone()));
        let workspaces = Arc::new(MemoryWorkspaceStore::default());
        let lanes = Arc::new(MemoryLaneStore::sharing(workspaces.clone()));
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            Arc::new(NoopEventSink),
        );
        core.register_tickets(
            tickets.clone(),
            projects.clone(),
            specs,
            Arc::new(MemoryTicketEvidence::default()),
        )
        .expect("the ticket operations register");
        core.register_workspaces(workspaces.clone(), projects.clone(), observer)
            .expect("the workspace operations register");
        core.register_lanes(
            lanes.clone(),
            projects.clone(),
            workspaces.clone(),
            tickets.clone(),
        )
        .expect("the lane operations register");
        LaneHarness {
            projects,
            workspaces,
            lanes,
            core,
        }
    }

    /// A harness with a silent git observer: paths read as missing.
    pub(crate) fn lane_harness() -> LaneHarness {
        lane_harness_with_observer(Arc::new(
            crate::workspace::testing::ScriptedObserver::default(),
        ))
    }
}

#[cfg(test)]
mod lane_assign {
    use std::collections::HashMap;
    use std::sync::Arc;

    use kanban_domain::ProjectState;
    use kanban_dto::ErrorCode;
    use serde_json::{Value, json};

    use super::testing::{LaneHarness, lane_harness, lane_harness_with_observer};
    use crate::WorkspaceGitSnapshot;

    /// A harness whose observer reads `path` as a clean clone.
    fn observed_harness(path: &str) -> LaneHarness {
        lane_harness_with_observer(clean_observer(path))
    }

    fn clean_observer(path: &str) -> Arc<crate::workspace::testing::ScriptedObserver> {
        Arc::new(crate::workspace::testing::ScriptedObserver {
            snapshots: HashMap::from([(
                path.to_owned(),
                WorkspaceGitSnapshot {
                    present: true,
                    repository_identity: Some("identity".to_owned()),
                    checkout: Some(kanban_domain::WorkspaceCheckout::Branch(
                        "feature".to_owned(),
                    )),
                    head: Some("abc123".to_owned()),
                    working_tree_clean: Some(true),
                    unique_unlanded_commits: Some(false),
                },
            )]),
        })
    }

    fn mutation(version: u64, key: &str) -> Value {
        json!({ "optimistic_version": version, "idempotency_key": key })
    }

    fn create_lane(project_id: u64, key: &str) -> Value {
        json!({ "mutation": mutation(0, key), "project_id": project_id })
    }

    fn assign_workspace(lane_id: u64, workspace_id: u64, key: &str, version: u64) -> Value {
        json!({
            "mutation": mutation(version, key),
            "lane_id": lane_id,
            "workspace_id": workspace_id,
        })
    }

    fn release_workspace(lane_id: u64, key: &str, version: u64) -> Value {
        json!({ "mutation": mutation(version, key), "lane_id": lane_id })
    }

    fn assign_ticket(lane_id: u64, ticket_id: u64, key: &str, version: u64) -> Value {
        json!({
            "mutation": mutation(version, key),
            "lane_id": lane_id,
            "ticket_id": ticket_id,
        })
    }

    fn release_ticket(lane_id: u64, key: &str, version: u64) -> Value {
        json!({ "mutation": mutation(version, key), "lane_id": lane_id })
    }

    fn register_workspace(harness: &LaneHarness, path: &str, key: &str) -> Value {
        harness
            .core
            .command(
                "workspace.register",
                &json!({ "mutation": mutation(0, key), "project_id": 1, "path": path }),
            )
            .expect("the workspace registers")
    }

    fn register_and_observe(harness: &LaneHarness, path: &str, key: &str) -> Value {
        let workspace_id = register_workspace(harness, path, key)["id"]
            .as_u64()
            .expect("the identity is a number");
        harness
            .core
            .command(
                "workspace.observe",
                &json!({
                    "mutation": mutation(1, "observe-key"),
                    "workspace_id": workspace_id,
                }),
            )
            .expect("the observation applies")
    }

    fn create_task_ticket(harness: &LaneHarness, title: &str, key: &str) -> Value {
        harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": mutation(0, key),
                    "project_id": 1,
                    "kind": "task",
                    "priority": "normal",
                    "title": title,
                    "subtype": "operational",
                    "mode": "agent",
                    "completion": ["The slot is worked to completion."],
                }),
            )
            .expect("the ticket creates")
    }

    fn lane_created(harness: &LaneHarness, key: &str) -> u64 {
        let response = harness
            .core
            .command("lane.create", &create_lane(1, key))
            .expect("the lane creates");
        response["id"].as_u64().expect("the identity is a number")
    }

    #[test]
    fn creating_a_lane_mints_a_durable_empty_slot() {
        let harness = lane_harness();

        let response = harness
            .core
            .command("lane.create", &create_lane(1, "key-1"))
            .expect("the lane creates");

        assert_eq!(response["id"], json!(1));
        assert_eq!(response["project_id"], json!(1));
        assert_eq!(response["workspace_id"], json!(null));
        assert_eq!(response["ticket_id"], json!(null));
        assert_eq!(response["version"], json!(1));
        let (_, timeline) = harness.lanes.snapshot();
        let created = timeline
            .iter()
            .find(|row| row.detail().get("action") == Some(&json!("created")))
            .expect("creation appends a timeline row");
        assert_eq!(
            created.entity().map(|entity| entity.kind),
            Some(kanban_dto::TimelineEntityKind::Lane)
        );
    }

    #[test]
    fn creating_a_lane_refuses_an_unknown_project() {
        let harness = lane_harness();

        let error = harness
            .core
            .command("lane.create", &create_lane(9, "key-1"))
            .expect_err("the unknown Project is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn creating_a_lane_refuses_an_archived_project() {
        let harness = lane_harness();
        harness.projects.seed(kanban_domain::Project::restore(
            kanban_domain::ProjectId::new(2),
            kanban_domain::ProjectRegistration::new(
                "OLD",
                "Retired work",
                "/repositories/kanban",
                "/workspaces/old.seed",
                "main",
                "old.seed",
                Some("old-main"),
                None,
            )
            .expect("the fixture registration validates"),
            ProjectState::Archived,
            kanban_domain::ProjectCounters::zeroed(),
            1,
        ));

        let error = harness
            .core
            .command("lane.create", &create_lane(2, "key-1"))
            .expect_err("the archived Project is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("archived"));
    }

    #[test]
    fn assigning_a_workspace_claims_it_for_the_lane() {
        let harness = observed_harness("/workspaces/kanban.feature");
        let lane_id = lane_created(&harness, "key-1");
        register_and_observe(&harness, "/workspaces/kanban.feature", "key-2");

        let response = harness
            .core
            .command(
                "lane.workspace.assign",
                &assign_workspace(lane_id, 1, "key-3", 1),
            )
            .expect("the assignment applies");

        assert_eq!(response["workspace_id"], json!(1));
        assert_eq!(response["version"], json!(2));
        let (workspaces, _) = harness.workspaces.snapshot();
        let workspace = &workspaces[0];
        assert_eq!(workspace.lane_id(), Some(lane_id));
        assert_eq!(
            workspace.health(),
            kanban_domain::WorkspaceHealth::Assigned,
            "the claim mirrors onto the Workspace health"
        );
        let (_, timeline) = harness.lanes.snapshot();
        let assigned = timeline
            .iter()
            .find(|row| row.detail().get("action") == Some(&json!("workspace_assigned")))
            .expect("the assignment appends a Lane row");
        assert_eq!(assigned.detail().get("workspace_id"), Some(&json!(1)));
        let (_, workspace_timeline) = harness.workspaces.snapshot();
        let health_change = workspace_timeline
            .iter()
            .find(|row| {
                row.detail().get("action") == Some(&json!("lane_assigned"))
                    && row.detail().get("from") == Some(&json!("available"))
                    && row.detail().get("to") == Some(&json!("assigned"))
            })
            .expect("the claim appends a Workspace health transition");
        assert_eq!(
            health_change.entity().map(|entity| entity.kind),
            Some(kanban_dto::TimelineEntityKind::Workspace)
        );
    }

    #[test]
    fn assigning_the_seed_workspace_is_refused_and_recorded() {
        let harness = lane_harness();
        let lane_id = lane_created(&harness, "key-1");
        register_workspace(&harness, "/workspaces/kanban.seed", "key-2");
        let seed = harness
            .core
            .query("workspace.list", &json!({ "project_id": 1 }))
            .expect("the listing serves");
        assert_eq!(seed["workspaces"][0]["is_seed"], json!(true));

        let error = harness
            .core
            .command(
                "lane.workspace.assign",
                &assign_workspace(lane_id, 1, "key-3", 1),
            )
            .expect_err("the Seed assignment is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("never be an execution Lane"),
            "the refusal names the rule: {}",
            error.message
        );
        let (workspaces, _) = harness.workspaces.snapshot();
        assert_eq!(workspaces[0].lane_id(), None, "the Seed stays unclaimed");
        let (lanes, timeline) = harness.lanes.snapshot();
        assert_eq!(lanes[0].workspace_id(), None);
        assert_eq!(lanes[0].version(), 1, "the refusal changed nothing");
        let refusal = timeline
            .iter()
            .find(|row| row.detail().get("action") == Some(&json!("seed_assignment_refused")))
            .expect("the refusal is recorded on the timeline");
        assert_eq!(
            refusal.detail().get("path"),
            Some(&json!("/workspaces/kanban.seed"))
        );
        assert_eq!(refusal.detail().get("reason"), Some(&json!("seed")));
    }

    #[test]
    fn assigning_a_workspace_held_by_another_lane_is_refused() {
        let harness = observed_harness("/workspaces/kanban.feature");
        let first = lane_created(&harness, "key-1");
        let second = lane_created(&harness, "key-2");
        register_and_observe(&harness, "/workspaces/kanban.feature", "key-3");
        harness
            .core
            .command(
                "lane.workspace.assign",
                &assign_workspace(first, 1, "key-4", 1),
            )
            .expect("the first Lane claims the Workspace");

        let error = harness
            .core
            .command(
                "lane.workspace.assign",
                &assign_workspace(second, 1, "key-5", 1),
            )
            .expect_err("a second Lane cannot claim the same Workspace");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("already belongs to Lane"),
            "the refusal names the holder: {}",
            error.message
        );
        let (workspaces, _) = harness.workspaces.snapshot();
        assert_eq!(workspaces[0].lane_id(), Some(first));
    }

    #[test]
    fn assigning_to_a_lane_already_holding_a_workspace_is_refused() {
        let harness = observed_harness("/workspaces/kanban.feature");
        let lane_id = lane_created(&harness, "key-1");
        register_workspace(&harness, "/workspaces/kanban.feature", "key-2");
        register_workspace(&harness, "/workspaces/kanban.other", "key-3");
        harness
            .core
            .command(
                "lane.workspace.assign",
                &assign_workspace(lane_id, 1, "key-4", 1),
            )
            .expect("the first Workspace is claimed");

        let error = harness
            .core
            .command(
                "lane.workspace.assign",
                &assign_workspace(lane_id, 2, "key-5", 2),
            )
            .expect_err("a Lane runs in at most one Workspace");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("already holds Workspace"),
            "the refusal names the held Workspace: {}",
            error.message
        );
    }

    #[test]
    fn assigning_an_unknown_workspace_is_not_found() {
        let harness = lane_harness();
        let lane_id = lane_created(&harness, "key-1");

        let error = harness
            .core
            .command(
                "lane.workspace.assign",
                &assign_workspace(lane_id, 9, "key-2", 1),
            )
            .expect_err("the unknown Workspace is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn assigning_to_an_unknown_lane_is_not_found() {
        let harness = lane_harness();

        let error = harness
            .core
            .command("lane.workspace.assign", &assign_workspace(9, 1, "key-1", 1))
            .expect_err("the unknown Lane is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn assigning_with_a_stale_lane_version_is_rejected() {
        let harness = lane_harness();
        let lane_id = lane_created(&harness, "key-1");
        register_workspace(&harness, "/workspaces/kanban.feature", "key-2");

        let error = harness
            .core
            .command(
                "lane.workspace.assign",
                &assign_workspace(lane_id, 1, "key-3", 9),
            )
            .expect_err("the stale version is refused");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(1));
    }

    #[test]
    fn assigning_the_workspace_already_claimed_changes_nothing() {
        let harness = observed_harness("/workspaces/kanban.feature");
        let lane_id = lane_created(&harness, "key-1");
        register_and_observe(&harness, "/workspaces/kanban.feature", "key-2");
        harness
            .core
            .command(
                "lane.workspace.assign",
                &assign_workspace(lane_id, 1, "key-3", 1),
            )
            .expect("the claim applies");
        let (_, before) = harness.lanes.snapshot();

        let response = harness
            .core
            .command(
                "lane.workspace.assign",
                &assign_workspace(lane_id, 1, "key-4", 2),
            )
            .expect("claiming the Workspace already claimed is the same state");

        assert_eq!(response["version"], json!(2), "no version moved");
        let (_, after) = harness.lanes.snapshot();
        assert_eq!(after.len(), before.len(), "no timeline row was appended");
    }

    #[test]
    fn releasing_a_workspace_frees_the_claim_and_restores_health() {
        let harness = observed_harness("/workspaces/kanban.feature");
        let lane_id = lane_created(&harness, "key-1");
        register_and_observe(&harness, "/workspaces/kanban.feature", "key-2");
        harness
            .core
            .command(
                "lane.workspace.assign",
                &assign_workspace(lane_id, 1, "key-3", 1),
            )
            .expect("the claim applies");

        let response = harness
            .core
            .command(
                "lane.workspace.release",
                &release_workspace(lane_id, "key-4", 2),
            )
            .expect("the release applies");

        assert_eq!(response["workspace_id"], json!(null));
        let (workspaces, _) = harness.workspaces.snapshot();
        assert_eq!(workspaces[0].lane_id(), None);
        assert_eq!(
            workspaces[0].health(),
            kanban_domain::WorkspaceHealth::Available,
            "the Workspace returns to its own computed health"
        );
        let (_, timeline) = harness.lanes.snapshot();
        assert!(
            timeline
                .iter()
                .any(|row| { row.detail().get("action") == Some(&json!("workspace_released")) })
        );
    }

    #[test]
    fn releasing_a_lane_with_no_workspace_is_refused() {
        let harness = lane_harness();
        let lane_id = lane_created(&harness, "key-1");

        let error = harness
            .core
            .command(
                "lane.workspace.release",
                &release_workspace(lane_id, "key-2", 1),
            )
            .expect_err("an empty claim cannot release");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("no Workspace to release"));
    }

    #[test]
    fn a_lane_holds_at_most_one_active_ticket() {
        let harness = lane_harness();
        let lane_id = lane_created(&harness, "key-1");
        create_task_ticket(&harness, "First slice", "key-2");
        create_task_ticket(&harness, "Second slice", "key-3");
        harness
            .core
            .command("lane.ticket.assign", &assign_ticket(lane_id, 1, "key-4", 1))
            .expect("the first Ticket holds the slot");

        let error = harness
            .core
            .command("lane.ticket.assign", &assign_ticket(lane_id, 2, "key-5", 2))
            .expect_err("a second Ticket cannot hold the same Lane");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("already holds Ticket"),
            "the refusal names the held Ticket: {}",
            error.message
        );
        let (lanes, _) = harness.lanes.snapshot();
        assert_eq!(lanes[0].ticket_id(), Some(kanban_domain::TicketId::new(1)));
    }

    #[test]
    fn releasing_a_ticket_frees_the_slot_for_the_next_one() {
        let harness = lane_harness();
        let lane_id = lane_created(&harness, "key-1");
        create_task_ticket(&harness, "First slice", "key-2");
        create_task_ticket(&harness, "Second slice", "key-3");
        harness
            .core
            .command("lane.ticket.assign", &assign_ticket(lane_id, 1, "key-4", 1))
            .expect("the first Ticket holds the slot");
        harness
            .core
            .command("lane.ticket.release", &release_ticket(lane_id, "key-5", 2))
            .expect("the release applies");

        let response = harness
            .core
            .command("lane.ticket.assign", &assign_ticket(lane_id, 2, "key-6", 3))
            .expect("the freed slot holds the next Ticket");

        assert_eq!(response["ticket_id"], json!(2));
    }

    #[test]
    fn assigning_a_ticket_from_another_project_is_refused() {
        let harness = lane_harness();
        let lane_id = lane_created(&harness, "key-1");
        harness.projects.seed(kanban_domain::Project::restore(
            kanban_domain::ProjectId::new(2),
            kanban_domain::ProjectRegistration::new(
                "EDGE",
                "Edge work",
                "/repositories/edge",
                "/workspaces/edge.seed",
                "main",
                "edge.seed",
                Some("edge-main"),
                None,
            )
            .expect("the fixture registration validates"),
            ProjectState::Active,
            kanban_domain::ProjectCounters::zeroed(),
            1,
        ));
        let foreign = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": mutation(0, "edge-key"),
                    "project_id": 2,
                    "kind": "task",
                    "priority": "normal",
                    "title": "Foreign slice",
                    "subtype": "operational",
                    "mode": "agent",
                    "completion": ["The foreign slice is worked to completion."],
                }),
            )
            .expect("the foreign Ticket creates");

        let error = harness
            .core
            .command(
                "lane.ticket.assign",
                &assign_ticket(
                    lane_id,
                    foreign["id"].as_u64().expect("the identity is a number"),
                    "key-2",
                    1,
                ),
            )
            .expect_err("the foreign Ticket is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("another Project"),
            "the refusal names the mismatch: {}",
            error.message
        );
    }

    #[test]
    fn assigning_an_unknown_ticket_is_not_found() {
        let harness = lane_harness();
        let lane_id = lane_created(&harness, "key-1");

        let error = harness
            .core
            .command("lane.ticket.assign", &assign_ticket(lane_id, 9, "key-2", 1))
            .expect_err("the unknown Ticket is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn listing_reports_lanes_with_their_claims() {
        let harness = observed_harness("/workspaces/kanban.feature");
        let lane_id = lane_created(&harness, "key-1");
        lane_created(&harness, "key-2");
        register_and_observe(&harness, "/workspaces/kanban.feature", "key-3");
        create_task_ticket(&harness, "One slice", "key-4");
        harness
            .core
            .command(
                "lane.workspace.assign",
                &assign_workspace(lane_id, 1, "key-5", 1),
            )
            .expect("the claim applies");
        harness
            .core
            .command("lane.ticket.assign", &assign_ticket(lane_id, 1, "key-6", 2))
            .expect("the Ticket holds the slot");

        let listing = harness
            .core
            .query("lane.list", &json!({ "project_id": 1 }))
            .expect("the listing serves");

        assert_eq!(listing["lanes"].as_array().expect("lanes list").len(), 2);
        assert_eq!(listing["lanes"][0]["workspace_id"], json!(1));
        assert_eq!(listing["lanes"][0]["ticket_id"], json!(1));
        assert_eq!(listing["lanes"][1]["workspace_id"], json!(null));
        assert_eq!(listing["lanes"][1]["ticket_id"], json!(null));
    }

    #[test]
    fn observing_after_assignment_reports_the_lane_and_assigned_health() {
        let harness = observed_harness("/workspaces/kanban.feature");
        let lane_id = lane_created(&harness, "key-1");
        register_and_observe(&harness, "/workspaces/kanban.feature", "key-2");
        harness
            .core
            .command(
                "lane.workspace.assign",
                &assign_workspace(lane_id, 1, "key-3", 1),
            )
            .expect("the claim applies");

        let response = harness
            .core
            .command(
                "workspace.observe",
                &json!({
                    "mutation": mutation(3, "key-4"),
                    "workspace_id": 1,
                }),
            )
            .expect("the observation applies");

        assert_eq!(response["health"], json!("assigned"));
        assert_eq!(
            response["observation"]["lane_assignment"],
            json!(1),
            "the durable claim reads back through observation"
        );
    }
}
