//! Ticket commands and queries: create a Ticket under its kind's
//! schema — an Implementation attached to exactly one Spec with its
//! slice and story-linked criteria, a Bug or Task with a title and an
//! optional attachment — and read Tickets back per Project
//! (KAN-S4-US1, KAN-S4-US2). Creation mints the Project's next
//! Ticket number, lands the row with the counter move and the
//! timeline append in one write, and announces live; no delete
//! exists. Lifecycle transitions, dependencies, and qualification
//! arrive with their own tickets.

use std::sync::Arc;

use kanban_domain::{
    AcceptanceCriterion, NumberKind, Priority as DomainPriority, Project, ProjectCode, ProjectId,
    SpecId, Ticket, TicketBody, TicketId, TicketKind as DomainKind, TicketNumber,
    TicketState as DomainState, UserStoryRef,
};
use kanban_dto::{
    ApiError, LiveEventName, TicketCreateRequest, TicketCriterion, TicketGetQuery, TicketKind,
    TicketListQuery, TicketListResponse, TicketPriority, TicketRecord, TicketState,
    TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::events::{EventSink, emit_catalogued};
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;
use crate::spec::SpecStore;
use crate::timeline::TimelineEnvelope;

/// The storage port Ticket commands call through. Implementations land
/// the row changes and the timeline envelope unchanged inside one
/// write, so a Ticket, and the Project counter its number minted,
/// never split across a crash boundary.
pub trait TicketStore: Send + Sync {
    /// Insert a fresh Ticket. `project` carries the minted Ticket
    /// number and the counter move that minted it; both land in the
    /// same write as the Ticket row. Storage assigns the Ticket's
    /// identity and asks `envelope` for the timeline row that identity
    /// belongs in.
    fn create(
        &self,
        project: &Project,
        number: TicketNumber,
        priority: DomainPriority,
        body: &TicketBody,
        envelope: &dyn Fn(TicketId) -> TimelineEnvelope,
    ) -> Result<Ticket, ApiError>;
    /// Load one Ticket, if it exists.
    fn find(&self, id: TicketId) -> Result<Option<Ticket>, ApiError>;
    /// Every Ticket of one Project in id order, terminal lifecycle
    /// states included.
    fn list(&self, project: ProjectId) -> Result<Vec<Ticket>, ApiError>;
}

/// The timeline row for one Ticket change: on the Project's own
/// timeline, about the Ticket, with `action` naming the change inside
/// the closed `transition` kind.
fn transition(
    project: ProjectId,
    ticket: TicketId,
    action: &str,
    facts: Value,
) -> TimelineEnvelope {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("Ticket transition facts are a JSON object");
    object.insert("action".to_owned(), Value::from(action));
    object.insert("id".to_owned(), Value::from(ticket.value()));
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

/// Report a refused domain rule as the stable invalid-request code.
fn refuse(error: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

/// The stores every Ticket command reads and writes through.
#[derive(Clone)]
struct TicketContext {
    tickets: Arc<dyn TicketStore>,
    projects: Arc<dyn ProjectStore>,
    specs: Arc<dyn SpecStore>,
}

impl Core {
    /// Register the Ticket operations against `tickets`, resolving
    /// Projects through `projects` and Spec attachments through
    /// `specs`.
    pub fn register_tickets(
        &mut self,
        tickets: Arc<dyn TicketStore>,
        projects: Arc<dyn ProjectStore>,
        specs: Arc<dyn SpecStore>,
    ) -> Result<(), RegistrationError> {
        let context = TicketContext {
            tickets,
            projects,
            specs,
        };
        self.register_command("ticket.create", Arc::new(CreateTicket(context.clone())))?;
        self.register_query(
            "ticket.list",
            Arc::new(ListTickets {
                tickets: context.tickets.clone(),
                projects: context.projects.clone(),
            }),
        )?;
        self.register_query(
            "ticket.get",
            Arc::new(GetTicket {
                tickets: context.tickets.clone(),
                projects: context.projects.clone(),
            }),
        )?;
        Ok(())
    }
}

/// Serves `ticket.create`.
struct CreateTicket(TicketContext);

impl CommandHandler for CreateTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketCreateRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
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
        let request: TicketCreateRequest = parse_payload(&command.payload)?;
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
        let priority = priority_of(request.priority);
        let body = body_of(&request, &project, self.0.specs.as_ref())?;
        let number = TicketNumber::new(project.mint(NumberKind::Ticket))
            .expect("a minted number is positive");
        let identity = project.id();
        let kind = body.kind().wire_name().to_owned();
        let ticket = self
            .0
            .tickets
            .create(&project, number, priority, &body, &|id| {
                transition(
                    identity,
                    id,
                    "created",
                    json!({
                        "project_id": identity.value(),
                        "number": number.value(),
                        "kind": kind,
                    }),
                )
            })?;
        announce(
            events,
            LiveEventName::TicketCreated,
            &ticket,
            project.code(),
        );
        encode_record(&ticket, project.code())
    }
}

/// Serves `ticket.list`.
struct ListTickets {
    tickets: Arc<dyn TicketStore>,
    projects: Arc<dyn ProjectStore>,
}

impl QueryHandler for ListTickets {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: TicketListQuery = parse_payload(payload)?;
        // An unknown Project holds no Tickets, exactly as the Spec
        // list reports it: empty, not a refusal.
        let Some(project) = self.projects.find(ProjectId::new(query.project_id))? else {
            let response = TicketListResponse {
                tickets: Vec::new(),
            };
            return serde_json::to_value(response)
                .map_err(|error| ApiError::internal(&error.to_string()));
        };
        let response = TicketListResponse {
            tickets: self
                .tickets
                .list(project.id())?
                .iter()
                .map(|ticket| record_of(ticket, project.code()))
                .collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `ticket.get`.
struct GetTicket {
    tickets: Arc<dyn TicketStore>,
    projects: Arc<dyn ProjectStore>,
}

impl QueryHandler for GetTicket {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: TicketGetQuery = parse_payload(payload)?;
        let ticket = self
            .tickets
            .find(TicketId::new(query.ticket_id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {}", query.ticket_id)))?;
        let project = self.projects.find(ticket.project())?.ok_or_else(|| {
            ApiError::internal(&format!(
                "ticket {} belongs to no stored Project",
                query.ticket_id
            ))
        })?;
        serde_json::to_value(record_of(&ticket, project.code()))
            .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Decode one request's kind-specific fields into the domain's
/// validated body, resolving the Spec attachment through `specs`.
fn body_of(
    request: &TicketCreateRequest,
    project: &Project,
    specs: &dyn SpecStore,
) -> Result<TicketBody, ApiError> {
    let attached = |spec_id: u64| -> Result<kanban_domain::Spec, ApiError> {
        let spec = specs
            .find(SpecId::new(spec_id))?
            .ok_or_else(|| ApiError::not_found(&format!("spec {spec_id}")))?;
        if spec.project() != project.id() {
            return Err(ApiError::invalid_request(
                "the Spec belongs to another Project",
            ));
        }
        Ok(spec)
    };
    match request.kind {
        TicketKind::Implementation => {
            let spec_id = request
                .spec_id
                .ok_or_else(|| refuse(kanban_domain::TicketError::UnattachedSpec))?;
            let spec = attached(spec_id)?;
            let criteria = criteria_of(request.criteria.as_deref().unwrap_or_default(), project)?;
            TicketBody::implementation(
                Some(spec.id()),
                spec.number(),
                request.slice.clone().unwrap_or_default(),
                criteria,
            )
            .map_err(refuse)
        }
        TicketKind::Bug => {
            let spec = request.spec_id.map(attached).transpose()?;
            TicketBody::bug(
                request.title.clone().unwrap_or_default(),
                spec.map(|spec| spec.id()),
            )
            .map_err(refuse)
        }
        TicketKind::Task => {
            let spec = request.spec_id.map(attached).transpose()?;
            TicketBody::task(
                request.title.clone().unwrap_or_default(),
                spec.map(|spec| spec.id()),
            )
            .map_err(refuse)
        }
    }
}

/// Decode wire criteria into the domain's rule-valid criteria,
/// parsing every story link against the Project's code.
fn criteria_of(
    proposed: &[TicketCriterion],
    project: &Project,
) -> Result<Vec<AcceptanceCriterion>, ApiError> {
    let mut criteria = Vec::with_capacity(proposed.len());
    for criterion in proposed {
        let mut stories = Vec::with_capacity(criterion.stories.len());
        for named in &criterion.stories {
            stories.push(UserStoryRef::parse(named, project.code()).map_err(refuse)?);
        }
        criteria
            .push(AcceptanceCriterion::new(criterion.outcome.clone(), stories).map_err(refuse)?);
    }
    Ok(criteria)
}

/// The domain form of one wire priority.
fn priority_of(priority: TicketPriority) -> DomainPriority {
    match priority {
        TicketPriority::Urgent => DomainPriority::Urgent,
        TicketPriority::High => DomainPriority::High,
        TicketPriority::Normal => DomainPriority::Normal,
        TicketPriority::Low => DomainPriority::Low,
    }
}

/// The wire form of one domain priority.
fn priority_named(priority: DomainPriority) -> TicketPriority {
    match priority {
        DomainPriority::Urgent => TicketPriority::Urgent,
        DomainPriority::High => TicketPriority::High,
        DomainPriority::Normal => TicketPriority::Normal,
        DomainPriority::Low => TicketPriority::Low,
    }
}

/// The wire form of one domain kind.
fn kind_of(kind: DomainKind) -> TicketKind {
    match kind {
        DomainKind::Implementation => TicketKind::Implementation,
        DomainKind::Bug => TicketKind::Bug,
        DomainKind::Task => TicketKind::Task,
    }
}

/// The wire form of one domain state.
fn state_of(state: DomainState) -> TicketState {
    match state {
        DomainState::Draft => TicketState::Draft,
        DomainState::Parked => TicketState::Parked,
        DomainState::Blocked => TicketState::Blocked,
        DomainState::Scheduled => TicketState::Scheduled,
        DomainState::Ready => TicketState::Ready,
        DomainState::Active => TicketState::Active,
        DomainState::InReview => TicketState::InReview,
        DomainState::Approved => TicketState::Approved,
        DomainState::Landing => TicketState::Landing,
        DomainState::Done => TicketState::Done,
        DomainState::Cancelled => TicketState::Cancelled,
        DomainState::Superseded => TicketState::Superseded,
    }
}

/// The DTO record for one Ticket. Story links render with the
/// Project's code, the full `CORE-S3-US6` form.
fn record_of(ticket: &Ticket, code: &ProjectCode) -> TicketRecord {
    TicketRecord {
        id: ticket.id().value(),
        project_id: ticket.project().value(),
        number: ticket.number().value(),
        kind: kind_of(ticket.kind()),
        priority: priority_named(ticket.priority()),
        state: state_of(ticket.state()),
        spec_id: ticket.spec().map(|spec| spec.value()),
        title: ticket.title().map(str::to_owned),
        slice: ticket.slice().map(str::to_owned),
        criteria: ticket
            .criteria()
            .iter()
            .map(|criterion| TicketCriterion {
                outcome: criterion.outcome().to_owned(),
                stories: criterion
                    .stories()
                    .iter()
                    .map(|story| story.render(code))
                    .collect(),
            })
            .collect(),
        version: ticket.version(),
    }
}

/// Encode a record for a command response.
fn encode_record(ticket: &Ticket, code: &ProjectCode) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(ticket, code))
        .map_err(|error| ApiError::internal(&error.to_string()))
}

/// Publish creation on the live event stream as exactly the record
/// the command returns.
fn announce(events: &dyn EventSink, name: LiveEventName, ticket: &Ticket, code: &ProjectCode) {
    emit_catalogued(events, name, &record_of(ticket, code));
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex};

    use kanban_domain::{ProjectId, Ticket, TicketBody, TicketId, TicketNumber};
    use kanban_dto::ApiError;

    use super::TicketStore;
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::EventSink;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::plan::testing::{MemoryPlans, MemoryProjects};
    use crate::spec::testing::MemorySpecs;
    use crate::timeline::TimelineEnvelope;

    /// An in-memory Ticket store: rows by id, the timeline envelopes
    /// it was asked to land, and the Project rows its writes moved.
    #[derive(Default)]
    pub(crate) struct MemoryTickets {
        state: Mutex<MemoryTicketState>,
        projects: Arc<MemoryProjects>,
    }

    #[derive(Default)]
    struct MemoryTicketState {
        tickets: Vec<Ticket>,
        next_id: u64,
        timeline: Vec<TimelineEnvelope>,
    }

    impl MemoryTickets {
        /// A ticket store sharing the Project rows the harness seeded.
        pub(crate) fn sharing(projects: Arc<MemoryProjects>) -> Self {
            Self {
                projects,
                ..Self::default()
            }
        }

        /// The stored rows and timeline envelopes, for assertions.
        pub(crate) fn snapshot(&self) -> (Vec<Ticket>, Vec<TimelineEnvelope>) {
            let state = self.state.lock().expect("the memory ticket lock is sound");
            (state.tickets.clone(), state.timeline.clone())
        }
    }

    impl TicketStore for MemoryTickets {
        fn create(
            &self,
            project: &kanban_domain::Project,
            number: TicketNumber,
            priority: kanban_domain::Priority,
            body: &TicketBody,
            envelope: &dyn Fn(TicketId) -> TimelineEnvelope,
        ) -> Result<Ticket, ApiError> {
            let mut state = self.state.lock().expect("the memory ticket lock is sound");
            // The minted counter lands on the Project row in the same
            // write as the Ticket row.
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
            let id = TicketId::new(state.next_id);
            let ticket = Ticket::new(id, project.id(), number, priority, body.clone());
            state.tickets.push(ticket.clone());
            state.timeline.push(envelope(id));
            Ok(ticket)
        }

        fn find(&self, id: TicketId) -> Result<Option<Ticket>, ApiError> {
            let state = self.state.lock().expect("the memory ticket lock is sound");
            Ok(state.tickets.iter().find(|row| row.id() == id).cloned())
        }

        fn list(&self, project: ProjectId) -> Result<Vec<Ticket>, ApiError> {
            let state = self.state.lock().expect("the memory ticket lock is sound");
            Ok(state
                .tickets
                .iter()
                .filter(|row| row.project() == project)
                .cloned()
                .collect())
        }
    }

    /// A core with the Plan, Spec, and Ticket operations wired to
    /// in-memory stores over one active Project.
    pub(crate) struct TicketHarness {
        pub(crate) tickets: Arc<MemoryTickets>,
        pub(crate) projects: Arc<MemoryProjects>,
        pub(crate) core: Core,
    }

    /// A harness whose event sink the test chooses.
    pub(crate) fn ticket_harness_with_sink(events: Arc<dyn EventSink>) -> TicketHarness {
        let projects = Arc::new(MemoryProjects::default());
        projects.seed(crate::plan::testing::active_project(
            1,
            "CORE",
            kanban_domain::ProjectCounters::restore(0, 0, 0),
        ));
        let plans = Arc::new(MemoryPlans::sharing(projects.clone()));
        let specs = Arc::new(MemorySpecs::sharing(projects.clone()));
        let tickets = Arc::new(MemoryTickets::sharing(projects.clone()));
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            events,
        );
        core.register_plans(plans.clone(), projects.clone(), specs.clone())
            .expect("the plan operations register");
        core.register_specs(specs.clone(), projects.clone(), plans.clone())
            .expect("the spec operations register");
        core.register_tickets(tickets.clone(), projects.clone(), specs.clone())
            .expect("the ticket operations register");
        TicketHarness {
            tickets,
            projects,
            core,
        }
    }

    /// A harness with a silent event sink.
    pub(crate) fn ticket_harness() -> TicketHarness {
        ticket_harness_with_sink(Arc::new(crate::events::NoopEventSink))
    }

    /// Author one Spec on the seeded Project, returning its identity.
    pub(crate) fn authored_spec(core: &Core, key: &str) -> u64 {
        let created = core
            .command(
                "spec.create",
                &crate::spec::testing::create(1, "Registration", key),
            )
            .expect("the Spec authors");
        created["id"].as_u64().expect("the identity is a number")
    }

    /// The wire PRD content the fixtures author, varied by name.
    pub(crate) fn wire_spec_content(name: &str) -> serde_json::Value {
        crate::spec::testing::wire_content(name)
    }
}

#[cfg(test)]
mod ticket_create {
    use kanban_dto::ErrorCode;
    use serde_json::{Value, json};

    use super::testing::{ticket_harness, ticket_harness_with_sink};

    /// An Implementation creation request against the seeded Project,
    /// with the fields a test varies.
    #[allow(clippy::too_many_arguments)]
    fn implementation(
        spec_id: Option<u64>,
        slice: Option<&str>,
        criteria: Value,
        priority: &str,
        key: &str,
    ) -> Value {
        let mut request = json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": 1,
            "kind": "implementation",
            "priority": priority,
            "criteria": criteria,
        });
        let object = request.as_object_mut().expect("the request is an object");
        if let Some(spec_id) = spec_id {
            object.insert("spec_id".to_owned(), json!(spec_id));
        }
        if let Some(slice) = slice {
            object.insert("slice".to_owned(), json!(slice));
        }
        request
    }

    /// A Bug or Task creation request with the fields a test varies.
    fn titled(
        kind: &str,
        title: Option<&str>,
        spec_id: Option<u64>,
        priority: &str,
        key: &str,
    ) -> Value {
        let mut request = json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": 1,
            "kind": kind,
            "priority": priority,
        });
        let object = request.as_object_mut().expect("the request is an object");
        if let Some(title) = title {
            object.insert("title".to_owned(), json!(title));
        }
        if let Some(spec_id) = spec_id {
            object.insert("spec_id".to_owned(), json!(spec_id));
        }
        request
    }

    fn one_criterion() -> Value {
        json!([
            { "outcome": "Specs mint unique numbers.", "stories": ["CORE-S1-US1"] }
        ])
    }

    #[test]
    fn creating_an_implementation_ticket_returns_the_record_and_mints_the_number() {
        let harness = ticket_harness();
        let spec = super::testing::authored_spec(&harness.core, "key-author");

        let response = harness
            .core
            .command(
                "ticket.create",
                &implementation(
                    Some(spec),
                    Some("Spec authoring creates content versions end to end"),
                    one_criterion(),
                    "high",
                    "key-1",
                ),
            )
            .expect("the Ticket creates");

        assert_eq!(
            response,
            json!({
                "id": 1,
                "project_id": 1,
                "number": 1,
                "kind": "implementation",
                "priority": "high",
                "state": "draft",
                "spec_id": spec,
                "title": null,
                "slice": "Spec authoring creates content versions end to end",
                "criteria": [
                    { "outcome": "Specs mint unique numbers.", "stories": ["CORE-S1-US1"] }
                ],
                "version": 1,
            })
        );
        assert_eq!(
            harness.projects.rows()[0]
                .counters()
                .last(kanban_domain::NumberKind::Ticket),
            1,
            "creating consumes the Project's first Ticket number"
        );
    }

    #[test]
    fn creating_a_bug_or_task_attaches_to_zero_or_one_spec() {
        let harness = ticket_harness();
        let spec = super::testing::authored_spec(&harness.core, "key-author");

        let standing = harness
            .core
            .command(
                "ticket.create",
                &titled(
                    "bug",
                    Some("Landing drops the integration branch"),
                    None,
                    "urgent",
                    "key-bug",
                ),
            )
            .expect("a Bug may stand alone");
        assert_eq!(standing["kind"], json!("bug"));
        assert_eq!(
            standing["title"],
            json!("Landing drops the integration branch")
        );
        assert_eq!(standing["spec_id"], json!(null));
        assert_eq!(standing["criteria"], json!([]));

        let attached = harness
            .core
            .command(
                "ticket.create",
                &titled(
                    "task",
                    Some("Archive the old register"),
                    Some(spec),
                    "low",
                    "key-task",
                ),
            )
            .expect("a Task may attach to one Spec");
        assert_eq!(attached["kind"], json!("task"));
        assert_eq!(attached["spec_id"], json!(spec));
        assert_eq!(attached["number"], json!(2), "the counter moves per Ticket");
    }

    #[test]
    fn creating_without_the_kinds_own_fields_is_refused() {
        let harness = ticket_harness();
        let spec = super::testing::authored_spec(&harness.core, "key-author");

        let unattached = harness
            .core
            .command(
                "ticket.create",
                &implementation(None, Some("A slice"), one_criterion(), "normal", "key-1"),
            )
            .expect_err("an Implementation attaches to exactly one Spec");
        assert_eq!(unattached.code, ErrorCode::InvalidRequest);
        assert_eq!(
            unattached.message,
            "an Implementation Ticket attaches to exactly one Spec"
        );

        let untitled = harness
            .core
            .command(
                "ticket.create",
                &titled("bug", Some("  "), None, "normal", "key-2"),
            )
            .expect_err("a Bug carries a title");
        assert_eq!(untitled.code, ErrorCode::InvalidRequest);
        assert_eq!(untitled.message, "a Ticket title cannot be blank");

        let unsliced = harness
            .core
            .command(
                "ticket.create",
                &implementation(Some(spec), Some("   "), one_criterion(), "normal", "key-3"),
            )
            .expect_err("an Implementation carries a slice description");
        assert_eq!(unsliced.code, ErrorCode::InvalidRequest);
        assert_eq!(
            unsliced.message,
            "a Ticket slice description cannot be blank"
        );

        let unclaimed = harness
            .core
            .command(
                "ticket.create",
                &implementation(Some(spec), Some("A slice"), json!([]), "normal", "key-4"),
            )
            .expect_err("an Implementation carries story-linked criteria");
        assert_eq!(unclaimed.code, ErrorCode::InvalidRequest);
        assert_eq!(
            unclaimed.message,
            "an Implementation Ticket carries story-linked criteria"
        );

        assert_eq!(
            harness.projects.rows()[0]
                .counters()
                .last(kanban_domain::NumberKind::Ticket),
            0,
            "a refused creation consumes no number"
        );
    }

    #[test]
    fn criteria_claim_the_stories_of_the_spec_the_slice_delivers() {
        let harness = ticket_harness();
        let spec = super::testing::authored_spec(&harness.core, "key-author");

        let foreign = harness
            .core
            .command(
                "ticket.create",
                &implementation(
                    Some(spec),
                    Some("A slice"),
                    json!([
                        { "outcome": "Well linked, just not here.", "stories": ["CORE-S9-US1"] }
                    ]),
                    "normal",
                    "key-1",
                ),
            )
            .expect_err("another Spec's story claims nothing here");
        assert_eq!(foreign.code, ErrorCode::InvalidRequest);
        assert_eq!(
            foreign.message,
            "an Implementation Ticket claims the stories of the Spec it delivers; \
             S9-US1 names another Spec"
        );

        let malformed = harness
            .core
            .command(
                "ticket.create",
                &implementation(
                    Some(spec),
                    Some("A slice"),
                    json!([{ "outcome": "Any outcome.", "stories": ["banana"] }]),
                    "normal",
                    "key-2",
                ),
            )
            .expect_err("a story link names a User Story");
        assert_eq!(malformed.code, ErrorCode::InvalidRequest);
        assert_eq!(
            malformed.message,
            "a User Story is named like `CORE-S3-US6` or `S3-US6`"
        );
    }

    #[test]
    fn creating_for_an_unknown_project_or_spec_is_refused() {
        let harness = ticket_harness();

        let mut elsewhere = titled("bug", Some("A Bug"), None, "normal", "key-1");
        elsewhere["project_id"] = json!(9);
        let error = harness
            .core
            .command("ticket.create", &elsewhere)
            .expect_err("the unknown Project is refused");
        assert_eq!(error.code, ErrorCode::NotFound);

        let spec = super::testing::authored_spec(&harness.core, "key-author");
        let error = harness
            .core
            .command(
                "ticket.create",
                &implementation(
                    Some(spec + 9),
                    Some("A slice"),
                    one_criterion(),
                    "normal",
                    "key-3",
                ),
            )
            .expect_err("the unknown Spec is refused");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn creating_against_another_projects_spec_is_refused() {
        let harness = ticket_harness();
        harness.projects.seed(crate::plan::testing::active_project(
            2,
            "EDGE",
            kanban_domain::ProjectCounters::restore(0, 0, 0),
        ));
        let created = harness
            .core
            .command(
                "spec.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-other-spec" },
                    "project_id": 2,
                    "content": super::testing::wire_spec_content("Elsewhere"),
                }),
            )
            .expect("the other Project's Spec authors");
        let other_spec = created["id"].as_u64().expect("the identity is a number");

        let error = harness
            .core
            .command(
                "ticket.create",
                &titled("bug", Some("A Bug"), Some(other_spec), "normal", "key-1"),
            )
            .expect_err("a Ticket attaches to no other Project's Spec");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "the Spec belongs to another Project");
    }

    #[test]
    fn creating_for_an_archived_project_is_refused() {
        let harness = ticket_harness();
        let mut project = harness.projects.rows()[0].clone();
        project.archive().expect("the fixture archives");
        harness.projects.replace(project);

        let error = harness
            .core
            .command(
                "ticket.create",
                &titled("bug", Some("A Bug"), None, "normal", "key-1"),
            )
            .expect_err("an archived Project accepts no further changes");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("archived"));
    }

    #[test]
    fn creating_consumes_the_counter_without_reuse() {
        let harness = ticket_harness();
        let spec = super::testing::authored_spec(&harness.core, "key-author");

        let first = harness
            .core
            .command(
                "ticket.create",
                &implementation(Some(spec), Some("One"), one_criterion(), "normal", "key-1"),
            )
            .expect("the first Ticket creates");
        // A refused creation mints nothing, so the next number is
        // neither reused nor skipped past.
        let _ = harness
            .core
            .command(
                "ticket.create",
                &titled("bug", Some("  "), None, "normal", "key-refused"),
            )
            .expect_err("the blank title is refused");
        let second = harness
            .core
            .command(
                "ticket.create",
                &titled("task", Some("Two"), None, "normal", "key-2"),
            )
            .expect("the second Ticket creates");
        // Minting a Spec number must not move the Ticket counter.
        super::testing::authored_spec(&harness.core, "key-author-2");
        let third = harness
            .core
            .command(
                "ticket.create",
                &titled("bug", Some("Three"), None, "normal", "key-3"),
            )
            .expect("the third Ticket creates");

        assert_eq!(
            (
                first["number"].as_u64(),
                second["number"].as_u64(),
                third["number"].as_u64()
            ),
            (Some(1), Some(2), Some(3)),
            "numbers are monotonic and never reused"
        );
        assert_eq!(
            harness.projects.rows()[0].counters(),
            kanban_domain::ProjectCounters::restore(0, 2, 3),
            "each counter moves alone"
        );
    }

    #[test]
    fn creation_appends_a_timeline_row_on_the_projects_own_timeline() {
        let harness = ticket_harness();

        harness
            .core
            .command(
                "ticket.create",
                &titled(
                    "bug",
                    Some("Landing drops the integration branch"),
                    None,
                    "urgent",
                    "key-1",
                ),
            )
            .expect("the Ticket creates");

        let (tickets, timeline) = harness.tickets.snapshot();
        assert_eq!(tickets.len(), 1);
        let created = timeline.last().expect("the creation appended");
        assert_eq!(created.kind(), kanban_dto::TimelineEventKind::Transition);
        assert_eq!(
            created
                .entity()
                .map(|entity| (entity.kind, entity.id.clone())),
            Some((kanban_dto::TimelineEntityKind::Ticket, "1".to_owned()))
        );
        assert_eq!(
            created.detail(),
            &json!({
                "action": "created",
                "id": 1,
                "project_id": 1,
                "number": 1,
                "kind": "bug",
            })
        );
    }

    #[test]
    fn creation_publishes_on_the_event_stream() {
        let sink = std::sync::Arc::new(crate::plan::testing::RecordingSink::default());
        let harness = ticket_harness_with_sink(sink.clone());
        let spec = super::testing::authored_spec(&harness.core, "key-author");

        harness
            .core
            .command(
                "ticket.create",
                &implementation(
                    Some(spec),
                    Some("A slice"),
                    one_criterion(),
                    "high",
                    "key-1",
                ),
            )
            .expect("the Ticket creates");

        let events = sink.events.lock().expect("the recorder lock is sound");
        let created = events
            .iter()
            .find(|(name, _)| name == "ticket.created")
            .expect("creation announces live");
        assert_eq!(created.1["kind"], json!("implementation"));
        assert_eq!(created.1["number"], json!(1));
    }

    #[test]
    fn a_retry_replays_without_reapplying() {
        let harness = ticket_harness();
        let request = titled(
            "bug",
            Some("Landing drops the integration branch"),
            None,
            "normal",
            "key-1",
        );

        let first = harness
            .core
            .command("ticket.create", &request)
            .expect("the Ticket creates");
        let replay = harness
            .core
            .command("ticket.create", &request)
            .expect("the retry replays");

        assert_eq!(first, replay);
        let (tickets, _) = harness.tickets.snapshot();
        assert_eq!(tickets.len(), 1, "the retry must not reapply");
    }

    #[test]
    fn commands_reject_unknown_fields() {
        let harness = ticket_harness();
        let mut request = titled("bug", Some("A Bug"), None, "normal", "key-1");
        request["surprise"] = json!(true);

        let error = harness
            .core
            .command("ticket.create", &request)
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }

    #[test]
    fn no_ticket_delete_operation_is_catalogued() {
        let names: Vec<_> = crate::catalog::exposed_operations()
            .iter()
            .map(|operation| operation.name)
            .collect();
        assert!(
            !names.contains(&"ticket.delete") && !names.contains(&"ticket.remove"),
            "Tickets are superseded or cancelled, never deleted"
        );
    }
}

#[cfg(test)]
mod ticket_queries {
    use serde_json::json;

    use super::testing::ticket_harness;

    /// Three Tickets of the seeded Project: an Implementation, a Bug,
    /// and a Task, numbers one through three.
    fn three_tickets(harness: &super::testing::TicketHarness) {
        let spec = super::testing::authored_spec(&harness.core, "key-author");
        for (kind, key) in [
            ("implementation", "key-1"),
            ("bug", "key-2"),
            ("task", "key-3"),
        ] {
            let mut request = json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": key },
                "project_id": 1,
                "kind": kind,
                "priority": "normal",
            });
            if kind == "implementation" {
                request["spec_id"] = json!(spec);
                request["slice"] = json!("Spec authoring creates content versions end to end");
                request["criteria"] = json!([
                    { "outcome": "Specs mint unique numbers.", "stories": ["CORE-S1-US1"] }
                ]);
            } else {
                request["title"] = json!("Landing drops the integration branch");
            }
            harness
                .core
                .command("ticket.create", &request)
                .expect("the Ticket creates");
        }
    }

    #[test]
    fn listing_covers_every_ticket_of_one_project() {
        let harness = ticket_harness();
        three_tickets(&harness);

        let listed = harness
            .core
            .query("ticket.list", &json!({ "project_id": 1 }))
            .expect("the list serves");

        let numbers: Vec<_> = listed["tickets"]
            .as_array()
            .expect("the tickets are a list")
            .iter()
            .map(|ticket| ticket["number"].clone())
            .collect();
        assert_eq!(numbers, vec![json!(1), json!(2), json!(3)]);
        assert!(
            harness
                .core
                .query("ticket.list", &json!({ "project_id": 9 }))
                .expect("the list serves")["tickets"]
                .as_array()
                .expect("the tickets are a list")
                .is_empty(),
            "another Project's Tickets stay out"
        );
    }

    #[test]
    fn reading_one_ticket_returns_its_record() {
        let harness = ticket_harness();
        three_tickets(&harness);

        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": 1 }))
            .expect("the get serves");

        assert_eq!(read["id"], json!(1));
        assert_eq!(read["kind"], json!("implementation"));
        assert_eq!(
            read["slice"],
            json!("Spec authoring creates content versions end to end")
        );

        let error = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": 9 }))
            .expect_err("an unknown Ticket is refused");
        assert_eq!(error.code, kanban_dto::ErrorCode::NotFound);
    }
}
