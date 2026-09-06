//! Ticket graph proposal commands and queries: record an agent's
//! complete dependency graph of Tickets against the approved Spec
//! version it proposes for, approve it through the human gate —
//! pinning every Ticket in the graph to that Spec content version —
//! and read the proposals of one Spec back (KAN-S4-US8, DR-PS-16,
//! DR-PS-17, DR-DE-06). A proposal joins the dependencies the store
//! already holds — inside its Tickets or crossing out of them across
//! Specs and Projects — so recording and the gate both refuse edges
//! that would close a cycle with them (DR-DE-02). Recording mutates
//! no Ticket and names only executable members — a Ticket pinned to
//! an earlier version or a terminal one stays history, never a
//! member of a new graph; approval's proposal move, Ticket pins, and
//! timeline rows land in one storage write, so a graph approval
//! never splits across a crash boundary. The gate also refuses a
//! graph whose Tickets carry assignments referencing profiles the
//! catalogue no longer offers (KAN-S7-US4, T38), so approval never
//! pins a Ticket nothing can dispatch.

use std::sync::Arc;

use kanban_domain::{
    GraphProposalError, GraphProposalId, GraphProposalState, Project, SpecId, SpecNumber,
    StoryScope, TicketDependency, TicketDependencyGraph, TicketGraphProposal, TicketId,
    enforce_acyclic_with_registered, enforce_approvable, enforce_assignable,
    enforce_executable_member,
};
use kanban_dto::{
    ApiError, TicketGraphApproveRequest, TicketGraphEdgeRecord, TicketGraphListQuery,
    TicketGraphListResponse, TicketGraphProposeRequest, TicketGraphRecord, TicketGraphState,
    TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dependency::DependencyStore;
use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::mutation::{CommandHandler, ParsedCommand, parse_payload};
use crate::profile::ProfileStore;
use crate::project::ProjectStore;
use crate::spec::SpecStore;
use crate::ticket::TicketStore;
use crate::timeline::TimelineEnvelope;

/// The storage port the graph proposal operations call through.
/// Implementations land the proposal row, the approval's Ticket pins,
/// and the timeline envelopes unchanged inside one write.
pub trait GraphProposalStore: Send + Sync {
    /// Insert a fresh proposal. Storage assigns the proposal's
    /// identity and asks `envelope` for the timeline row that identity
    /// belongs in. The parts arrived domain-validated.
    fn create(
        &self,
        spec: SpecId,
        spec_version: u64,
        tickets: Vec<TicketId>,
        edges: Vec<TicketDependency>,
        envelope: &dyn Fn(GraphProposalId) -> TimelineEnvelope,
    ) -> Result<TicketGraphProposal, ApiError>;
    /// Load one proposal, if it exists.
    fn find(&self, id: GraphProposalId) -> Result<Option<TicketGraphProposal>, ApiError>;
    /// Every proposal recorded against one Spec, oldest first.
    fn list(&self, spec: SpecId) -> Result<Vec<TicketGraphProposal>, ApiError>;
    /// Land one approval: the proposal row moves to approved and every
    /// pinned Ticket row moves with it, all in one write guarded by
    /// the versions the aggregates moved from.
    fn apply_approval(
        &self,
        proposal: &TicketGraphProposal,
        pinned: &[kanban_domain::Ticket],
        envelopes: &[TimelineEnvelope],
    ) -> Result<(), ApiError>;
}

/// The timeline row for one graph change: on the Spec's Project
/// timeline, about the Spec, with `action` naming the change inside
/// the closed `transition` kind.
fn transition(
    project: kanban_domain::ProjectId,
    spec: SpecId,
    action: &str,
    facts: Value,
) -> TimelineEnvelope {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("graph transition facts are a JSON object");
    object.insert("action".to_owned(), Value::from(action));
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

/// The timeline row for one Ticket's pin: on the Ticket's own Project
/// timeline, about the Ticket, naming the approval that pinned it.
fn ticket_transition(
    project: kanban_domain::ProjectId,
    ticket: TicketId,
    proposal: GraphProposalId,
    spec_version: u64,
) -> TimelineEnvelope {
    TimelineEnvelope::project(
        project.value(),
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Ticket,
            id: ticket.value().to_string(),
        }),
        json!({
            "action": "pinned",
            "id": ticket.value(),
            "proposal_id": proposal.value(),
            "spec_version": spec_version,
        }),
    )
}

/// The wire form of one domain proposal state.
fn state_of(state: GraphProposalState) -> TicketGraphState {
    match state {
        GraphProposalState::Proposed => TicketGraphState::Proposed,
        GraphProposalState::Approved => TicketGraphState::Approved,
    }
}

/// The wire record for one proposal.
fn record_of(proposal: &TicketGraphProposal) -> TicketGraphRecord {
    TicketGraphRecord {
        id: proposal.id().value(),
        spec_id: proposal.spec().value(),
        spec_version: proposal.spec_version(),
        state: state_of(*proposal.state()),
        tickets: proposal
            .tickets()
            .iter()
            .map(|ticket| ticket.value())
            .collect(),
        edges: proposal
            .edges()
            .iter()
            .map(|edge| TicketGraphEdgeRecord {
                from_ticket: edge.from().value(),
                to_ticket: edge.to().value(),
            })
            .collect(),
        version: proposal.version(),
    }
}

/// Encode a record for a command response.
fn encode_record(proposal: &TicketGraphProposal) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(proposal))
        .map_err(|error| ApiError::internal(&error.to_string()))
}

/// The stores every graph proposal operation reads and writes through.
#[derive(Clone)]
struct GraphContext {
    proposals: Arc<dyn GraphProposalStore>,
    dependencies: Arc<dyn DependencyStore>,
    tickets: Arc<dyn TicketStore>,
    specs: Arc<dyn SpecStore>,
    projects: Arc<dyn ProjectStore>,
    profiles: Arc<dyn ProfileStore>,
}

impl GraphContext {
    /// The whole registered dependency graph, cross-Project edges
    /// included, as the proposal joins with and the approval installs
    /// into.
    fn registered(&self) -> Result<TicketDependencyGraph, ApiError> {
        Ok(TicketDependencyGraph::restore(
            self.dependencies.list_dependencies()?,
        ))
    }

    /// The Spec a command addresses with its Project, refusing an
    /// unknown Spec and the terminal archived-Project state.
    fn open_spec(&self, id: u64) -> Result<(Project, kanban_domain::Spec), ApiError> {
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

    /// The Spec version a proposal names, refusing a version the Spec
    /// does not hold and one that is not approved: only an approved
    /// version receives a Ticket graph (DR-PS-16).
    fn approved_version(
        &self,
        spec: &kanban_domain::Spec,
        version: u64,
    ) -> Result<kanban_domain::SpecVersion, ApiError> {
        let held = spec
            .pinned_version(version)
            .ok_or_else(|| ApiError::not_found(&format!("version {version}")))?;
        if held.state() != kanban_domain::SpecContentState::Approved {
            return Err(ApiError::invalid_request(
                "only an approved Spec version receives a Ticket graph",
            ));
        }
        Ok(held.clone())
    }

    /// Every Ticket attached to the Spec, in id order, each holding
    /// the Project the gate's rules read.
    fn attached(
        &self,
        project: &Project,
        spec: SpecId,
    ) -> Result<Vec<kanban_domain::Ticket>, ApiError> {
        Ok(self
            .tickets
            .list(project.id())?
            .into_iter()
            .filter(|ticket| ticket.spec() == Some(spec))
            .collect())
    }

    /// The story scope of one Spec version, as the Project's code
    /// claims it.
    fn scope(
        &self,
        project: &Project,
        number: SpecNumber,
        user_stories: &str,
    ) -> Result<StoryScope, ApiError> {
        StoryScope::extract(project.code(), number, user_stories).map_err(refuse)
    }
}

impl Core {
    /// Register the Ticket graph proposal operations against
    /// `proposals`, joining the registered dependency graph through
    /// `dependencies`, resolving Tickets through `tickets`, Specs
    /// through `specs`, Projects through `projects`, and profile
    /// references through `profiles`.
    pub fn register_graph_proposals(
        &mut self,
        proposals: Arc<dyn GraphProposalStore>,
        dependencies: Arc<dyn DependencyStore>,
        tickets: Arc<dyn TicketStore>,
        specs: Arc<dyn SpecStore>,
        projects: Arc<dyn ProjectStore>,
        profiles: Arc<dyn ProfileStore>,
    ) -> Result<(), RegistrationError> {
        let context = GraphContext {
            proposals,
            dependencies,
            tickets,
            specs,
            projects,
            profiles,
        };
        self.register_command(
            "ticket.graph.propose",
            Arc::new(ProposeGraph(context.clone())),
        )?;
        self.register_command(
            "ticket.graph.approve",
            Arc::new(ApproveGraph(context.clone())),
        )?;
        self.register_query(
            "ticket.graph.list",
            Arc::new(ListGraphProposals {
                proposals: context.proposals.clone(),
                specs: context.specs.clone(),
            }),
        )?;
        Ok(())
    }
}

/// Serves `ticket.graph.propose`.
struct ProposeGraph(GraphContext);

impl CommandHandler for ProposeGraph {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketGraphProposeRequest>(payload)?;
        ParsedCommand::lift("ticket-graph", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        // A fresh aggregate is created at version 0.
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _events: &dyn crate::mutation::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketGraphProposeRequest = parse_payload(&command.payload)?;
        let (project, spec) = self.0.open_spec(request.spec_id)?;
        self.0.approved_version(&spec, request.spec_version)?;
        let mut tickets = Vec::with_capacity(request.tickets.len());
        for named in &request.tickets {
            let ticket = self
                .0
                .tickets
                .find(TicketId::new(*named))?
                .ok_or_else(|| ApiError::not_found(&format!("ticket {named}")))?;
            if ticket.project() != project.id() {
                return Err(ApiError::invalid_request(
                    "the Ticket belongs to another Project",
                ));
            }
            if ticket.spec() != Some(spec.id()) {
                return Err(ApiError::invalid_request(
                    "the Ticket is not attached to the Spec",
                ));
            }
            enforce_executable_member(&ticket).map_err(refuse)?;
            tickets.push(ticket.id());
        }
        let edges: Vec<TicketDependency> = request
            .edges
            .iter()
            .map(|edge| {
                TicketDependency::new(
                    TicketId::new(edge.from_ticket),
                    TicketId::new(edge.to_ticket),
                )
            })
            .collect();
        TicketGraphProposal::validate(&tickets, &edges).map_err(refuse)?;
        // The proposal joins the dependencies the store already holds,
        // so it records only a graph that can join them without a
        // cycle (DR-DE-02); approval rechecks against whatever the
        // store holds by then.
        enforce_acyclic_with_registered(&edges, &self.0.registered()?)
            .map_err(|reason| GraphProposalError::RegisteredCycle { reason })
            .map_err(refuse)?;
        let spec_id = spec.id();
        let spec_version = request.spec_version;
        let named = tickets.clone();
        let proposal = self
            .0
            .proposals
            .create(spec_id, spec_version, tickets, edges, &|id| {
                transition(
                    project.id(),
                    spec_id,
                    "graph_proposed",
                    json!({
                        "proposal_id": id.value(),
                        "spec_id": spec_id.value(),
                        "spec_version": spec_version,
                        "tickets": named.iter().map(|ticket| ticket.value()).collect::<Vec<_>>(),
                    }),
                )
            })?;
        encode_record(&proposal)
    }
}

/// Serves `ticket.graph.approve`.
struct ApproveGraph(GraphContext);

impl CommandHandler for ApproveGraph {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketGraphApproveRequest>(payload)?;
        ParsedCommand::lift("ticket-graph", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketGraphApproveRequest = parse_payload(&command.payload)?;
        let proposal = self
            .0
            .proposals
            .find(GraphProposalId::new(request.proposal_id))?
            .ok_or_else(|| ApiError::not_found(&format!("proposal {}", request.proposal_id)))?;
        Ok(proposal.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _events: &dyn crate::mutation::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketGraphApproveRequest = parse_payload(&command.payload)?;
        let mut proposal = self
            .0
            .proposals
            .find(GraphProposalId::new(request.proposal_id))?
            .ok_or_else(|| ApiError::not_found(&format!("proposal {}", request.proposal_id)))?;
        let (project, spec) = self.0.open_spec(proposal.spec().value())?;
        let version = self.0.approved_version(&spec, proposal.spec_version())?;
        if self.0.proposals.list(spec.id())?.iter().any(|held| {
            held.id() != proposal.id()
                && *held.state() == GraphProposalState::Approved
                && held.spec_version() == proposal.spec_version()
        }) {
            return Err(ApiError::invalid_request(
                "the Spec version already carries an approved Ticket graph",
            ));
        }
        let attached = self.0.attached(&project, spec.id())?;
        let scope = self
            .0
            .scope(&project, spec.number(), version.content().user_stories())?;
        // The gate joins the dependencies the store holds now, not the
        // ones it held at recording: edges registered in between still
        // close cycles the gate must refuse.
        let registered = self.0.registered()?;
        enforce_approvable(&proposal, &registered, &scope, &attached).map_err(refuse)?;
        // The graph's shape holds; the assignments it would execute
        // must resolve too, or approval pins Tickets nothing can
        // dispatch (KAN-S7-US4).
        let catalogue = crate::profile::catalogue_of(self.0.profiles.as_ref())?;
        enforce_assignable(&catalogue, &attached).map_err(refuse)?;
        // Pin every Ticket the graph holds — each one exactly the
        // version the proposal named — and record the decision.
        let mut pinned = Vec::with_capacity(proposal.tickets().len());
        let mut envelopes = vec![transition(
            project.id(),
            spec.id(),
            "graph_approved",
            json!({
                "proposal_id": proposal.id().value(),
                "spec_id": spec.id().value(),
                "spec_version": proposal.spec_version(),
                "tickets": proposal
                    .tickets()
                    .iter()
                    .map(|ticket| ticket.value())
                    .collect::<Vec<_>>(),
            }),
        )];
        for id in proposal.tickets() {
            let mut ticket = attached
                .iter()
                .find(|held| held.id() == *id)
                .expect("the gate proved every named Ticket attached")
                .clone();
            ticket.pin_to(proposal.spec_version()).map_err(refuse)?;
            envelopes.push(ticket_transition(
                project.id(),
                ticket.id(),
                proposal.id(),
                proposal.spec_version(),
            ));
            pinned.push(ticket);
        }
        proposal.approve().map_err(refuse)?;
        self.0
            .proposals
            .apply_approval(&proposal, &pinned, &envelopes)?;
        encode_record(&proposal)
    }
}

/// Serves `ticket.graph.list`.
struct ListGraphProposals {
    proposals: Arc<dyn GraphProposalStore>,
    specs: Arc<dyn SpecStore>,
}

impl QueryHandler for ListGraphProposals {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: TicketGraphListQuery = parse_payload(payload)?;
        let spec = self
            .specs
            .find(SpecId::new(query.spec_id))?
            .ok_or_else(|| ApiError::not_found(&format!("spec {}", query.spec_id)))?;
        let response = TicketGraphListResponse {
            proposals: self
                .proposals
                .list(spec.id())?
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

    use kanban_domain::{
        BlockerDescription, ExternalBlocker, ExternalBlockerId, GraphProposalId,
        GraphProposalState, SpecId, Ticket, TicketDependency, TicketGraphProposal, TicketId,
    };
    use kanban_dto::ApiError;

    use super::GraphProposalStore;
    use crate::dependency::DependencyStore;
    use crate::ticket::testing::{MemoryTickets, TicketHarness, ticket_harness_with_sink};
    use crate::timeline::TimelineEnvelope;

    /// The dependency rows the graph operations join with and install
    /// into: edges and blockers recorded against the Ticket rows the
    /// harness seeded, moved the way the durable store moves them.
    #[derive(Default)]
    pub(crate) struct MemoryGraphDependencies {
        state: Mutex<MemoryGraphDependencyState>,
        tickets: Arc<MemoryTickets>,
    }

    #[derive(Default)]
    struct MemoryGraphDependencyState {
        edges: Vec<TicketDependency>,
        blockers: Vec<ExternalBlocker>,
        next_blocker_id: u64,
        timeline: Vec<TimelineEnvelope>,
    }

    impl MemoryGraphDependencies {
        /// A dependency store sharing the Ticket rows the harness
        /// seeded.
        pub(crate) fn sharing(tickets: Arc<MemoryTickets>) -> Self {
            Self {
                tickets,
                ..Self::default()
            }
        }

        /// Seed one registered edge as-is, standing in for an operator
        /// registration the proposal joins with.
        pub(crate) fn seed_edge(&self, edge: TicketDependency) {
            self.state
                .lock()
                .expect("the memory dependency lock is sound")
                .edges
                .push(edge);
        }

        /// The registered edges and timeline envelopes, for assertions.
        pub(crate) fn snapshot(&self) -> (Vec<TicketDependency>, Vec<TimelineEnvelope>) {
            let state = self
                .state
                .lock()
                .expect("the memory dependency lock is sound");
            (state.edges.clone(), state.timeline.clone())
        }

        /// The waiting Ticket as one applied change leaves it:
        /// identical, with its aggregate version moved forward by one.
        fn moved(waiting: &Ticket) -> Ticket {
            Ticket::restore(
                waiting.id(),
                waiting.project(),
                waiting.number(),
                waiting.priority(),
                waiting.state(),
                waiting.body().clone(),
                waiting.predecessor(),
                waiting.profile().cloned(),
                waiting.pinned_version(),
                waiting.version() + 1,
            )
        }
    }

    impl DependencyStore for MemoryGraphDependencies {
        fn add_dependency(
            &self,
            waiting: &Ticket,
            edge: TicketDependency,
            envelope: &dyn Fn() -> TimelineEnvelope,
        ) -> Result<Ticket, ApiError> {
            let mut state = self
                .state
                .lock()
                .expect("the memory dependency lock is sound");
            state.edges.push(edge);
            state.timeline.push(envelope());
            drop(state);
            let moved = Self::moved(waiting);
            self.tickets.replace_pinned(moved.clone())?;
            Ok(moved)
        }

        fn remove_dependency(
            &self,
            waiting: &Ticket,
            edge: TicketDependency,
            envelope: &dyn Fn() -> TimelineEnvelope,
        ) -> Result<Ticket, ApiError> {
            let mut state = self
                .state
                .lock()
                .expect("the memory dependency lock is sound");
            state.edges.retain(|held| *held != edge);
            state.timeline.push(envelope());
            drop(state);
            let moved = Self::moved(waiting);
            self.tickets.replace_pinned(moved.clone())?;
            Ok(moved)
        }

        fn add_blocker(
            &self,
            waiting: &Ticket,
            description: &BlockerDescription,
            envelope: &dyn Fn(ExternalBlockerId) -> TimelineEnvelope,
        ) -> Result<(Ticket, ExternalBlocker), ApiError> {
            let mut state = self
                .state
                .lock()
                .expect("the memory dependency lock is sound");
            state.next_blocker_id += 1;
            let blocker = ExternalBlocker::restore(
                ExternalBlockerId::new(state.next_blocker_id),
                waiting.id(),
                description.clone(),
            );
            state.blockers.push(blocker.clone());
            state.timeline.push(envelope(blocker.id()));
            drop(state);
            let moved = Self::moved(waiting);
            self.tickets.replace_pinned(moved.clone())?;
            Ok((moved, blocker))
        }

        fn remove_blocker(
            &self,
            waiting: &Ticket,
            blocker: ExternalBlocker,
            envelope: &dyn Fn() -> TimelineEnvelope,
        ) -> Result<Ticket, ApiError> {
            let mut state = self
                .state
                .lock()
                .expect("the memory dependency lock is sound");
            state.blockers.retain(|held| held.id() != blocker.id());
            state.timeline.push(envelope());
            drop(state);
            let moved = Self::moved(waiting);
            self.tickets.replace_pinned(moved.clone())?;
            Ok(moved)
        }

        fn list_dependencies(&self) -> Result<Vec<TicketDependency>, ApiError> {
            Ok(self
                .state
                .lock()
                .expect("the memory dependency lock is sound")
                .edges
                .clone())
        }

        fn blockers_of(&self, ticket: TicketId) -> Result<Vec<ExternalBlocker>, ApiError> {
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

    /// An in-memory proposal store: rows by id, the pinned Ticket rows
    /// its approvals moved, and the timeline envelopes it was asked to
    /// land.
    #[derive(Default)]
    pub(crate) struct MemoryGraphProposals {
        state: Mutex<MemoryGraphState>,
        tickets: Arc<MemoryTickets>,
        dependencies: Arc<MemoryGraphDependencies>,
    }

    #[derive(Default)]
    struct MemoryGraphState {
        proposals: Vec<TicketGraphProposal>,
        next_id: u64,
        timeline: Vec<TimelineEnvelope>,
    }

    impl MemoryGraphProposals {
        /// A proposal store sharing the Ticket and dependency rows the
        /// harness seeded.
        pub(crate) fn sharing(
            tickets: Arc<MemoryTickets>,
            dependencies: Arc<MemoryGraphDependencies>,
        ) -> Self {
            Self {
                tickets,
                dependencies,
                ..Self::default()
            }
        }

        /// The dependency rows the proposals join with and install
        /// into, for seeding and assertions.
        pub(crate) fn dependencies(&self) -> &Arc<MemoryGraphDependencies> {
            &self.dependencies
        }

        /// The stored rows and timeline envelopes, for assertions.
        pub(crate) fn snapshot(&self) -> (Vec<TicketGraphProposal>, Vec<TimelineEnvelope>) {
            let state = self
                .state
                .lock()
                .expect("the memory proposal lock is sound");
            (state.proposals.clone(), state.timeline.clone())
        }
    }

    impl GraphProposalStore for MemoryGraphProposals {
        fn create(
            &self,
            spec: SpecId,
            spec_version: u64,
            tickets: Vec<TicketId>,
            edges: Vec<TicketDependency>,
            envelope: &dyn Fn(GraphProposalId) -> TimelineEnvelope,
        ) -> Result<TicketGraphProposal, ApiError> {
            let mut state = self
                .state
                .lock()
                .expect("the memory proposal lock is sound");
            state.next_id += 1;
            let proposal = TicketGraphProposal::restore(
                GraphProposalId::new(state.next_id),
                spec,
                spec_version,
                tickets,
                edges,
                GraphProposalState::Proposed,
                1,
            );
            state.proposals.push(proposal.clone());
            state.timeline.push(envelope(proposal.id()));
            Ok(proposal)
        }

        fn find(&self, id: GraphProposalId) -> Result<Option<TicketGraphProposal>, ApiError> {
            Ok(self
                .state
                .lock()
                .expect("the memory proposal lock is sound")
                .proposals
                .iter()
                .find(|row| row.id() == id)
                .cloned())
        }

        fn list(&self, spec: SpecId) -> Result<Vec<TicketGraphProposal>, ApiError> {
            Ok(self
                .state
                .lock()
                .expect("the memory proposal lock is sound")
                .proposals
                .iter()
                .filter(|row| row.spec() == spec)
                .cloned()
                .collect())
        }

        fn apply_approval(
            &self,
            proposal: &TicketGraphProposal,
            pinned: &[kanban_domain::Ticket],
            envelopes: &[TimelineEnvelope],
        ) -> Result<(), ApiError> {
            // Stage every guarded write against the stored rows first
            // and commit only when each guard holds, the way the
            // durable store's one transaction does: a mid-write
            // refusal moves no row at all.
            let mut state = self
                .state
                .lock()
                .expect("the memory proposal lock is sound");
            let preceding = proposal.version() - 1;
            let held = state
                .proposals
                .iter()
                .find(|row| row.id() == proposal.id())
                .expect("the approval names a stored proposal");
            if held.version() != preceding {
                return Err(ApiError::stale_version(preceding, held.version()));
            }
            let (rows, _) = self.tickets.snapshot();
            for ticket in pinned {
                let stored = rows
                    .iter()
                    .find(|row| row.id() == ticket.id())
                    .ok_or_else(|| ApiError::not_found(&format!("ticket {}", ticket.id())))?;
                let ticket_preceding = ticket.version() - 1;
                if stored.version() != ticket_preceding {
                    return Err(ApiError::stale_version(ticket_preceding, stored.version()));
                }
            }
            *state
                .proposals
                .iter_mut()
                .find(|row| row.id() == proposal.id())
                .expect("the approval names a stored proposal") = proposal.clone();
            drop(state);
            for ticket in pinned {
                self.tickets
                    .replace_pinned(ticket.clone())
                    .expect("the staged guard proved the row ready");
            }
            self.state
                .lock()
                .expect("the memory proposal lock is sound")
                .timeline
                .extend(envelopes.iter().cloned());
            Ok(())
        }
    }

    /// The ticket harness with the graph proposal operations wired to
    /// an in-memory proposal store sharing its Ticket rows.
    pub(crate) fn graph_harness() -> (TicketHarness, Arc<MemoryGraphProposals>) {
        graph_harness_with_sink(Arc::new(crate::events::NoopEventSink))
    }

    /// The harness the event sink a test chooses.
    pub(crate) fn graph_harness_with_sink(
        events: Arc<dyn crate::events::EventSink>,
    ) -> (TicketHarness, Arc<MemoryGraphProposals>) {
        let mut harness = ticket_harness_with_sink(events);
        let dependencies = Arc::new(MemoryGraphDependencies::sharing(harness.tickets.clone()));
        harness
            .core
            .register_dependencies(
                dependencies.clone(),
                harness.tickets.clone(),
                harness.projects.clone(),
            )
            .expect("the dependency operations register");
        let proposals = Arc::new(MemoryGraphProposals::sharing(
            harness.tickets.clone(),
            dependencies.clone(),
        ));
        let profiles = Arc::new(crate::profile::testing::MemoryProfiles::default());
        harness
            .core
            .register_graph_proposals(
                proposals.clone(),
                dependencies,
                harness.tickets.clone(),
                harness.specs.clone(),
                harness.projects.clone(),
                profiles,
            )
            .expect("the graph operations register");
        (harness, proposals)
    }
}

#[cfg(test)]
mod registered_cycles {
    use kanban_dto::ErrorCode;
    use serde_json::{Value, json};

    use super::graph_approval::{approve, approved_spec, implementation};
    use super::testing::graph_harness;

    /// Two covered Implementation Tickets attached to the Spec, ready
    /// for a proposal that joins the registered dependencies.
    pub(super) fn covered_pair(core: &crate::dispatch::Core, spec: u64) -> (u64, u64) {
        let first = implementation(
            core,
            spec,
            "Graphs record completely",
            json!([
                { "outcome": "Graphs record completely.", "stories": ["CORE-S1-US1"] },
                { "outcome": "Slices stay granular.", "stories": ["CORE-S1-US2"] },
            ]),
            "key-ticket-1",
        );
        let second = implementation(
            core,
            spec,
            "Stories stay covered",
            json!([{ "outcome": "Stories stay covered.", "stories": ["CORE-S1-US3"] }]),
            "key-ticket-2",
        );
        (first, second)
    }

    /// One proposal request over `tickets` and `edges`.
    fn propose(spec: u64, tickets: Value, edges: Value, key: &str) -> Value {
        json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "spec_id": spec,
            "spec_version": 1,
            "tickets": tickets,
            "edges": edges,
        })
    }

    #[test]
    fn a_proposal_refuses_an_edge_that_reverses_a_registered_one() {
        let (harness, proposals) = graph_harness();
        let spec = approved_spec(&harness.core);
        let (first, second) = covered_pair(&harness.core, spec);
        // The operator separately registered second → first before
        // the graph was proposed (DR-DE-02).
        proposals.dependencies().seed_edge(edge(second, first));

        let error = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(
                    spec,
                    json!([first, second]),
                    json!([{ "from_ticket": first, "to_ticket": second }]),
                    "key-propose",
                ),
            )
            .expect_err("the proposal reverses a registered edge");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            format!(
                "a Ticket graph edge would close a cycle with the registered dependencies; \
                 the dependency from Ticket {first} to Ticket {second} would close a cycle"
            )
        );
        let (rows, _) = proposals.snapshot();
        assert!(rows.is_empty(), "the refusal recorded no proposal");
    }

    #[test]
    fn a_proposal_accepts_an_edge_the_store_already_holds() {
        let (harness, proposals) = graph_harness();
        let spec = approved_spec(&harness.core);
        let (first, second) = covered_pair(&harness.core, spec);
        proposals.dependencies().seed_edge(edge(first, second));

        let recorded = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(
                    spec,
                    json!([first, second]),
                    json!([{ "from_ticket": first, "to_ticket": second }]),
                    "key-propose",
                ),
            )
            .expect("an edge the store already holds joins nothing new");

        assert_eq!(recorded["state"], json!("proposed"));
    }

    #[test]
    fn a_longer_registered_cycle_refuses_the_proposal() {
        let (harness, proposals) = graph_harness();
        let spec = approved_spec(&harness.core);
        let (first, second) = covered_pair(&harness.core, spec);
        // A standing Ticket outside the graph carries the chain
        // second → outside → first across the boundary, the way a
        // cross-Spec or cross-Project edge would (DR-DE-02).
        let standing = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-standing" },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "normal",
                    "title": "Landing drops the integration branch",
                    "actual_behaviour": "The integration branch is dropped after a review lands.",
                    "reporter_evidence": "The landing log names the drop immediately after the merge.",
                }),
            )
            .expect("the standing Bug quick captures");
        let standing = standing["id"].as_u64().expect("the identity is a number");
        proposals.dependencies().seed_edge(edge(second, standing));
        proposals.dependencies().seed_edge(edge(standing, first));

        let error = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(
                    spec,
                    json!([first, second]),
                    json!([{ "from_ticket": first, "to_ticket": second }]),
                    "key-propose",
                ),
            )
            .expect_err("the chain closes through the standing Ticket");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            format!(
                "a Ticket graph edge would close a cycle with the registered dependencies; \
                 the dependency from Ticket {first} to Ticket {second} would close a cycle"
            )
        );
    }

    #[test]
    fn approval_rechecks_the_registered_dependencies_at_the_gate() {
        let (harness, proposals) = graph_harness();
        let spec = approved_spec(&harness.core);
        let (first, second) = covered_pair(&harness.core, spec);
        // The proposal records against a clean store.
        let recorded = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(
                    spec,
                    json!([first, second]),
                    json!([{ "from_ticket": first, "to_ticket": second }]),
                    "key-propose",
                ),
            )
            .expect("the graph records");
        let proposal = recorded["id"].as_u64().expect("the identity is a number");

        // The operator then registers the opposite edge before the
        // human gate decides; only the gate's recheck catches it.
        harness
            .core
            .command(
                "ticket.dependency.add",
                &json!({
                    "mutation": { "optimistic_version": 1, "idempotency_key": "key-reverse" },
                    "from_ticket": second,
                    "to_ticket": first,
                }),
            )
            .expect("the opposite edge registers against the unpinned Tickets");

        let error = harness
            .core
            .command("ticket.graph.approve", &approve(proposal, 1, "key-gate"))
            .expect_err("the gate joins the edges registered after recording");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            format!(
                "the Ticket graph is not acyclic against the registered dependencies; \
                 the dependency from Ticket {first} to Ticket {second} would close a cycle"
            )
        );
        let (rows, _) = proposals.snapshot();
        let refused = rows
            .iter()
            .find(|row| row.id().value() == proposal)
            .expect("the proposal stands");
        assert!(
            matches!(refused.state(), kanban_domain::GraphProposalState::Proposed),
            "the refusal approved nothing"
        );
        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": first }))
            .expect("the get serves");
        assert_eq!(
            read["pinned_spec_version"],
            json!(null),
            "the refusal pinned nothing"
        );
        let (edges, _) = proposals.dependencies().snapshot();
        assert_eq!(
            edges,
            vec![edge(second, first)],
            "the refusal installed nothing"
        );
    }

    /// One dependency edge between two Ticket identities.
    fn edge(from: u64, to: u64) -> kanban_domain::TicketDependency {
        kanban_domain::TicketDependency::new(
            kanban_domain::TicketId::new(from),
            kanban_domain::TicketId::new(to),
        )
    }
}

#[cfg(test)]
mod graph_proposal_recording {
    use serde_json::{Value, json};

    use super::testing::graph_harness;

    /// The PRD wire content with a story section naming the three
    /// stories the gate's fixtures claim.
    pub(super) fn graph_content(name: &str) -> Value {
        json!({
            "name": name,
            "short_description": "Versioned Plan graphs of Specs",
            "problem_statement": "Planning must survive change without losing truth.",
            "solution": "Enforced story coverage.",
            "user_stories": "\
- CORE-S1-US1: As an operator, I want complete graphs.
- CORE-S1-US2: As an operator, I want granular slices.
- CORE-S1-US3: As an operator, I want covered stories.
",
            "implementation_decisions": "The gate is consumed by graph approval.",
            "testing_decisions": "Application tests prove the gate refuses gaps.",
            "out_of_scope": "Dispatch of ready tickets.",
            "further_notes": "None",
        })
    }

    /// Author one Spec with the graph story section and approve its
    /// version one, returning the identity.
    pub(super) fn approved_spec(core: &crate::dispatch::Core) -> u64 {
        let created = core
            .command(
                "spec.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-author" },
                    "project_id": 1,
                    "content": graph_content("Registration"),
                }),
            )
            .expect("the Spec authors");
        let id = created["id"].as_u64().expect("the identity is a number");
        core.command(
            "spec.version.approve",
            &json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "key-approve" },
                "spec_id": id,
            }),
        )
        .expect("the draft approves");
        id
    }

    /// Create one Implementation Ticket attached to `spec` claiming
    /// `stories`, returning its identity.
    pub(super) fn implementation(
        core: &crate::dispatch::Core,
        spec: u64,
        slice: &str,
        stories: Value,
        key: &str,
    ) -> u64 {
        let created = core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": key },
                    "project_id": 1,
                    "kind": "implementation",
                    "priority": "normal",
                    "spec_id": spec,
                    "slice": slice,
                    "criteria": stories,
                }),
            )
            .expect("the Ticket creates");
        created["id"].as_u64().expect("the identity is a number")
    }

    /// One proposal request over `tickets` and `edges`.
    fn propose(spec: u64, tickets: Value, edges: Value, key: &str) -> Value {
        json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "spec_id": spec,
            "spec_version": 1,
            "tickets": tickets,
            "edges": edges,
        })
    }

    #[test]
    fn an_approved_spec_receives_a_proposal_against_its_version() {
        let (harness, _proposals) = graph_harness();
        let spec = approved_spec(&harness.core);
        let first = implementation(
            &harness.core,
            spec,
            "Graphs record completely",
            json!([{ "outcome": "Graphs record completely.", "stories": ["CORE-S1-US1"] }]),
            "key-ticket-1",
        );
        let second = implementation(
            &harness.core,
            spec,
            "Slices stay granular",
            json!([
                { "outcome": "Slices stay granular.", "stories": ["CORE-S1-US2"] },
                { "outcome": "Stories stay covered.", "stories": ["CORE-S1-US3"] },
            ]),
            "key-ticket-2",
        );

        let response = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(
                    spec,
                    json!([first, second]),
                    json!([{ "from_ticket": first, "to_ticket": second }]),
                    "key-propose",
                ),
            )
            .expect("the graph records");

        assert_eq!(
            response,
            json!({
                "id": 1,
                "spec_id": spec,
                "spec_version": 1,
                "state": "proposed",
                "tickets": [first, second],
                "edges": [{ "from_ticket": first, "to_ticket": second }],
                "version": 1,
            }),
            "the proposal is recorded against the approved version (DR-PS-16)"
        );

        let listed = harness
            .core
            .query("ticket.graph.list", &json!({ "spec_id": spec }))
            .expect("the list serves");
        assert_eq!(listed["proposals"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn a_draft_or_superseded_version_receives_no_graph() {
        let (harness, _) = graph_harness();
        let created = harness
            .core
            .command(
                "spec.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-author" },
                    "project_id": 1,
                    "content": graph_content("Registration"),
                }),
            )
            .expect("the Spec authors");
        let spec = created["id"].as_u64().expect("the identity is a number");

        let error = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(spec, json!([1]), json!([]), "key-draft"),
            )
            .expect_err("a draft version receives no Ticket graph");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "only an approved Spec version receives a Ticket graph"
        );
    }

    #[test]
    fn recording_refuses_unattached_and_foreign_tickets_and_edges() {
        let (harness, _) = graph_harness();
        let spec = approved_spec(&harness.core);
        let attached = implementation(
            &harness.core,
            spec,
            "Graphs record completely",
            json!([{ "outcome": "Graphs record completely.", "stories": ["CORE-S1-US1"] }]),
            "key-ticket-1",
        );
        let standing = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-standing" },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "normal",
                    "title": "Landing drops the integration branch",
                    "actual_behaviour": "The integration branch is dropped after a review lands.",
                    "reporter_evidence": "The landing log names the drop immediately after the merge.",
                }),
            )
            .expect("the standing Bug creates");
        let standing = standing["id"].as_u64().expect("the identity is a number");

        let detached = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(spec, json!([attached, standing]), json!([]), "key-detached"),
            )
            .expect_err("a Ticket of no attachment to the Spec is refused");
        assert_eq!(detached.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(detached.message, "the Ticket is not attached to the Spec");

        let outside = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(
                    spec,
                    json!([attached]),
                    json!([{ "from_ticket": attached, "to_ticket": standing }]),
                    "key-outside",
                ),
            )
            .expect_err("an edge naming an outside Ticket is refused");
        assert_eq!(outside.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            outside.message,
            "a Ticket graph edge runs between the Tickets it holds; 2 is outside the graph"
        );

        let unknown = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(spec, json!([99]), json!([]), "key-unknown"),
            )
            .expect_err("an unknown Ticket is refused");
        assert_eq!(unknown.code, kanban_dto::ErrorCode::NotFound);

        let empty = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(spec, json!([]), json!([]), "key-empty"),
            )
            .expect_err("a graph of no Tickets is refused");
        assert_eq!(empty.message, "a Ticket graph names at least one Ticket");
    }

    #[test]
    fn a_retry_replays_without_reapplying() {
        let (harness, proposals) = graph_harness();
        let spec = approved_spec(&harness.core);
        let ticket = implementation(
            &harness.core,
            spec,
            "Graphs record completely",
            json!([{ "outcome": "Graphs record completely.", "stories": ["CORE-S1-US1"] }]),
            "key-ticket-1",
        );
        let request = propose(spec, json!([ticket]), json!([]), "key-once");

        let first = harness
            .core
            .command("ticket.graph.propose", &request)
            .expect("the graph records");
        let replay = harness
            .core
            .command("ticket.graph.propose", &request)
            .expect("the retry replays");

        assert_eq!(first, replay);
        let (rows, _) = proposals.snapshot();
        assert_eq!(rows.len(), 1, "the retry must not reapply");
    }

    #[test]
    fn commands_reject_unknown_fields() {
        let (harness, _) = graph_harness();
        let spec = approved_spec(&harness.core);
        let mut request = propose(spec, json!([1]), json!([]), "key-1");
        request["surprise"] = json!(true);

        let error = harness
            .core
            .command("ticket.graph.propose", &request)
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, kanban_dto::ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }
}

#[cfg(test)]
mod graph_approval {
    use serde_json::{Value, json};

    use super::testing::graph_harness;

    /// The PRD wire content with a story section naming the three
    /// stories the gate's fixtures claim.
    pub(super) fn graph_content(name: &str) -> Value {
        json!({
            "name": name,
            "short_description": "Versioned Plan graphs of Specs",
            "problem_statement": "Planning must survive change without losing truth.",
            "solution": "Enforced story coverage.",
            "user_stories": "\
- CORE-S1-US1: As an operator, I want complete graphs.
- CORE-S1-US2: As an operator, I want granular slices.
- CORE-S1-US3: As an operator, I want covered stories.
",
            "implementation_decisions": "The gate is consumed by graph approval.",
            "testing_decisions": "Application tests prove the gate refuses gaps.",
            "out_of_scope": "Dispatch of ready tickets.",
            "further_notes": "None",
        })
    }

    /// Author one Spec with the graph story section and approve its
    /// version one, returning the identity.
    pub(super) fn approved_spec(core: &crate::dispatch::Core) -> u64 {
        let created = core
            .command(
                "spec.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-author" },
                    "project_id": 1,
                    "content": graph_content("Registration"),
                }),
            )
            .expect("the Spec authors");
        let id = created["id"].as_u64().expect("the identity is a number");
        core.command(
            "spec.version.approve",
            &json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "key-approve" },
                "spec_id": id,
            }),
        )
        .expect("the draft approves");
        id
    }

    /// Create one Implementation Ticket attached to `spec` claiming
    /// `stories`, returning its identity.
    pub(super) fn implementation(
        core: &crate::dispatch::Core,
        spec: u64,
        slice: &str,
        stories: Value,
        key: &str,
    ) -> u64 {
        let created = core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": key },
                    "project_id": 1,
                    "kind": "implementation",
                    "priority": "normal",
                    "spec_id": spec,
                    "slice": slice,
                    "criteria": stories,
                }),
            )
            .expect("the Ticket creates");
        created["id"].as_u64().expect("the identity is a number")
    }

    /// One proposal request over `tickets`.
    fn propose(spec: u64, tickets: Value, key: &str) -> Value {
        json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "spec_id": spec,
            "spec_version": 1,
            "tickets": tickets,
            "edges": [],
        })
    }

    /// One approval request for `proposal` at `version`.
    pub(super) fn approve(proposal: u64, version: u64, key: &str) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "proposal_id": proposal,
        })
    }

    /// A Spec whose three stories are covered by two Implementation
    /// Tickets, with the graph proposed and awaiting approval.
    pub(super) fn covered_graph(core: &crate::dispatch::Core) -> (u64, u64, Vec<u64>) {
        let spec = approved_spec(core);
        let first = implementation(
            core,
            spec,
            "Graphs record completely",
            json!([{ "outcome": "Graphs record completely.", "stories": ["CORE-S1-US1"] }]),
            "key-ticket-1",
        );
        let second = implementation(
            core,
            spec,
            "Slices stay granular",
            json!([
                { "outcome": "Slices stay granular.", "stories": ["CORE-S1-US2"] },
                { "outcome": "Stories stay covered.", "stories": ["CORE-S1-US3"] },
            ]),
            "key-ticket-2",
        );
        let proposed = core
            .command(
                "ticket.graph.propose",
                &propose(spec, json!([first, second]), "key-propose"),
            )
            .expect("the graph records");
        let proposal = proposed["id"].as_u64().expect("the identity is a number");
        (spec, proposal, vec![first, second])
    }

    #[test]
    fn approval_pins_every_ticket_to_its_spec_version() {
        let (harness, proposals) = graph_harness();
        let (spec, proposal, tickets) = covered_graph(&harness.core);

        let response = harness
            .core
            .command("ticket.graph.approve", &approve(proposal, 1, "key-gate"))
            .expect("the human gate approves");

        assert_eq!(response["state"], json!("approved"));
        assert_eq!(response["version"], json!(2));
        for ticket in &tickets {
            let read = harness
                .core
                .query("ticket.get", &json!({ "ticket_id": ticket }))
                .expect("the get serves");
            assert_eq!(
                read["pinned_spec_version"],
                json!(1),
                "approval pins every Ticket in the graph (DR-DE-06)"
            );
            assert_eq!(read["spec_id"], json!(spec));
        }

        let (_, timeline) = proposals.snapshot();
        let pinned = timeline
            .iter()
            .find(|row| {
                row.detail()
                    .get("action")
                    .and_then(|action| action.as_str())
                    == Some("pinned")
            })
            .expect("the pin lands on the timeline");
        assert_eq!(
            pinned.detail(),
            &json!({
                "action": "pinned",
                "id": tickets[0],
                "proposal_id": proposal,
                "spec_version": 1,
            })
        );
    }

    #[test]
    fn the_gate_refuses_an_incomplete_uncovered_or_unverifiable_graph() {
        let (harness, _) = graph_harness();
        let spec = approved_spec(&harness.core);
        // The graph holds only the first Ticket; the second stays
        // outside it and the third story stays uncovered.
        let first = implementation(
            &harness.core,
            spec,
            "Graphs record completely",
            json!([{ "outcome": "Graphs record completely.", "stories": ["CORE-S1-US1"] }]),
            "key-ticket-1",
        );
        let _second = implementation(
            &harness.core,
            spec,
            "Slices stay granular",
            json!([{ "outcome": "Slices stay granular.", "stories": ["CORE-S1-US2"] }]),
            "key-ticket-2",
        );
        let proposed = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(spec, json!([first]), "key-propose"),
            )
            .expect("the graph records");
        let proposal = proposed["id"].as_u64().expect("the identity is a number");

        let error = harness
            .core
            .command("ticket.graph.approve", &approve(proposal, 1, "key-gate"))
            .expect_err("an incomplete graph is refused");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the Ticket graph is not complete; Tickets 2 sit outside it"
        );

        let (tickets, _) = harness.tickets.snapshot();
        assert_eq!(
            tickets
                .iter()
                .find(|ticket| ticket.id().value() == first)
                .expect("the Ticket stands")
                .pinned_version(),
            None,
            "the refusal pinned nothing"
        );
    }

    #[test]
    fn the_gate_refuses_an_unqualified_bug_in_the_graph() {
        let (harness, _) = graph_harness();
        let spec = approved_spec(&harness.core);
        let first = implementation(
            &harness.core,
            spec,
            "Graphs record completely",
            json!([
                { "outcome": "Graphs record completely.", "stories": ["CORE-S1-US1"] },
                { "outcome": "Slices stay granular.", "stories": ["CORE-S1-US2"] },
            ]),
            "key-ticket-1",
        );
        let bug = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-bug" },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "normal",
                    "spec_id": spec,
                    "title": "Landing drops the integration branch",
                    "actual_behaviour": "The integration branch is dropped after a review lands.",
                    "reporter_evidence": "The landing log names the drop immediately after the merge.",
                }),
            )
            .expect("the Bug quick captures");
        let bug = bug["id"].as_u64().expect("the identity is a number");

        let proposed = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(spec, json!([first, bug]), "key-propose"),
            )
            .expect("the graph records");
        let proposal = proposed["id"].as_u64().expect("the identity is a number");

        let error = harness
            .core
            .command("ticket.graph.approve", &approve(proposal, 1, "key-gate"))
            .expect_err("an unqualified Bug makes the graph unverifiable");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the Ticket graph is not verifiable; Bug 2 is not yet qualified"
        );
    }

    #[test]
    fn a_superseded_version_or_repeat_approval_is_refused() {
        let (harness, _) = graph_harness();
        let (spec, proposal, tickets) = covered_graph(&harness.core);
        // A material change past approval mints a draft and the
        // operator supersedes the version the graph proposed against.
        harness
            .core
            .command(
                "spec.version.supersede",
                &json!({
                    "mutation": { "optimistic_version": 2, "idempotency_key": "key-supersede" },
                    "spec_id": spec,
                    "version": 1,
                }),
            )
            .expect("the approved version supersedes");

        let error = harness
            .core
            .command("ticket.graph.approve", &approve(proposal, 1, "key-gate"))
            .expect_err("a superseded version approves no graph");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "only an approved Spec version receives a Ticket graph"
        );

        // A material change past the supersession mints a fresh draft,
        // which approves into version two.
        harness
            .core
            .command(
                "spec.content.update",
                &json!({
                    "mutation": { "optimistic_version": 3, "idempotency_key": "key-revise" },
                    "spec_id": spec,
                    "content": graph_content("Registration again"),
                }),
            )
            .expect("the material change mints a draft");
        harness
            .core
            .command(
                "spec.version.approve",
                &json!({
                    "mutation": { "optimistic_version": 4, "idempotency_key": "key-reapprove" },
                    "spec_id": spec,
                }),
            )
            .expect("version two approves");

        let replacement = harness
            .core
            .command(
                "ticket.graph.propose",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-repropose" },
                    "spec_id": spec,
                    "spec_version": 2,
                    "tickets": tickets,
                    "edges": [],
                }),
            )
            .expect("the graph records against version two");
        let second = replacement["id"]
            .as_u64()
            .expect("the identity is a number");
        // The rival graph records before the second one approves:
        // after that approval its members are pinned to version two,
        // and a later graph may no longer name them.
        let again = harness
            .core
            .command(
                "ticket.graph.propose",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-third" },
                    "spec_id": spec,
                    "spec_version": 2,
                    "tickets": tickets,
                    "edges": [],
                }),
            )
            .expect("a third graph records");
        let third = again["id"].as_u64().expect("the identity is a number");
        harness
            .core
            .command(
                "ticket.graph.approve",
                &json!({
                    "mutation": { "optimistic_version": 1, "idempotency_key": "key-second" },
                    "proposal_id": second,
                }),
            )
            .expect("the second graph approves");
        let error = harness
            .core
            .command(
                "ticket.graph.approve",
                &json!({
                    "mutation": { "optimistic_version": 1, "idempotency_key": "key-gate-third" },
                    "proposal_id": third,
                }),
            )
            .expect_err("the version already carries an approved graph");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the Spec version already carries an approved Ticket graph"
        );
    }

    #[test]
    fn a_stale_or_replayed_approval_is_refused_or_replayed() {
        let (harness, _) = graph_harness();
        let (_spec, proposal, _tickets) = covered_graph(&harness.core);

        let stale = harness
            .core
            .command(
                "ticket.graph.approve",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-stale" },
                    "proposal_id": proposal,
                }),
            )
            .expect_err("the stale version is refused");
        assert_eq!(stale.code, kanban_dto::ErrorCode::StaleVersion);

        let request = approve(proposal, 1, "key-once");
        let first = harness
            .core
            .command("ticket.graph.approve", &request)
            .expect("the gate approves");
        let replay = harness
            .core
            .command("ticket.graph.approve", &request)
            .expect("the retry replays");
        assert_eq!(first, replay);
    }

    #[test]
    fn an_unknown_proposal_is_not_found() {
        let (harness, _) = graph_harness();

        let error = harness
            .core
            .command("ticket.graph.approve", &approve(9, 1, "key-unknown"))
            .expect_err("an unknown proposal is refused");
        assert_eq!(error.code, kanban_dto::ErrorCode::NotFound);
    }
}

#[cfg(test)]
mod later_versions {
    use kanban_domain::{GraphProposalId, Ticket, TicketId, TicketState};
    use serde_json::{Value, json};

    use super::GraphProposalStore;
    use super::graph_approval::{approve, covered_graph, graph_content, implementation};
    use super::testing::graph_harness;
    use crate::ticket::TicketStore;
    use crate::ticket::testing::TicketHarness;

    /// Rewrite one stored Ticket row the way the lifecycle slice
    /// moves it, so a test can stand a member in a state the graph
    /// commands refuse to reach themselves.
    fn force_state(harness: &TicketHarness, id: u64, state: TicketState) {
        let standing = harness
            .tickets
            .find(TicketId::new(id))
            .expect("the find serves")
            .expect("the Ticket stands");
        let moved = Ticket::restore(
            standing.id(),
            standing.project(),
            standing.number(),
            standing.priority(),
            state,
            standing.body().clone(),
            standing.predecessor(),
            standing.profile().cloned(),
            standing.pinned_version(),
            standing.version() + 1,
        );
        harness
            .tickets
            .replace_pinned(moved)
            .expect("the row moves");
    }

    /// Rewrite one stored Ticket row to reference the predecessor a
    /// reassignment created it from (DR-DE-07), the way the
    /// reassignment command writes the reference.
    fn carry_predecessor(harness: &TicketHarness, id: u64, predecessor: u64) {
        let standing = harness
            .tickets
            .find(TicketId::new(id))
            .expect("the find serves")
            .expect("the Ticket stands");
        let moved = Ticket::restore(
            standing.id(),
            standing.project(),
            standing.number(),
            standing.priority(),
            standing.state(),
            standing.body().clone(),
            Some(TicketId::new(predecessor)),
            standing.profile().cloned(),
            standing.pinned_version(),
            standing.version() + 1,
        );
        harness
            .tickets
            .replace_pinned(moved)
            .expect("the row moves");
    }

    /// Supersede the Spec's approved version one and approve a second
    /// version of the same stories: the later graph's Spec state.
    fn second_version(core: &crate::dispatch::Core, spec: u64) {
        core.command(
            "spec.version.supersede",
            &json!({
                "mutation": { "optimistic_version": 2, "idempotency_key": "key2-supersede" },
                "spec_id": spec,
                "version": 1,
            }),
        )
        .expect("the approved version supersedes");
        core.command(
            "spec.content.update",
            &json!({
                "mutation": { "optimistic_version": 3, "idempotency_key": "key2-revise" },
                "spec_id": spec,
                "content": graph_content("Registration again"),
            }),
        )
        .expect("the material change mints a draft");
        core.command(
            "spec.version.approve",
            &json!({
                "mutation": { "optimistic_version": 4, "idempotency_key": "key2-approve" },
                "spec_id": spec,
            }),
        )
        .expect("the second version approves");
    }

    /// One proposal request over `tickets`, against the Spec
    /// `version` a test chooses.
    fn propose(spec: u64, version: u64, tickets: Value, key: &str) -> Value {
        json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "spec_id": spec,
            "spec_version": version,
            "tickets": tickets,
            "edges": [],
        })
    }

    /// The stored row of one Ticket, by identity.
    fn stored_ticket(harness: &TicketHarness, id: u64) -> Option<Ticket> {
        harness
            .tickets
            .find(TicketId::new(id))
            .expect("the find serves")
    }

    #[test]
    fn a_second_version_approves_active_unpinned_members_alone() {
        let (harness, _proposals) = graph_harness();
        let (spec, first, earlier) = covered_graph(&harness.core);
        harness
            .core
            .command("ticket.graph.approve", &approve(first, 1, "key2-first"))
            .expect("the first graph approves");
        second_version(&harness.core, spec);

        // The changed work of the second version: fresh active
        // unpinned Tickets claiming the same stories.
        let third = implementation(
            &harness.core,
            spec,
            "Graphs record completely again",
            json!([
                { "outcome": "Graphs record completely.", "stories": ["CORE-S1-US1"] },
                { "outcome": "Slices stay granular.", "stories": ["CORE-S1-US2"] },
            ]),
            "key2-ticket-3",
        );
        let fourth = implementation(
            &harness.core,
            spec,
            "Stories stay covered again",
            json!([{ "outcome": "Stories stay covered.", "stories": ["CORE-S1-US3"] }]),
            "key2-ticket-4",
        );

        let proposed = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(spec, 2, json!([third, fourth]), "key2-propose"),
            )
            .expect("the second-version graph records");
        let second = proposed["id"].as_u64().expect("the identity is a number");
        let response = harness
            .core
            .command(
                "ticket.graph.approve",
                &json!({
                    "mutation": { "optimistic_version": 1, "idempotency_key": "key2-gate" },
                    "proposal_id": second,
                }),
            )
            .expect("the second graph approves over active unpinned members");

        assert_eq!(response["state"], json!("approved"));
        for id in [third, fourth] {
            let read = harness
                .core
                .query("ticket.get", &json!({ "ticket_id": id }))
                .expect("the get serves");
            assert_eq!(read["pinned_spec_version"], json!(2));
        }
        for id in earlier {
            let read = harness
                .core
                .query("ticket.get", &json!({ "ticket_id": id }))
                .expect("the get serves");
            assert_eq!(
                read["pinned_spec_version"],
                json!(1),
                "the earlier version's pins are never rewritten"
            );
        }
    }

    #[test]
    fn recording_refuses_terminal_and_pinned_members() {
        let (harness, proposals) = graph_harness();
        let (spec, first, earlier) = covered_graph(&harness.core);
        harness
            .core
            .command("ticket.graph.approve", &approve(first, 1, "key2-first"))
            .expect("the first graph approves");
        second_version(&harness.core, spec);
        let cancelled = implementation(
            &harness.core,
            spec,
            "Graphs record completely again",
            json!([{ "outcome": "Graphs record completely.", "stories": ["CORE-S1-US1"] }]),
            "key2-ticket-3",
        );
        force_state(&harness, cancelled, TicketState::Cancelled);

        let pinned = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(spec, 2, json!([earlier[0]]), "key2-pinned"),
            )
            .expect_err("a member already pinned to the earlier version is refused");
        assert_eq!(pinned.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            pinned.message,
            "the Ticket graph names Ticket 1, already pinned to Spec version 1; \
             a pin is never rewritten or inherited"
        );

        let terminal = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(spec, 2, json!([cancelled]), "key2-terminal"),
            )
            .expect_err("a cancelled member is refused");
        assert_eq!(terminal.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            terminal.message,
            "the Ticket graph names Ticket 3, which is cancelled or superseded; \
             a terminal Ticket stays history, never an executable member of a new graph"
        );

        let (rows, _) = proposals.snapshot();
        assert_eq!(
            rows.len(),
            1,
            "the refusals recorded no proposal; only the first graph stands"
        );
    }

    #[test]
    fn an_eligible_attachment_left_outside_stays_incomplete() {
        let (harness, _) = graph_harness();
        let (spec, first, _earlier) = covered_graph(&harness.core);
        harness
            .core
            .command("ticket.graph.approve", &approve(first, 1, "key2-first"))
            .expect("the first graph approves");
        second_version(&harness.core, spec);
        let third = implementation(
            &harness.core,
            spec,
            "Graphs record completely again",
            json!([
                { "outcome": "Graphs record completely.", "stories": ["CORE-S1-US1"] },
                { "outcome": "Slices stay granular.", "stories": ["CORE-S1-US2"] },
            ]),
            "key2-ticket-3",
        );
        let fourth = implementation(
            &harness.core,
            spec,
            "Stories stay covered again",
            json!([{ "outcome": "Stories stay covered.", "stories": ["CORE-S1-US3"] }]),
            "key2-ticket-4",
        );

        // The graph names the eligible third Ticket and leaves the
        // eligible fourth outside it.
        let proposed = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(spec, 2, json!([third]), "key2-propose"),
            )
            .expect("the graph records");
        let second = proposed["id"].as_u64().expect("the identity is a number");
        let error = harness
            .core
            .command("ticket.graph.approve", &approve(second, 1, "key2-gate"))
            .expect_err("an eligible attachment never silently drops completeness");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the Ticket graph is not complete; Tickets 4 sit outside it"
        );
        assert_eq!(
            stored_ticket(&harness, fourth)
                .expect("the Ticket stands")
                .pinned_version(),
            None,
            "the refusal pinned nothing"
        );
    }

    #[test]
    fn a_replacement_enters_the_new_graph_alone() {
        let (harness, _) = graph_harness();
        let (spec, first, earlier) = covered_graph(&harness.core);
        harness
            .core
            .command("ticket.graph.approve", &approve(first, 1, "key2-first"))
            .expect("the first graph approves");
        second_version(&harness.core, spec);
        // Reassignment replaces the first slice: the original turns
        // superseded and the replacement references it (DR-DE-07).
        let replacement = implementation(
            &harness.core,
            spec,
            "Graphs record completely anew",
            json!([
                { "outcome": "Graphs record completely.", "stories": ["CORE-S1-US1"] },
                { "outcome": "Slices stay granular.", "stories": ["CORE-S1-US2"] },
            ]),
            "key2-replacement",
        );
        carry_predecessor(&harness, replacement, earlier[0]);
        force_state(&harness, earlier[0], TicketState::Superseded);
        let fresh = implementation(
            &harness.core,
            spec,
            "Stories stay covered again",
            json!([{ "outcome": "Stories stay covered.", "stories": ["CORE-S1-US3"] }]),
            "key2-ticket-4",
        );

        let proposed = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(spec, 2, json!([replacement, fresh]), "key2-propose"),
            )
            .expect("the replacement's graph records");
        let second = proposed["id"].as_u64().expect("the identity is a number");
        harness
            .core
            .command("ticket.graph.approve", &approve(second, 1, "key2-gate"))
            .expect("the replacement and the fresh slice approve");

        let superseded = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": earlier[0] }))
            .expect("the get serves");
        assert_eq!(superseded["state"], json!("superseded"));
        assert_eq!(
            superseded["spec_id"],
            json!(spec),
            "the superseded original stays visible as attached history"
        );
        assert_eq!(
            superseded["pinned_spec_version"],
            json!(1),
            "the old pin is never inherited"
        );
        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": replacement }))
            .expect("the get serves");
        assert_eq!(
            read["pinned_spec_version"],
            json!(2),
            "the replacement earns its own pin, never the old one"
        );
        assert_eq!(read["predecessor_id"], json!(earlier[0]));
    }

    #[test]
    fn a_cancelled_member_fails_the_approval_with_a_named_error() {
        let (harness, proposals) = graph_harness();
        let (spec, first, _earlier) = covered_graph(&harness.core);
        harness
            .core
            .command("ticket.graph.approve", &approve(first, 1, "key2-first"))
            .expect("the first graph approves");
        second_version(&harness.core, spec);
        let third = implementation(
            &harness.core,
            spec,
            "Graphs record completely again",
            json!([
                { "outcome": "Graphs record completely.", "stories": ["CORE-S1-US1"] },
                { "outcome": "Slices stay granular.", "stories": ["CORE-S1-US2"] },
            ]),
            "key2-ticket-3",
        );
        let fourth = implementation(
            &harness.core,
            spec,
            "Stories stay covered again",
            json!([{ "outcome": "Stories stay covered.", "stories": ["CORE-S1-US3"] }]),
            "key2-ticket-4",
        );
        let proposed = harness
            .core
            .command(
                "ticket.graph.propose",
                &propose(spec, 2, json!([third, fourth]), "key2-propose"),
            )
            .expect("the graph records");
        let second = proposed["id"].as_u64().expect("the identity is a number");
        // The member cancels between the proposal and the gate.
        force_state(&harness, third, TicketState::Cancelled);
        let (_, timeline_before) = proposals.snapshot();

        let error = harness
            .core
            .command("ticket.graph.approve", &approve(second, 1, "key2-gate"))
            .expect_err("a cancelled member fails the gate by name");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the Ticket graph names Ticket 3, which is cancelled or superseded; \
             a terminal Ticket stays history, never an executable member of a new graph"
        );
        let (rows, timeline) = proposals.snapshot();
        assert_eq!(
            rows.iter()
                .find(|row| row.id().value() == second)
                .expect("the proposal stands")
                .state()
                .wire_name(),
            "proposed",
            "the refusal moved no proposal row"
        );
        for id in [third, fourth] {
            assert_eq!(
                stored_ticket(&harness, id)
                    .expect("the Ticket stands")
                    .pinned_version(),
                None,
                "the refusal pinned nothing"
            );
        }
        assert_eq!(
            timeline.len(),
            timeline_before.len(),
            "the refusal appended no timeline row"
        );
        let history = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": third }))
            .expect("the get serves");
        assert_eq!(history["state"], json!("cancelled"));
        assert_eq!(
            history["spec_id"],
            json!(spec),
            "the cancelled attachment stays visible as history"
        );
    }

    #[test]
    fn a_mid_write_refusal_rolls_the_whole_approval_back() {
        let (harness, proposals) = graph_harness();
        let (_spec, first, earlier) = covered_graph(&harness.core);
        let proposed = proposals
            .find(GraphProposalId::new(first))
            .expect("the find serves")
            .expect("the proposal stands");
        let mut approved = proposed.clone();
        approved.approve().expect("the gate approves");
        let mut pinned = Vec::new();
        for (index, id) in earlier.iter().enumerate() {
            let row = stored_ticket(&harness, *id).expect("the Ticket stands");
            let mut member = if index == 0 {
                row.clone()
            } else {
                // The second aggregate stands one version behind the
                // stored row, so its guarded pin is refused mid-write.
                Ticket::restore(
                    row.id(),
                    row.project(),
                    row.number(),
                    row.priority(),
                    row.state(),
                    row.body().clone(),
                    row.predecessor(),
                    row.profile().cloned(),
                    None,
                    row.version() - 1,
                )
            };
            member.pin_to(1).expect("the approval pins");
            pinned.push(member);
        }
        let (_, timeline_before) = proposals.snapshot();

        let error = proposals
            .apply_approval(&approved, &pinned, &[])
            .expect_err("the mid-write refusal stops the whole approval");

        assert_eq!(error.code, kanban_dto::ErrorCode::StaleVersion);
        let (rows, timeline) = proposals.snapshot();
        assert_eq!(
            rows.iter()
                .find(|row| row.id() == approved.id())
                .expect("the proposal stands")
                .state()
                .wire_name(),
            "proposed",
            "no proposal row remains moved"
        );
        assert_eq!(
            stored_ticket(&harness, earlier[0])
                .expect("the Ticket stands")
                .pinned_version(),
            None,
            "no Ticket pin remains written"
        );
        assert_eq!(timeline.len(), timeline_before.len());
    }
}

#[cfg(test)]
mod pinning {
    use serde_json::{Value, json};

    use crate::ticket::TicketStore;

    use super::graph_approval::{approve, approved_spec, covered_graph, graph_content};
    use super::testing::graph_harness;

    /// One `ticket.spec.move` request at `version`.
    fn moved(ticket: u64, spec: u64, version: u64, key: &str) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "ticket_id": ticket,
            "spec_id": spec,
        })
    }

    #[test]
    fn a_draft_ticket_moves_between_specs_of_its_project() {
        let (harness, _) = graph_harness();
        let _first = approved_spec(&harness.core);
        // A second Spec of the same Project.
        let authored = harness
            .core
            .command(
                "spec.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-author-2" },
                    "project_id": 1,
                    "content": graph_content("Registration again"),
                }),
            )
            .expect("the second Spec authors");
        let destination = authored["id"].as_u64().expect("the identity is a number");

        // A standing Bug moves and attaches by the same command.
        let bug = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-bug" },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "normal",
                    "title": "Landing drops the integration branch",
                    "actual_behaviour": "The integration branch is dropped after a review lands.",
                    "reporter_evidence": "The landing log names the drop immediately after the merge.",
                }),
            )
            .expect("the Bug quick captures");
        let bug = bug["id"].as_u64().expect("the identity is a number");

        let response = harness
            .core
            .command("ticket.spec.move", &moved(bug, destination, 1, "key-move"))
            .expect("a draft, unpinned Ticket moves (DR-DE-05)");

        assert_eq!(response["spec_id"], json!(destination));
        assert_eq!(response["version"], json!(2));
        assert_eq!(response["pinned_spec_version"], json!(null));

        let (_, timeline) = harness.tickets.snapshot();
        let moved_row = timeline
            .iter()
            .find(|row| row.detail().get("action").and_then(|a| a.as_str()) == Some("spec_moved"))
            .expect("the move lands on the timeline");
        assert_eq!(
            moved_row.detail(),
            &json!({
                "action": "spec_moved",
                "id": bug,
                "spec_id": destination,
                "version": 2,
            })
        );
    }

    #[test]
    fn an_implementation_keeps_claiming_the_spec_it_delivers() {
        let (harness, _) = graph_harness();
        let spec = approved_spec(&harness.core);
        let ticket = super::graph_approval::implementation(
            &harness.core,
            spec,
            "Graphs record completely",
            json!([{ "outcome": "Graphs record completely.", "stories": ["CORE-S1-US1"] }]),
            "key-ticket-1",
        );
        let authored = harness
            .core
            .command(
                "spec.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-author-2" },
                    "project_id": 1,
                    "content": graph_content("Registration again"),
                }),
            )
            .expect("the second Spec authors");
        let destination = authored["id"].as_u64().expect("the identity is a number");

        let error = harness
            .core
            .command(
                "ticket.spec.move",
                &moved(ticket, destination, 1, "key-move"),
            )
            .expect_err("the slice's criteria claim another Spec's stories");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "an Implementation Ticket claims the stories of the Spec it delivers; \
             S1-US1 names another Spec"
        );
    }

    #[test]
    fn a_pinned_ticket_stays_with_its_spec_and_version() {
        let (harness, _) = graph_harness();
        let (spec, proposal, tickets) = covered_graph(&harness.core);
        harness
            .core
            .command("ticket.graph.approve", &approve(proposal, 1, "key-gate"))
            .expect("the gate approves");

        // A second Spec of the same Project to move to.
        let authored = harness
            .core
            .command(
                "spec.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-author-2" },
                    "project_id": 1,
                    "content": graph_content("Registration again"),
                }),
            )
            .expect("the second Spec authors");
        let destination = authored["id"].as_u64().expect("the identity is a number");

        let error = harness
            .core
            .command(
                "ticket.spec.move",
                &moved(tickets[0], destination, 2, "key-move"),
            )
            .expect_err("a pinned Ticket stays pinned (DR-DE-06)");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "a pinned Ticket stays with the Spec version it was approved against"
        );
        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": tickets[0] }))
            .expect("the get serves");
        assert_eq!(read["spec_id"], json!(spec));
        assert_eq!(read["pinned_spec_version"], json!(1));
    }

    #[test]
    fn an_executed_ticket_never_moves() {
        let (harness, _) = graph_harness();
        let spec = approved_spec(&harness.core);
        let created = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-bug" },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "normal",
                    "title": "Landing drops the integration branch",
                    "actual_behaviour": "The integration branch is dropped after a review lands.",
                    "reporter_evidence": "The landing log names the drop immediately after the merge.",
                }),
            )
            .expect("the Bug quick captures");
        let bug = created["id"].as_u64().expect("the identity is a number");
        // Execution begins: the row moves past draft the way the
        // lifecycle slice will move it.
        let standing = harness
            .tickets
            .find(kanban_domain::TicketId::new(bug))
            .expect("the find serves")
            .expect("the Ticket stands");
        let executing = kanban_domain::Ticket::restore(
            standing.id(),
            standing.project(),
            standing.number(),
            standing.priority(),
            kanban_domain::TicketState::Active,
            standing.body().clone(),
            standing.predecessor(),
            standing.profile().cloned(),
            standing.pinned_version(),
            standing.version() + 1,
        );
        harness
            .tickets
            .replace_pinned(executing)
            .expect("the row moves");

        let error = harness
            .core
            .command("ticket.spec.move", &moved(bug, spec, 2, "key-move"))
            .expect_err("an executed Ticket never moves");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(error.message, "only a draft Ticket moves between Specs");
    }

    #[test]
    fn a_move_to_another_projects_spec_is_refused() {
        let (harness, _) = graph_harness();
        harness.projects.seed(crate::plan::testing::active_project(
            2,
            "EDGE",
            kanban_domain::ProjectCounters::restore(0, 0, 0),
        ));
        let authored = harness
            .core
            .command(
                "spec.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-edge" },
                    "project_id": 2,
                    "content": graph_content("Elsewhere"),
                }),
            )
            .expect("the EDGE Spec authors");
        let elsewhere = authored["id"].as_u64().expect("the identity is a number");

        let created = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-bug" },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "normal",
                    "title": "Landing drops the integration branch",
                    "actual_behaviour": "The integration branch is dropped after a review lands.",
                    "reporter_evidence": "The landing log names the drop immediately after the merge.",
                }),
            )
            .expect("the Bug quick captures");
        let bug = created["id"].as_u64().expect("the identity is a number");

        let error = harness
            .core
            .command("ticket.spec.move", &moved(bug, elsewhere, 1, "key-move"))
            .expect_err("a Ticket moves inside its own Project only");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(error.message, "the Spec belongs to another Project");
    }
}
