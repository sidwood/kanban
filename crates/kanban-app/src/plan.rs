//! Plan commands and queries: compose the ordered dependency graph,
//! freeze it into immutable versions at activation, and replan with
//! an auditable replacement (KAN-S3-US1, KAN-S3-US2, KAN-S3-US3).
//! Display order and dependency edges are edited through separate
//! commands, every change appends a timeline row on the Project's own
//! timeline inside the same write, and no delete exists.

use std::sync::Arc;

use kanban_domain::{
    NumberKind, Plan, PlanId, PlanState, PlanVersion, Project, ProjectId, SpecNumber,
};
use kanban_dto::{
    ApiError, PlanCreateRequest, PlanEdge, PlanEdgeAddRequest, PlanEdgeRemoveRequest, PlanGetQuery,
    PlanGetResponse, PlanListQuery, PlanListResponse, PlanRecord, PlanSpecAddRequest,
    PlanSpecMoveRequest, PlanSpecRemoveRequest, PlanState as WireState, PlanVersionRecord,
    TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::event_catalog::event_descriptor;
use crate::events::{EventSink, emit_catalogued};
use crate::mutation::{CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;
use crate::timeline::TimelineEnvelope;

/// The storage port Plan commands call through. Implementations land
/// the row changes and the timeline envelope unchanged inside one
/// write, so a plan, its graph, its frozen versions, and the Project
/// counter a mint moves never split across a crash boundary.
pub trait PlanStore: Send + Sync {
    /// Insert a fresh draft Plan. `project` carries the minted Plan
    /// number and the counter move that minted it; both land in the
    /// same write as the Plan row. Storage assigns the Plan's identity
    /// and asks `envelope` for the timeline row that identity belongs
    /// in.
    fn create(
        &self,
        project: &Project,
        number: u64,
        envelope: &dyn Fn(PlanId) -> TimelineEnvelope,
    ) -> Result<Plan, ApiError>;
    /// Load one Plan, if it exists.
    fn find(&self, id: PlanId) -> Result<Option<Plan>, ApiError>;
    /// Persist an applied transition, the working graph, and — when
    /// `freeze` is present — the immutable version the transition
    /// minted, with the timeline envelope, all in one write.
    fn save(
        &self,
        plan: &Plan,
        freeze: Option<&PlanVersion>,
        envelope: TimelineEnvelope,
    ) -> Result<(), ApiError>;
    /// Every Plan of one Project in id order, terminal states
    /// included.
    fn list(&self, project: ProjectId) -> Result<Vec<Plan>, ApiError>;
}

/// The timeline row for one Plan change: on the Project's own
/// timeline, about the Plan, with `action` naming the change inside
/// the closed `transition` kind.
fn transition(project: ProjectId, plan: PlanId, action: &str, facts: Value) -> TimelineEnvelope {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("Plan transition facts are a JSON object");
    object.insert("action".to_owned(), Value::from(action));
    object.insert("id".to_owned(), Value::from(plan.value()));
    TimelineEnvelope::project(
        &project.value().to_string(),
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Plan,
            id: plan.value().to_string(),
        }),
        detail,
    )
    .expect("a minted Plan identity names a Plan")
}

/// Report a refused domain rule as the stable invalid-request code.
fn refuse(error: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

/// The stores every Plan command reads and writes through.
#[derive(Clone)]
struct PlanContext {
    plans: Arc<dyn PlanStore>,
    projects: Arc<dyn ProjectStore>,
    specs: Arc<dyn crate::spec::SpecStore>,
}

impl PlanContext {
    /// The Plan a command addresses and its Project, refusing an
    /// unknown Plan and the terminal archived-Project state.
    fn open(&self, id: u64) -> Result<(Project, Plan), ApiError> {
        let plan = self
            .plans
            .find(PlanId::new(id))?
            .ok_or_else(|| ApiError::not_found(&format!("plan {id}")))?;
        let project = self.projects.find(plan.project())?.ok_or_else(|| {
            ApiError::internal(&format!("plan {id} belongs to no stored Project"))
        })?;
        if project.is_archived() {
            return Err(ApiError::invalid_request(
                "archived is terminal; the Project accepts no further changes",
            ));
        }
        Ok((project, plan))
    }

    /// The Spec number a command names, refusing zero and numbers no
    /// authored Spec carries: a Plan is composed only of Specs that
    /// exist (DR-PS-06).
    fn authored_spec(&self, project: &Project, number: u64) -> Result<SpecNumber, ApiError> {
        let spec = SpecNumber::new(number)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        if self.specs.find_by_number(project.id(), spec)?.is_none() {
            return Err(ApiError::invalid_request(&format!(
                "`{}` has not been authored by this Project",
                NumberKind::Spec.render(project.code(), number),
            )));
        }
        Ok(spec)
    }

    /// The Spec a Plan wants to hold, refusing one already belonging
    /// to another Plan: a Spec belongs to one Plan at a time
    /// (DR-PS-06).
    fn unbound_spec(
        &self,
        project: &Project,
        plan: PlanId,
        spec: SpecNumber,
    ) -> Result<(), ApiError> {
        let bound = self
            .specs
            .find_by_number(project.id(), spec)?
            .and_then(|held| held.plan().filter(|existing| *existing != plan));
        if let Some(existing) = bound {
            return Err(ApiError::invalid_request(&format!(
                "Spec {} already belongs to Plan {existing}",
                spec.value(),
            )));
        }
        Ok(())
    }

    /// Free the Spec a Plan just gave up: a member holding this Plan's
    /// binding loses it and its execution restarts at unplanned, so
    /// the Spec may join another Plan later (DR-PS-06). A member whose
    /// execution already ended keeps that terminal state, so finished
    /// work cannot be rescheduled by a removal. A member bound to
    /// another Plan — one that joined elsewhere — keeps its binding,
    /// and an unbound member writes nothing.
    fn release_spec(
        &self,
        project: &Project,
        plan: PlanId,
        spec: SpecNumber,
    ) -> Result<(), ApiError> {
        if let Some(mut held) = self.specs.find_by_number(project.id(), spec)? {
            if held.plan() == Some(plan) {
                held.leave_plan(plan).map_err(refuse)?;
                let execution = held.execution().wire_name();
                return self.specs.save(
                    &held,
                    crate::spec::transition(
                        project.id(),
                        held.id(),
                        execution,
                        json!({ "plan_id": plan.value() }),
                    ),
                );
            }
        }
        Ok(())
    }
}

impl Core {
    /// Register the Plan operations against `plans`, resolving
    /// Projects through `projects` and authored Specs through
    /// `specs`.
    pub fn register_plans(
        &mut self,
        plans: Arc<dyn PlanStore>,
        projects: Arc<dyn ProjectStore>,
        specs: Arc<dyn crate::spec::SpecStore>,
    ) -> Result<(), RegistrationError> {
        let context = PlanContext {
            plans: plans.clone(),
            projects,
            specs,
        };
        self.register_command("plan.create", Arc::new(CreatePlan(context.clone())))?;
        self.register_command("plan.spec.add", Arc::new(AddSpec(context.clone())))?;
        self.register_command("plan.spec.remove", Arc::new(RemoveSpec(context.clone())))?;
        self.register_command("plan.spec.move", Arc::new(MoveSpec(context.clone())))?;
        self.register_command("plan.edge.add", Arc::new(AddEdge(context.clone())))?;
        self.register_command("plan.edge.remove", Arc::new(RemoveEdge(context.clone())))?;
        self.register_command("plan.activate", Arc::new(ActivatePlan(context.clone())))?;
        self.register_command("plan.replan", Arc::new(ReplanPlan(context.clone())))?;
        self.register_command("plan.complete", Arc::new(CompletePlan(context.clone())))?;
        self.register_command("plan.cancel", Arc::new(CancelPlan(context.clone())))?;
        self.register_command("plan.archive", Arc::new(ArchivePlan(context.clone())))?;
        self.register_query("plan.list", Arc::new(ListPlans { plans }))?;
        self.register_query(
            "plan.get",
            Arc::new(GetPlan {
                plans: context.plans.clone(),
            }),
        )?;
        Ok(())
    }
}

/// Serves `plan.create`.
struct CreatePlan(PlanContext);

impl CommandHandler for CreatePlan {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<PlanCreateRequest>(payload)?;
        ParsedCommand::lift("plan", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        // A fresh aggregate is created at version 0.
        Ok(0)
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: PlanCreateRequest = parse_payload(&command.payload)?;
        let mut project = self
            .0
            .projects
            .find(ProjectId::new(request.project_id))?
            .ok_or_else(|| ApiError::not_found(&format!("project {}", request.project_id)))?;
        if project.is_archived() {
            return Err(ApiError::invalid_request(
                "archived is terminal; the Project accepts no further changes",
            ));
        }
        let number = project.mint(NumberKind::Plan);
        let identity = project.id();
        let plan = self.0.plans.create(&project, number, &|id| {
            transition(
                identity,
                id,
                "created",
                json!({ "project_id": identity.value(), "number": number }),
            )
        })?;
        announce(events, "plan.created", &plan);
        encode_record(&plan)
    }
}

/// Serves `plan.spec.add`.
struct AddSpec(PlanContext);

impl CommandHandler for AddSpec {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<PlanSpecAddRequest>(payload)?;
        ParsedCommand::lift("plan", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: PlanSpecAddRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.plan_id)?.1.version())
    }

    fn apply(&self, command: &ParsedCommand, _events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: PlanSpecAddRequest = parse_payload(&command.payload)?;
        let (project, mut plan) = self.0.open(request.plan_id)?;
        let spec = self.0.authored_spec(&project, request.spec_number)?;
        self.0.unbound_spec(&project, plan.id(), spec)?;
        plan.add_spec(spec).map_err(refuse)?;
        self.0.plans.save(
            &plan,
            None,
            transition(
                project.id(),
                plan.id(),
                "spec_added",
                json!({ "spec_number": request.spec_number }),
            ),
        )?;
        encode_record(&plan)
    }
}

/// Serves `plan.spec.remove`.
struct RemoveSpec(PlanContext);

impl CommandHandler for RemoveSpec {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<PlanSpecRemoveRequest>(payload)?;
        ParsedCommand::lift("plan", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: PlanSpecRemoveRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.plan_id)?.1.version())
    }

    fn apply(&self, command: &ParsedCommand, _events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: PlanSpecRemoveRequest = parse_payload(&command.payload)?;
        let (project, mut plan) = self.0.open(request.plan_id)?;
        let spec = self.0.authored_spec(&project, request.spec_number)?;
        plan.remove_spec(spec).map_err(refuse)?;
        self.0.plans.save(
            &plan,
            None,
            transition(
                project.id(),
                plan.id(),
                "spec_removed",
                json!({ "spec_number": request.spec_number }),
            ),
        )?;
        self.0.release_spec(&project, plan.id(), spec)?;
        encode_record(&plan)
    }
}

/// Serves `plan.spec.move`.
struct MoveSpec(PlanContext);

impl CommandHandler for MoveSpec {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<PlanSpecMoveRequest>(payload)?;
        ParsedCommand::lift("plan", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: PlanSpecMoveRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.plan_id)?.1.version())
    }

    fn apply(&self, command: &ParsedCommand, _events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: PlanSpecMoveRequest = parse_payload(&command.payload)?;
        let (project, mut plan) = self.0.open(request.plan_id)?;
        let spec = self.0.authored_spec(&project, request.spec_number)?;
        let position = usize::try_from(request.position)
            .map_err(|_| PositionOverflow)
            .map_err(refuse)?;
        plan.move_spec(spec, position).map_err(refuse)?;
        self.0.plans.save(
            &plan,
            None,
            transition(
                project.id(),
                plan.id(),
                "spec_moved",
                json!({ "spec_number": request.spec_number, "position": request.position }),
            ),
        )?;
        encode_record(&plan)
    }
}

/// A position no display order can hold; usize's width is not a
/// client concern.
#[derive(Debug)]
struct PositionOverflow;

impl std::fmt::Display for PositionOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the position overflows the display order")
    }
}

impl std::error::Error for PositionOverflow {}

/// Serves `plan.edge.add`.
struct AddEdge(PlanContext);

impl CommandHandler for AddEdge {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<PlanEdgeAddRequest>(payload)?;
        ParsedCommand::lift("plan", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: PlanEdgeAddRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.plan_id)?.1.version())
    }

    fn apply(&self, command: &ParsedCommand, _events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: PlanEdgeAddRequest = parse_payload(&command.payload)?;
        let (project, mut plan) = self.0.open(request.plan_id)?;
        let from = self.0.authored_spec(&project, request.from_spec)?;
        let to = self.0.authored_spec(&project, request.to_spec)?;
        plan.add_edge(from, to).map_err(refuse)?;
        self.0.plans.save(
            &plan,
            None,
            transition(
                project.id(),
                plan.id(),
                "edge_added",
                json!({ "from_spec": request.from_spec, "to_spec": request.to_spec }),
            ),
        )?;
        encode_record(&plan)
    }
}

/// Serves `plan.edge.remove`.
struct RemoveEdge(PlanContext);

impl CommandHandler for RemoveEdge {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<PlanEdgeRemoveRequest>(payload)?;
        ParsedCommand::lift("plan", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: PlanEdgeRemoveRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.plan_id)?.1.version())
    }

    fn apply(&self, command: &ParsedCommand, _events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: PlanEdgeRemoveRequest = parse_payload(&command.payload)?;
        let (project, mut plan) = self.0.open(request.plan_id)?;
        let from = self.0.authored_spec(&project, request.from_spec)?;
        let to = self.0.authored_spec(&project, request.to_spec)?;
        plan.remove_edge(from, to).map_err(refuse)?;
        self.0.plans.save(
            &plan,
            None,
            transition(
                project.id(),
                plan.id(),
                "edge_removed",
                json!({ "from_spec": request.from_spec, "to_spec": request.to_spec }),
            ),
        )?;
        encode_record(&plan)
    }
}

/// Serves `plan.activate`.
struct ActivatePlan(PlanContext);

impl CommandHandler for ActivatePlan {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<kanban_dto::PlanActivateRequest>(payload)?;
        ParsedCommand::lift("plan", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: kanban_dto::PlanActivateRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.plan_id)?.1.version())
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: kanban_dto::PlanActivateRequest = parse_payload(&command.payload)?;
        let (project, mut plan) = self.0.open(request.plan_id)?;
        let frozen = plan.activate().map_err(refuse)?;
        self.0.plans.save(
            &plan,
            Some(&frozen),
            transition(
                project.id(),
                plan.id(),
                "activated",
                json!({
                    "frozen_version": frozen.number(),
                    "spec_numbers": order_of(&plan),
                    "edges": edges_of(&plan),
                }),
            ),
        )?;
        announce(events, "plan.activated", &plan);
        encode_record(&plan)
    }
}

/// Serves `plan.replan`.
struct ReplanPlan(PlanContext);

impl CommandHandler for ReplanPlan {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<kanban_dto::PlanReplanRequest>(payload)?;
        ParsedCommand::lift("plan", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: kanban_dto::PlanReplanRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.plan_id)?.1.version())
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: kanban_dto::PlanReplanRequest = parse_payload(&command.payload)?;
        let (project, mut plan) = self.0.open(request.plan_id)?;
        let superseded = plan
            .versions()
            .last()
            .map(|version| version.number())
            .unwrap_or_default();
        let reserved = plan.replan().map_err(refuse)?;
        self.0.plans.save(
            &plan,
            None,
            transition(
                project.id(),
                plan.id(),
                "replanned",
                json!({ "reserved_version": reserved, "superseded_version": superseded }),
            ),
        )?;
        announce(events, "plan.replanned", &plan);
        encode_record(&plan)
    }
}

/// Serves `plan.complete`.
struct CompletePlan(PlanContext);

impl CommandHandler for CompletePlan {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<kanban_dto::PlanCompleteRequest>(payload)?;
        ParsedCommand::lift("plan", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: kanban_dto::PlanCompleteRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.plan_id)?.1.version())
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: kanban_dto::PlanCompleteRequest = parse_payload(&command.payload)?;
        let (project, mut plan) = self.0.open(request.plan_id)?;
        plan.complete().map_err(refuse)?;
        self.0.plans.save(
            &plan,
            None,
            transition(project.id(), plan.id(), "completed", json!({})),
        )?;
        announce(events, "plan.completed", &plan);
        encode_record(&plan)
    }
}

/// Serves `plan.cancel`.
struct CancelPlan(PlanContext);

impl CommandHandler for CancelPlan {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<kanban_dto::PlanCancelRequest>(payload)?;
        ParsedCommand::lift("plan", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: kanban_dto::PlanCancelRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.plan_id)?.1.version())
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: kanban_dto::PlanCancelRequest = parse_payload(&command.payload)?;
        let (project, mut plan) = self.0.open(request.plan_id)?;
        plan.cancel().map_err(refuse)?;
        self.0.plans.save(
            &plan,
            None,
            transition(project.id(), plan.id(), "cancelled", json!({})),
        )?;
        announce(events, "plan.cancelled", &plan);
        encode_record(&plan)
    }
}

/// Serves `plan.archive`.
struct ArchivePlan(PlanContext);

impl CommandHandler for ArchivePlan {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<kanban_dto::PlanArchiveRequest>(payload)?;
        ParsedCommand::lift("plan", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: kanban_dto::PlanArchiveRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.plan_id)?.1.version())
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: kanban_dto::PlanArchiveRequest = parse_payload(&command.payload)?;
        let (project, mut plan) = self.0.open(request.plan_id)?;
        plan.archive().map_err(refuse)?;
        self.0.plans.save(
            &plan,
            None,
            transition(project.id(), plan.id(), "archived", json!({})),
        )?;
        announce(events, "plan.archived", &plan);
        encode_record(&plan)
    }
}

/// Serves `plan.list`.
struct ListPlans {
    plans: Arc<dyn PlanStore>,
}

impl QueryHandler for ListPlans {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: PlanListQuery = parse_payload(payload)?;
        let response = PlanListResponse {
            plans: self
                .plans
                .list(ProjectId::new(query.project_id))?
                .iter()
                .map(record_of)
                .collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `plan.get`.
struct GetPlan {
    plans: Arc<dyn PlanStore>,
}

impl QueryHandler for GetPlan {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: PlanGetQuery = parse_payload(payload)?;
        let plan = self
            .plans
            .find(PlanId::new(query.plan_id))?
            .ok_or_else(|| ApiError::not_found(&format!("plan {}", query.plan_id)))?;
        let response = PlanGetResponse {
            plan: record_of(&plan),
            versions: plan.versions().iter().map(version_of).collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// The wire form of one lifecycle state.
fn state_of(state: PlanState) -> WireState {
    match state {
        PlanState::Draft => WireState::Draft,
        PlanState::Active => WireState::Active,
        PlanState::Complete => WireState::Complete,
        PlanState::Cancelled => WireState::Cancelled,
        PlanState::Archived => WireState::Archived,
    }
}

/// The display order as JSON, for records and timeline facts.
fn order_of(plan: &Plan) -> Value {
    json!(
        plan.order()
            .iter()
            .map(|spec| spec.value())
            .collect::<Vec<_>>()
    )
}

/// The dependency edges as JSON, for records and timeline facts.
fn edges_of(plan: &Plan) -> Value {
    json!(
        plan.edges()
            .iter()
            .map(|edge| {
                json!({
                    "from_spec": edge.from().value(),
                    "to_spec": edge.to().value(),
                })
            })
            .collect::<Vec<_>>()
    )
}

/// The DTO record for one Plan.
fn record_of(plan: &Plan) -> PlanRecord {
    PlanRecord {
        id: plan.id().value(),
        project_id: plan.project().value(),
        number: plan.number(),
        state: state_of(plan.state()),
        spec_numbers: plan.order().iter().map(|spec| spec.value()).collect(),
        edges: plan
            .edges()
            .iter()
            .map(|edge| PlanEdge {
                from_spec: edge.from().value(),
                to_spec: edge.to().value(),
            })
            .collect(),
        version: plan.version(),
    }
}

/// The DTO record for one frozen version.
fn version_of(version: &PlanVersion) -> PlanVersionRecord {
    PlanVersionRecord {
        number: version.number(),
        spec_numbers: version.order().iter().map(|spec| spec.value()).collect(),
        edges: version
            .edges()
            .iter()
            .map(|edge| PlanEdge {
                from_spec: edge.from().value(),
                to_spec: edge.to().value(),
            })
            .collect(),
    }
}

/// Encode a record for a command response.
fn encode_record(plan: &Plan) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(plan)).map_err(|error| ApiError::internal(&error.to_string()))
}

/// Publish a lifecycle change on the live event stream as exactly the
/// record the command returns. Shape edits announce nothing live: the
/// editor that drives them holds the response, while lifecycle
/// changes interest every surface.
fn announce(events: &dyn EventSink, name: &str, plan: &Plan) {
    emit_catalogued(events, event_descriptor(name), &record_of(plan));
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex};

    use kanban_domain::{
        Plan, PlanId, PlanVersion, Project, ProjectCounters, ProjectId, ProjectRegistration,
        ProjectState,
    };
    use kanban_dto::ApiError;
    use serde_json::Value;

    use super::PlanStore;
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::EventSink;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::project::ProjectStore;
    use crate::timeline::TimelineEnvelope;

    /// An in-memory Project store: rows by id.
    #[derive(Default)]
    pub(crate) struct MemoryProjects {
        pub(crate) state: Mutex<MemoryProjectState>,
    }

    #[derive(Default)]
    pub(crate) struct MemoryProjectState {
        pub(crate) projects: Vec<Project>,
    }

    impl MemoryProjects {
        /// The stored rows, for assertions.
        pub(crate) fn rows(&self) -> Vec<Project> {
            self.state
                .lock()
                .expect("the memory project lock is sound")
                .projects
                .clone()
        }

        /// Insert a stored Project as-is, standing in for one with
        /// minted counters.
        pub(crate) fn seed(&self, project: Project) {
            self.state
                .lock()
                .expect("the memory project lock is sound")
                .projects
                .push(project);
        }

        /// Replace one stored Project row, keeping its identity.
        pub(crate) fn replace(&self, project: Project) {
            let mut state = self.state.lock().expect("the memory project lock is sound");
            if let Some(row) = state
                .projects
                .iter_mut()
                .find(|row| row.id() == project.id())
            {
                *row = project;
            }
        }
    }

    impl ProjectStore for MemoryProjects {
        fn create(
            &self,
            _registration: &ProjectRegistration,
            _envelope: &dyn Fn(ProjectId) -> TimelineEnvelope,
        ) -> Result<Project, ApiError> {
            Err(ApiError::internal(
                "the plan fixtures seed Projects directly",
            ))
        }

        fn find(&self, id: ProjectId) -> Result<Option<Project>, ApiError> {
            Ok(self
                .state
                .lock()
                .expect("the memory project lock is sound")
                .projects
                .iter()
                .find(|row| row.id() == id)
                .cloned())
        }

        fn save(&self, project: &Project, _envelope: TimelineEnvelope) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory project lock is sound");
            if let Some(row) = state
                .projects
                .iter_mut()
                .find(|row| row.id() == project.id())
            {
                *row = project.clone();
            }
            Ok(())
        }

        fn list(&self) -> Result<Vec<Project>, ApiError> {
            Ok(self.rows())
        }
    }

    /// An in-memory Plan store: rows by id, the timeline envelopes it
    /// was asked to land, and the Project rows its writes moved.
    #[derive(Default)]
    pub(crate) struct MemoryPlans {
        state: Mutex<MemoryPlanState>,
        projects: Arc<MemoryProjects>,
    }

    #[derive(Default)]
    struct MemoryPlanState {
        plans: Vec<Plan>,
        next_id: u64,
        timeline: Vec<TimelineEnvelope>,
    }

    impl MemoryPlans {
        /// A plan store sharing the Project rows the harness seeded.
        pub(crate) fn sharing(projects: Arc<MemoryProjects>) -> Self {
            Self {
                projects,
                ..Self::default()
            }
        }

        /// The stored rows and timeline envelopes, for assertions.
        pub(crate) fn snapshot(&self) -> (Vec<Plan>, Vec<TimelineEnvelope>) {
            let state = self.state.lock().expect("the memory plan lock is sound");
            (state.plans.clone(), state.timeline.clone())
        }
    }

    impl PlanStore for MemoryPlans {
        fn create(
            &self,
            project: &Project,
            number: u64,
            envelope: &dyn Fn(PlanId) -> TimelineEnvelope,
        ) -> Result<Plan, ApiError> {
            let mut state = self.state.lock().expect("the memory plan lock is sound");
            // The minted counter lands on the Project row in the same
            // write as the Plan row.
            let projects = &self.projects;
            let mut project_state = projects
                .state
                .lock()
                .expect("the memory project lock is sound");
            if let Some(row) = project_state
                .projects
                .iter_mut()
                .find(|row| row.id() == project.id())
            {
                *row = project.clone();
            }
            state.next_id += 1;
            let id = PlanId::new(state.next_id);
            let plan = Plan::new(id, project.id(), number);
            state.plans.push(plan.clone());
            state.timeline.push(envelope(id));
            Ok(plan)
        }

        fn find(&self, id: PlanId) -> Result<Option<Plan>, ApiError> {
            let state = self.state.lock().expect("the memory plan lock is sound");
            Ok(state.plans.iter().find(|row| row.id() == id).cloned())
        }

        fn save(
            &self,
            plan: &Plan,
            _freeze: Option<&PlanVersion>,
            envelope: TimelineEnvelope,
        ) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory plan lock is sound");
            if let Some(row) = state.plans.iter_mut().find(|row| row.id() == plan.id()) {
                *row = plan.clone();
            }
            state.timeline.push(envelope);
            Ok(())
        }

        fn list(&self, project: ProjectId) -> Result<Vec<Plan>, ApiError> {
            let state = self.state.lock().expect("the memory plan lock is sound");
            Ok(state
                .plans
                .iter()
                .filter(|row| row.project() == project)
                .cloned()
                .collect())
        }
    }

    #[derive(Debug, Default)]
    pub(crate) struct RecordingSink {
        pub(crate) events: Mutex<Vec<(String, Value)>>,
    }

    impl EventSink for RecordingSink {
        fn emit(&self, event_type: &str, payload: Value) {
            self.events
                .lock()
                .expect("the recorder lock is sound")
                .push((event_type.to_owned(), payload));
        }
    }

    /// A core with the Plan operations wired to in-memory stores and
    /// one active Project holding three authored Specs.
    pub(super) struct Harness {
        pub(super) plans: Arc<MemoryPlans>,
        pub(super) projects: Arc<MemoryProjects>,
        pub(super) specs: Arc<crate::spec::testing::MemorySpecs>,
        pub(super) core: Core,
    }

    pub(super) fn harness() -> Harness {
        harness_with_sink(Arc::new(crate::events::NoopEventSink))
    }

    /// A core whose event sink the test chooses.
    pub(super) fn harness_with_sink(events: Arc<dyn EventSink>) -> Harness {
        let projects = Arc::new(MemoryProjects::default());
        projects.seed(active_project(1, "CORE", ProjectCounters::restore(0, 3, 0)));
        let plans = Arc::new(MemoryPlans::sharing(projects.clone()));
        let specs = Arc::new(crate::spec::testing::MemorySpecs::sharing(projects.clone()));
        specs.seed_authored(ProjectId::new(1), &[1, 2, 3]);
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            events,
        );
        core.register_plans(plans.clone(), projects.clone(), specs.clone())
            .expect("the plan operations register");
        Harness {
            plans,
            projects,
            specs,
            core,
        }
    }

    /// One active Project with the counters a test chooses.
    pub(crate) fn active_project(id: u64, code: &str, counters: ProjectCounters) -> Project {
        let registration = ProjectRegistration::new(
            code,
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            None,
        )
        .expect("the fixture registration validates");
        Project::restore(
            ProjectId::new(id),
            registration,
            ProjectState::Active,
            counters,
            1,
        )
    }

    /// A create request against the seeded Project.
    pub(super) fn create(project_id: u64, key: &str) -> Value {
        serde_json::json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": project_id,
        })
    }

    /// A command against one Plan, with the fields tests vary.
    pub(super) fn command(plan_id: u64, body: serde_json::Value, version: u64, key: &str) -> Value {
        let mut request = serde_json::json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "plan_id": plan_id,
        });
        let request_object = request
            .as_object_mut()
            .expect("the command is a JSON object");
        for (field, value) in body.as_object().expect("the body is a JSON object") {
            request_object.insert(field.clone(), value.clone());
        }
        request
    }

    /// The observable record of one freshly created Plan of the
    /// seeded Project.
    pub(super) fn created_record() -> Value {
        serde_json::json!({
            "id": 1,
            "project_id": 1,
            "number": 1,
            "state": "draft",
            "spec_numbers": [],
            "edges": [],
            "version": 1,
        })
    }

    /// A shaped draft: membership 1, 3, 2 with edges 1 → 2 and 3 → 2,
    /// returned as the record's observable fields.
    pub(super) fn shaped_record() -> Value {
        serde_json::json!({
            "spec_numbers": [1, 3, 2],
            "edges": [
                { "from_spec": 1, "to_spec": 2 },
                { "from_spec": 3, "to_spec": 2 },
            ],
        })
    }

    /// Drive one Plan into the shaped draft, returning its aggregate
    /// version after the last edit.
    pub(super) fn shaped_draft(core: &Core) -> u64 {
        shaped_plan(core, "shaped").1
    }

    /// One plan of the seeded Project, shaped with membership 1, 3, 2
    /// and edges 1 → 2 and 3 → 2; returns the plan identity and the
    /// aggregate version.
    pub(super) fn shaped_plan(core: &Core, key: &str) -> (u64, u64) {
        let created = core
            .command("plan.create", &create(1, &format!("{key}-create")))
            .expect("the Plan creates");
        let id = created["id"].as_u64().expect("the identity is a number");
        let mut version = created["version"]
            .as_u64()
            .expect("the version is a number");
        for spec in [1, 3, 2] {
            let response = core
                .command(
                    "plan.spec.add",
                    &command(
                        id,
                        serde_json::json!({ "spec_number": spec }),
                        version,
                        &format!("{key}-add-{spec}"),
                    ),
                )
                .expect("the Spec joins");
            version = response["version"]
                .as_u64()
                .expect("the version is a number");
        }
        for (from, to) in [(1, 2), (3, 2)] {
            let response = core
                .command(
                    "plan.edge.add",
                    &command(
                        id,
                        serde_json::json!({ "from_spec": from, "to_spec": to }),
                        version,
                        &format!("{key}-edge-{from}-{to}"),
                    ),
                )
                .expect("the edge joins");
            version = response["version"]
                .as_u64()
                .expect("the version is a number");
        }
        (id, version)
    }
}

#[cfg(test)]
mod plan_commands {
    use kanban_dto::ErrorCode;
    use serde_json::json;

    use super::testing::{command, create, created_record, harness, shaped_draft, shaped_record};

    #[test]
    fn creating_returns_the_draft_record_and_mints_the_project_number() {
        let harness = harness();

        let response = harness
            .core
            .command("plan.create", &create(1, "key-1"))
            .expect("the Plan creates");

        assert_eq!(response, created_record());
        let projects = harness.projects.rows();
        assert_eq!(
            projects[0].counters().last(kanban_domain::NumberKind::Plan),
            1,
            "creating mints the Project's first Plan number"
        );
    }

    #[test]
    fn creating_a_second_plan_mints_the_next_number() {
        let harness = harness();
        harness
            .core
            .command("plan.create", &create(1, "key-1"))
            .expect("the first Plan creates");

        let response = harness
            .core
            .command("plan.create", &create(1, "key-2"))
            .expect("the second Plan creates");

        assert_eq!(response["number"], json!(2));
        assert_eq!(response["id"], json!(2));
        let projects = harness.projects.rows();
        assert_eq!(
            projects[0].counters().last(kanban_domain::NumberKind::Plan),
            2
        );
    }

    #[test]
    fn creating_for_an_unknown_project_is_not_found() {
        let harness = harness();

        let error = harness
            .core
            .command("plan.create", &create(9, "key-1"))
            .expect_err("the unknown Project is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn creating_for_an_archived_project_is_refused() {
        let harness = harness();
        let mut project = harness.projects.rows()[0].clone();
        project.archive().expect("the fixture archives");
        harness.projects.replace(project);

        let error = harness
            .core
            .command("plan.create", &create(1, "key-1"))
            .expect_err("an archived Project accepts no further changes");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("archived"));
    }

    #[test]
    fn adding_a_spec_appends_to_the_display_order() {
        let harness = harness();
        let created = harness
            .core
            .command("plan.create", &create(1, "key-1"))
            .expect("the Plan creates");
        let id = created["id"].as_u64().expect("the identity is a number");

        let response = harness
            .core
            .command(
                "plan.spec.add",
                &command(id, json!({ "spec_number": 2 }), 1, "key-add"),
            )
            .expect("the Spec joins");

        assert_eq!(response["spec_numbers"], json!([2]));
        assert_eq!(response["version"], json!(2));
    }

    #[test]
    fn adding_an_unminted_spec_number_is_refused() {
        let harness = harness();
        let created = harness
            .core
            .command("plan.create", &create(1, "key-1"))
            .expect("the Plan creates");
        let id = created["id"].as_u64().expect("the identity is a number");

        let error = harness
            .core
            .command(
                "plan.spec.add",
                &command(id, json!({ "spec_number": 9 }), 1, "key-add"),
            )
            .expect_err("an un-authored Spec number is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "`CORE-S9` has not been authored by this Project"
        );
    }

    #[test]
    fn adding_a_spec_bound_to_another_plan_is_refused() {
        let harness = harness();
        let created = harness
            .core
            .command("plan.create", &create(1, "key-1"))
            .expect("the Plan creates");
        let id = created["id"].as_u64().expect("the identity is a number");
        // Spec 1 already planned onto another Plan.
        let mut bound = harness
            .specs
            .snapshot()
            .0
            .into_iter()
            .find(|spec| spec.number().value() == 1)
            .expect("the fixture Spec exists");
        bound
            .assign_to_plan(kanban_domain::PlanId::new(9))
            .expect("the fixture binds");
        harness.specs.replace(bound);

        let error = harness
            .core
            .command(
                "plan.spec.add",
                &command(id, json!({ "spec_number": 1 }), 1, "key-add"),
            )
            .expect_err("a Spec belongs to one Plan at a time");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "Spec 1 already belongs to Plan 9");
    }

    #[test]
    fn adding_spec_zero_is_refused() {
        let harness = harness();
        let created = harness
            .core
            .command("plan.create", &create(1, "key-1"))
            .expect("the Plan creates");
        let id = created["id"].as_u64().expect("the identity is a number");

        let error = harness
            .core
            .command(
                "plan.spec.add",
                &command(id, json!({ "spec_number": 0 }), 1, "key-add"),
            )
            .expect_err("zero names no Spec");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "a Spec number starts at one");
    }

    #[test]
    fn moving_a_spec_changes_only_the_display_order() {
        let harness = harness();
        let version = shaped_draft(&harness.core);

        let response = harness
            .core
            .command(
                "plan.spec.move",
                &command(
                    1,
                    json!({ "spec_number": 2, "position": 0 }),
                    version,
                    "key-move",
                ),
            )
            .expect("the move applies");

        assert_eq!(response["spec_numbers"], json!([2, 1, 3]));
        assert_eq!(
            response["edges"],
            json!([
                { "from_spec": 1, "to_spec": 2 },
                { "from_spec": 3, "to_spec": 2 },
            ]),
            "the edges are a separate relation and do not move"
        );
    }

    #[test]
    fn removing_a_spec_that_carries_edges_is_refused() {
        let harness = harness();
        let version = shaped_draft(&harness.core);

        let error = harness
            .core
            .command(
                "plan.spec.remove",
                &command(1, json!({ "spec_number": 2 }), version, "key-remove"),
            )
            .expect_err("a Spec carrying edges is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "Spec 2 still carries dependency edges; remove them first"
        );
    }

    #[test]
    fn an_edge_leaving_the_single_plan_is_refused() {
        let harness = harness();
        let created = harness
            .core
            .command("plan.create", &create(1, "key-1"))
            .expect("the Plan creates");
        let id = created["id"].as_u64().expect("the identity is a number");
        let mut version = 1;
        for spec in [1, 2] {
            let response = harness
                .core
                .command(
                    "plan.spec.add",
                    &command(
                        id,
                        json!({ "spec_number": spec }),
                        version,
                        &format!("key-add-{spec}"),
                    ),
                )
                .expect("the Spec joins");
            version = response["version"]
                .as_u64()
                .expect("the version is a number");
        }

        // Spec 3 is minted by the Project but is not a member of this
        // Plan: the edge reaches outside the single Plan (DR-DE-01).
        let error = harness
            .core
            .command(
                "plan.edge.add",
                &command(
                    id,
                    json!({ "from_spec": 1, "to_spec": 3 }),
                    version,
                    "key-edge",
                ),
            )
            .expect_err("the cross-Plan edge is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the dependency from Spec 1 to Spec 3 leaves this Plan; edges are legal only within one Plan"
        );
    }

    #[test]
    fn edges_remove_and_free_their_endpoints() {
        let harness = harness();
        let version = shaped_draft(&harness.core);

        let removed = harness
            .core
            .command(
                "plan.edge.remove",
                &command(
                    1,
                    json!({ "from_spec": 1, "to_spec": 2 }),
                    version,
                    "key-edge-remove",
                ),
            )
            .expect("the edge leaves");
        let version = removed["version"]
            .as_u64()
            .expect("the version is a number");

        // The other edge still touches Spec 2, so it cannot leave yet.
        let refused = harness
            .core
            .command(
                "plan.spec.remove",
                &command(1, json!({ "spec_number": 2 }), version, "key-remove"),
            )
            .expect_err("the endpoint is still held");
        assert_eq!(refused.code, ErrorCode::InvalidRequest);

        let removed = harness
            .core
            .command(
                "plan.edge.remove",
                &command(
                    1,
                    json!({ "from_spec": 3, "to_spec": 2 }),
                    version,
                    "key-edge-remove-2",
                ),
            )
            .expect("the last edge leaves");
        let version = removed["version"]
            .as_u64()
            .expect("the version is a number");

        let response = harness
            .core
            .command(
                "plan.spec.remove",
                &command(1, json!({ "spec_number": 2 }), version, "key-remove-2"),
            )
            .expect("the freed endpoint leaves");

        assert_eq!(response["spec_numbers"], json!([1, 3]));
        assert_eq!(response["edges"], json!([]));
    }

    #[test]
    fn a_command_with_a_stale_version_is_rejected_with_the_current_one() {
        let harness = harness();
        let created = harness
            .core
            .command("plan.create", &create(1, "key-1"))
            .expect("the Plan creates");
        let id = created["id"].as_u64().expect("the identity is a number");
        harness
            .core
            .command(
                "plan.spec.add",
                &command(id, json!({ "spec_number": 1 }), 1, "key-add"),
            )
            .expect("the Spec joins");

        let error = harness
            .core
            .command(
                "plan.spec.add",
                &command(id, json!({ "spec_number": 2 }), 1, "key-stale"),
            )
            .expect_err("the stale version is rejected");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(2));
    }

    #[test]
    fn a_retry_replays_without_reapplying() {
        let harness = harness();
        let created = harness
            .core
            .command("plan.create", &create(1, "key-1"))
            .expect("the Plan creates");
        let id = created["id"].as_u64().expect("the identity is a number");
        let request = command(id, json!({ "spec_number": 1 }), 1, "key-add");

        let first = harness
            .core
            .command("plan.spec.add", &request)
            .expect("the Spec joins");
        let replay = harness
            .core
            .command("plan.spec.add", &request)
            .expect("the retry replays");

        assert_eq!(first, replay);
        let (plans, _) = harness.plans.snapshot();
        assert_eq!(plans[0].order().len(), 1, "the retry must not reapply");
    }

    #[test]
    fn commands_reject_unknown_fields() {
        let harness = harness();
        let mut request = command(1, json!({ "spec_number": 1 }), 1, "key-add");
        request["surprise"] = json!(true);

        let error = harness
            .core
            .command("plan.spec.add", &request)
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }

    #[test]
    fn the_shaped_draft_round_trips_through_the_record() {
        let harness = harness();
        let _version = shaped_draft(&harness.core);

        let response = harness
            .core
            .query("plan.get", &json!({ "plan_id": 1 }))
            .expect("the Plan reads");

        for (field, expected) in shaped_record().as_object().expect("the shape is an object") {
            assert_eq!(
                &response["plan"][field], expected,
                "the record carries {field}"
            );
        }
    }
}

#[cfg(test)]
mod plan_lifecycle {
    use kanban_dto::ErrorCode;
    use serde_json::json;

    use super::testing::{command, create, harness, shaped_draft, shaped_plan};

    #[test]
    fn activating_freezes_the_shape_into_version_one() {
        let harness = harness();
        let version = shaped_draft(&harness.core);

        let response = harness
            .core
            .command(
                "plan.activate",
                &command(1, json!({}), version, "key-activate"),
            )
            .expect("the shaped draft activates");

        assert_eq!(response["state"], json!("active"));
        let detail = harness
            .core
            .query("plan.get", &json!({ "plan_id": 1 }))
            .expect("the Plan reads");
        assert_eq!(
            detail["versions"],
            json!([{
                "number": 1,
                "spec_numbers": [1, 3, 2],
                "edges": [
                    { "from_spec": 1, "to_spec": 2 },
                    { "from_spec": 3, "to_spec": 2 },
                ],
            }]),
            "activation freezes membership, order, and graph"
        );
    }

    #[test]
    fn activating_an_empty_plan_is_refused() {
        let harness = harness();
        harness
            .core
            .command("plan.create", &create(1, "key-1"))
            .expect("the Plan creates");

        let error = harness
            .core
            .command("plan.activate", &command(1, json!({}), 1, "key-activate"))
            .expect_err("an empty Plan cannot activate");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "a Plan needs at least one Spec before it can activate"
        );
    }

    #[test]
    fn editing_after_activation_is_refused() {
        let harness = harness();
        let version = shaped_draft(&harness.core);
        harness
            .core
            .command(
                "plan.activate",
                &command(1, json!({}), version, "key-activate"),
            )
            .expect("the Plan activates");

        let error = harness
            .core
            .command(
                "plan.spec.add",
                &command(1, json!({ "spec_number": 2 }), version + 1, "key-add"),
            )
            .expect_err("a frozen Plan accepts no shape edits");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "only a draft Plan accepts this change");
    }

    #[test]
    fn activation_appends_the_frozen_shape_to_the_project_timeline() {
        let harness = harness();
        let version = shaped_draft(&harness.core);

        harness
            .core
            .command(
                "plan.activate",
                &command(1, json!({}), version, "key-activate"),
            )
            .expect("the Plan activates");

        let (_, timeline) = harness.plans.snapshot();
        let activation = timeline.last().expect("the activation appended");
        assert_eq!(activation.kind(), kanban_dto::TimelineEventKind::Transition);
        assert_eq!(
            activation
                .entity()
                .map(|entity| (entity.kind, entity.id.clone())),
            Some((kanban_dto::TimelineEntityKind::Plan, "1".to_owned()))
        );
        assert_eq!(
            activation.detail()["action"],
            json!("activated"),
            "the freeze is an event, not a mutation"
        );
        assert_eq!(activation.detail()["frozen_version"], json!(1));
        assert_eq!(activation.detail()["spec_numbers"], json!([1, 3, 2]));
    }

    #[test]
    fn terminal_states_stay_listed_but_off_the_active_surface() {
        let harness = harness();

        // Three plans, one per terminal state; each shapes first
        // because activation requires membership.
        let (completing, mut version) = shaped_plan(&harness.core, "key-complete");
        let activated = harness
            .core
            .command(
                "plan.activate",
                &command(completing, json!({}), version, "key-activate"),
            )
            .expect("the Plan activates");
        version = activated["version"]
            .as_u64()
            .expect("the version is a number");
        harness
            .core
            .command(
                "plan.complete",
                &command(completing, json!({}), version, "key-complete"),
            )
            .expect("the Plan completes");

        let (cancelling, version) = shaped_plan(&harness.core, "key-cancel");
        harness
            .core
            .command(
                "plan.cancel",
                &command(cancelling, json!({}), version, "key-cancel"),
            )
            .expect("the Plan cancels");

        let (archiving, version) = shaped_plan(&harness.core, "key-archive");
        harness
            .core
            .command(
                "plan.archive",
                &command(archiving, json!({}), version, "key-archive"),
            )
            .expect("the Plan archives");

        let listed = harness
            .core
            .query("plan.list", &json!({ "project_id": 1 }))
            .expect("the list serves");

        let states: Vec<_> = listed["plans"]
            .as_array()
            .expect("the plans are a list")
            .iter()
            .map(|plan| (plan["state"].clone(), plan["version"].is_u64()))
            .collect();
        assert_eq!(
            states,
            vec![
                (json!("complete"), true),
                (json!("cancelled"), true),
                (json!("archived"), true),
            ],
            "every terminal Plan stays listed and queryable"
        );
    }

    #[test]
    fn no_plan_delete_operation_is_catalogued() {
        let names: Vec<_> = crate::catalog::exposed_operations()
            .iter()
            .map(|operation| operation.name)
            .collect();
        assert!(
            !names.contains(&"plan.delete") && !names.contains(&"plan.remove"),
            "Plans are archived, never deleted"
        );
    }
}

#[cfg(test)]
mod replan {
    use kanban_dto::ErrorCode;
    use serde_json::json;

    use super::testing::{command, create, harness, shaped_draft};
    use crate::dispatch::Core;

    /// An activated shaped plan, returning its aggregate version.
    fn activated(core: &Core) -> u64 {
        let version = shaped_draft(core);
        let response = core
            .command(
                "plan.activate",
                &command(1, json!({}), version, "key-activate"),
            )
            .expect("the Plan activates");
        response["version"]
            .as_u64()
            .expect("the version is a number")
    }

    #[test]
    fn replanning_reopens_the_draft_and_reserves_the_second_version() {
        let harness = harness();
        let version = activated(&harness.core);

        let response = harness
            .core
            .command("plan.replan", &command(1, json!({}), version, "key-replan"))
            .expect("the active Plan replans");

        assert_eq!(response["state"], json!("draft"));
        assert_eq!(
            response["spec_numbers"],
            json!([1, 3, 2]),
            "the reopened draft carries the frozen shape"
        );
        let detail = harness
            .core
            .query("plan.get", &json!({ "plan_id": 1 }))
            .expect("the Plan reads");
        assert_eq!(detail["versions"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn replanning_then_reactivating_mints_the_replacement_version() {
        let harness = harness();
        let version = activated(&harness.core);
        let replanned = harness
            .core
            .command("plan.replan", &command(1, json!({}), version, "key-replan"))
            .expect("the Plan replans");
        let version = replanned["version"]
            .as_u64()
            .expect("the version is a number");
        let moved = harness
            .core
            .command(
                "plan.spec.move",
                &command(
                    1,
                    json!({ "spec_number": 2, "position": 0 }),
                    version,
                    "key-move",
                ),
            )
            .expect("the shape changes");
        let version = moved["version"].as_u64().expect("the version is a number");

        let response = harness
            .core
            .command(
                "plan.activate",
                &command(1, json!({}), version, "key-reactivate"),
            )
            .expect("the replacement freezes");

        assert_eq!(response["state"], json!("active"));
        let detail = harness
            .core
            .query("plan.get", &json!({ "plan_id": 1 }))
            .expect("the Plan reads");
        assert_eq!(
            detail["versions"],
            json!([
                {
                    "number": 1,
                    "spec_numbers": [1, 3, 2],
                    "edges": [
                        { "from_spec": 1, "to_spec": 2 },
                        { "from_spec": 3, "to_spec": 2 },
                    ],
                },
                {
                    "number": 2,
                    "spec_numbers": [2, 1, 3],
                    "edges": [
                        { "from_spec": 1, "to_spec": 2 },
                        { "from_spec": 3, "to_spec": 2 },
                    ],
                },
            ]),
            "the replacement is minted while the first version stays queryable and unchanged"
        );
    }

    #[test]
    fn replanning_a_draft_is_refused() {
        let harness = harness();
        harness
            .core
            .command("plan.create", &create(1, "key-1"))
            .expect("the Plan creates");

        let error = harness
            .core
            .command("plan.replan", &command(1, json!({}), 1, "key-replan"))
            .expect_err("a draft Plan has nothing to replace");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "only an active Plan accepts this change");
    }

    #[test]
    fn replanning_appends_an_auditable_timeline_row() {
        let harness = harness();
        let version = activated(&harness.core);

        harness
            .core
            .command("plan.replan", &command(1, json!({}), version, "key-replan"))
            .expect("the Plan replans");

        let (_, timeline) = harness.plans.snapshot();
        let replanned = timeline.last().expect("the replan appended");
        assert_eq!(replanned.detail()["action"], json!("replanned"));
        assert_eq!(replanned.detail()["reserved_version"], json!(2));
        assert_eq!(replanned.detail()["superseded_version"], json!(1));
    }

    #[test]
    fn replanning_publishes_on_the_event_stream() {
        let sink = std::sync::Arc::new(super::testing::RecordingSink::default());
        let harness = super::testing::harness_with_sink(sink.clone());
        let version = activated(&harness.core);

        harness
            .core
            .command("plan.replan", &command(1, json!({}), version, "key-replan"))
            .expect("the Plan replans");

        let events = sink.events.lock().expect("the recorder lock is sound");
        let names: Vec<_> = events.iter().map(|(name, _)| name.as_str()).collect();
        assert!(
            names.contains(&"plan.replanned"),
            "the replan announces live"
        );
        let replanned = events
            .iter()
            .find(|(name, _)| name == "plan.replanned")
            .expect("the replan event is present");
        assert_eq!(replanned.1["state"], json!("draft"));
    }
}
