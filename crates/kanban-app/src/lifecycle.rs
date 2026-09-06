//! Ticket lifecycle commands: the drag surface and the named human
//! commands — park, unpark, schedule, cancel, review decisions,
//! prioritise, and edit — every one guarded by the domain's
//! transition, ownership, and readiness rules, and the one audited
//! emergency override recovery uses to move past them (KAN-S4-US6,
//! DR-LC-06 to DR-LC-10). A drag arriving through this layer is a
//! human's, so it serves Task Tickets and refuses Implementation and
//! Bug Tickets with the explanation that their transitions are
//! agent-owned; the named commands serve every kind because they are
//! commands, not drags. Every change appends its timeline row inside
//! the same write as the row change, and the override's row records
//! who ran it, what moved, and why. Readiness stays the computed
//! projection KAN-T20 owns; these commands read it and never widen it.

use std::sync::Arc;

use kanban_domain::{
    Actor, DependencyState, HumanCommand, OverrideJustification, Priority as DomainPriority,
    Project, Readiness, ReadinessInputs, ReviewDecision, Ticket, TicketDependencyGraph, TicketId,
    TicketState as DomainState, apply_command, apply_drag, apply_override, compute_readiness,
};
use kanban_dto::{
    ApiError, LiveEventName, TicketCancelRequest, TicketEditRequest,
    TicketEmergencyOverrideRequest, TicketParkRequest, TicketPrioritiseRequest, TicketPriority,
    TicketReviewDecision, TicketReviewRequest, TicketScheduleRequest, TicketState,
    TicketTransitionRequest, TicketUnparkRequest, TimelineEntityKind, TimelineEntityRef,
    TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dependency::DependencyStore;
use crate::dispatch::{Core, RegistrationError};
use crate::events::emit_catalogued;
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;
use crate::ticket::{TicketStore, record_of};
use crate::timeline::TimelineEnvelope;

/// Report a refused domain rule as the stable invalid-request code.
fn refuse(error: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

/// The domain form of one wire state.
fn domain_state(state: TicketState) -> DomainState {
    match state {
        TicketState::Draft => DomainState::Draft,
        TicketState::Parked => DomainState::Parked,
        TicketState::Blocked => DomainState::Blocked,
        TicketState::Scheduled => DomainState::Scheduled,
        TicketState::Ready => DomainState::Ready,
        TicketState::Active => DomainState::Active,
        TicketState::InReview => DomainState::InReview,
        TicketState::Approved => DomainState::Approved,
        TicketState::Landing => DomainState::Landing,
        TicketState::Done => DomainState::Done,
        TicketState::Cancelled => DomainState::Cancelled,
        TicketState::Superseded => DomainState::Superseded,
    }
}

/// The domain form of one wire priority.
fn domain_priority(priority: TicketPriority) -> DomainPriority {
    match priority {
        TicketPriority::Urgent => DomainPriority::Urgent,
        TicketPriority::High => DomainPriority::High,
        TicketPriority::Normal => DomainPriority::Normal,
        TicketPriority::Low => DomainPriority::Low,
    }
}

/// The domain form of one wire review decision.
fn domain_decision(decision: TicketReviewDecision) -> ReviewDecision {
    match decision {
        TicketReviewDecision::Approve => ReviewDecision::Approve,
        TicketReviewDecision::Reject => ReviewDecision::Reject,
    }
}

/// The stores every lifecycle command reads and writes through.
#[derive(Clone)]
struct LifecycleContext {
    tickets: Arc<dyn TicketStore>,
    dependencies: Arc<dyn DependencyStore>,
    projects: Arc<dyn ProjectStore>,
}

impl LifecycleContext {
    /// The Ticket a command addresses with its Project, refusing an
    /// unknown Ticket, the terminal Ticket states, and the terminal
    /// archived-Project state.
    fn open(&self, id: u64) -> Result<(Project, Ticket), ApiError> {
        let (project, ticket) = self.find(id)?;
        if ticket.state().is_terminal() {
            return Err(ApiError::invalid_request(
                "cancelled and superseded are terminal; the Ticket accepts no further changes",
            ));
        }
        Ok((project, ticket))
    }

    /// The Ticket the emergency override addresses with its Project:
    /// recovery reaches past terminal Ticket states, because undoing a
    /// mistaken cancel is exactly the recovery this command exists
    /// for. An archived Project still refuses everything.
    fn open_for_override(&self, id: u64) -> Result<(Project, Ticket), ApiError> {
        self.find(id)
    }

    /// The stored Ticket and its Project, refusing the unknown and
    /// the archived.
    fn find(&self, id: u64) -> Result<(Project, Ticket), ApiError> {
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
        Ok((project, ticket))
    }

    /// The readiness these commands answer: the projection KAN-T20
    /// computes from the Ticket's dependencies and external blockers,
    /// read fresh, never widened.
    fn readiness_of(&self, ticket: &Ticket) -> Result<Readiness, ApiError> {
        let mut states = Vec::new();
        for edge in TicketDependencyGraph::restore(self.dependencies.list_dependencies()?)
            .required_by(ticket.id())
        {
            let blocking = self.tickets.find(edge.from())?.ok_or_else(|| {
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
        let blockers = self.dependencies.blockers_of(ticket.id())?;
        Ok(compute_readiness(ReadinessInputs {
            dependencies: &states,
            blockers: &blockers,
        }))
    }

    /// Apply one lifecycle movement and land everything it owes
    /// together: the domain move, the Ticket row under its version
    /// guard, the timeline row naming the action and both states, and
    /// the live announcement of the record the command returns. A
    /// refused movement writes nothing.
    fn land<E: std::fmt::Display>(
        &self,
        project: &Project,
        ticket: &mut Ticket,
        landing: Landing<'_>,
        movement: impl FnOnce(&mut Ticket, &Readiness) -> Result<(), E>,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let from = ticket.state().wire_name().to_owned();
        let readiness = self.readiness_of(ticket)?;
        movement(ticket, &readiness).map_err(refuse)?;
        let to = ticket.state().wire_name().to_owned();
        let mut detail = landing.facts;
        let object = detail
            .as_object_mut()
            .expect("lifecycle transition facts are a JSON object");
        object.insert("action".to_owned(), Value::from(landing.action));
        object.insert("id".to_owned(), Value::from(ticket.id().value()));
        object.insert("from".to_owned(), Value::from(from));
        object.insert("to".to_owned(), Value::from(to));
        self.tickets.save(
            ticket,
            TimelineEnvelope::project(
                project.id().value(),
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Ticket,
                    id: ticket.id().value().to_string(),
                }),
                detail,
            ),
        )?;
        emit_catalogued(
            effects,
            landing.announced,
            &record_of(ticket, project.code()),
        );
        serde_json::to_value(record_of(ticket, project.code()))
            .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// One lifecycle landing: the action the timeline row names, the
/// command-specific facts it carries, and the live event the change
/// announces.
struct Landing<'a> {
    action: &'a str,
    facts: Value,
    announced: LiveEventName,
}

impl Core {
    /// Register the Ticket lifecycle operations against `tickets`,
    /// resolving readiness through `dependencies` and Projects through
    /// `projects`.
    pub fn register_lifecycle(
        &mut self,
        tickets: Arc<dyn TicketStore>,
        dependencies: Arc<dyn DependencyStore>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), RegistrationError> {
        let context = LifecycleContext {
            tickets,
            dependencies,
            projects,
        };
        self.register_command(
            "ticket.transition",
            Arc::new(TransitionTicket(context.clone())),
        )?;
        self.register_command("ticket.park", Arc::new(ParkTicket(context.clone())))?;
        self.register_command("ticket.unpark", Arc::new(UnparkTicket(context.clone())))?;
        self.register_command("ticket.schedule", Arc::new(ScheduleTicket(context.clone())))?;
        self.register_command("ticket.cancel", Arc::new(CancelTicket(context.clone())))?;
        self.register_command("ticket.review", Arc::new(ReviewTicket(context.clone())))?;
        self.register_command(
            "ticket.prioritise",
            Arc::new(PrioritiseTicket(context.clone())),
        )?;
        self.register_command("ticket.edit", Arc::new(EditTicket(context.clone())))?;
        self.register_command(
            "ticket.emergency.override",
            Arc::new(EmergencyOverride(context)),
        )?;
        Ok(())
    }
}

/// Serves `ticket.transition`: the drag surface. The operator's
/// transport is a human's, so the drag serves Task Tickets and refuses
/// Implementation and Bug Tickets with the ownership explanation
/// (DR-LC-07, DR-LC-08).
struct TransitionTicket(LifecycleContext);

impl CommandHandler for TransitionTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketTransitionRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketTransitionRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketTransitionRequest = parse_payload(&command.payload)?;
        let (project, mut ticket) = self.0.open(request.ticket_id)?;
        let to = domain_state(request.to);
        self.0.land(
            &project,
            &mut ticket,
            Landing {
                action: "moved",
                facts: json!({}),
                announced: LiveEventName::TicketStateChanged,
            },
            |ticket, readiness| apply_drag(ticket, to, Actor::Human, readiness),
            effects,
        )
    }
}

/// Serves `ticket.park` (DR-LC-09).
struct ParkTicket(LifecycleContext);

impl CommandHandler for ParkTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketParkRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketParkRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketParkRequest = parse_payload(&command.payload)?;
        let (project, mut ticket) = self.0.open(request.ticket_id)?;
        self.0.land(
            &project,
            &mut ticket,
            Landing {
                action: "parked",
                facts: json!({}),
                announced: LiveEventName::TicketStateChanged,
            },
            |ticket, readiness| apply_command(ticket, HumanCommand::Park, readiness),
            effects,
        )
    }
}

/// Serves `ticket.unpark` (DR-LC-09).
struct UnparkTicket(LifecycleContext);

impl CommandHandler for UnparkTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketUnparkRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketUnparkRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketUnparkRequest = parse_payload(&command.payload)?;
        let (project, mut ticket) = self.0.open(request.ticket_id)?;
        self.0.land(
            &project,
            &mut ticket,
            Landing {
                action: "unparked",
                facts: json!({}),
                announced: LiveEventName::TicketStateChanged,
            },
            |ticket, readiness| apply_command(ticket, HumanCommand::Unpark, readiness),
            effects,
        )
    }
}

/// Serves `ticket.schedule` (DR-LC-09); activation behaviour is
/// KAN-S11's.
struct ScheduleTicket(LifecycleContext);

impl CommandHandler for ScheduleTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketScheduleRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketScheduleRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketScheduleRequest = parse_payload(&command.payload)?;
        let (project, mut ticket) = self.0.open(request.ticket_id)?;
        self.0.land(
            &project,
            &mut ticket,
            Landing {
                action: "scheduled",
                facts: json!({}),
                announced: LiveEventName::TicketStateChanged,
            },
            |ticket, readiness| apply_command(ticket, HumanCommand::Schedule, readiness),
            effects,
        )
    }
}

/// Serves `ticket.cancel` (DR-LC-09). Cancelled is terminal and absent
/// from the active board (DR-LC-02).
struct CancelTicket(LifecycleContext);

impl CommandHandler for CancelTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketCancelRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketCancelRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketCancelRequest = parse_payload(&command.payload)?;
        let (project, mut ticket) = self.0.open(request.ticket_id)?;
        self.0.land(
            &project,
            &mut ticket,
            Landing {
                action: "cancelled",
                facts: json!({}),
                announced: LiveEventName::TicketStateChanged,
            },
            |ticket, readiness| apply_command(ticket, HumanCommand::Cancel, readiness),
            effects,
        )
    }
}

/// Serves `ticket.review`: one explicit human review decision
/// (DR-LC-09). The review flows that stage findings are KAN-S10's.
struct ReviewTicket(LifecycleContext);

impl CommandHandler for ReviewTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketReviewRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketReviewRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketReviewRequest = parse_payload(&command.payload)?;
        let (project, mut ticket) = self.0.open(request.ticket_id)?;
        let decision = domain_decision(request.decision);
        let wire_decision = decision.as_str();
        self.0.land(
            &project,
            &mut ticket,
            Landing {
                action: "reviewed",
                facts: json!({ "decision": wire_decision }),
                announced: LiveEventName::TicketStateChanged,
            },
            |ticket, readiness| apply_command(ticket, HumanCommand::Review(decision), readiness),
            effects,
        )
    }
}

/// Serves `ticket.prioritise` (DR-LC-09, DR-LC-12).
struct PrioritiseTicket(LifecycleContext);

impl CommandHandler for PrioritiseTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketPrioritiseRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketPrioritiseRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketPrioritiseRequest = parse_payload(&command.payload)?;
        let (project, mut ticket) = self.0.open(request.ticket_id)?;
        let priority = domain_priority(request.priority);
        let wire_priority = priority.wire_name();
        self.0.land(
            &project,
            &mut ticket,
            Landing {
                action: "prioritised",
                facts: json!({ "priority": wire_priority }),
                announced: LiveEventName::TicketEdited,
            },
            |ticket, _| ticket.prioritise(priority),
            effects,
        )
    }
}

/// Serves `ticket.edit`: the title a Bug or Task carries or the slice
/// description an Implementation carries (DR-LC-09). Each kind sends
/// exactly the field it owns.
struct EditTicket(LifecycleContext);

impl CommandHandler for EditTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketEditRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketEditRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketEditRequest = parse_payload(&command.payload)?;
        let (project, mut ticket) = self.0.open(request.ticket_id)?;
        let landing = Landing {
            action: "edited",
            facts: json!({}),
            announced: LiveEventName::TicketEdited,
        };
        match (request.title, request.slice) {
            (Some(title), None) => self.0.land(
                &project,
                &mut ticket,
                Landing {
                    facts: json!({ "field": "title" }),
                    ..landing
                },
                |ticket, _| ticket.retitle(title),
                effects,
            ),
            (None, Some(slice)) => self.0.land(
                &project,
                &mut ticket,
                Landing {
                    facts: json!({ "field": "slice" }),
                    ..landing
                },
                |ticket, _| ticket.redescribe(slice),
                effects,
            ),
            (Some(_), Some(_)) => Err(ApiError::invalid_request(
                "an edit carries the title or the slice description, never both",
            )),
            (None, None) => Err(ApiError::invalid_request(
                "an edit carries the title or the slice description",
            )),
        }
    }
}

/// Serves `ticket.emergency.override`: recovery's one audited way past
/// the rules (DR-LC-10). The timeline row records who ran it, what
/// moved, and why; no unrestricted drag exists beside it.
struct EmergencyOverride(LifecycleContext);

impl CommandHandler for EmergencyOverride {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketEmergencyOverrideRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketEmergencyOverrideRequest = parse_payload(&command.payload)?;
        Ok(self.0.open_for_override(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketEmergencyOverrideRequest = parse_payload(&command.payload)?;
        let justification =
            OverrideJustification::new(&request.who, &request.why).map_err(refuse)?;
        let (project, mut ticket) = self.0.open_for_override(request.ticket_id)?;
        let to = domain_state(request.to);
        let who = justification.who().to_owned();
        let why = justification.why().to_owned();
        self.0.land(
            &project,
            &mut ticket,
            Landing {
                action: "emergency_override",
                facts: json!({ "who": who, "why": why }),
                announced: LiveEventName::TicketStateChanged,
            },
            |ticket, _| apply_override(ticket, to, &justification),
            effects,
        )
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex};

    use kanban_domain::{
        BlockerDescription, ExternalBlocker, ExternalBlockerId, Priority, ProjectId, TicketBody,
        TicketDependency, TicketNumber, TicketState,
    };
    use kanban_dto::ApiError;

    use super::super::dependency::DependencyStore;
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::EventSink;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::plan::testing::{MemoryPlans, MemoryProjects, active_project};
    use crate::spec::testing::MemorySpecs;
    use crate::ticket::TicketStore;
    use crate::ticket::testing::MemoryTicketEvidence;
    use crate::timeline::TimelineEnvelope;

    /// The in-memory rows the lifecycle tests run against: Tickets
    /// created through the commands, dependency edges and blockers
    /// seeded straight onto them, and the timeline envelopes the
    /// writes were asked to land.
    #[derive(Default)]
    pub(crate) struct LifecycleRows {
        state: Mutex<LifecycleRowState>,
        projects: Arc<MemoryProjects>,
    }

    #[derive(Default)]
    struct LifecycleRowState {
        tickets: Vec<kanban_domain::Ticket>,
        next_id: u64,
        edges: Vec<TicketDependency>,
        blockers: Vec<ExternalBlocker>,
        next_blocker_id: u64,
        timeline: Vec<TimelineEnvelope>,
    }

    impl LifecycleRows {
        /// A row store sharing the Project rows the harness seeded.
        pub(crate) fn sharing(projects: Arc<MemoryProjects>) -> Self {
            Self {
                projects,
                ..Self::default()
            }
        }

        /// Seed one dependency edge straight onto the stored rows, the
        /// way a registered edge would stand.
        pub(crate) fn seed_edge(&self, from: u64, to: u64) {
            self.state
                .lock()
                .expect("the memory lifecycle lock is sound")
                .edges
                .push(TicketDependency::new(
                    kanban_domain::TicketId::new(from),
                    kanban_domain::TicketId::new(to),
                ));
        }

        /// Seed one external blocker straight onto a stored Ticket.
        pub(crate) fn seed_blocker(&self, ticket: u64, description: &str) {
            let mut state = self
                .state
                .lock()
                .expect("the memory lifecycle lock is sound");
            state.next_blocker_id += 1;
            let id = state.next_blocker_id;
            state.blockers.push(ExternalBlocker::restore(
                ExternalBlockerId::new(id),
                kanban_domain::TicketId::new(ticket),
                BlockerDescription::new(description).expect("the fixture description validates"),
            ));
        }

        /// Force one stored Ticket into a state, counting the change,
        /// for fixtures that need a Ticket already far along its
        /// lifecycle.
        pub(crate) fn force_state(&self, id: u64, state: TicketState) {
            let mut rows = self
                .state
                .lock()
                .expect("the memory lifecycle lock is sound");
            if let Some(row) = rows.tickets.iter_mut().find(|row| row.id().value() == id) {
                let moved = kanban_domain::Ticket::restore(
                    row.id(),
                    row.project(),
                    row.number(),
                    row.priority(),
                    state,
                    row.body().clone(),
                    row.profile().cloned(),
                    row.version() + 1,
                );
                *row = moved;
            }
        }

        /// The stored Tickets and timeline envelopes, for assertions.
        pub(crate) fn snapshot(&self) -> (Vec<kanban_domain::Ticket>, Vec<TimelineEnvelope>) {
            let state = self
                .state
                .lock()
                .expect("the memory lifecycle lock is sound");
            (state.tickets.clone(), state.timeline.clone())
        }
    }

    impl TicketStore for LifecycleRows {
        fn create(
            &self,
            project: &kanban_domain::Project,
            number: TicketNumber,
            priority: Priority,
            body: &TicketBody,
            envelope: &dyn Fn(kanban_domain::TicketId) -> TimelineEnvelope,
        ) -> Result<kanban_domain::Ticket, ApiError> {
            let mut state = self
                .state
                .lock()
                .expect("the memory lifecycle lock is sound");
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
            let id = kanban_domain::TicketId::new(state.next_id);
            let ticket =
                kanban_domain::Ticket::new(id, project.id(), number, priority, body.clone());
            state.tickets.push(ticket.clone());
            state.timeline.push(envelope(id));
            Ok(ticket)
        }

        fn find(
            &self,
            id: kanban_domain::TicketId,
        ) -> Result<Option<kanban_domain::Ticket>, ApiError> {
            let state = self
                .state
                .lock()
                .expect("the memory lifecycle lock is sound");
            Ok(state.tickets.iter().find(|row| row.id() == id).cloned())
        }

        fn save(
            &self,
            ticket: &kanban_domain::Ticket,
            envelope: TimelineEnvelope,
        ) -> Result<(), ApiError> {
            let mut state = self
                .state
                .lock()
                .expect("the memory lifecycle lock is sound");
            let preceding = ticket.version() - 1;
            let index = state.tickets.iter().position(|row| row.id() == ticket.id());
            match index {
                Some(index) if state.tickets[index].version() == preceding => {
                    state.tickets[index] = ticket.clone();
                    state.timeline.push(envelope);
                    Ok(())
                }
                Some(index) => Err(ApiError::stale_version(
                    preceding,
                    state.tickets[index].version(),
                )),
                None => Err(ApiError::not_found(&format!("ticket {}", ticket.id()))),
            }
        }

        fn list(&self, project: ProjectId) -> Result<Vec<kanban_domain::Ticket>, ApiError> {
            let state = self
                .state
                .lock()
                .expect("the memory lifecycle lock is sound");
            Ok(state
                .tickets
                .iter()
                .filter(|row| row.project() == project)
                .cloned()
                .collect())
        }
    }

    impl DependencyStore for LifecycleRows {
        fn add_dependency(
            &self,
            _waiting: &kanban_domain::Ticket,
            _edge: TicketDependency,
            _envelope: &dyn Fn() -> TimelineEnvelope,
        ) -> Result<kanban_domain::Ticket, ApiError> {
            Err(ApiError::internal(
                "the lifecycle fixtures seed edges directly",
            ))
        }

        fn remove_dependency(
            &self,
            _waiting: &kanban_domain::Ticket,
            _edge: TicketDependency,
            _envelope: &dyn Fn() -> TimelineEnvelope,
        ) -> Result<kanban_domain::Ticket, ApiError> {
            Err(ApiError::internal(
                "the lifecycle fixtures seed edges directly",
            ))
        }

        fn add_blocker(
            &self,
            _waiting: &kanban_domain::Ticket,
            _description: &BlockerDescription,
            _envelope: &dyn Fn(ExternalBlockerId) -> TimelineEnvelope,
        ) -> Result<(kanban_domain::Ticket, ExternalBlocker), ApiError> {
            Err(ApiError::internal(
                "the lifecycle fixtures seed blockers directly",
            ))
        }

        fn remove_blocker(
            &self,
            _waiting: &kanban_domain::Ticket,
            _blocker: ExternalBlocker,
            _envelope: &dyn Fn() -> TimelineEnvelope,
        ) -> Result<kanban_domain::Ticket, ApiError> {
            Err(ApiError::internal(
                "the lifecycle fixtures seed blockers directly",
            ))
        }

        fn list_dependencies(&self) -> Result<Vec<TicketDependency>, ApiError> {
            let state = self
                .state
                .lock()
                .expect("the memory lifecycle lock is sound");
            Ok(state.edges.clone())
        }

        fn blockers_of(
            &self,
            ticket: kanban_domain::TicketId,
        ) -> Result<Vec<ExternalBlocker>, ApiError> {
            let state = self
                .state
                .lock()
                .expect("the memory lifecycle lock is sound");
            Ok(state
                .blockers
                .iter()
                .filter(|blocker| blocker.ticket() == ticket)
                .cloned()
                .collect())
        }
    }

    /// A core with the Plan, Spec, Ticket, and lifecycle operations
    /// wired to in-memory rows over one active Project.
    pub(crate) struct LifecycleHarness {
        pub(crate) rows: Arc<LifecycleRows>,
        pub(crate) projects: Arc<MemoryProjects>,
        pub(crate) core: Core,
    }

    /// The harness the lifecycle tests run against.
    pub(crate) fn lifecycle_harness() -> LifecycleHarness {
        lifecycle_harness_with_sink(Arc::new(crate::events::NoopEventSink))
    }

    /// A harness whose event sink the test chooses.
    pub(crate) fn lifecycle_harness_with_sink(events: Arc<dyn EventSink>) -> LifecycleHarness {
        let projects = Arc::new(MemoryProjects::default());
        projects.seed(active_project(
            1,
            "CORE",
            kanban_domain::ProjectCounters::restore(0, 0, 0),
        ));
        let plans = Arc::new(MemoryPlans::sharing(projects.clone()));
        let specs = Arc::new(MemorySpecs::sharing(projects.clone()));
        let rows = Arc::new(LifecycleRows::sharing(projects.clone()));
        let evidence = Arc::new(MemoryTicketEvidence::default());
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            events,
        );
        core.register_plans(plans.clone(), projects.clone(), specs.clone())
            .expect("the plan operations register");
        core.register_specs(specs.clone(), projects.clone(), plans.clone())
            .expect("the spec operations register");
        core.register_tickets(
            rows.clone(),
            projects.clone(),
            specs.clone(),
            evidence.clone(),
        )
        .expect("the ticket operations register");
        core.register_lifecycle(rows.clone(), rows.clone(), projects.clone())
            .expect("the lifecycle operations register");
        LifecycleHarness {
            rows,
            projects,
            core,
        }
    }
}

#[cfg(test)]
mod lifecycle_commands {
    use kanban_dto::ErrorCode;
    use serde_json::{Value, json};

    use super::testing::lifecycle_harness;

    /// One mutation context addressed to `version`.
    fn mutation(version: u64, key: &str) -> Value {
        json!({
            "optimistic_version": version,
            "idempotency_key": key,
        })
    }

    /// One lifecycle command with the body a test chooses.
    fn command(body: Value, version: u64, key: &str) -> Value {
        let mut request = json!({ "mutation": mutation(version, key) });
        let object = request
            .as_object_mut()
            .expect("the command is a JSON object");
        for (field, value) in body.as_object().expect("the body is a JSON object") {
            object.insert(field.clone(), value.clone());
        }
        request
    }

    /// Create one Task Ticket, returning its identity.
    fn task(harness: &super::testing::LifecycleHarness, key: &str) -> u64 {
        let created = harness
            .core
            .command(
                "ticket.create",
                &command(
                    json!({
                        "project_id": 1,
                        "kind": "task",
                        "priority": "normal",
                        "title": "Archive the old register",
                        "subtype": "operational",
                        "mode": "human",
                        "completion": ["The register is archived."],
                    }),
                    0,
                    key,
                ),
            )
            .expect("the Task creates");
        created["id"].as_u64().expect("the identity is a number")
    }

    /// Create one quick-captured Bug, returning its identity.
    fn captured_bug(harness: &super::testing::LifecycleHarness, key: &str) -> u64 {
        let created = harness
            .core
            .command(
                "ticket.create",
                &command(
                    json!({
                        "project_id": 1,
                        "kind": "bug",
                        "priority": "normal",
                        "title": "Landing drops the integration branch",
                        "actual_behaviour": "The integration branch is dropped after a review lands.",
                        "reporter_evidence":
                            "The landing log names the drop immediately after the merge.",
                    }),
                    0,
                    key,
                ),
            )
            .expect("the Bug quick captures");
        created["id"].as_u64().expect("the identity is a number")
    }

    /// Qualify one Bug through the command surface.
    fn qualify(harness: &super::testing::LifecycleHarness, bug: u64, version: u64, key: &str) {
        harness
            .core
            .command(
                "ticket.bug.qualify",
                &command(
                    json!({
                        "ticket_id": bug,
                        "qualification": crate::ticket::testing::wire_qualification("high"),
                    }),
                    version,
                    key,
                ),
            )
            .expect("the Bug qualifies");
    }

    /// Drag one Ticket to `to` through the drag surface.
    fn drag(
        harness: &super::testing::LifecycleHarness,
        ticket: u64,
        to: &str,
        version: u64,
        key: &str,
    ) -> Result<Value, kanban_dto::ApiError> {
        harness.core.command(
            "ticket.transition",
            &command(json!({ "ticket_id": ticket, "to": to }), version, key),
        )
    }

    #[test]
    fn a_drag_moves_a_task_ticket_through_a_legal_transition() {
        let harness = lifecycle_harness();
        let chore = task(&harness, "key-task");

        let moved = drag(&harness, chore, "ready", 1, "key-drag")
            .expect("a human drags a Task Ticket through a legal transition");

        assert_eq!(moved["state"], json!("ready"));
        assert_eq!(moved["version"], json!(2));
        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": chore }))
            .expect("the get serves");
        assert_eq!(read["state"], json!("ready"), "the move persisted");

        let (_, timeline) = harness.rows.snapshot();
        let appended = timeline.last().expect("the move appended");
        assert_eq!(appended.kind(), kanban_dto::TimelineEventKind::Transition);
        assert_eq!(
            appended
                .entity()
                .map(|entity| (entity.kind, entity.id.clone())),
            Some((kanban_dto::TimelineEntityKind::Ticket, chore.to_string()))
        );
        assert_eq!(
            appended.detail(),
            &json!({
                "action": "moved",
                "id": chore,
                "from": "draft",
                "to": "ready",
            })
        );
    }

    #[test]
    fn a_drag_of_an_agent_owned_kind_is_refused_with_the_explanation() {
        let harness = lifecycle_harness();
        let bug = captured_bug(&harness, "key-bug");
        qualify(&harness, bug, 1, "key-qualify");
        let (_, timeline_before) = harness.rows.snapshot();

        let refused = drag(&harness, bug, "ready", 2, "key-drag")
            .expect_err("Implementation and Bug transitions are agent-owned");

        assert_eq!(refused.code, ErrorCode::InvalidRequest);
        assert_eq!(
            refused.message,
            "bug transitions are agent-owned; a human may drag only Task Tickets"
        );
        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": bug }))
            .expect("the get serves");
        assert_eq!(read["state"], json!("draft"), "the refusal moved nothing");
        assert_eq!(read["version"], json!(2));
        let (_, timeline) = harness.rows.snapshot();
        assert_eq!(
            timeline.len(),
            timeline_before.len(),
            "the refusal appended no timeline row"
        );
    }

    #[test]
    fn an_illegal_drag_is_refused_and_names_both_states() {
        let harness = lifecycle_harness();
        let chore = task(&harness, "key-task");

        let error =
            drag(&harness, chore, "done", 1, "key-drag").expect_err("draft holds no edge to done");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "a Ticket moves along the canonical lifecycle; draft to done is not one of its moves"
        );
    }

    #[test]
    fn readiness_holds_a_ticket_back_from_becoming_ready() {
        let harness = lifecycle_harness();
        let blocker = task(&harness, "key-blocker");
        let waiting = task(&harness, "key-waiting");
        harness.rows.seed_edge(blocker, waiting);

        let error = drag(&harness, waiting, "ready", 1, "key-drag")
            .expect_err("an unlanded dependency holds the Ticket back");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the Ticket is held back by 1 unresolved dependencies or external blockers"
        );

        // The blocker lands and the same drag passes; readiness is
        // read fresh, never cached.
        harness
            .rows
            .force_state(blocker, kanban_domain::TicketState::Done);
        let moved = drag(&harness, waiting, "ready", 1, "key-drag-again")
            .expect("a landed dependency stops holding the Ticket");
        assert_eq!(moved["state"], json!("ready"));

        // An external blocker holds work back from starting just the
        // same (DR-DE-04).
        let chore = task(&harness, "key-chore");
        harness.rows.seed_blocker(chore, "The vendor SDK 4 upgrade");
        harness
            .rows
            .force_state(chore, kanban_domain::TicketState::Ready);
        let error = drag(&harness, chore, "active", 2, "key-start")
            .expect_err("starting work re-checks readiness");
        assert_eq!(
            error.message,
            "the Ticket is held back by 1 unresolved dependencies or external blockers"
        );
    }

    #[test]
    fn the_named_commands_serve_every_kind() {
        let harness = lifecycle_harness();
        let bug = captured_bug(&harness, "key-bug");
        qualify(&harness, bug, 1, "key-qualify");

        let parked = harness
            .core
            .command(
                "ticket.park",
                &command(json!({ "ticket_id": bug }), 2, "key-park"),
            )
            .expect("a human parks an agent-owned kind by command");
        assert_eq!(parked["state"], json!("parked"));

        let unparked = harness
            .core
            .command(
                "ticket.unpark",
                &command(json!({ "ticket_id": bug }), 3, "key-unpark"),
            )
            .expect("unpark returns the Bug to circulation");
        assert_eq!(unparked["state"], json!("ready"));

        let scheduled_bug = captured_bug(&harness, "key-bug-2");
        qualify(&harness, scheduled_bug, 1, "key-qualify-2");
        let scheduled = harness
            .core
            .command(
                "ticket.schedule",
                &command(json!({ "ticket_id": scheduled_bug }), 2, "key-schedule"),
            )
            .expect("a qualified Bug schedules");
        assert_eq!(scheduled["state"], json!("scheduled"));

        let cancelled = harness
            .core
            .command(
                "ticket.cancel",
                &command(json!({ "ticket_id": bug }), 4, "key-cancel"),
            )
            .expect("a human cancels every kind");
        assert_eq!(cancelled["state"], json!("cancelled"));
    }

    #[test]
    fn an_unqualified_bug_is_sealed_into_draft_by_the_commands() {
        let harness = lifecycle_harness();
        let captured = captured_bug(&harness, "key-bug");

        let sealed = harness
            .core
            .command(
                "ticket.park",
                &command(json!({ "ticket_id": captured }), 1, "key-park"),
            )
            .expect_err("a captured Bug stays draft until qualified");

        assert_eq!(sealed.code, ErrorCode::InvalidRequest);
        assert_eq!(
            sealed.message,
            "a Bug stays draft until it carries a complete qualification"
        );

        // Cancel is the one way out of draft an unqualified Bug keeps.
        let cancelled = harness
            .core
            .command(
                "ticket.cancel",
                &command(json!({ "ticket_id": captured }), 1, "key-cancel"),
            )
            .expect("a quick-captured Bug cancels without qualifying");
        assert_eq!(cancelled["state"], json!("cancelled"));

        let terminal = harness
            .core
            .command(
                "ticket.park",
                &command(json!({ "ticket_id": captured }), 2, "key-park-again"),
            )
            .expect_err("cancelled is terminal");
        assert_eq!(
            terminal.message,
            "cancelled and superseded are terminal; the Ticket accepts no further changes"
        );
    }

    #[test]
    fn review_decisions_resolve_an_in_review_ticket() {
        let harness = lifecycle_harness();
        let chore = task(&harness, "key-task");
        for (to, key) in [
            ("ready", "key-1"),
            ("active", "key-2"),
            ("in_review", "key-3"),
        ] {
            let version = match to {
                "ready" => 1,
                "active" => 2,
                _ => 3,
            };
            drag(&harness, chore, to, version, key).expect("the drag lands");
        }

        let approved = harness
            .core
            .command(
                "ticket.review",
                &command(
                    json!({ "ticket_id": chore, "decision": "approve" }),
                    4,
                    "key-approve",
                ),
            )
            .expect("the review approves");
        assert_eq!(approved["state"], json!("approved"));

        let (_, timeline) = harness.rows.snapshot();
        assert_eq!(
            timeline.last().expect("the decision appended").detail(),
            &json!({
                "action": "reviewed",
                "id": chore,
                "decision": "approve",
                "from": "in_review",
                "to": "approved",
            })
        );

        // A rejection returns work to active.
        let other = task(&harness, "key-task-2");
        for (to, version, key) in [
            ("ready", 1, "key-a"),
            ("active", 2, "key-b"),
            ("in_review", 3, "key-c"),
        ] {
            drag(&harness, other, to, version, key).expect("the drag lands");
        }
        let rejected = harness
            .core
            .command(
                "ticket.review",
                &command(
                    json!({ "ticket_id": other, "decision": "reject" }),
                    4,
                    "key-reject",
                ),
            )
            .expect("the review rejects");
        assert_eq!(rejected["state"], json!("active"));
    }

    #[test]
    fn prioritising_sets_the_closed_vocabulary() {
        let harness = lifecycle_harness();
        let chore = task(&harness, "key-task");

        let prioritised = harness
            .core
            .command(
                "ticket.prioritise",
                &command(
                    json!({ "ticket_id": chore, "priority": "urgent" }),
                    1,
                    "key-prioritise",
                ),
            )
            .expect("the Ticket prioritises");

        assert_eq!(prioritised["priority"], json!("urgent"));
        assert_eq!(prioritised["state"], json!("draft"), "no lifecycle move");
        assert_eq!(prioritised["version"], json!(2));
        let (_, timeline) = harness.rows.snapshot();
        assert_eq!(
            timeline.last().expect("the change appended").detail(),
            &json!({
                "action": "prioritised",
                "id": chore,
                "priority": "urgent",
                "from": "draft",
                "to": "draft",
            })
        );
    }

    #[test]
    fn editing_serves_exactly_the_field_the_kind_carries() {
        let harness = lifecycle_harness();
        let spec = crate::ticket::testing::authored_spec(&harness.core, "key-spec");
        let bug = captured_bug(&harness, "key-bug");
        let retitled = harness
            .core
            .command(
                "ticket.edit",
                &command(
                    json!({
                        "ticket_id": bug,
                        "title": "Landing drops every branch",
                    }),
                    1,
                    "key-edit",
                ),
            )
            .expect("a Bug retitles");
        assert_eq!(retitled["title"], json!("Landing drops every branch"));
        assert_eq!(retitled["version"], json!(2));

        let sliced = harness
            .core
            .command(
                "ticket.create",
                &command(
                    json!({
                        "project_id": 1,
                        "kind": "implementation",
                        "priority": "normal",
                        "spec_id": spec,
                        "slice": "Specs approve end to end",
                        "criteria": [
                            { "outcome": "Approval freezes content.", "stories": ["CORE-S1-US1"] }
                        ],
                    }),
                    0,
                    "key-implementation",
                ),
            )
            .expect("the Implementation creates");
        let redescribed = harness
            .core
            .command(
                "ticket.edit",
                &command(
                    json!({
                        "ticket_id": sliced["id"],
                        "slice": "Specs approve end to end, again",
                    }),
                    1,
                    "key-edit-slice",
                ),
            )
            .expect("an Implementation carries an edited slice");
        assert_eq!(
            redescribed["slice"],
            json!("Specs approve end to end, again")
        );

        let wrong_field = harness
            .core
            .command(
                "ticket.edit",
                &command(
                    json!({ "ticket_id": bug, "slice": "A slice" }),
                    2,
                    "key-edit-wrong",
                ),
            )
            .expect_err("a Bug carries no slice description");
        assert_eq!(wrong_field.code, ErrorCode::InvalidRequest);
        assert_eq!(
            wrong_field.message,
            "only an Implementation Ticket carries a slice description"
        );

        let both = harness
            .core
            .command(
                "ticket.edit",
                &command(
                    json!({
                        "ticket_id": bug,
                        "title": "A title",
                        "slice": "A slice",
                    }),
                    2,
                    "key-edit-both",
                ),
            )
            .expect_err("an edit sends one field");
        assert_eq!(
            both.message,
            "an edit carries the title or the slice description, never both"
        );

        let neither = harness
            .core
            .command(
                "ticket.edit",
                &command(json!({ "ticket_id": bug }), 2, "key-edit-none"),
            )
            .expect_err("an edit that edits nothing is refused");
        assert_eq!(
            neither.message,
            "an edit carries the title or the slice description"
        );
    }

    #[test]
    fn an_archived_projects_tickets_accept_no_lifecycle_commands() {
        let harness = lifecycle_harness();
        let chore = task(&harness, "key-task");
        let mut project = harness.projects.rows()[0].clone();
        project.archive().expect("the fixture archives");
        harness.projects.replace(project);

        let error = drag(&harness, chore, "ready", 1, "key-drag")
            .expect_err("an archived Project accepts no further changes");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("archived"));
    }

    #[test]
    fn unknown_tickets_are_refused_and_unknown_fields_rejected() {
        let harness = lifecycle_harness();

        let error = drag(&harness, 99, "ready", 1, "key-unknown")
            .expect_err("an unknown Ticket is refused");
        assert_eq!(error.code, ErrorCode::NotFound);

        let mut surprise = command(json!({ "ticket_id": 1, "to": "ready" }), 1, "key-1");
        surprise["surprise"] = json!(true);
        let error = harness
            .core
            .command("ticket.transition", &surprise)
            .expect_err("unknown fields are rejected");
        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }

    #[test]
    fn a_stale_command_is_refused_by_the_optimistic_version() {
        let harness = lifecycle_harness();
        let chore = task(&harness, "key-task");

        let (_, timeline_before) = harness.rows.snapshot();
        let error = drag(&harness, chore, "ready", 0, "key-stale")
            .expect_err("the stale version is rejected");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(1));
        let (_, timeline) = harness.rows.snapshot();
        assert_eq!(
            timeline.len(),
            timeline_before.len(),
            "a stale command appends no timeline row"
        );
    }

    #[test]
    fn a_retry_replays_without_reapplying() {
        let harness = lifecycle_harness();
        let chore = task(&harness, "key-task");
        let request = command(json!({ "ticket_id": chore, "to": "ready" }), 1, "key-once");

        let first = harness
            .core
            .command("ticket.transition", &request)
            .expect("the drag lands");
        let replay = harness
            .core
            .command("ticket.transition", &request)
            .expect("the retry replays");

        assert_eq!(first, replay);
        assert_eq!(replay["version"], json!(2), "the retry must not reapply");
    }

    #[test]
    fn lifecycle_changes_announce_on_the_event_stream() {
        let sink = std::sync::Arc::new(crate::plan::testing::RecordingSink::default());
        let harness = super::testing::lifecycle_harness_with_sink(sink.clone());
        let chore = task(&harness, "key-task");

        drag(&harness, chore, "ready", 1, "key-drag").expect("the drag lands");
        harness
            .core
            .command(
                "ticket.prioritise",
                &command(
                    json!({ "ticket_id": chore, "priority": "high" }),
                    2,
                    "key-prioritise",
                ),
            )
            .expect("the Ticket prioritises");

        let events = sink.events.lock().expect("the recorder lock is sound");
        let changed = events
            .iter()
            .find(|(name, _)| name == "ticket.state.changed")
            .expect("a lifecycle move announces live");
        assert_eq!(changed.1["state"], json!("ready"));
        let edited = events
            .iter()
            .find(|(name, _)| name == "ticket.edited")
            .expect("an edit announces live");
        assert_eq!(edited.1["priority"], json!("high"));
    }
}

#[cfg(test)]
mod emergency_override {
    use kanban_dto::ErrorCode;
    use serde_json::{Value, json};

    use super::testing::lifecycle_harness;

    /// One mutation context addressed to `version`.
    fn mutation(version: u64, key: &str) -> Value {
        json!({
            "optimistic_version": version,
            "idempotency_key": key,
        })
    }

    /// One command with the body a test chooses.
    fn command(body: Value, version: u64, key: &str) -> Value {
        let mut request = json!({ "mutation": mutation(version, key) });
        let object = request
            .as_object_mut()
            .expect("the command is a JSON object");
        for (field, value) in body.as_object().expect("the body is a JSON object") {
            object.insert(field.clone(), value.clone());
        }
        request
    }

    /// Create one Task Ticket, returning its identity.
    fn task(harness: &super::testing::LifecycleHarness, key: &str) -> u64 {
        let created = harness
            .core
            .command(
                "ticket.create",
                &command(
                    json!({
                        "project_id": 1,
                        "kind": "task",
                        "priority": "normal",
                        "title": "Archive the old register",
                        "subtype": "operational",
                        "mode": "human",
                        "completion": ["The register is archived."],
                    }),
                    0,
                    key,
                ),
            )
            .expect("the Task creates");
        created["id"].as_u64().expect("the identity is a number")
    }

    /// One emergency override request against `ticket`.
    fn overriding(ticket: u64, to: &str, who: &str, why: &str, version: u64, key: &str) -> Value {
        command(
            json!({ "ticket_id": ticket, "to": to, "who": who, "why": why }),
            version,
            key,
        )
    }

    #[test]
    fn the_override_moves_past_the_rules_and_records_who_what_and_why() {
        let harness = lifecycle_harness();
        let chore = task(&harness, "key-task");
        harness
            .rows
            .force_state(chore, kanban_domain::TicketState::Active);

        let recovered = harness
            .core
            .command(
                "ticket.emergency.override",
                &overriding(
                    chore,
                    "ready",
                    "Sid Wood",
                    "Recovery after the core crashed mid move",
                    2,
                    "key-override",
                ),
            )
            .expect("recovery moves against the canonical order");

        assert_eq!(recovered["state"], json!("ready"));
        assert_eq!(recovered["version"], json!(3));

        let (_, timeline) = harness.rows.snapshot();
        let audited = timeline.last().expect("the override appended");
        assert_eq!(
            audited.detail(),
            &json!({
                "action": "emergency_override",
                "id": chore,
                "who": "Sid Wood",
                "why": "Recovery after the core crashed mid move",
                "from": "active",
                "to": "ready",
            }),
            "the audit row records who ran it, what moved, and why"
        );
    }

    #[test]
    fn the_override_ignores_the_kind_and_readiness_gates() {
        let harness = lifecycle_harness();

        // An unqualified Bug leaves draft through the override alone.
        let captured = harness
            .core
            .command(
                "ticket.create",
                &command(
                    json!({
                        "project_id": 1,
                        "kind": "bug",
                        "priority": "normal",
                        "title": "Landing drops the integration branch",
                        "actual_behaviour": "The integration branch is dropped after a review lands.",
                        "reporter_evidence":
                            "The landing log names the drop immediately after the merge.",
                    }),
                    0,
                    "key-bug",
                ),
            )
            .expect("the Bug quick captures");
        let bug = captured["id"].as_u64().expect("the identity is a number");
        let moved = harness
            .core
            .command(
                "ticket.emergency.override",
                &overriding(
                    bug,
                    "ready",
                    "Sid Wood",
                    "Captured in error",
                    1,
                    "key-override",
                ),
            )
            .expect("the override answers no qualification gate");
        assert_eq!(moved["state"], json!("ready"));

        // A held-back Ticket moves through the override too: the
        // override is audited recovery, not a second rule set.
        let chore = task(&harness, "key-task");
        harness.rows.seed_blocker(chore, "The vendor SDK 4 upgrade");
        let moved = harness
            .core
            .command(
                "ticket.emergency.override",
                &overriding(
                    chore,
                    "ready",
                    "Sid Wood",
                    "Blocker cleared out of band",
                    1,
                    "key-2",
                ),
            )
            .expect("the override answers no readiness gate");
        assert_eq!(moved["state"], json!("ready"));
    }

    #[test]
    fn the_override_reaches_past_terminal_states_and_no_drag_does() {
        let harness = lifecycle_harness();
        let chore = task(&harness, "key-task");

        let cancelled = harness
            .core
            .command(
                "ticket.cancel",
                &command(json!({ "ticket_id": chore }), 1, "key-cancel"),
            )
            .expect("the Ticket cancels");
        assert_eq!(cancelled["state"], json!("cancelled"));

        // Every rule-bound surface refuses a terminal Ticket.
        for (name, body) in [
            (
                "ticket.transition",
                json!({ "ticket_id": chore, "to": "ready" }),
            ),
            ("ticket.park", json!({ "ticket_id": chore })),
            ("ticket.unpark", json!({ "ticket_id": chore })),
            ("ticket.schedule", json!({ "ticket_id": chore })),
            ("ticket.cancel", json!({ "ticket_id": chore })),
            (
                "ticket.review",
                json!({ "ticket_id": chore, "decision": "approve" }),
            ),
            (
                "ticket.prioritise",
                json!({ "ticket_id": chore, "priority": "high" }),
            ),
            (
                "ticket.edit",
                json!({ "ticket_id": chore, "title": "A title" }),
            ),
        ] {
            let error = harness
                .core
                .command(name, &command(body, 2, "key-refused"))
                .expect_err(&format!("{name} must refuse a terminal Ticket"));
            assert_eq!(
                error.message,
                "cancelled and superseded are terminal; the Ticket accepts no further changes",
                "{name} must refuse a terminal Ticket"
            );
        }

        // The override alone reaches past, and the audit row says why.
        let revived = harness
            .core
            .command(
                "ticket.emergency.override",
                &overriding(
                    chore,
                    "ready",
                    "Sid Wood",
                    "Cancelled by mistake; the work continues",
                    2,
                    "key-override",
                ),
            )
            .expect("recovery reaches past terminal states");
        assert_eq!(revived["state"], json!("ready"));
    }

    #[test]
    fn the_override_requires_an_operator_and_a_reason() {
        let harness = lifecycle_harness();
        let chore = task(&harness, "key-task");
        let timeline_before = harness.rows.snapshot().1.len();

        let nameless = harness
            .core
            .command(
                "ticket.emergency.override",
                &overriding(chore, "ready", "  ", "Because.", 1, "key-1"),
            )
            .expect_err("an override names its operator");
        assert_eq!(nameless.code, ErrorCode::InvalidRequest);
        assert_eq!(
            nameless.message,
            "an emergency override operator cannot be blank"
        );

        let reasonless = harness
            .core
            .command(
                "ticket.emergency.override",
                &overriding(chore, "ready", "Sid Wood", "", 1, "key-2"),
            )
            .expect_err("an override states its reason");
        assert_eq!(
            reasonless.message,
            "an emergency override reason cannot be blank"
        );

        let (_, timeline) = harness.rows.snapshot();
        assert_eq!(
            timeline.len(),
            timeline_before,
            "the refusals appended no timeline row"
        );
    }

    #[test]
    fn an_override_to_the_held_state_is_refused() {
        let harness = lifecycle_harness();
        let chore = task(&harness, "key-task");

        let error = harness
            .core
            .command(
                "ticket.emergency.override",
                &overriding(chore, "draft", "Sid Wood", "No-op", 1, "key-1"),
            )
            .expect_err("an override to the held state recovers nothing");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "the Ticket already holds that state");
    }

    #[test]
    fn an_archived_project_accepts_no_overrides() {
        let harness = lifecycle_harness();
        let chore = task(&harness, "key-task");
        let mut project = harness.projects.rows()[0].clone();
        project.archive().expect("the fixture archives");
        harness.projects.replace(project);

        let error = harness
            .core
            .command(
                "ticket.emergency.override",
                &overriding(chore, "ready", "Sid Wood", "Recovery", 1, "key-1"),
            )
            .expect_err("an archived Project accepts no further changes");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("archived"));
    }

    #[test]
    fn the_override_announces_on_the_event_stream_and_replays_on_retry() {
        let sink = std::sync::Arc::new(crate::plan::testing::RecordingSink::default());
        let harness = super::testing::lifecycle_harness_with_sink(sink.clone());
        let chore = task(&harness, "key-task");
        let request = overriding(
            chore,
            "ready",
            "Sid Wood",
            "Recovery after the core crashed mid move",
            1,
            "key-once",
        );

        let first = harness
            .core
            .command("ticket.emergency.override", &request)
            .expect("the override lands");
        let replay = harness
            .core
            .command("ticket.emergency.override", &request)
            .expect("the retry replays");
        assert_eq!(first, replay);
        assert_eq!(
            replay["version"],
            json!(2),
            "the retry must not reapply the override"
        );

        let events = sink.events.lock().expect("the recorder lock is sound");
        assert!(
            events
                .iter()
                .any(|(name, payload)| name == "ticket.state.changed"
                    && payload["id"] == json!(chore)),
            "the override announces live"
        );
    }
}
