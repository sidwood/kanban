//! Ticket dependency commands and queries: register dependencies
//! between Tickets — across Specs and across registered Projects,
//! with cycles refused — record the explicit external blockers that
//! carry unregistered waiting work, and compute readiness as a
//! projection of exactly those facts (KAN-S4-US5, DR-DE-02,
//! DR-DE-03, DR-DE-04). Readiness never mutates state; dispatch
//! (KAN-T42) consumes it. Every change appends a timeline row on the
//! waiting Ticket's Project timeline inside the same write as the
//! row change, and guards on the waiting Ticket's aggregate version.

use std::sync::Arc;

use kanban_domain::{
    BlockerDescription, DependencyState, ExternalBlocker, ExternalBlockerId, Project, ProjectId,
    Readiness, ReadinessBlocker, ReadinessInputs, Ticket, TicketDependency, TicketDependencyGraph,
    TicketId, TicketState as DomainState, compute_readiness,
};
use kanban_dto::{
    ApiError, TicketBlockerAddRequest, TicketBlockerRecord, TicketBlockerRemoveRequest,
    TicketDependenciesQuery, TicketDependenciesResponse, TicketDependencyAddRequest,
    TicketDependencyRecord, TicketDependencyRemoveRequest, TicketReadinessBlocker,
    TicketReadinessQuery, TicketReadinessResponse, TicketState, TimelineEntityKind,
    TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::mutation::{CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;
use crate::ticket::TicketStore;
use crate::timeline::TimelineEnvelope;

/// The storage port the dependency operations call through.
/// Implementations land the edge or blocker row, the waiting
/// Ticket's version bump, and the timeline envelope unchanged inside
/// one write, so a dependency change never splits across a crash
/// boundary.
pub trait DependencyStore: Send + Sync {
    /// Insert one dependency edge, moving the waiting Ticket's
    /// aggregate version forward by one under its optimistic guard.
    /// Returns the Ticket as it now stands.
    fn add_dependency(
        &self,
        waiting: &Ticket,
        edge: TicketDependency,
        envelope: &dyn Fn() -> TimelineEnvelope,
    ) -> Result<Ticket, ApiError>;
    /// Remove one dependency edge under the same guard.
    fn remove_dependency(
        &self,
        waiting: &Ticket,
        edge: TicketDependency,
        envelope: &dyn Fn() -> TimelineEnvelope,
    ) -> Result<Ticket, ApiError>;
    /// Insert one external blocker. Storage assigns the blocker's
    /// identity and asks `envelope` for the timeline row that
    /// identity belongs in. Returns the blocker and the Ticket as it
    /// now stands.
    fn add_blocker(
        &self,
        waiting: &Ticket,
        description: &BlockerDescription,
        envelope: &dyn Fn(ExternalBlockerId) -> TimelineEnvelope,
    ) -> Result<(Ticket, ExternalBlocker), ApiError>;
    /// Remove one external blocker under the same guard.
    fn remove_blocker(
        &self,
        waiting: &Ticket,
        blocker: ExternalBlocker,
        envelope: &dyn Fn() -> TimelineEnvelope,
    ) -> Result<Ticket, ApiError>;
    /// Every registered edge in the whole graph, cross-Project edges
    /// included, in registration order.
    fn list_dependencies(&self) -> Result<Vec<TicketDependency>, ApiError>;
    /// The external blockers recorded against one Ticket, in
    /// recording order.
    fn blockers_of(&self, ticket: TicketId) -> Result<Vec<ExternalBlocker>, ApiError>;
}

/// The timeline row for one dependency change: on the waiting
/// Ticket's Project timeline, about the Ticket, with `action` naming
/// the change inside the closed `transition` kind.
fn transition(
    project: ProjectId,
    ticket: TicketId,
    action: &str,
    facts: Value,
) -> TimelineEnvelope {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("dependency transition facts are a JSON object");
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

/// The stores every dependency operation reads and writes through.
#[derive(Clone)]
struct DependencyContext {
    dependencies: Arc<dyn DependencyStore>,
    tickets: Arc<dyn TicketStore>,
    projects: Arc<dyn ProjectStore>,
}

impl DependencyContext {
    /// The waiting Ticket a command addresses, with its Project,
    /// refusing an unknown Ticket, the terminal Ticket states, and
    /// the terminal archived-Project state.
    fn open(&self, id: u64) -> Result<(Project, Ticket), ApiError> {
        let ticket = self
            .tickets
            .find(TicketId::new(id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {id}")))?;
        let project = self.projects.find(ticket.project())?.ok_or_else(|| {
            ApiError::internal(&format!("ticket {id} belongs to no stored Project"))
        })?;
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

    /// The whole registered graph, cross-Project edges included.
    fn graph(&self) -> Result<TicketDependencyGraph, ApiError> {
        Ok(TicketDependencyGraph::restore(
            self.dependencies.list_dependencies()?,
        ))
    }

    /// The blocking Ticket a dependency names; only a registered
    /// Ticket may be depended on (DR-DE-02).
    fn registered(&self, id: u64) -> Result<Ticket, ApiError> {
        self.tickets
            .find(TicketId::new(id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {id}")))
    }

    /// The `ticket.dependencies` record for one Ticket.
    fn dependencies_response(&self, ticket: &Ticket) -> Result<Value, ApiError> {
        let response = TicketDependenciesResponse {
            ticket_id: ticket.id().value(),
            version: ticket.version(),
            dependencies: self
                .graph()?
                .required_by(ticket.id())
                .iter()
                .map(|edge| self.dependency_record(*edge))
                .collect::<Result<Vec<_>, ApiError>>()?,
            blockers: self
                .dependencies
                .blockers_of(ticket.id())?
                .iter()
                .map(|blocker| TicketBlockerRecord {
                    id: blocker.id().value(),
                    ticket_id: blocker.ticket().value(),
                    description: blocker.description().as_str().to_owned(),
                })
                .collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }

    /// The wire record for one registered dependency, resolving the
    /// blocking Ticket. Storage only ever holds registered edges, so
    /// a missing endpoint is corruption, not an answer.
    fn dependency_record(
        &self,
        edge: TicketDependency,
    ) -> Result<TicketDependencyRecord, ApiError> {
        let blocking = self.tickets.find(edge.from())?.ok_or_else(|| {
            ApiError::internal(&format!(
                "dependency {} names no stored Ticket",
                edge.from().value()
            ))
        })?;
        Ok(TicketDependencyRecord {
            from_ticket_id: blocking.id().value(),
            from_project_id: blocking.project().value(),
            from_number: blocking.number().value(),
            from_state: state_of(blocking.state()),
        })
    }
}

impl Core {
    /// Register the Ticket dependency operations against
    /// `dependencies`, resolving Tickets through `tickets` and
    /// Projects through `projects`.
    pub fn register_dependencies(
        &mut self,
        dependencies: Arc<dyn DependencyStore>,
        tickets: Arc<dyn TicketStore>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), RegistrationError> {
        let context = DependencyContext {
            dependencies,
            tickets,
            projects,
        };
        self.register_command(
            "ticket.dependency.add",
            Arc::new(AddDependency(context.clone())),
        )?;
        self.register_command(
            "ticket.dependency.remove",
            Arc::new(RemoveDependency(context.clone())),
        )?;
        self.register_command("ticket.blocker.add", Arc::new(AddBlocker(context.clone())))?;
        self.register_command(
            "ticket.blocker.remove",
            Arc::new(RemoveBlocker(context.clone())),
        )?;
        self.register_query(
            "ticket.dependencies",
            Arc::new(GetDependencies {
                tickets: context.tickets.clone(),
                dependencies: context.dependencies.clone(),
            }),
        )?;
        self.register_query(
            "ticket.readiness",
            Arc::new(GetReadiness {
                tickets: context.tickets.clone(),
                dependencies: context.dependencies.clone(),
            }),
        )?;
        Ok(())
    }
}

/// Serves `ticket.dependency.add`.
struct AddDependency(DependencyContext);

impl CommandHandler for AddDependency {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketDependencyAddRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketDependencyAddRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.to_ticket)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn crate::mutation::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketDependencyAddRequest = parse_payload(&command.payload)?;
        let (project, waiting) = self.0.open(request.to_ticket)?;
        let blocking = self.0.registered(request.from_ticket)?;
        let edge = TicketDependency::new(blocking.id(), waiting.id());
        self.0
            .graph()?
            .add(blocking.id(), waiting.id())
            .map_err(refuse)?;
        let moved = self.0.dependencies.add_dependency(&waiting, edge, &|| {
            transition(
                project.id(),
                waiting.id(),
                "dependency_added",
                json!({
                    "from_ticket": blocking.id().value(),
                    "from_project_id": blocking.project().value(),
                    "to_ticket": waiting.id().value(),
                }),
            )
        })?;
        self.0.dependencies_response(&moved)
    }
}

/// Serves `ticket.dependency.remove`.
struct RemoveDependency(DependencyContext);

impl CommandHandler for RemoveDependency {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketDependencyRemoveRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketDependencyRemoveRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.to_ticket)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn crate::mutation::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketDependencyRemoveRequest = parse_payload(&command.payload)?;
        let (project, waiting) = self.0.open(request.to_ticket)?;
        let edge = TicketDependency::new(TicketId::new(request.from_ticket), waiting.id());
        self.0
            .graph()?
            .remove(edge.from(), edge.to())
            .map_err(refuse)?;
        let moved = self.0.dependencies.remove_dependency(&waiting, edge, &|| {
            transition(
                project.id(),
                waiting.id(),
                "dependency_removed",
                json!({
                    "from_ticket": edge.from().value(),
                    "to_ticket": waiting.id().value(),
                }),
            )
        })?;
        self.0.dependencies_response(&moved)
    }
}

/// Serves `ticket.blocker.add`.
struct AddBlocker(DependencyContext);

impl CommandHandler for AddBlocker {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketBlockerAddRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketBlockerAddRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn crate::mutation::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketBlockerAddRequest = parse_payload(&command.payload)?;
        let (project, waiting) = self.0.open(request.ticket_id)?;
        let description = BlockerDescription::new(&request.description).map_err(refuse)?;
        // The same waiting work is recorded once: a second blocker
        // naming it hides nothing and clears twice.
        if self
            .0
            .dependencies
            .blockers_of(waiting.id())?
            .iter()
            .any(|recorded| recorded.description() == &description)
        {
            return Err(ApiError::invalid_request(
                "that external blocker is already recorded on this Ticket",
            ));
        }
        let (moved, _) =
            self.0
                .dependencies
                .add_blocker(&waiting, &description, &|blocker_id| {
                    transition(
                        project.id(),
                        waiting.id(),
                        "blocker_added",
                        json!({
                            "blocker_id": blocker_id.value(),
                            "description": description.as_str(),
                        }),
                    )
                })?;
        self.0.dependencies_response(&moved)
    }
}

/// Serves `ticket.blocker.remove`.
struct RemoveBlocker(DependencyContext);

impl CommandHandler for RemoveBlocker {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketBlockerRemoveRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketBlockerRemoveRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn crate::mutation::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketBlockerRemoveRequest = parse_payload(&command.payload)?;
        let (project, waiting) = self.0.open(request.ticket_id)?;
        let blocker = self
            .0
            .dependencies
            .blockers_of(waiting.id())?
            .into_iter()
            .find(|recorded| recorded.id().value() == request.blocker_id)
            .ok_or_else(|| ApiError::not_found(&format!("blocker {}", request.blocker_id)))?;
        let blocker_id = blocker.id();
        let moved = self.0.dependencies.remove_blocker(&waiting, blocker, &|| {
            transition(
                project.id(),
                waiting.id(),
                "blocker_removed",
                json!({ "blocker_id": blocker_id.value() }),
            )
        })?;
        self.0.dependencies_response(&moved)
    }
}

/// Serves `ticket.dependencies`.
struct GetDependencies {
    tickets: Arc<dyn TicketStore>,
    dependencies: Arc<dyn DependencyStore>,
}

impl QueryHandler for GetDependencies {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: TicketDependenciesQuery = parse_payload(payload)?;
        let ticket = self
            .tickets
            .find(TicketId::new(query.ticket_id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {}", query.ticket_id)))?;
        let response = TicketDependenciesResponse {
            ticket_id: ticket.id().value(),
            version: ticket.version(),
            dependencies: TicketDependencyGraph::restore(self.dependencies.list_dependencies()?)
                .required_by(ticket.id())
                .iter()
                .map(|edge| {
                    let blocking = self.tickets.find(edge.from())?.ok_or_else(|| {
                        ApiError::internal(&format!(
                            "dependency {} names no stored Ticket",
                            edge.from().value()
                        ))
                    })?;
                    Ok(TicketDependencyRecord {
                        from_ticket_id: blocking.id().value(),
                        from_project_id: blocking.project().value(),
                        from_number: blocking.number().value(),
                        from_state: state_of(blocking.state()),
                    })
                })
                .collect::<Result<Vec<_>, ApiError>>()?,
            blockers: self
                .dependencies
                .blockers_of(ticket.id())?
                .iter()
                .map(|blocker| TicketBlockerRecord {
                    id: blocker.id().value(),
                    ticket_id: blocker.ticket().value(),
                    description: blocker.description().as_str().to_owned(),
                })
                .collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `ticket.readiness`.
struct GetReadiness {
    tickets: Arc<dyn TicketStore>,
    dependencies: Arc<dyn DependencyStore>,
}

impl QueryHandler for GetReadiness {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: TicketReadinessQuery = parse_payload(payload)?;
        let ticket = self
            .tickets
            .find(TicketId::new(query.ticket_id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {}", query.ticket_id)))?;
        // Pair every dependency the Ticket waits on with its blocker
        // and that blocker's state; the projection reads exactly
        // these and nothing else (DR-DE-03).
        let mut waiters = Vec::new();
        for edge in TicketDependencyGraph::restore(self.dependencies.list_dependencies()?)
            .required_by(ticket.id())
        {
            let blocking = self.tickets.find(edge.from())?.ok_or_else(|| {
                ApiError::internal(&format!(
                    "dependency {} names no stored Ticket",
                    edge.from().value()
                ))
            })?;
            waiters.push((edge, blocking));
        }
        let blockers = self.dependencies.blockers_of(ticket.id())?;
        let states: Vec<DependencyState> = waiters
            .iter()
            .map(|(edge, blocking)| DependencyState {
                dependency: *edge,
                state: blocking.state(),
            })
            .collect();
        let readiness: Readiness = compute_readiness(ReadinessInputs {
            dependencies: &states,
            blockers: &blockers,
        });
        let blocked_by = readiness
            .blocked_by()
            .iter()
            .map(|blocker| match blocker {
                ReadinessBlocker::Ticket { waiting } => {
                    let blocking = &waiters
                        .iter()
                        .find(|(edge, _)| *edge == waiting.dependency)
                        .expect("every reported dependency was paired")
                        .1;
                    TicketReadinessBlocker::Ticket {
                        from_ticket_id: blocking.id().value(),
                        from_project_id: blocking.project().value(),
                        from_number: blocking.number().value(),
                        from_state: state_of(waiting.state),
                    }
                }
                ReadinessBlocker::External { blocker } => TicketReadinessBlocker::External {
                    blocker_id: blocker.id().value(),
                    description: blocker.description().as_str().to_owned(),
                },
            })
            .collect();
        let response = TicketReadinessResponse {
            ticket_id: ticket.id().value(),
            state: state_of(ticket.state()),
            ready: readiness.is_ready(),
            blocked_by,
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex};

    use kanban_domain::{
        BlockerDescription, Priority, ProjectId, TicketBody, TicketNumber, TicketState,
    };
    use kanban_dto::ApiError;
    use serde_json::{Value, json};

    use super::DependencyStore;
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::NoopEventSink;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::plan::testing::{MemoryProjects, active_project};
    use crate::ticket::TicketStore;
    use crate::timeline::TimelineEnvelope;

    /// The in-memory rows the dependency tests run against: Tickets,
    /// the edges and blockers recorded against them, and the timeline
    /// envelopes the writes were asked to land.
    #[derive(Default)]
    pub(crate) struct MemoryDependencyRows {
        state: Mutex<MemoryDependencyState>,
    }

    #[derive(Default)]
    struct MemoryDependencyState {
        tickets: Vec<kanban_domain::Ticket>,
        edges: Vec<kanban_domain::TicketDependency>,
        blockers: Vec<kanban_domain::ExternalBlocker>,
        next_blocker_id: u64,
        timeline: Vec<TimelineEnvelope>,
    }

    impl MemoryDependencyRows {
        /// Seed one stored Ticket as-is, standing in for a created
        /// row.
        pub(crate) fn seed(&self, ticket: kanban_domain::Ticket) {
            self.state
                .lock()
                .expect("the memory dependency lock is sound")
                .tickets
                .push(ticket);
        }

        /// Replace one stored Ticket row, keeping its identity, so a
        /// test can move a blocker's lifecycle state.
        pub(crate) fn replace(&self, ticket: kanban_domain::Ticket) {
            let mut state = self
                .state
                .lock()
                .expect("the memory dependency lock is sound");
            if let Some(row) = state.tickets.iter_mut().find(|row| row.id() == ticket.id()) {
                *row = ticket;
            }
        }

        /// The stored rows, edges, and timeline envelopes, for
        /// assertions.
        pub(crate) fn snapshot(
            &self,
        ) -> (
            Vec<kanban_domain::Ticket>,
            Vec<kanban_domain::TicketDependency>,
            Vec<TimelineEnvelope>,
        ) {
            let state = self
                .state
                .lock()
                .expect("the memory dependency lock is sound");
            (
                state.tickets.clone(),
                state.edges.clone(),
                state.timeline.clone(),
            )
        }
    }

    impl TicketStore for MemoryDependencyRows {
        fn create(
            &self,
            _project: &kanban_domain::Project,
            _number: TicketNumber,
            _priority: Priority,
            _body: &TicketBody,
            _envelope: &dyn Fn(kanban_domain::TicketId) -> TimelineEnvelope,
        ) -> Result<kanban_domain::Ticket, ApiError> {
            Err(ApiError::internal(
                "the dependency fixtures seed Tickets directly",
            ))
        }

        fn save(
            &self,
            _ticket: &kanban_domain::Ticket,
            _envelope: TimelineEnvelope,
        ) -> Result<(), ApiError> {
            Err(ApiError::internal(
                "the dependency fixtures seed Tickets directly",
            ))
        }

        fn find(
            &self,
            id: kanban_domain::TicketId,
        ) -> Result<Option<kanban_domain::Ticket>, ApiError> {
            Ok(self
                .state
                .lock()
                .expect("the memory dependency lock is sound")
                .tickets
                .iter()
                .find(|row| row.id() == id)
                .cloned())
        }

        fn list(&self, project: ProjectId) -> Result<Vec<kanban_domain::Ticket>, ApiError> {
            Ok(self
                .state
                .lock()
                .expect("the memory dependency lock is sound")
                .tickets
                .iter()
                .filter(|row| row.project() == project)
                .cloned()
                .collect())
        }
    }

    impl DependencyStore for MemoryDependencyRows {
        fn add_dependency(
            &self,
            waiting: &kanban_domain::Ticket,
            edge: kanban_domain::TicketDependency,
            envelope: &dyn Fn() -> TimelineEnvelope,
        ) -> Result<kanban_domain::Ticket, ApiError> {
            let mut state = self
                .state
                .lock()
                .expect("the memory dependency lock is sound");
            state.edges.push(edge);
            state.timeline.push(envelope());
            let moved = kanban_domain::Ticket::restore(
                waiting.id(),
                waiting.project(),
                waiting.number(),
                waiting.priority(),
                waiting.state(),
                waiting.body().clone(),
                waiting.profile().cloned(),
                waiting.version() + 1,
            );
            state.tickets = state
                .tickets
                .iter()
                .map(|row| {
                    if row.id() == moved.id() {
                        moved.clone()
                    } else {
                        row.clone()
                    }
                })
                .collect();
            Ok(moved)
        }

        fn remove_dependency(
            &self,
            waiting: &kanban_domain::Ticket,
            edge: kanban_domain::TicketDependency,
            envelope: &dyn Fn() -> TimelineEnvelope,
        ) -> Result<kanban_domain::Ticket, ApiError> {
            let mut state = self
                .state
                .lock()
                .expect("the memory dependency lock is sound");
            state.edges.retain(|held| *held != edge);
            state.timeline.push(envelope());
            let moved = kanban_domain::Ticket::restore(
                waiting.id(),
                waiting.project(),
                waiting.number(),
                waiting.priority(),
                waiting.state(),
                waiting.body().clone(),
                waiting.profile().cloned(),
                waiting.version() + 1,
            );
            state.tickets = state
                .tickets
                .iter()
                .map(|row| {
                    if row.id() == moved.id() {
                        moved.clone()
                    } else {
                        row.clone()
                    }
                })
                .collect();
            Ok(moved)
        }

        fn add_blocker(
            &self,
            waiting: &kanban_domain::Ticket,
            description: &BlockerDescription,
            envelope: &dyn Fn(kanban_domain::ExternalBlockerId) -> TimelineEnvelope,
        ) -> Result<(kanban_domain::Ticket, kanban_domain::ExternalBlocker), ApiError> {
            let mut state = self
                .state
                .lock()
                .expect("the memory dependency lock is sound");
            state.next_blocker_id += 1;
            let blocker = kanban_domain::ExternalBlocker::restore(
                kanban_domain::ExternalBlockerId::new(state.next_blocker_id),
                waiting.id(),
                description.clone(),
            );
            state.blockers.push(blocker.clone());
            state.timeline.push(envelope(blocker.id()));
            let moved = kanban_domain::Ticket::restore(
                waiting.id(),
                waiting.project(),
                waiting.number(),
                waiting.priority(),
                waiting.state(),
                waiting.body().clone(),
                waiting.profile().cloned(),
                waiting.version() + 1,
            );
            state.tickets = state
                .tickets
                .iter()
                .map(|row| {
                    if row.id() == moved.id() {
                        moved.clone()
                    } else {
                        row.clone()
                    }
                })
                .collect();
            Ok((moved, blocker))
        }

        fn remove_blocker(
            &self,
            waiting: &kanban_domain::Ticket,
            blocker: kanban_domain::ExternalBlocker,
            envelope: &dyn Fn() -> TimelineEnvelope,
        ) -> Result<kanban_domain::Ticket, ApiError> {
            let mut state = self
                .state
                .lock()
                .expect("the memory dependency lock is sound");
            state.blockers.retain(|held| held.id() != blocker.id());
            state.timeline.push(envelope());
            let moved = kanban_domain::Ticket::restore(
                waiting.id(),
                waiting.project(),
                waiting.number(),
                waiting.priority(),
                waiting.state(),
                waiting.body().clone(),
                waiting.profile().cloned(),
                waiting.version() + 1,
            );
            state.tickets = state
                .tickets
                .iter()
                .map(|row| {
                    if row.id() == moved.id() {
                        moved.clone()
                    } else {
                        row.clone()
                    }
                })
                .collect();
            Ok(moved)
        }

        fn list_dependencies(&self) -> Result<Vec<kanban_domain::TicketDependency>, ApiError> {
            Ok(self
                .state
                .lock()
                .expect("the memory dependency lock is sound")
                .edges
                .clone())
        }

        fn blockers_of(
            &self,
            ticket: kanban_domain::TicketId,
        ) -> Result<Vec<kanban_domain::ExternalBlocker>, ApiError> {
            Ok(self
                .state
                .lock()
                .expect("the memory dependency lock is sound")
                .blockers
                .iter()
                .filter(|blocker| blocker.ticket() == ticket)
                .cloned()
                .collect())
        }
    }

    /// A core with the dependency operations wired to in-memory rows
    /// over two Projects: CORE holds Tickets 1 and 3, EDGE holds
    /// Ticket 2, and Ticket 5 stands superseded in CORE.
    pub(crate) struct DependencyHarness {
        pub(crate) rows: Arc<MemoryDependencyRows>,
        pub(crate) projects: Arc<MemoryProjects>,
        pub(crate) core: Core,
    }

    /// One stored Ticket of the harness, in the state a test chooses.
    fn ticket(id: u64, project: u64, number: u64, state: TicketState) -> kanban_domain::Ticket {
        kanban_domain::Ticket::restore(
            kanban_domain::TicketId::new(id),
            ProjectId::new(project),
            TicketNumber::new(number).expect("the fixture number is positive"),
            Priority::Normal,
            state,
            TicketBody::bug(
                "Landing drops the integration branch",
                None,
                "The integration branch is dropped after a review lands.",
                "The landing log names the drop immediately after the merge.",
            )
            .expect("the fixture body validates"),
            None,
            1,
        )
    }

    /// The harness the dependency tests run against, seeded with the
    /// two Projects and their Tickets.
    pub(crate) fn dependency_harness() -> DependencyHarness {
        let projects = Arc::new(MemoryProjects::default());
        projects.seed(active_project(
            1,
            "CORE",
            kanban_domain::ProjectCounters::restore(0, 0, 1),
        ));
        projects.seed(active_project(
            2,
            "EDGE",
            kanban_domain::ProjectCounters::restore(0, 0, 1),
        ));
        let rows = Arc::new(MemoryDependencyRows::default());
        rows.seed(ticket(1, 1, 1, TicketState::Active));
        rows.seed(ticket(2, 2, 1, TicketState::Draft));
        rows.seed(ticket(3, 2, 2, TicketState::Draft));
        rows.seed(ticket(5, 1, 2, TicketState::Superseded));
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            Arc::new(NoopEventSink),
        );
        core.register_dependencies(rows.clone(), rows.clone(), projects.clone())
            .expect("the dependency operations register");
        DependencyHarness {
            rows,
            projects,
            core,
        }
    }

    /// One command with the fields a test varies, addressed to the
    /// Ticket at `version`.
    pub(crate) fn command(body: serde_json::Value, version: u64, key: &str) -> Value {
        let mut request = json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
        });
        let request_object = request
            .as_object_mut()
            .expect("the command is a JSON object");
        for (field, value) in body.as_object().expect("the body is a JSON object") {
            request_object.insert(field.clone(), value.clone());
        }
        request
    }
}

#[cfg(test)]
mod cross_project_deps {
    use kanban_dto::ErrorCode;
    use serde_json::json;

    use super::testing::{command, dependency_harness};

    /// Register that Ticket 1 of CORE blocks Ticket 2 of EDGE, at the
    /// waiting Ticket's current version.
    fn core_blocks_edge(
        harness: &super::testing::DependencyHarness,
        key: &str,
    ) -> serde_json::Value {
        harness
            .core
            .command(
                "ticket.dependency.add",
                &command(json!({ "from_ticket": 1, "to_ticket": 2 }), 1, key),
            )
            .expect("the cross-Project dependency registers")
    }

    #[test]
    fn a_dependency_may_cross_projects_and_specs() {
        let harness = dependency_harness();

        let response = core_blocks_edge(&harness, "key-cross");

        assert_eq!(
            response,
            json!({
                "ticket_id": 2,
                "version": 2,
                "dependencies": [{
                    "from_ticket_id": 1,
                    "from_project_id": 1,
                    "from_number": 1,
                    "from_state": "active",
                }],
                "blockers": [],
            }),
            "the EDGE Ticket waits on the CORE Ticket's landing"
        );

        // A second edge inside one Project, across the two Specs the
        // waiting Tickets attach to in spirit, lands beside it.
        let response = harness
            .core
            .command(
                "ticket.dependency.add",
                &command(json!({ "from_ticket": 3, "to_ticket": 2 }), 2, "key-same"),
            )
            .expect("the same-Project dependency registers");
        assert_eq!(response["dependencies"].as_array().map(Vec::len), Some(2));
        assert_eq!(
            response["version"],
            json!(3),
            "each change moves the Ticket"
        );
    }

    #[test]
    fn a_cycle_across_projects_is_refused() {
        let harness = dependency_harness();
        core_blocks_edge(&harness, "key-cross");
        let (_, edges_before, timeline_before) = harness.rows.snapshot();

        let error = harness
            .core
            .command(
                "ticket.dependency.add",
                &command(json!({ "from_ticket": 2, "to_ticket": 1 }), 1, "key-cycle"),
            )
            .expect_err("the reverse edge closes a cycle");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the dependency from Ticket 2 to Ticket 1 would close a cycle"
        );
        let (_, edges, timeline) = harness.rows.snapshot();
        assert_eq!(
            edges.len(),
            edges_before.len(),
            "the refusal landed no edge"
        );
        assert_eq!(
            timeline.len(),
            timeline_before.len(),
            "the refusal appended nothing"
        );
    }

    #[test]
    fn an_unregistered_dependency_target_is_refused() {
        let harness = dependency_harness();

        let error = harness
            .core
            .command(
                "ticket.dependency.add",
                &command(
                    json!({ "from_ticket": 99, "to_ticket": 2 }),
                    1,
                    "key-unknown",
                ),
            )
            .expect_err("only a registered Ticket may be depended on");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(error.message, "ticket 99 was not found");

        let error = harness
            .core
            .command(
                "ticket.dependency.add",
                &command(
                    json!({ "from_ticket": 1, "to_ticket": 99 }),
                    1,
                    "key-unknown-to",
                ),
            )
            .expect_err("an unknown waiting Ticket is refused");
        assert_eq!(error.code, ErrorCode::NotFound);

        let error = harness
            .core
            .query("ticket.readiness", &json!({ "ticket_id": 99 }))
            .expect_err("readiness serves registered Tickets only");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn cross_project_dependencies_drive_readiness_automatically() {
        let harness = dependency_harness();
        core_blocks_edge(&harness, "key-cross");

        let blocked = harness
            .core
            .query("ticket.readiness", &json!({ "ticket_id": 2 }))
            .expect("the readiness serves");
        assert_eq!(blocked["ticket_id"], json!(2));
        assert_eq!(blocked["state"], json!("draft"));
        assert_eq!(blocked["ready"], json!(false));
        assert_eq!(
            blocked["blocked_by"],
            json!([
                { "Ticket": {
                    "from_ticket_id": 1,
                    "from_project_id": 1,
                    "from_number": 1,
                    "from_state": "active",
                }},
            ]),
            "the CORE Ticket holds the EDGE Ticket back"
        );

        // The blocking Ticket lands. Nothing touches the EDGE Ticket;
        // readiness is recomputed from the new state on read.
        let mut landed = kanban_domain::Ticket::restore(
            kanban_domain::TicketId::new(1),
            kanban_domain::ProjectId::new(1),
            kanban_domain::TicketNumber::new(1).expect("the fixture number is positive"),
            kanban_domain::Priority::Normal,
            kanban_domain::TicketState::Done,
            kanban_domain::TicketBody::bug(
                "Landing drops the integration branch",
                None,
                "The integration branch is dropped after a review lands.",
                "The landing log names the drop immediately after the merge.",
            )
            .expect("the fixture body validates"),
            None,
            4,
        );
        harness.rows.replace(landed.clone());
        landed = kanban_domain::Ticket::restore(
            kanban_domain::TicketId::new(2),
            kanban_domain::ProjectId::new(2),
            kanban_domain::TicketNumber::new(1).expect("the fixture number is positive"),
            kanban_domain::Priority::Normal,
            kanban_domain::TicketState::Draft,
            kanban_domain::TicketBody::bug(
                "Landing drops the integration branch",
                None,
                "The integration branch is dropped after a review lands.",
                "The landing log names the drop immediately after the merge.",
            )
            .expect("the fixture body validates"),
            None,
            2,
        );
        harness.rows.replace(landed);

        let ready = harness
            .core
            .query("ticket.readiness", &json!({ "ticket_id": 2 }))
            .expect("the readiness serves");
        assert_eq!(ready["ready"], json!(true));
        assert_eq!(ready["blocked_by"], json!([]));
        let (tickets, edges, _) = harness.rows.snapshot();
        assert_eq!(edges.len(), 1, "the projection never edits the graph");
        assert_eq!(
            tickets
                .iter()
                .find(|ticket| ticket.id().value() == 2)
                .expect("the waiting Ticket stands")
                .version(),
            2,
            "the projection never mutates the waiting Ticket"
        );
    }

    #[test]
    fn unregistered_waiting_work_is_an_external_blocker() {
        let harness = dependency_harness();

        let response = harness
            .core
            .command(
                "ticket.blocker.add",
                &command(
                    json!({ "ticket_id": 2, "description": "The vendor SDK 4 upgrade" }),
                    1,
                    "key-blocker",
                ),
            )
            .expect("the external blocker records");

        assert_eq!(
            response,
            json!({
                "ticket_id": 2,
                "version": 2,
                "dependencies": [],
                "blockers": [
                    { "id": 1, "ticket_id": 2, "description": "The vendor SDK 4 upgrade" },
                ],
            })
        );

        let blocked = harness
            .core
            .query("ticket.readiness", &json!({ "ticket_id": 2 }))
            .expect("the readiness serves");
        assert_eq!(blocked["ready"], json!(false));
        assert_eq!(
            blocked["blocked_by"],
            json!([
                { "External": {
                    "blocker_id": 1,
                    "description": "The vendor SDK 4 upgrade",
                }},
            ])
        );

        // Removal is the explicit operator action that clears it.
        let cleared = harness
            .core
            .command(
                "ticket.blocker.remove",
                &command(json!({ "ticket_id": 2, "blocker_id": 1 }), 2, "key-clear"),
            )
            .expect("the blocker removes");
        assert_eq!(cleared["blockers"], json!([]));
        assert_eq!(cleared["version"], json!(3));
        let ready = harness
            .core
            .query("ticket.readiness", &json!({ "ticket_id": 2 }))
            .expect("the readiness serves");
        assert_eq!(ready["ready"], json!(true));
    }

    #[test]
    fn blocker_commands_refuse_blank_and_duplicate_descriptions() {
        let harness = dependency_harness();
        harness
            .core
            .command(
                "ticket.blocker.add",
                &command(
                    json!({ "ticket_id": 2, "description": "Design sign-off" }),
                    1,
                    "key-1",
                ),
            )
            .expect("the fixture blocker records");

        let blank = harness
            .core
            .command(
                "ticket.blocker.add",
                &command(
                    json!({ "ticket_id": 2, "description": "   " }),
                    2,
                    "key-blank",
                ),
            )
            .expect_err("a blank description names nothing");
        assert_eq!(blank.code, ErrorCode::InvalidRequest);
        assert_eq!(
            blank.message,
            "an external blocker description cannot be blank"
        );

        let duplicate = harness
            .core
            .command(
                "ticket.blocker.add",
                &command(
                    json!({ "ticket_id": 2, "description": "Design sign-off" }),
                    2,
                    "key-2",
                ),
            )
            .expect_err("the same waiting work is recorded once");
        assert_eq!(duplicate.code, ErrorCode::InvalidRequest);
        assert_eq!(
            duplicate.message,
            "that external blocker is already recorded on this Ticket"
        );

        let unknown = harness
            .core
            .command(
                "ticket.blocker.remove",
                &command(json!({ "ticket_id": 2, "blocker_id": 9 }), 2, "key-3"),
            )
            .expect_err("only a recorded blocker removes");
        assert_eq!(unknown.code, ErrorCode::NotFound);
    }

    #[test]
    fn a_duplicate_dependency_is_refused() {
        let harness = dependency_harness();
        core_blocks_edge(&harness, "key-cross");

        let error = harness
            .core
            .command(
                "ticket.dependency.add",
                &command(
                    json!({ "from_ticket": 1, "to_ticket": 2 }),
                    2,
                    "key-duplicate",
                ),
            )
            .expect_err("the same dependency registers once");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "that Ticket dependency already exists");
    }

    #[test]
    fn dependency_removal_frees_the_waiter() {
        let harness = dependency_harness();
        core_blocks_edge(&harness, "key-cross");

        let removed = harness
            .core
            .command(
                "ticket.dependency.remove",
                &command(json!({ "from_ticket": 1, "to_ticket": 2 }), 2, "key-remove"),
            )
            .expect("the dependency removes");
        assert_eq!(removed["dependencies"], json!([]));
        assert_eq!(removed["version"], json!(3));

        let ready = harness
            .core
            .query("ticket.readiness", &json!({ "ticket_id": 2 }))
            .expect("the readiness serves");
        assert_eq!(ready["ready"], json!(true));

        let error = harness
            .core
            .command(
                "ticket.dependency.remove",
                &command(
                    json!({ "from_ticket": 1, "to_ticket": 2 }),
                    3,
                    "key-remove-again",
                ),
            )
            .expect_err("the edge is gone");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "that Ticket dependency does not exist");
    }

    #[test]
    fn a_terminal_ticket_accepts_no_dependency_changes() {
        let harness = dependency_harness();

        let error = harness
            .core
            .command(
                "ticket.dependency.add",
                &command(
                    json!({ "from_ticket": 1, "to_ticket": 5 }),
                    1,
                    "key-terminal",
                ),
            )
            .expect_err("a superseded Ticket waits on nothing further");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "cancelled and superseded are terminal; the Ticket accepts no further changes"
        );
    }

    #[test]
    fn an_archived_projects_tickets_accept_no_dependency_changes() {
        let harness = dependency_harness();
        let mut project = harness.projects.rows()[1].clone();
        project.archive().expect("the fixture archives");
        harness.projects.replace(project);

        let error = harness
            .core
            .command(
                "ticket.dependency.add",
                &command(
                    json!({ "from_ticket": 1, "to_ticket": 2 }),
                    1,
                    "key-archived",
                ),
            )
            .expect_err("an archived Project accepts no further changes");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("archived"));
    }

    #[test]
    fn a_stale_dependency_command_is_refused_with_the_current_version() {
        let harness = dependency_harness();
        core_blocks_edge(&harness, "key-cross");

        let error = harness
            .core
            .command(
                "ticket.dependency.add",
                &command(json!({ "from_ticket": 3, "to_ticket": 2 }), 1, "key-stale"),
            )
            .expect_err("the stale version is rejected");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(2));
    }

    #[test]
    fn a_key_spent_on_add_cannot_replay_for_remove() {
        let harness = dependency_harness();
        let added = core_blocks_edge(&harness, "key-clash");

        // The same key, aggregate, and body shape spent on the
        // opposite operation must not borrow the add's outcome.
        let error = harness
            .core
            .command(
                "ticket.dependency.remove",
                &command(json!({ "from_ticket": 1, "to_ticket": 2 }), 2, "key-clash"),
            )
            .expect_err("a key spent on add cannot serve remove");

        assert_eq!(error.code, ErrorCode::DuplicateIdempotencyKey);
        assert!(
            error.message.contains("key-clash"),
            "the message should name the reused key: {}",
            error.message
        );

        // The refusal applied nothing: the edge stands and the waiting
        // Ticket has not moved, so the remove lands under a fresh key
        // at the version the add left behind.
        let removed = harness
            .core
            .command(
                "ticket.dependency.remove",
                &command(json!({ "from_ticket": 1, "to_ticket": 2 }), 2, "key-remove"),
            )
            .expect("the remove applies under its own key");
        assert_eq!(removed["dependencies"], json!([]));
        assert_eq!(removed["version"], json!(3));
        let (_, edges, _) = harness.rows.snapshot();
        assert_eq!(edges.len(), 0, "exactly the one remove applied");

        // The operation that spent the key still owns its replay.
        let replay = harness
            .core
            .command(
                "ticket.dependency.add",
                &command(json!({ "from_ticket": 1, "to_ticket": 2 }), 1, "key-clash"),
            )
            .expect("the add's own retry still replays");
        assert_eq!(replay, added, "the recorded outcome is the add's alone");
    }

    #[test]
    fn a_retry_replays_without_reapplying() {
        let harness = dependency_harness();
        let request = command(json!({ "from_ticket": 1, "to_ticket": 2 }), 1, "key-once");

        let first = harness
            .core
            .command("ticket.dependency.add", &request)
            .expect("the dependency registers");
        let replay = harness
            .core
            .command("ticket.dependency.add", &request)
            .expect("the retry replays");

        assert_eq!(first, replay);
        let (_, edges, _) = harness.rows.snapshot();
        assert_eq!(edges.len(), 1, "the retry must not reapply");
    }

    #[test]
    fn commands_reject_unknown_fields() {
        let harness = dependency_harness();
        let mut request = command(json!({ "from_ticket": 1, "to_ticket": 2 }), 1, "key-1");
        request["surprise"] = json!(true);

        let error = harness
            .core
            .command("ticket.dependency.add", &request)
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }

    #[test]
    fn dependency_changes_append_the_timeline_on_the_waiting_side() {
        let harness = dependency_harness();
        core_blocks_edge(&harness, "key-cross");
        harness
            .core
            .command(
                "ticket.blocker.add",
                &command(
                    json!({ "ticket_id": 2, "description": "The vendor SDK 4 upgrade" }),
                    2,
                    "key-blocker",
                ),
            )
            .expect("the blocker records");

        let (_, _, timeline) = harness.rows.snapshot();
        let edge_added = &timeline[timeline.len() - 2];
        assert_eq!(edge_added.kind(), kanban_dto::TimelineEventKind::Transition);
        assert_eq!(
            edge_added
                .entity()
                .map(|entity| (entity.kind, entity.id.clone())),
            Some((kanban_dto::TimelineEntityKind::Ticket, "2".to_owned()))
        );
        assert_eq!(
            edge_added.detail(),
            &json!({
                "action": "dependency_added",
                "id": 2,
                "from_ticket": 1,
                "from_project_id": 1,
                "to_ticket": 2,
            }),
            "the cross-Project change lands on the waiting Ticket's own timeline"
        );

        let blocker_added = timeline.last().expect("the blocker appended");
        assert_eq!(
            blocker_added.detail(),
            &json!({
                "action": "blocker_added",
                "id": 2,
                "blocker_id": 1,
                "description": "The vendor SDK 4 upgrade",
            })
        );
    }
}
