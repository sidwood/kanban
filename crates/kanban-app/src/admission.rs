//! The application half of execution admission (KAN-T138-AC3): load
//! the Ticket, its Project, and its readiness as they stand now and
//! answer the domain rule. One instance serves every ordinary
//! admission path — the claim, the acknowledgement, and the
//! Coordinator loop that drives them — so a second transport or a
//! stale queue record answers the same invariant. The command
//! handlers call it inside their write span, so the state read here
//! is the state the admitted write joins.

use std::sync::Arc;

use kanban_domain::{
    AdmissionInputs, AdmissionRole, DependencyState, GraphProposalState, Readiness,
    ReadinessInputs, Ticket, TicketDependencyGraph, TicketId, admit_execution, compute_readiness,
};
use kanban_dto::ApiError;

use crate::dependency::DependencyStore;
use crate::graph_proposal::GraphProposalStore;
use crate::project::ProjectStore;
use crate::ticket::TicketStore;

/// Answers whether a Dispatch Request's Ticket may execute now.
pub struct ExecutionAdmission {
    tickets: Arc<dyn TicketStore>,
    projects: Arc<dyn ProjectStore>,
    dependencies: Arc<dyn DependencyStore>,
    proposals: Arc<dyn GraphProposalStore>,
}

impl ExecutionAdmission {
    /// Wire the admission over the stores that hold the current
    /// Ticket, Project, dependency, and Ticket graph facts.
    pub fn new(
        tickets: Arc<dyn TicketStore>,
        projects: Arc<dyn ProjectStore>,
        dependencies: Arc<dyn DependencyStore>,
        proposals: Arc<dyn GraphProposalStore>,
    ) -> Self {
        Self {
            tickets,
            projects,
            dependencies,
            proposals,
        }
    }

    /// Admit a run of `role` for `ticket` on current authoritative
    /// state, answering the Ticket as read so the caller reads no
    /// second snapshot. A refusal carries the typed reason as an
    /// invalid-request error and changes nothing.
    pub fn admit(&self, ticket: TicketId, role: AdmissionRole) -> Result<Ticket, ApiError> {
        let ticket = self
            .tickets
            .find(ticket)?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {}", ticket.value())))?;
        let project = self.projects.find(ticket.project())?.ok_or_else(|| {
            ApiError::internal(&format!(
                "ticket {} belongs to no stored Project",
                ticket.id().value()
            ))
        })?;
        let readiness = self.readiness_of(&ticket)?;
        admit_execution(AdmissionInputs {
            ticket: &ticket,
            project_archived: project.is_archived(),
            awaits_graph_approval: self.awaits_graph_approval(&ticket)?,
            readiness: &readiness,
            role,
        })
        .map_err(|refusal| ApiError::invalid_request(&refusal.to_string()))?;
        Ok(ticket)
    }

    /// Whether a Ticket graph proposed for the Ticket's Spec names it
    /// and still waits for the human gate. A pin already answers the
    /// gate, and a Ticket attached to no Spec joins no graph, so
    /// neither reaches the proposal store.
    fn awaits_graph_approval(&self, ticket: &Ticket) -> Result<bool, ApiError> {
        if ticket.pinned_version().is_some() {
            return Ok(false);
        }
        let Some(spec) = ticket.spec() else {
            return Ok(false);
        };
        Ok(self.proposals.list(spec)?.iter().any(|proposal| {
            proposal.state() == &GraphProposalState::Proposed
                && proposal.tickets().contains(&ticket.id())
        }))
    }

    /// The readiness projection KAN-T20 computes, read fresh from the
    /// Ticket's registered dependencies and external blockers.
    pub fn readiness_of(&self, ticket: &Ticket) -> Result<Readiness, ApiError> {
        let graph = TicketDependencyGraph::restore(self.dependencies.list_dependencies()?);
        let mut states = Vec::new();
        for edge in graph.required_by(ticket.id()) {
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
}
