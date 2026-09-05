//! Spec commands and queries: author the PRD, edit the working
//! content, approve and supersede immutable versions, join a Plan,
//! and move execution along its closed state set (KAN-S3-US4,
//! KAN-S3-US5). Every change appends a timeline row on the Project's
//! own timeline inside the same write, no delete exists, and the
//! execution track never touches a content version.

use kanban_dto::LiveEventName;
use std::sync::Arc;

use kanban_domain::{
    ContentChange, NumberKind, PlanId, Project, ProjectId, Spec, SpecContent as DomainContent,
    SpecContentState as DomainContentState, SpecExecutionState as DomainExecution, SpecId,
    SpecNumber, SpecVersion,
};
use kanban_dto::{
    ApiError, SpecContent as WireContent, SpecContentState as WireContentState,
    SpecContentUpdateRequest, SpecCreateRequest, SpecExecutionMoveRequest,
    SpecExecutionState as WireExecution, SpecGetQuery, SpecGetResponse, SpecListQuery,
    SpecListResponse, SpecPlanJoinRequest, SpecRecord, SpecVersionApproveRequest,
    SpecVersionGetQuery, SpecVersionRecord, SpecVersionSupersedeRequest, TimelineEntityKind,
    TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::events::{EventSink, emit_catalogued};
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::plan::PlanStore;
use crate::project::ProjectStore;
use crate::timeline::TimelineEnvelope;

/// The storage port Spec commands call through. Implementations land
/// the row changes and the timeline envelope unchanged inside one
/// write, so a Spec, its content versions, and the Project counter a
/// mint moves never split across a crash boundary.
pub trait SpecStore: Send + Sync {
    /// Insert a fresh Spec. `project` carries the minted Spec number
    /// and the counter move that minted it; both land in the same
    /// write as the Spec row and its opening draft version. Storage
    /// assigns the Spec's identity and asks `envelope` for the
    /// timeline row that identity belongs in.
    fn create(
        &self,
        project: &Project,
        number: SpecNumber,
        content: &DomainContent,
        envelope: &dyn Fn(SpecId) -> TimelineEnvelope,
    ) -> Result<Spec, ApiError>;
    /// Load one Spec, if it exists.
    fn find(&self, id: SpecId) -> Result<Option<Spec>, ApiError>;
    /// Load one Project's Spec by its minted number, if it exists.
    fn find_by_number(
        &self,
        project: ProjectId,
        number: SpecNumber,
    ) -> Result<Option<Spec>, ApiError>;
    /// Persist the applied Spec — its execution, Plan binding, and
    /// every content version — with the timeline envelope, all in one
    /// write.
    fn save(&self, spec: &Spec, envelope: TimelineEnvelope) -> Result<(), ApiError>;
    /// Every Spec of one Project in id order, terminal execution
    /// states included.
    fn list(&self, project: ProjectId) -> Result<Vec<Spec>, ApiError>;
}

/// The timeline row for one Spec change: on the Project's own
/// timeline, about the Spec, with `action` naming the change inside
/// the closed `transition` kind. Shared with the Plan commands that
/// free a Spec a Plan gave up.
pub(crate) fn transition(
    project: ProjectId,
    spec: SpecId,
    action: &str,
    facts: Value,
) -> TimelineEnvelope {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("Spec transition facts are a JSON object");
    object.insert("action".to_owned(), Value::from(action));
    object.insert("id".to_owned(), Value::from(spec.value()));
    TimelineEnvelope::project(
        project.value(),
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Spec,
            id: spec.value().to_string(),
        }),
        detail,
    )
}

/// Report a refused domain rule as the stable invalid-request code.
fn refuse(error: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

/// The stores every Spec command reads and writes through.
#[derive(Clone)]
struct SpecContext {
    specs: Arc<dyn SpecStore>,
    projects: Arc<dyn ProjectStore>,
    plans: Arc<dyn PlanStore>,
}

impl SpecContext {
    /// The Spec a command addresses and its Project, refusing an
    /// unknown Spec and the terminal archived-Project state.
    fn open(&self, id: u64) -> Result<(Project, Spec), ApiError> {
        let spec = self
            .specs
            .find(SpecId::new(id))?
            .ok_or_else(|| ApiError::not_found(&format!("spec {id}")))?;
        let project = self.projects.find(spec.project())?.ok_or_else(|| {
            ApiError::internal(&format!("spec {id} belongs to no stored Project"))
        })?;
        if project.is_archived() {
            return Err(ApiError::invalid_request(
                "archived is terminal; the Project accepts no further changes",
            ));
        }
        Ok((project, spec))
    }
}

impl Core {
    /// Register the Spec operations against `specs`, resolving
    /// Projects through `projects` and membership through `plans`.
    pub fn register_specs(
        &mut self,
        specs: Arc<dyn SpecStore>,
        projects: Arc<dyn ProjectStore>,
        plans: Arc<dyn PlanStore>,
    ) -> Result<(), RegistrationError> {
        let context = SpecContext {
            specs: specs.clone(),
            projects,
            plans,
        };
        self.register_command("spec.create", Arc::new(CreateSpec(context.clone())))?;
        self.register_command(
            "spec.content.update",
            Arc::new(UpdateContent(context.clone())),
        )?;
        self.register_command(
            "spec.version.approve",
            Arc::new(ApproveVersion(context.clone())),
        )?;
        self.register_command(
            "spec.version.supersede",
            Arc::new(SupersedeVersion(context.clone())),
        )?;
        self.register_command("spec.plan.join", Arc::new(JoinPlan(context.clone())))?;
        self.register_command(
            "spec.execution.move",
            Arc::new(MoveExecution(context.clone())),
        )?;
        self.register_query("spec.list", Arc::new(ListSpecs { specs }))?;
        self.register_query(
            "spec.get",
            Arc::new(GetSpec {
                specs: context.specs.clone(),
            }),
        )?;
        self.register_query(
            "spec.version.get",
            Arc::new(GetVersion {
                specs: context.specs.clone(),
            }),
        )?;
        self.register_coverage_check(context.specs.clone(), context.projects.clone())?;
        Ok(())
    }
}

/// Serves `spec.create`.
struct CreateSpec(SpecContext);

impl CommandHandler for CreateSpec {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<SpecCreateRequest>(payload)?;
        ParsedCommand::lift("spec", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        // A fresh aggregate is created at version 0.
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: SpecCreateRequest = parse_payload(&command.payload)?;
        let mut project = self
            .0
            .projects
            .find(ProjectId::new(request.project_id))?
            .ok_or_else(|| ApiError::not_found(&format!("project {}", request.project_id)))?;
        let content = content_of(&request.content)?;
        let number = SpecNumber::new(project.mint(NumberKind::Spec).map_err(refuse)?)
            .expect("a minted number is positive");
        let identity = project.id();
        let spec = self.0.specs.create(&project, number, &content, &|id| {
            transition(
                identity,
                id,
                "created",
                json!({ "project_id": identity.value(), "number": number.value() }),
            )
        })?;
        announce(effects, LiveEventName::SpecCreated, &spec);
        encode_record(&spec)
    }
}

/// Serves `spec.content.update`.
struct UpdateContent(SpecContext);

impl CommandHandler for UpdateContent {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<SpecContentUpdateRequest>(payload)?;
        ParsedCommand::lift("spec", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: SpecContentUpdateRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.spec_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: SpecContentUpdateRequest = parse_payload(&command.payload)?;
        let (project, mut spec) = self.0.open(request.spec_id)?;
        let content = content_of(&request.content)?;
        let change = spec.update_content(content).map_err(refuse)?;
        let (action, number) = match change {
            ContentChange::Edited { number } => ("content_edited", number),
            ContentChange::Minted { number } => ("version_minted", number),
        };
        self.0.specs.save(
            &spec,
            transition(
                project.id(),
                spec.id(),
                action,
                json!({ "version": number }),
            ),
        )?;
        encode_record(&spec)
    }
}

/// Serves `spec.version.approve`.
struct ApproveVersion(SpecContext);

impl CommandHandler for ApproveVersion {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<SpecVersionApproveRequest>(payload)?;
        ParsedCommand::lift("spec", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: SpecVersionApproveRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.spec_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: SpecVersionApproveRequest = parse_payload(&command.payload)?;
        let (project, mut spec) = self.0.open(request.spec_id)?;
        let approved = spec.approve_version().map_err(refuse)?;
        self.0.specs.save(
            &spec,
            transition(
                project.id(),
                spec.id(),
                "version_approved",
                json!({ "version": approved }),
            ),
        )?;
        announce(effects, LiveEventName::SpecVersionApproved, &spec);
        encode_record(&spec)
    }
}

/// Serves `spec.version.supersede`.
struct SupersedeVersion(SpecContext);

impl CommandHandler for SupersedeVersion {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<SpecVersionSupersedeRequest>(payload)?;
        ParsedCommand::lift("spec", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: SpecVersionSupersedeRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.spec_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: SpecVersionSupersedeRequest = parse_payload(&command.payload)?;
        let (project, mut spec) = self.0.open(request.spec_id)?;
        spec.supersede_version(request.version).map_err(refuse)?;
        self.0.specs.save(
            &spec,
            transition(
                project.id(),
                spec.id(),
                "version_superseded",
                json!({ "version": request.version }),
            ),
        )?;
        announce(effects, LiveEventName::SpecVersionSuperseded, &spec);
        encode_record(&spec)
    }
}

/// Serves `spec.plan.join`.
struct JoinPlan(SpecContext);

impl CommandHandler for JoinPlan {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<SpecPlanJoinRequest>(payload)?;
        ParsedCommand::lift("spec", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: SpecPlanJoinRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.spec_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: SpecPlanJoinRequest = parse_payload(&command.payload)?;
        let (project, mut spec) = self.0.open(request.spec_id)?;
        let plan = self
            .0
            .plans
            .find(PlanId::new(request.plan_id))?
            .ok_or_else(|| ApiError::not_found(&format!("plan {}", request.plan_id)))?;
        if plan.project() != spec.project() {
            return Err(ApiError::invalid_request(
                "the Plan belongs to another Project",
            ));
        }
        if plan.state().is_terminal() {
            return Err(ApiError::invalid_request(
                "only a draft or active Plan can take on a Spec",
            ));
        }
        if !plan.order().contains(&spec.number()) {
            return Err(ApiError::invalid_request(&format!(
                "`{}` is not a member of Plan {}; add the Spec to the Plan first",
                NumberKind::Spec.render(project.code(), spec.number().value()),
                request.plan_id,
            )));
        }
        spec.assign_to_plan(plan.id()).map_err(refuse)?;
        self.0.specs.save(
            &spec,
            transition(
                project.id(),
                spec.id(),
                "planned",
                json!({ "plan_id": plan.id().value() }),
            ),
        )?;
        announce(effects, LiveEventName::SpecPlanned, &spec);
        encode_record(&spec)
    }
}

/// Serves `spec.execution.move`.
struct MoveExecution(SpecContext);

impl CommandHandler for MoveExecution {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<SpecExecutionMoveRequest>(payload)?;
        ParsedCommand::lift("spec", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: SpecExecutionMoveRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.spec_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: SpecExecutionMoveRequest = parse_payload(&command.payload)?;
        let (project, mut spec) = self.0.open(request.spec_id)?;
        let from = spec.execution();
        let to = execution_named(request.execution);
        spec.transition_execution(to).map_err(refuse)?;
        self.0.specs.save(
            &spec,
            transition(
                project.id(),
                spec.id(),
                "execution_moved",
                json!({
                    "from": from.wire_name(),
                    "to": to.wire_name(),
                }),
            ),
        )?;
        announce(effects, LiveEventName::SpecExecutionMoved, &spec);
        encode_record(&spec)
    }
}

/// Serves `spec.list`.
struct ListSpecs {
    specs: Arc<dyn SpecStore>,
}

impl QueryHandler for ListSpecs {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: SpecListQuery = parse_payload(payload)?;
        let response = SpecListResponse {
            specs: self
                .specs
                .list(ProjectId::new(query.project_id))?
                .iter()
                .map(record_of)
                .collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `spec.get`.
struct GetSpec {
    specs: Arc<dyn SpecStore>,
}

impl QueryHandler for GetSpec {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: SpecGetQuery = parse_payload(payload)?;
        let spec = self
            .specs
            .find(SpecId::new(query.spec_id))?
            .ok_or_else(|| ApiError::not_found(&format!("spec {}", query.spec_id)))?;
        let response = SpecGetResponse {
            spec: record_of(&spec),
            versions: spec.versions().iter().map(version_of).collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `spec.version.get` — the lookup a Ticket's pin resolves
/// through, superseded versions included (DR-PS-11).
struct GetVersion {
    specs: Arc<dyn SpecStore>,
}

impl QueryHandler for GetVersion {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: SpecVersionGetQuery = parse_payload(payload)?;
        let spec = self
            .specs
            .find(SpecId::new(query.spec_id))?
            .ok_or_else(|| ApiError::not_found(&format!("spec {}", query.spec_id)))?;
        let pinned = spec
            .pinned_version(query.number)
            .ok_or_else(|| ApiError::not_found(&format!("version {}", query.number)))?;
        serde_json::to_value(version_of(pinned))
            .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Decode wire PRD content into the domain's validated content.
fn content_of(payload: &WireContent) -> Result<DomainContent, ApiError> {
    DomainContent::new(
        payload.name.clone(),
        payload.short_description.clone(),
        payload.problem_statement.clone(),
        payload.solution.clone(),
        payload.user_stories.clone(),
        payload.implementation_decisions.clone(),
        payload.testing_decisions.clone(),
        payload.out_of_scope.clone(),
        payload.further_notes.clone(),
    )
    .map_err(refuse)
}

/// Encode domain content as the wire PRD payload.
fn payload_of(content: &DomainContent) -> WireContent {
    WireContent {
        name: content.name().to_owned(),
        short_description: content.short_description().to_owned(),
        problem_statement: content.problem_statement().to_owned(),
        solution: content.solution().to_owned(),
        user_stories: content.user_stories().to_owned(),
        implementation_decisions: content.implementation_decisions().to_owned(),
        testing_decisions: content.testing_decisions().to_owned(),
        out_of_scope: content.out_of_scope().to_owned(),
        further_notes: content.further_notes().to_owned(),
    }
}

/// The wire form of one content state.
fn content_state_of(state: DomainContentState) -> WireContentState {
    match state {
        DomainContentState::Draft => WireContentState::Draft,
        DomainContentState::Approved => WireContentState::Approved,
        DomainContentState::Superseded => WireContentState::Superseded,
    }
}

/// The domain form of one wire execution state. The payload decoded
/// through the closed vocabulary or never reached the handler.
fn execution_named(state: WireExecution) -> DomainExecution {
    match state {
        WireExecution::Unplanned => DomainExecution::Unplanned,
        WireExecution::Planned => DomainExecution::Planned,
        WireExecution::Blocked => DomainExecution::Blocked,
        WireExecution::Ready => DomainExecution::Ready,
        WireExecution::Active => DomainExecution::Active,
        WireExecution::IntegrationReview => DomainExecution::IntegrationReview,
        WireExecution::Complete => DomainExecution::Complete,
        WireExecution::Cancelled => DomainExecution::Cancelled,
    }
}

/// The wire form of one execution state.
fn execution_of(state: DomainExecution) -> WireExecution {
    match state {
        DomainExecution::Unplanned => WireExecution::Unplanned,
        DomainExecution::Planned => WireExecution::Planned,
        DomainExecution::Blocked => WireExecution::Blocked,
        DomainExecution::Ready => WireExecution::Ready,
        DomainExecution::Active => WireExecution::Active,
        DomainExecution::IntegrationReview => WireExecution::IntegrationReview,
        DomainExecution::Complete => WireExecution::Complete,
        DomainExecution::Cancelled => WireExecution::Cancelled,
    }
}

/// The DTO record for one Spec.
fn record_of(spec: &Spec) -> SpecRecord {
    SpecRecord {
        id: spec.id().value(),
        project_id: spec.project().value(),
        number: spec.number().value(),
        name: spec.name().to_owned(),
        execution: execution_of(spec.execution()),
        plan_id: spec.plan().map(|plan| plan.value()),
        version: spec.version(),
    }
}

/// The DTO record for one content version.
fn version_of(version: &SpecVersion) -> SpecVersionRecord {
    SpecVersionRecord {
        number: version.number(),
        state: content_state_of(version.state()),
        content: payload_of(version.content()),
    }
}

/// Encode a record for a command response.
fn encode_record(spec: &Spec) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(spec)).map_err(|error| ApiError::internal(&error.to_string()))
}

/// Publish a lifecycle change on the live event stream as exactly the
/// record the command returns. Content edits announce nothing live:
/// the editor that drives them holds the response, while lifecycle
/// changes interest every surface.
fn announce(events: &dyn EventSink, name: LiveEventName, spec: &Spec) {
    emit_catalogued(events, name, &record_of(spec));
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex};

    use kanban_domain::{ProjectId, Spec, SpecContent as DomainContent, SpecId, SpecNumber};
    use kanban_dto::ApiError;

    use super::SpecStore;
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::EventSink;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::plan::testing::{MemoryPlans, MemoryProjects};
    use crate::timeline::TimelineEnvelope;

    /// An in-memory Spec store: rows by id, the timeline envelopes it
    /// was asked to land, and the Project rows its writes moved.
    #[derive(Default)]
    pub(crate) struct MemorySpecs {
        state: Mutex<MemorySpecState>,
        projects: Arc<MemoryProjects>,
    }

    #[derive(Default)]
    struct MemorySpecState {
        specs: Vec<Spec>,
        next_id: u64,
        timeline: Vec<TimelineEnvelope>,
    }

    impl MemorySpecs {
        /// A spec store sharing the Project rows the harness seeded.
        pub(crate) fn sharing(projects: Arc<MemoryProjects>) -> Self {
            Self {
                projects,
                ..Self::default()
            }
        }

        /// Insert authored Spec rows as-is, standing in for Specs the
        /// Project minted before the fixture began.
        pub(crate) fn seed_authored(&self, project: ProjectId, numbers: &[u64]) {
            let mut state = self.state.lock().expect("the memory spec lock is sound");
            for number in numbers {
                let spec = Spec::new(
                    SpecId::new(state.next_id + 1),
                    project,
                    SpecNumber::new(*number).expect("the fixture number is positive"),
                    DomainContent::new(format!("Fixture {number}"), "", "", "", "", "", "", "", "")
                        .expect("the fixture content validates"),
                )
                .expect("the fixture content validates");
                state.next_id += 1;
                state.specs.push(spec);
            }
        }

        /// Replace one stored Spec row, keeping its identity.
        pub(crate) fn replace(&self, spec: Spec) {
            let mut state = self.state.lock().expect("the memory spec lock is sound");
            if let Some(row) = state.specs.iter_mut().find(|row| row.id() == spec.id()) {
                *row = spec;
            }
        }

        /// The stored rows and timeline envelopes, for assertions.
        pub(crate) fn snapshot(&self) -> (Vec<Spec>, Vec<TimelineEnvelope>) {
            let state = self.state.lock().expect("the memory spec lock is sound");
            (state.specs.clone(), state.timeline.clone())
        }
    }

    impl SpecStore for MemorySpecs {
        fn create(
            &self,
            project: &kanban_domain::Project,
            number: SpecNumber,
            content: &DomainContent,
            envelope: &dyn Fn(SpecId) -> TimelineEnvelope,
        ) -> Result<Spec, ApiError> {
            let mut state = self.state.lock().expect("the memory spec lock is sound");
            // The minted counter lands on the Project row in the same
            // write as the Spec row.
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
            let id = SpecId::new(state.next_id);
            let spec = Spec::new(id, project.id(), number, content.clone())
                .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
            state.specs.push(spec.clone());
            state.timeline.push(envelope(id));
            Ok(spec)
        }

        fn find(&self, id: SpecId) -> Result<Option<Spec>, ApiError> {
            let state = self.state.lock().expect("the memory spec lock is sound");
            Ok(state.specs.iter().find(|row| row.id() == id).cloned())
        }

        fn find_by_number(
            &self,
            project: kanban_domain::ProjectId,
            number: SpecNumber,
        ) -> Result<Option<Spec>, ApiError> {
            let state = self.state.lock().expect("the memory spec lock is sound");
            Ok(state
                .specs
                .iter()
                .find(|row| row.project() == project && row.number() == number)
                .cloned())
        }

        fn save(&self, spec: &Spec, envelope: TimelineEnvelope) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory spec lock is sound");
            if let Some(row) = state.specs.iter_mut().find(|row| row.id() == spec.id()) {
                *row = spec.clone();
            }
            state.timeline.push(envelope);
            Ok(())
        }

        fn list(&self, project: kanban_domain::ProjectId) -> Result<Vec<Spec>, ApiError> {
            let state = self.state.lock().expect("the memory spec lock is sound");
            Ok(state
                .specs
                .iter()
                .filter(|row| row.project() == project)
                .cloned()
                .collect())
        }
    }

    /// A core with the Spec and Plan operations wired to in-memory
    /// stores over one active Project.
    pub(crate) struct SpecHarness {
        pub(crate) specs: Arc<MemorySpecs>,
        pub(crate) projects: Arc<MemoryProjects>,
        pub(crate) core: Core,
    }

    /// A harness whose event sink the test chooses.
    pub(crate) fn spec_harness_with_sink(events: Arc<dyn EventSink>) -> SpecHarness {
        spec_harness_composed(events, Arc::new(crate::diagnostics::AbsentCatalogue))
    }

    /// A harness whose planning diagnostics read the profile catalogue
    /// given: the seam the execution profile catalogue (KAN-S7, T38)
    /// fills.
    pub(crate) fn spec_harness_with_catalogue(
        profiles: Arc<dyn crate::diagnostics::ProfileCatalogue>,
    ) -> SpecHarness {
        spec_harness_composed(Arc::new(crate::events::NoopEventSink), profiles)
    }

    /// A harness over in-memory stores whose planning diagnostics read
    /// the catalogue given; registering them again replaces the absent
    /// catalogue `register_plans` installs.
    fn spec_harness_composed(
        events: Arc<dyn EventSink>,
        profiles: Arc<dyn crate::diagnostics::ProfileCatalogue>,
    ) -> SpecHarness {
        let projects = Arc::new(MemoryProjects::default());
        projects.seed(crate::plan::testing::active_project(
            1,
            "CORE",
            kanban_domain::ProjectCounters::restore(0, 0, 0),
        ));
        let plans = Arc::new(MemoryPlans::sharing(projects.clone()));
        let specs = Arc::new(MemorySpecs::sharing(projects.clone()));
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            events,
        );
        core.register_plans(plans.clone(), projects.clone(), specs.clone())
            .expect("the plan operations register");
        core.register_plan_diagnostics(plans.clone(), projects.clone(), specs.clone(), profiles)
            .expect("the diagnostics register against the catalogue");
        core.register_specs(specs.clone(), projects.clone(), plans.clone())
            .expect("the spec operations register");
        SpecHarness {
            specs,
            projects,
            core,
        }
    }

    /// A harness with a silent event sink.
    pub(crate) fn spec_harness() -> SpecHarness {
        spec_harness_with_sink(Arc::new(crate::events::NoopEventSink))
    }

    /// The wire PRD content the tests vary by name.
    pub(crate) fn wire_content(name: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "short_description": "Versioned Plan graphs of Specs",
            "problem_statement": "Planning must survive change without losing truth.",
            "solution": "Immutable approved versions.",
            "user_stories": "KAN-S3-US4",
            "implementation_decisions": "Supersession is explicit.",
            "testing_decisions": "Domain tests prove immutability.",
            "out_of_scope": "The Ticket graph proposal.",
            "further_notes": "None",
        })
    }

    /// A create request against the seeded Project.
    pub(crate) fn create(project_id: u64, name: &str, key: &str) -> serde_json::Value {
        serde_json::json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": project_id,
            "content": wire_content(name),
        })
    }

    /// A command against one Spec, with the fields tests vary.
    pub(crate) fn command(
        spec_id: u64,
        body: serde_json::Value,
        version: u64,
        key: &str,
    ) -> serde_json::Value {
        let mut request = serde_json::json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "spec_id": spec_id,
        });
        let request_object = request
            .as_object_mut()
            .expect("the command is a JSON object");
        for (field, value) in body.as_object().expect("the body is a JSON object") {
            request_object.insert(field.clone(), value.clone());
        }
        request
    }

    /// Author one Spec on the seeded Project, returning its identity
    /// and aggregate version.
    pub(crate) fn authored(core: &Core, name: &str, key: &str) -> (u64, u64) {
        let created = core
            .command("spec.create", &create(1, name, key))
            .expect("the Spec authors");
        (
            created["id"].as_u64().expect("the identity is a number"),
            created["version"]
                .as_u64()
                .expect("the version is a number"),
        )
    }

    /// Approve the working draft of one Spec, returning the aggregate
    /// version after the approval.
    pub(crate) fn approved(core: &Core, spec_id: u64, version: u64, key: &str) -> u64 {
        let response = core
            .command(
                "spec.version.approve",
                &command(spec_id, serde_json::json!({}), version, key),
            )
            .expect("the draft approves");
        response["version"]
            .as_u64()
            .expect("the version is a number")
    }

    /// Compose and keep a draft Plan holding every authored Spec
    /// number in `members`, returning the Plan identity.
    pub(crate) fn plan_holding(core: &Core, members: &[u64], key: &str) -> u64 {
        let created = core
            .command(
                "plan.create",
                &serde_json::json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": format!("{key}-create") },
                    "project_id": 1,
                }),
            )
            .expect("the Plan creates");
        let plan_id = created["id"].as_u64().expect("the identity is a number");
        let mut version = created["version"]
            .as_u64()
            .expect("the version is a number");
        for member in members {
            let response = core
                .command(
                    "plan.spec.add",
                    &serde_json::json!({
                        "mutation": {
                            "optimistic_version": version,
                            "idempotency_key": format!("{key}-add-{member}"),
                        },
                        "plan_id": plan_id,
                        "spec_number": member,
                    }),
                )
                .expect("the Spec joins the Plan");
            version = response["version"]
                .as_u64()
                .expect("the version is a number");
        }
        plan_id
    }

    /// Join one Spec to a Plan holding it, returning the aggregate
    /// version after the join.
    pub(crate) fn joined(core: &Core, spec_id: u64, plan_id: u64, version: u64) -> u64 {
        let response = core
            .command(
                "spec.plan.join",
                &command(
                    spec_id,
                    serde_json::json!({ "plan_id": plan_id }),
                    version,
                    "key-join",
                ),
            )
            .expect("the Spec joins its Plan");
        response["version"]
            .as_u64()
            .expect("the version is a number")
    }
}

#[cfg(test)]
mod spec_timeline {
    use kanban_domain::{ProjectId, SpecId};
    use kanban_dto::{TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineScope};
    use serde_json::json;

    use super::transition;

    #[test]
    fn created_transition_names_the_spec_on_the_project_timeline() {
        let envelope = transition(
            ProjectId::new(1),
            SpecId::new(4),
            "created",
            json!({ "project_id": 1, "number": 2 }),
        );

        assert_eq!(envelope.kind(), TimelineEventKind::Transition);
        assert!(matches!(envelope.scope(), TimelineScope::Project(1)));
        assert_eq!(
            envelope.entity(),
            Some(&TimelineEntityRef {
                kind: TimelineEntityKind::Spec,
                id: "4".to_owned(),
            })
        );
        assert_eq!(envelope.detail()["action"], json!("created"));
        assert_eq!(envelope.detail()["id"], json!(4));
        assert_eq!(envelope.detail()["number"], json!(2));
    }

    #[test]
    fn execution_move_transition_records_from_and_to_states() {
        let envelope = transition(
            ProjectId::new(1),
            SpecId::new(4),
            "execution_moved",
            json!({ "from": "planned", "to": "ready" }),
        );

        assert_eq!(envelope.detail()["action"], json!("execution_moved"));
        assert_eq!(envelope.detail()["from"], json!("planned"));
        assert_eq!(envelope.detail()["to"], json!("ready"));
    }
}

#[cfg(test)]
mod spec_commands {
    use kanban_dto::ErrorCode;
    use serde_json::json;

    use super::testing::{authored, command, create, spec_harness, wire_content};

    #[test]
    fn creating_returns_the_record_and_mints_the_project_number() {
        let harness = spec_harness();

        let response = harness
            .core
            .command("spec.create", &create(1, "Registration", "key-1"))
            .expect("the Spec authors");

        assert_eq!(
            response,
            json!({
                "id": 1,
                "project_id": 1,
                "number": 1,
                "name": "Registration",
                "execution": "unplanned",
                "plan_id": null,
                "version": 1,
            })
        );
        let projects = harness.projects.rows();
        assert_eq!(
            projects[0].counters().last(kanban_domain::NumberKind::Spec),
            1,
            "creating mints the Project's first Spec number"
        );
    }

    #[test]
    fn creating_a_second_spec_mints_the_next_number() {
        let harness = spec_harness();
        harness
            .core
            .command("spec.create", &create(1, "Registration", "key-1"))
            .expect("the first Spec authors");

        let response = harness
            .core
            .command("spec.create", &create(1, "Timeline", "key-2"))
            .expect("the second Spec authors");

        assert_eq!(response["number"], json!(2));
        assert_eq!(response["id"], json!(2));
    }

    #[test]
    fn creating_for_an_unknown_project_is_not_found() {
        let harness = spec_harness();

        let error = harness
            .core
            .command("spec.create", &create(9, "Registration", "key-1"))
            .expect_err("the unknown Project is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn creating_for_an_archived_project_is_refused() {
        let harness = spec_harness();
        let mut project = harness.projects.rows()[0].clone();
        project.archive().expect("the fixture archives");
        harness.projects.replace(project);

        let error = harness
            .core
            .command("spec.create", &create(1, "Registration", "key-1"))
            .expect_err("an archived Project accepts no further changes");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("archived"));
    }

    #[test]
    fn unnamed_content_is_refused() {
        let harness = spec_harness();
        let mut request = create(1, "Registration", "key-1");
        request["content"] = json!({
            "name": "   ",
            "short_description": "",
            "problem_statement": "",
            "solution": "",
            "user_stories": "",
            "implementation_decisions": "",
            "testing_decisions": "",
            "out_of_scope": "",
            "further_notes": "",
        });

        let error = harness
            .core
            .command("spec.create", &request)
            .expect_err("a Spec needs a name");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "a Spec needs a name");
    }

    #[test]
    fn updating_the_draft_edits_it_without_minting() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");

        let response = harness
            .core
            .command(
                "spec.content.update",
                &command(
                    id,
                    json!({ "content": wire_content("Registration, revised") }),
                    version,
                    "key-update",
                ),
            )
            .expect("the draft edits");

        assert_eq!(response["name"], json!("Registration, revised"));
        let detail = harness
            .core
            .query("spec.get", &json!({ "spec_id": id }))
            .expect("the Spec reads");
        assert_eq!(detail["versions"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            detail["versions"][0]["content"]["name"],
            json!("Registration, revised")
        );
    }

    #[test]
    fn a_material_change_after_approval_mints_a_new_version() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");
        let version = super::testing::approved(&harness.core, id, version, "key-approve");

        let response = harness
            .core
            .command(
                "spec.content.update",
                &command(
                    id,
                    json!({ "content": wire_content("Registration, revised") }),
                    version,
                    "key-update",
                ),
            )
            .expect("the material change mints");

        assert_eq!(response["name"], json!("Registration, revised"));
        let detail = harness
            .core
            .query("spec.get", &json!({ "spec_id": id }))
            .expect("the Spec reads");
        assert_eq!(
            detail["versions"],
            json!([
                {
                    "number": 1,
                    "state": "approved",
                    "content": wire_content("Registration"),
                },
                {
                    "number": 2,
                    "state": "draft",
                    "content": wire_content("Registration, revised"),
                },
            ]),
            "the approved version is unchanged beside its replacement"
        );
    }

    #[test]
    fn approving_without_a_draft_is_refused() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");
        let version = super::testing::approved(&harness.core, id, version, "key-approve");

        let error = harness
            .core
            .command(
                "spec.version.approve",
                &command(id, json!({}), version, "key-approve-again"),
            )
            .expect_err("approved content is no draft to approve");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "only a draft content version can be approved"
        );
    }

    #[test]
    fn superseding_keeps_the_pinned_version_queryable() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");
        let version = super::testing::approved(&harness.core, id, version, "key-approve");

        harness
            .core
            .command(
                "spec.version.supersede",
                &command(id, json!({ "version": 1 }), version, "key-supersede"),
            )
            .expect("the version supersedes");

        // A Ticket pinned to version one keeps resolving, superseded
        // or not (DR-PS-11).
        let pinned = harness
            .core
            .query("spec.version.get", &json!({ "spec_id": id, "number": 1 }))
            .expect("the pinned version reads");
        assert_eq!(pinned["state"], json!("superseded"));
        assert_eq!(pinned["content"]["name"], json!("Registration"));

        let error = harness
            .core
            .query("spec.version.get", &json!({ "spec_id": id, "number": 9 }))
            .expect_err("an unknown version is refused");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn a_superseded_version_cannot_supersede_again() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");
        let version = super::testing::approved(&harness.core, id, version, "key-approve");
        let response = harness
            .core
            .command(
                "spec.version.supersede",
                &command(id, json!({ "version": 1 }), version, "key-supersede"),
            )
            .expect("the version supersedes");
        let version = response["version"]
            .as_u64()
            .expect("the version is a number");

        let error = harness
            .core
            .command(
                "spec.version.supersede",
                &command(id, json!({ "version": 1 }), version, "key-supersede-again"),
            )
            .expect_err("superseded is terminal");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "version 1 is already superseded");
    }

    #[test]
    fn version_moves_append_auditable_timeline_rows() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");

        harness
            .core
            .command(
                "spec.version.approve",
                &command(id, json!({}), version, "key-approve"),
            )
            .expect("the draft approves");

        let (_, timeline) = harness.specs.snapshot();
        let approval = timeline.last().expect("the approval appended");
        assert_eq!(approval.kind(), kanban_dto::TimelineEventKind::Transition);
        assert_eq!(
            approval
                .entity()
                .map(|entity| (entity.kind, entity.id.clone())),
            Some((kanban_dto::TimelineEntityKind::Spec, "1".to_owned()))
        );
        assert_eq!(approval.detail()["action"], json!("version_approved"));
        assert_eq!(approval.detail()["version"], json!(1));
    }

    #[test]
    fn a_stale_command_is_rejected_with_the_current_version() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");

        let error = harness
            .core
            .command(
                "spec.version.approve",
                &command(id, json!({}), version - 1, "key-stale"),
            )
            .expect_err("the stale version is rejected");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(version));
    }

    #[test]
    fn a_retry_replays_without_reapplying() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");
        let request = command(id, json!({}), version, "key-approve");

        let first = harness
            .core
            .command("spec.version.approve", &request)
            .expect("the draft approves");
        let replay = harness
            .core
            .command("spec.version.approve", &request)
            .expect("the retry replays");

        assert_eq!(first, replay);
        let (specs, _) = harness.specs.snapshot();
        assert_eq!(
            specs[0].approved_version().map(|held| held.number()),
            Some(1),
            "the retry must not reapply"
        );
    }

    #[test]
    fn commands_reject_unknown_fields() {
        let harness = spec_harness();
        let mut request = command(1, json!({}), 1, "key-approve");
        request["surprise"] = json!(true);

        let error = harness
            .core
            .command("spec.version.approve", &request)
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }

    #[test]
    fn an_unknown_spec_is_not_found() {
        let harness = spec_harness();

        let error = harness
            .core
            .command(
                "spec.version.approve",
                &command(9, json!({}), 1, "key-approve"),
            )
            .expect_err("the unknown Spec is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn listing_covers_every_spec_of_one_project() {
        let harness = spec_harness();
        authored(&harness.core, "Registration", "key-1");
        authored(&harness.core, "Timeline", "key-2");

        let listed = harness
            .core
            .query("spec.list", &json!({ "project_id": 1 }))
            .expect("the list serves");

        let numbers: Vec<_> = listed["specs"]
            .as_array()
            .expect("the specs are a list")
            .iter()
            .map(|spec| spec["number"].clone())
            .collect();
        assert_eq!(numbers, vec![json!(1), json!(2)]);
        assert!(
            harness
                .core
                .query("spec.list", &json!({ "project_id": 9 }))
                .expect("the list serves")["specs"]
                .as_array()
                .expect("the specs are a list")
                .is_empty(),
            "another Project's Specs stay out"
        );
    }

    #[test]
    fn no_spec_delete_operation_is_catalogued() {
        let names: Vec<_> = crate::catalog::exposed_operations()
            .iter()
            .map(|operation| operation.name)
            .collect();
        assert!(
            !names.contains(&"spec.delete") && !names.contains(&"spec.remove"),
            "Specs are superseded or archived, never deleted"
        );
    }
}

#[cfg(test)]
mod spec_planning {
    use kanban_dto::ErrorCode;
    use serde_json::json;

    use super::testing::{authored, command, joined, plan_holding, spec_harness};

    #[test]
    fn joining_a_plan_plans_the_spec() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");
        let plan = plan_holding(&harness.core, &[1], "key-plan");

        let response = harness
            .core
            .command(
                "spec.plan.join",
                &command(id, json!({ "plan_id": plan }), version, "key-join"),
            )
            .expect("the Spec joins its Plan");

        assert_eq!(response["execution"], json!("planned"));
        assert_eq!(response["plan_id"], json!(plan));
    }

    #[test]
    fn joining_a_plan_of_another_project_is_refused() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");
        // A second Project's plan: seeded directly, holding nothing.
        let other = crate::plan::testing::active_project(
            2,
            "EDGE",
            kanban_domain::ProjectCounters::restore(0, 0, 0),
        );
        harness.projects.seed(other);
        let created = harness
            .core
            .command(
                "plan.create",
                &serde_json::json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-other-plan" },
                    "project_id": 2,
                }),
            )
            .expect("the other Plan creates");

        let error = harness
            .core
            .command(
                "spec.plan.join",
                &command(
                    id,
                    json!({ "plan_id": created["id"].as_u64().expect("the identity is a number") }),
                    version,
                    "key-join",
                ),
            )
            .expect_err("a Spec joins no other Project's Plan");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "the Plan belongs to another Project");
    }

    #[test]
    fn joining_a_plan_that_does_not_hold_the_spec_is_refused() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");
        let plan = plan_holding(&harness.core, &[], "key-plan");

        let error = harness
            .core
            .command(
                "spec.plan.join",
                &command(id, json!({ "plan_id": plan }), version, "key-join"),
            )
            .expect_err("the Plan must hold the Spec first");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "`CORE-S1` is not a member of Plan 1; add the Spec to the Plan first"
        );
    }

    #[test]
    fn joining_a_terminal_plan_is_refused() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");
        let plan = plan_holding(&harness.core, &[1], "key-plan");
        harness
            .core
            .command(
                "plan.cancel",
                &serde_json::json!({
                    "mutation": { "optimistic_version": 2, "idempotency_key": "key-cancel" },
                    "plan_id": plan,
                }),
            )
            .expect("the Plan cancels");

        let error = harness
            .core
            .command(
                "spec.plan.join",
                &command(id, json!({ "plan_id": plan }), version, "key-join"),
            )
            .expect_err("a terminal Plan takes on no Spec");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "only a draft or active Plan can take on a Spec"
        );
    }

    #[test]
    fn joining_a_second_plan_is_refused() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");
        let first = plan_holding(&harness.core, &[1], "key-plan-1");
        let second = plan_holding(&harness.core, &[1], "key-plan-2");
        let version = joined(&harness.core, id, first, version);

        let error = harness
            .core
            .command(
                "spec.plan.join",
                &command(id, json!({ "plan_id": second }), version, "key-join-2"),
            )
            .expect_err("a Spec belongs to one Plan at a time");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "the Spec already belongs to Plan 1");
    }

    #[test]
    fn removing_a_joined_spec_frees_it_to_join_another_plan() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");
        let plan = plan_holding(&harness.core, &[1], "key-plan");
        joined(&harness.core, id, plan, version);

        // A joined Spec can only leave through a draft Plan, so the
        // Plan activates and replans first.
        let mut plan_version = harness
            .core
            .query("plan.get", &json!({ "plan_id": plan }))
            .expect("the Plan reads")["plan"]["version"]
            .as_u64()
            .expect("the version is a number");
        for (name, key) in [
            ("plan.activate", "key-activate"),
            ("plan.replan", "key-replan"),
        ] {
            let response = harness
                .core
                .command(
                    name,
                    &serde_json::json!({
                        "mutation": { "optimistic_version": plan_version, "idempotency_key": key },
                        "plan_id": plan,
                    }),
                )
                .expect("the Plan reopens as a draft");
            plan_version = response["version"]
                .as_u64()
                .expect("the version is a number");
        }

        let removed = harness
            .core
            .command(
                "plan.spec.remove",
                &serde_json::json!({
                    "mutation": { "optimistic_version": plan_version, "idempotency_key": "key-remove" },
                    "plan_id": plan,
                    "spec_number": 1,
                }),
            )
            .expect("the joined Spec leaves the Plan");

        assert_eq!(removed["spec_numbers"], json!([]));
        let detail = harness
            .core
            .query("spec.get", &json!({ "spec_id": id }))
            .expect("the Spec reads");
        assert_eq!(
            detail["spec"]["plan_id"],
            json!(null),
            "the removal clears the binding"
        );
        assert_eq!(
            detail["spec"]["execution"],
            json!("unplanned"),
            "the removal clears the execution state"
        );
        let (_, timeline) = harness.specs.snapshot();
        let unplanned = timeline.last().expect("the removal appended");
        assert_eq!(unplanned.detail()["action"], json!("unplanned"));
        assert_eq!(unplanned.detail()["plan_id"], json!(plan));

        // The freed Spec plans again, onto a Plan that could not have
        // taken it while the stale binding stood.
        let second = plan_holding(&harness.core, &[1], "key-plan-2");
        let freed_version = detail["spec"]["version"]
            .as_u64()
            .expect("the version is a number");
        let response = harness
            .core
            .command(
                "spec.plan.join",
                &command(
                    id,
                    json!({ "plan_id": second }),
                    freed_version,
                    "key-join-2",
                ),
            )
            .expect("the freed Spec joins another Plan");

        assert_eq!(response["plan_id"], json!(second));
        assert_eq!(response["execution"], json!("planned"));
    }

    #[test]
    fn removing_a_complete_spec_keeps_its_terminal_execution() {
        let harness = spec_harness();
        let (id, mut version) = authored(&harness.core, "Registration", "key-author");
        let plan = plan_holding(&harness.core, &[1], "key-plan");
        version = joined(&harness.core, id, plan, version);
        for to in ["ready", "active", "integration_review", "complete"] {
            version = harness
                .core
                .command(
                    "spec.execution.move",
                    &command(
                        id,
                        json!({ "execution": to }),
                        version,
                        &format!("key-{to}"),
                    ),
                )
                .expect("the execution walks to complete")["version"]
                .as_u64()
                .expect("the version is a number");
        }

        // A joined Spec can only leave through a draft Plan, so the
        // Plan activates and replans first.
        let mut plan_version = harness
            .core
            .query("plan.get", &json!({ "plan_id": plan }))
            .expect("the Plan reads")["plan"]["version"]
            .as_u64()
            .expect("the version is a number");
        for (name, key) in [
            ("plan.activate", "key-activate"),
            ("plan.replan", "key-replan"),
        ] {
            let response = harness
                .core
                .command(
                    name,
                    &serde_json::json!({
                        "mutation": { "optimistic_version": plan_version, "idempotency_key": key },
                        "plan_id": plan,
                    }),
                )
                .expect("the Plan reopens as a draft");
            plan_version = response["version"]
                .as_u64()
                .expect("the version is a number");
        }

        let removed = harness
            .core
            .command(
                "plan.spec.remove",
                &serde_json::json!({
                    "mutation": { "optimistic_version": plan_version, "idempotency_key": "key-remove" },
                    "plan_id": plan,
                    "spec_number": 1,
                }),
            )
            .expect("the complete Spec leaves the Plan");

        assert_eq!(removed["spec_numbers"], json!([]));
        let detail = harness
            .core
            .query("spec.get", &json!({ "spec_id": id }))
            .expect("the Spec reads");
        assert_eq!(
            detail["spec"]["plan_id"],
            json!(null),
            "the removal clears the binding"
        );
        assert_eq!(
            detail["spec"]["execution"],
            json!("complete"),
            "the removal preserves terminal execution"
        );
        let (_, timeline) = harness.specs.snapshot();
        let released = timeline.last().expect("the removal appended");
        assert_eq!(released.detail()["action"], json!("complete"));
        assert_eq!(released.detail()["plan_id"], json!(plan));

        // Preserved terminal execution admits no silent rescheduling:
        // the complete Spec joins no further Plan.
        let second = plan_holding(&harness.core, &[1], "key-plan-2");
        let freed_version = detail["spec"]["version"]
            .as_u64()
            .expect("the version is a number");
        let error = harness
            .core
            .command(
                "spec.plan.join",
                &command(
                    id,
                    json!({ "plan_id": second }),
                    freed_version,
                    "key-join-2",
                ),
            )
            .expect_err("complete work joins no further Plan");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "only an unplanned Spec joins a Plan");
    }

    #[test]
    fn removing_a_cancelled_spec_keeps_its_terminal_execution() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");
        let plan = plan_holding(&harness.core, &[1], "key-plan");
        let version = joined(&harness.core, id, plan, version);
        harness
            .core
            .command(
                "spec.execution.move",
                &command(
                    id,
                    json!({ "execution": "cancelled" }),
                    version,
                    "key-cancel",
                ),
            )
            .expect("planned work may cancel");

        // A joined Spec can only leave through a draft Plan, so the
        // Plan activates and replans first.
        let mut plan_version = harness
            .core
            .query("plan.get", &json!({ "plan_id": plan }))
            .expect("the Plan reads")["plan"]["version"]
            .as_u64()
            .expect("the version is a number");
        for (name, key) in [
            ("plan.activate", "key-activate"),
            ("plan.replan", "key-replan"),
        ] {
            let response = harness
                .core
                .command(
                    name,
                    &serde_json::json!({
                        "mutation": { "optimistic_version": plan_version, "idempotency_key": key },
                        "plan_id": plan,
                    }),
                )
                .expect("the Plan reopens as a draft");
            plan_version = response["version"]
                .as_u64()
                .expect("the version is a number");
        }

        let removed = harness
            .core
            .command(
                "plan.spec.remove",
                &serde_json::json!({
                    "mutation": { "optimistic_version": plan_version, "idempotency_key": "key-remove" },
                    "plan_id": plan,
                    "spec_number": 1,
                }),
            )
            .expect("the cancelled Spec leaves the Plan");

        assert_eq!(removed["spec_numbers"], json!([]));
        let detail = harness
            .core
            .query("spec.get", &json!({ "spec_id": id }))
            .expect("the Spec reads");
        assert_eq!(
            detail["spec"]["plan_id"],
            json!(null),
            "the removal clears the binding"
        );
        assert_eq!(
            detail["spec"]["execution"],
            json!("cancelled"),
            "the removal preserves terminal execution"
        );
        let (_, timeline) = harness.specs.snapshot();
        let released = timeline.last().expect("the removal appended");
        assert_eq!(released.detail()["action"], json!("cancelled"));
        assert_eq!(released.detail()["plan_id"], json!(plan));

        // Preserved terminal execution admits no silent rescheduling:
        // the cancelled Spec joins no further Plan.
        let second = plan_holding(&harness.core, &[1], "key-plan-2");
        let freed_version = detail["spec"]["version"]
            .as_u64()
            .expect("the version is a number");
        let error = harness
            .core
            .command(
                "spec.plan.join",
                &command(
                    id,
                    json!({ "plan_id": second }),
                    freed_version,
                    "key-join-2",
                ),
            )
            .expect_err("cancelled work joins no further Plan");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "only an unplanned Spec joins a Plan");
    }
}

#[cfg(test)]
mod spec_execution {
    use kanban_dto::ErrorCode;
    use serde_json::json;

    use super::testing::{
        authored, command, joined, plan_holding, spec_harness, spec_harness_with_sink,
    };

    /// An authored, planned spec, returning its identity and version.
    fn planned(harness: &super::testing::SpecHarness, key: &str) -> (u64, u64) {
        let (id, version) = authored(&harness.core, "Registration", &format!("{key}-author"));
        let plan = plan_holding(&harness.core, &[1], &format!("{key}-plan"));
        let version = joined(&harness.core, id, plan, version);
        (id, version)
    }

    /// Move one Spec's execution, returning the aggregate version.
    fn moved(
        harness: &super::testing::SpecHarness,
        id: u64,
        to: &str,
        version: u64,
        key: &str,
    ) -> u64 {
        let response = harness
            .core
            .command(
                "spec.execution.move",
                &command(id, json!({ "execution": to }), version, key),
            )
            .expect("the execution moves");
        response["version"]
            .as_u64()
            .expect("the version is a number")
    }

    #[test]
    fn execution_walks_the_full_path_to_complete() {
        let harness = spec_harness();
        let (id, mut version) = planned(&harness, "key-walk");

        for (to, expected) in [
            ("ready", "ready"),
            ("active", "active"),
            ("integration_review", "integration_review"),
            ("complete", "complete"),
        ] {
            let response = harness
                .core
                .command(
                    "spec.execution.move",
                    &command(
                        id,
                        json!({ "execution": to }),
                        version,
                        &format!("key-{to}"),
                    ),
                )
                .expect("the execution moves");
            assert_eq!(response["execution"], json!(expected));
            version = response["version"]
                .as_u64()
                .expect("the version is a number");
        }

        let error = harness
            .core
            .command(
                "spec.execution.move",
                &command(
                    id,
                    json!({ "execution": "active" }),
                    version,
                    "key-terminal",
                ),
            )
            .expect_err("terminal execution accepts no further movement");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "execution cannot move from complete to active"
        );
    }

    #[test]
    fn blocked_is_recoverable_in_both_directions() {
        let harness = spec_harness();
        let (id, version) = planned(&harness, "key-block");

        let version = moved(&harness, id, "blocked", version, "key-block");
        let version = moved(&harness, id, "ready", version, "key-ready");
        let version = moved(&harness, id, "blocked", version, "key-block-again");
        let _ = moved(&harness, id, "cancelled", version, "key-cancel");
    }

    #[test]
    fn every_open_state_may_cancel() {
        let harness = spec_harness();
        let (id, version) = planned(&harness, "key-cancel");

        let response = harness
            .core
            .command(
                "spec.execution.move",
                &command(
                    id,
                    json!({ "execution": "cancelled" }),
                    version,
                    "key-cancel",
                ),
            )
            .expect("planned work may cancel");

        assert_eq!(response["execution"], json!("cancelled"));
        assert_eq!(response["plan_id"], json!(1), "the binding stays recorded");
    }

    #[test]
    fn moving_into_planned_is_refused() {
        let harness = spec_harness();
        let (id, version) = authored(&harness.core, "Registration", "key-author");

        let error = harness
            .core
            .command(
                "spec.execution.move",
                &command(id, json!({ "execution": "planned" }), version, "key-move"),
            )
            .expect_err("joining a Plan plans a Spec, no free move does");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "execution cannot move from unplanned to planned"
        );
    }

    #[test]
    fn skipping_states_is_refused() {
        let harness = spec_harness();
        let (id, version) = planned(&harness, "key-skip");

        let error = harness
            .core
            .command(
                "spec.execution.move",
                &command(id, json!({ "execution": "active" }), version, "key-skip"),
            )
            .expect_err("planned work activates through ready");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "execution cannot move from planned to active"
        );
    }

    #[test]
    fn execution_moves_never_touch_content_versions() {
        let harness = spec_harness();
        let (id, version) = planned(&harness, "key-independent");
        let version = super::testing::approved(&harness.core, id, version, "key-approve");
        let before = harness
            .core
            .query("spec.get", &json!({ "spec_id": id }))
            .expect("the Spec reads");

        let version = moved(&harness, id, "ready", version, "key-ready");
        let _ = moved(&harness, id, "active", version, "key-active");

        let after = harness
            .core
            .query("spec.get", &json!({ "spec_id": id }))
            .expect("the Spec reads");
        assert_eq!(
            after["versions"], before["versions"],
            "progress never rewrites documents"
        );
        assert_eq!(after["spec"]["execution"], json!("active"));
    }

    #[test]
    fn content_moves_never_touch_execution() {
        let harness = spec_harness();
        let (id, version) = planned(&harness, "key-independent-2");
        let version = super::testing::approved(&harness.core, id, version, "key-approve");
        let version = harness
            .core
            .command(
                "spec.version.supersede",
                &command(id, json!({ "version": 1 }), version, "key-supersede"),
            )
            .expect("the approved version supersedes")["version"]
            .as_u64()
            .expect("the version is a number");

        let detail = harness
            .core
            .query("spec.get", &json!({ "spec_id": id }))
            .expect("the Spec reads");
        assert_eq!(
            detail["spec"]["execution"],
            json!("planned"),
            "documents moving never moves progress"
        );

        // And the tracks stay crossable in both directions: execution
        // still moves after every content move.
        let response = harness
            .core
            .command(
                "spec.execution.move",
                &command(id, json!({ "execution": "ready" }), version, "key-ready"),
            )
            .expect("the execution still moves");
        assert_eq!(response["execution"], json!("ready"));
    }

    #[test]
    fn execution_moves_append_auditable_timeline_rows() {
        let harness = spec_harness();
        let (id, version) = planned(&harness, "key-timeline");

        harness
            .core
            .command(
                "spec.execution.move",
                &command(id, json!({ "execution": "blocked" }), version, "key-block"),
            )
            .expect("the execution moves");

        let (_, timeline) = harness.specs.snapshot();
        let blocked = timeline.last().expect("the move appended");
        assert_eq!(blocked.kind(), kanban_dto::TimelineEventKind::Transition);
        assert_eq!(
            blocked
                .entity()
                .map(|entity| (entity.kind, entity.id.clone())),
            Some((kanban_dto::TimelineEntityKind::Spec, "1".to_owned()))
        );
        assert_eq!(blocked.detail()["action"], json!("execution_moved"));
        assert_eq!(blocked.detail()["from"], json!("planned"));
        assert_eq!(blocked.detail()["to"], json!("blocked"));
    }

    #[test]
    fn execution_moves_publish_on_the_event_stream() {
        let sink = std::sync::Arc::new(crate::plan::testing::RecordingSink::default());
        let harness = spec_harness_with_sink(sink.clone());
        let (id, version) = planned(&harness, "key-events");

        harness
            .core
            .command(
                "spec.execution.move",
                &command(id, json!({ "execution": "ready" }), version, "key-ready"),
            )
            .expect("the execution moves");

        let events = sink.events.lock().expect("the recorder lock is sound");
        let names: Vec<_> = events.iter().map(|(name, _)| name.as_str()).collect();
        for expected in [
            "spec.created",
            "plan.created",
            "spec.planned",
            "spec.execution.moved",
        ] {
            assert!(
                names.contains(&expected),
                "`{expected}` should announce live"
            );
        }
        let moved = events
            .iter()
            .find(|(name, _)| name == "spec.execution.moved")
            .expect("the execution event is present");
        assert_eq!(moved.1["execution"], json!("ready"));
    }
}
