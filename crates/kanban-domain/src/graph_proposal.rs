//! The Ticket graph proposal: the record an agent's complete
//! dependency graph of Tickets for one approved Spec version is held
//! as while a human decides on it, and the approval gate that decides
//! (CONTEXT.md, DR-PS-16, DR-PS-17). A proposal names the Spec
//! version it is proposed against, every Ticket the graph holds, and
//! the dependency edges between them; it never mutates a Ticket on
//! the way in. Approval is the last human gate before execution: the
//! graph must be complete, granular, verifiable, and story-covered
//! (DR-PS-17), and approving pins every Ticket in the graph to the
//! Spec content version it was approved against (DR-DE-06). The
//! story rules themselves — scope extraction, linked criteria, the
//! executable gate — belong to `coverage`; this module owns the
//! record and the approval mechanics around them.

use std::fmt;

use crate::coverage::{AcceptanceCriterion, StoryScope, UserStoryRef};
use crate::dependency::{DependencyError, TicketDependency, TicketDependencyGraph};
use crate::spec::SpecId;
use crate::ticket::{Ticket, TicketId};

/// The identity of one Ticket graph proposal. Assigned once by
/// storage and immutable afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GraphProposalId(u64);

impl GraphProposalId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for GraphProposalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The closed Ticket graph proposal lifecycle (DR-PS-16, DR-PS-17):
/// a proposal is recorded against an approved Spec version, then the
/// human gate approves it or it stands while the graph is repaired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GraphProposalState {
    /// Recorded, awaiting the human approval gate.
    Proposed,
    /// Approved; every Ticket in the graph is pinned to the Spec
    /// content version the proposal named.
    Approved,
}

impl GraphProposalState {
    /// Every state, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Proposed, Self::Approved];

    /// The stored and wire name of this state.
    pub fn wire_name(&self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Approved => "approved",
        }
    }

    /// The state a stored row names, or `None` outside the
    /// vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|state| state.wire_name() == stored)
            .cloned()
    }
}

impl fmt::Display for GraphProposalState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.wire_name())
    }
}

/// Why a Ticket graph proposal was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphProposalError {
    /// A graph of no Tickets covers nothing and executes nothing
    /// (DR-PS-16): the complete graph names at least one Ticket.
    EmptyTicketSet,
    /// The graph named one Ticket twice. The set is a set; a repeat
    /// hides nothing.
    DuplicateTicket {
        /// The Ticket the repeat named.
        ticket: TicketId,
    },
    /// An edge named a Ticket outside the graph. The proposal's edges
    /// run between the Tickets it holds.
    EdgeOutsideSet {
        /// The endpoint that names no Ticket in the graph.
        ticket: TicketId,
    },
    /// An edge broke a dependency rule: a self-edge, a duplicate, or
    /// an edge that would close a cycle (DR-DE-02).
    IllegalEdge {
        /// The refusal the dependency rule reported.
        reason: DependencyError,
    },
    /// Only a proposed graph approaches the approval gate; an
    /// approved one already passed it.
    ApproveRequiresProposed,
}

impl fmt::Display for GraphProposalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTicketSet => {
                write!(f, "a Ticket graph names at least one Ticket")
            }
            Self::DuplicateTicket { ticket } => {
                write!(f, "the graph names Ticket {ticket} twice")
            }
            Self::EdgeOutsideSet { ticket } => write!(
                f,
                "a Ticket graph edge runs between the Tickets it holds; {ticket} is outside the graph"
            ),
            Self::IllegalEdge { reason } => write!(f, "{reason}"),
            Self::ApproveRequiresProposed => {
                write!(f, "only a proposed Ticket graph approaches approval")
            }
        }
    }
}

impl std::error::Error for GraphProposalError {}

/// One proposed Ticket graph (DR-PS-16): the complete dependency
/// graph of Tickets for one Spec version — the Tickets it holds, the
/// edges between them, and the Spec version it is proposed against.
/// The version counts applied changes: recording lands at 1 and
/// approval bumps it, so a stored version is all a caller needs for
/// optimistic checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketGraphProposal {
    id: GraphProposalId,
    spec: SpecId,
    spec_version: u64,
    tickets: Vec<TicketId>,
    edges: Vec<TicketDependency>,
    state: GraphProposalState,
    version: u64,
}

impl TicketGraphProposal {
    /// Record one graph against the Spec version it proposes for,
    /// refusing a Ticket set that names nothing or one Ticket twice,
    /// and every edge a dependency rule refuses or that names a
    /// Ticket outside the set (DR-DE-02).
    pub fn new(
        id: GraphProposalId,
        spec: SpecId,
        spec_version: u64,
        tickets: Vec<TicketId>,
        edges: Vec<TicketDependency>,
    ) -> Result<Self, GraphProposalError> {
        let proposal = Self {
            id,
            spec,
            spec_version,
            tickets,
            edges,
            state: GraphProposalState::Proposed,
            version: 1,
        };
        Self::validate(&proposal.tickets, &proposal.edges)?;
        Ok(proposal)
    }

    /// Rehydrate a stored proposal exactly as it was recorded. Every
    /// stored value passed `validate` on the way in, so a row that
    /// fails here is corruption the caller must hear about, not
    /// silently accept.
    pub fn restore(
        id: GraphProposalId,
        spec: SpecId,
        spec_version: u64,
        tickets: Vec<TicketId>,
        edges: Vec<TicketDependency>,
        state: GraphProposalState,
        version: u64,
    ) -> Self {
        Self {
            id,
            spec,
            spec_version,
            tickets,
            edges,
            state,
            version,
        }
    }

    /// Refuse what the record refuses: an empty or repeated Ticket
    /// set, and every edge outside the set or outside the dependency
    /// rules. Free of the identity, so the application layer can
    /// validate a graph before storage assigns one.
    pub fn validate(
        tickets: &[TicketId],
        edges: &[TicketDependency],
    ) -> Result<(), GraphProposalError> {
        if tickets.is_empty() {
            return Err(GraphProposalError::EmptyTicketSet);
        }
        for (position, ticket) in tickets.iter().enumerate() {
            if tickets[..position].contains(ticket) {
                return Err(GraphProposalError::DuplicateTicket { ticket: *ticket });
            }
        }
        let mut graph = TicketDependencyGraph::new();
        for edge in edges {
            for endpoint in [edge.from(), edge.to()] {
                if !tickets.contains(&endpoint) {
                    return Err(GraphProposalError::EdgeOutsideSet { ticket: endpoint });
                }
            }
            graph
                .add(edge.from(), edge.to())
                .map_err(|reason| GraphProposalError::IllegalEdge { reason })?;
        }
        Ok(())
    }

    /// The immutable identity.
    pub fn id(&self) -> GraphProposalId {
        self.id
    }

    /// The Spec this graph proposes for.
    pub fn spec(&self) -> SpecId {
        self.spec
    }

    /// The Spec content version the graph is proposed against.
    pub fn spec_version(&self) -> u64 {
        self.spec_version
    }

    /// Every Ticket the graph holds, in the order they were named.
    pub fn tickets(&self) -> &[TicketId] {
        &self.tickets
    }

    /// The dependency edges between the Tickets the graph holds, in
    /// proposal order.
    pub fn edges(&self) -> &[TicketDependency] {
        &self.edges
    }

    /// The proposal's lifecycle state.
    pub fn state(&self) -> &GraphProposalState {
        &self.state
    }

    /// The number of applied changes, for optimistic version checks.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Pass the human gate (DR-PS-17): the proposal moves from
    /// proposed to approved. The gate's rules are
    /// [`enforce_approvable`]'s; this move only records the decision,
    /// and refuses a graph that already passed the gate. The applied
    /// change bumps the version.
    pub fn approve(&mut self) -> Result<(), GraphProposalError> {
        if self.state != GraphProposalState::Proposed {
            return Err(GraphProposalError::ApproveRequiresProposed);
        }
        self.state = GraphProposalState::Approved;
        self.version += 1;
        Ok(())
    }
}

/// Why the human gate refused to approve a Ticket graph (DR-PS-17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphApprovalRefusal {
    /// The graph names no Ticket: a graph of nothing is not complete.
    Empty,
    /// The graph named a Ticket the Spec does not hold, so it is not
    /// the Spec's complete graph.
    Detached {
        /// The Ticket that belongs to no attachment to this Spec.
        ticket: TicketId,
    },
    /// A Ticket attached to the Spec sits outside the graph, so the
    /// graph is not complete.
    Incomplete {
        /// Every attached Ticket the graph left out, in attachment
        /// order.
        tickets: Vec<TicketId>,
    },
    /// An Implementation Ticket in the graph claims no User Story of
    /// the Spec version, so the graph is not granular: a slice
    /// delivers a claimed behaviour or it is not a slice of this
    /// Spec.
    NotGranular {
        /// The Ticket that delivers nothing of this version.
        ticket: TicketId,
    },
    /// A Bug in the graph is not yet qualified, so the graph is not
    /// verifiable: an unqualified Bug carries no criteria and no
    /// Verification Steps (DR-TK-09).
    NotVerifiable {
        /// The Bug that waits for its qualification.
        ticket: TicketId,
    },
    /// A User Story of the Spec version is claimed by no criterion of
    /// any Ticket in the graph, so the graph is not story-covered and
    /// never becomes executable (DR-PS-14).
    Uncovered {
        /// Every uncovered story, in scope order.
        stories: Vec<UserStoryRef>,
    },
}

impl GraphApprovalRefusal {
    /// The uncovered stories this refusal names, empty for every
    /// other refusal.
    pub fn uncovered(&self) -> &[UserStoryRef] {
        match self {
            Self::Uncovered { stories } => stories.as_slice(),
            _ => &[],
        }
    }
}

impl fmt::Display for GraphApprovalRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "the Ticket graph names no Ticket to approve"),
            Self::Detached { ticket } => write!(
                f,
                "the Ticket graph is not complete; Ticket {ticket} is not attached to the Spec"
            ),
            Self::Incomplete { tickets } => {
                let named: Vec<String> = tickets.iter().map(|ticket| ticket.to_string()).collect();
                write!(
                    f,
                    "the Ticket graph is not complete; Tickets {} sit outside it",
                    named.join(", ")
                )
            }
            Self::NotGranular { ticket } => write!(
                f,
                "the Ticket graph is not granular; Ticket {ticket} claims no User Story of the \
                 Spec version"
            ),
            Self::NotVerifiable { ticket } => write!(
                f,
                "the Ticket graph is not verifiable; Bug {ticket} is not yet qualified"
            ),
            Self::Uncovered { stories } => {
                let named: Vec<String> = stories.iter().map(|story| story.wire_name()).collect();
                write!(
                    f,
                    "the Ticket graph is not story-covered; User Stories {} stay uncovered",
                    named.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for GraphApprovalRefusal {}

/// The human approval gate (DR-PS-17): a Ticket graph may be approved
/// only when it is complete, granular, verifiable, and story-covered.
///
/// - Complete: the graph names at least one Ticket, every Ticket it
///   names is attached to the Spec it proposes for, and every Ticket
///   attached to that Spec is in the graph.
/// - Granular: every Implementation Ticket in the graph claims at
///   least one User Story of the Spec version's scope — a small
///   vertical slice delivers a claimed behaviour (DR-TK-04).
/// - Verifiable: every Bug in the graph is qualified, carrying its
///   criteria and Verification Steps (DR-TK-09); criteria are
///   verifiable by construction, because only observable outcomes
///   linked to stories can exist (DR-PS-13, DR-PS-15).
/// - Story-covered: every User Story the version claims is covered by
///   a criterion of at least one Ticket in the graph (DR-PS-14).
///
/// `attached` carries every Ticket attached to the Spec, as stored;
/// the gate reads it and the proposal, and nothing else.
pub fn enforce_approvable(
    proposal: &TicketGraphProposal,
    scope: &StoryScope,
    attached: &[Ticket],
) -> Result<(), GraphApprovalRefusal> {
    let named = proposal.tickets();
    if named.is_empty() {
        return Err(GraphApprovalRefusal::Empty);
    }
    let held: Vec<&Ticket> = named
        .iter()
        .map(|ticket| {
            attached
                .iter()
                .find(|held| held.id() == *ticket)
                .ok_or(GraphApprovalRefusal::Detached { ticket: *ticket })
        })
        .collect::<Result<_, _>>()?;
    let outside: Vec<TicketId> = attached
        .iter()
        .map(|ticket| ticket.id())
        .filter(|ticket| !named.contains(ticket))
        .collect();
    if !outside.is_empty() {
        return Err(GraphApprovalRefusal::Incomplete { tickets: outside });
    }
    for ticket in &held {
        match ticket.body() {
            crate::ticket::TicketBody::Implementation(_) => {
                let delivering = ticket.criteria().iter().any(|criterion| {
                    criterion
                        .stories()
                        .iter()
                        .any(|story| scope.contains(*story))
                });
                if !delivering {
                    return Err(GraphApprovalRefusal::NotGranular {
                        ticket: ticket.id(),
                    });
                }
            }
            crate::ticket::TicketBody::Bug(bug) => {
                if !bug.is_qualified() {
                    return Err(GraphApprovalRefusal::NotVerifiable {
                        ticket: ticket.id(),
                    });
                }
            }
            crate::ticket::TicketBody::Task(_) => {}
        }
    }
    let uncovered = scope.uncovered(&claimed_criteria_collected(&held));
    if !uncovered.is_empty() {
        return Err(GraphApprovalRefusal::Uncovered { stories: uncovered });
    }
    Ok(())
}

/// Every criterion the graph's Tickets claim, in graph order — the
/// claims the story-covered gate accumulates. An Implementation
/// claims through its criteria; a qualified Bug claims through its
/// qualification's criteria (DR-TK-09); a Task claims nothing
/// (DR-TK-07).
fn claimed_criteria_collected(held: &[&Ticket]) -> Vec<AcceptanceCriterion> {
    held.iter()
        .flat_map(|ticket| match ticket.bug() {
            Some(bug) => bug
                .qualification()
                .map(|record| record.criteria().to_vec())
                .unwrap_or_default(),
            None => ticket.criteria().to_vec(),
        })
        .collect()
}

#[cfg(test)]
mod graph_rules {
    use super::{
        GraphApprovalRefusal, GraphProposalError, GraphProposalId, GraphProposalState, SpecId,
        TicketGraphProposal, enforce_approvable,
    };
    use crate::coverage::{AcceptanceCriterion, StoryScope, UserStoryRef, VerificationStep};
    use crate::dependency::TicketDependency;
    use crate::plan::SpecNumber;
    use crate::project::{ProjectCode, ProjectId};
    use crate::ticket::{
        BugQualification, Priority, Severity, TaskMode, TaskSubtype, TaskTiming, Ticket,
        TicketBody, TicketId, TicketNumber, TicketState,
    };

    fn code() -> ProjectCode {
        ProjectCode::new("CORE").expect("the fixture code is well formed")
    }

    fn spec(number: u64) -> SpecNumber {
        SpecNumber::new(number).expect("the fixture number is positive")
    }

    fn ticket(value: u64) -> TicketId {
        TicketId::new(value)
    }

    fn story(spec_number: u64, ordinal: u64) -> UserStoryRef {
        UserStoryRef::new(spec(spec_number), ordinal).expect("the fixture ordinal is positive")
    }

    fn criterion(spec_number: u64, ordinal: u64, outcome: &str) -> AcceptanceCriterion {
        AcceptanceCriterion::new(outcome, vec![story(spec_number, ordinal)])
            .expect("the fixture criterion links")
    }

    /// The story section a scope of three stories on Spec 1 claims.
    const STORIES: &str = "\
- CORE-S1-US1: As an operator, I want complete graphs.
- CORE-S1-US2: As an operator, I want granular slices.
- CORE-S1-US3: As an operator, I want covered stories.
";

    fn scope() -> StoryScope {
        StoryScope::extract(&code(), spec(1), STORIES)
            .expect("the fixture section claims its stories")
    }

    /// One stored Ticket in the state a test chooses, attached to the
    /// Spec a test chooses, with the criteria a test chooses.
    fn implementation(id: u64, spec_id: u64, criteria: Vec<AcceptanceCriterion>) -> Ticket {
        Ticket::restore(
            ticket(id),
            ProjectId::new(1),
            TicketNumber::new(id).expect("the fixture number is positive"),
            Priority::Normal,
            TicketState::Draft,
            TicketBody::implementation(
                Some(SpecId::new(spec_id)),
                spec(1),
                "Registration creates Projects end to end",
                criteria,
            )
            .expect("the fixture body validates"),
            None,
            None,
            1,
        )
    }

    /// One stored Bug attached to the Spec, qualified when `qualified`
    /// says so.
    fn bug(id: u64, spec_id: u64, qualified: bool) -> Ticket {
        let qualification = qualified.then(|| {
            BugQualification::new(
                "The integration branch survives every landing.",
                "Re land a reviewed change; the branch list still names it.",
                "macOS 26, Kanban 0.1.0.",
                Severity::High,
                "Every landing so far.",
                "All landing reviews.",
                "Duplicate landings and lost review state.",
                vec![criterion(
                    1,
                    3,
                    "The integration branch survives a landing.",
                )],
                vec![
                    VerificationStep::new("cargo test -p kanban-storage tickets")
                        .expect("the fixture step carries its command"),
                ],
            )
            .expect("the fixture qualification is complete")
        });
        let mut bug = Ticket::new(
            ticket(id),
            ProjectId::new(1),
            TicketNumber::new(id).expect("the fixture number is positive"),
            Priority::Normal,
            TicketBody::bug(
                "Landing drops the integration branch",
                Some(SpecId::new(spec_id)),
                "The integration branch is dropped after a review lands.",
                "The landing log names the drop immediately after the merge.",
            )
            .expect("the fixture body validates"),
        );
        if let Some(qualification) = qualification {
            bug.qualify(qualification).expect("the fixture qualifies");
        }
        bug
    }

    /// One stored Task attached to the Spec, claiming no story
    /// (DR-TK-07).
    fn task(id: u64, spec_id: u64) -> Ticket {
        Ticket::new(
            ticket(id),
            ProjectId::new(1),
            TicketNumber::new(id).expect("the fixture number is positive"),
            Priority::Normal,
            TicketBody::task(
                "Archive the old register",
                Some(SpecId::new(spec_id)),
                Some(TaskSubtype::Operational),
                Some(TaskMode::Human),
                vec![
                    crate::ticket::CompletionCriterion::new("The register is archived.")
                        .expect("the fixture outcome binds"),
                ],
                TaskTiming::none(),
            )
            .expect("the fixture body validates"),
        )
    }

    /// A proposal over `tickets` and `edges`, against Spec 1's
    /// version one, as recording left it.
    fn recorded(
        tickets: Vec<TicketId>,
        edges: Vec<TicketDependency>,
    ) -> Result<TicketGraphProposal, GraphProposalError> {
        TicketGraphProposal::new(GraphProposalId::new(7), SpecId::new(1), 1, tickets, edges)
    }

    /// A valid proposal over `tickets` and `edges`.
    fn proposal(tickets: Vec<TicketId>, edges: Vec<TicketDependency>) -> TicketGraphProposal {
        recorded(tickets, edges).expect("the fixture proposal validates")
    }

    #[test]
    fn a_graph_names_at_least_one_ticket() {
        assert_eq!(
            TicketGraphProposal::new(
                GraphProposalId::new(1),
                SpecId::new(1),
                1,
                Vec::new(),
                Vec::new(),
            )
            .unwrap_err(),
            GraphProposalError::EmptyTicketSet,
            "a graph of no Tickets is not the Spec's complete graph (DR-PS-16)"
        );
    }

    #[test]
    fn a_graph_names_each_ticket_once() {
        assert_eq!(
            recorded(vec![ticket(1), ticket(1)], Vec::new()).unwrap_err(),
            GraphProposalError::DuplicateTicket { ticket: ticket(1) }
        );
    }

    #[test]
    fn a_graphs_edges_stay_inside_its_tickets() {
        for endpoint in [ticket(9), ticket(1)] {
            let edge = if endpoint == ticket(9) {
                TicketDependency::new(ticket(9), ticket(2))
            } else {
                TicketDependency::new(ticket(1), ticket(9))
            };
            assert_eq!(
                recorded(vec![ticket(1), ticket(2)], vec![edge]).unwrap_err(),
                GraphProposalError::EdgeOutsideSet { ticket: ticket(9) },
                "an edge names only Tickets the graph holds"
            );
        }
    }

    #[test]
    fn a_graphs_edges_follow_the_dependency_rules() {
        let refused = recorded(
            vec![ticket(1), ticket(2)],
            vec![
                TicketDependency::new(ticket(1), ticket(2)),
                TicketDependency::new(ticket(2), ticket(1)),
            ],
        )
        .unwrap_err();

        assert_eq!(
            refused,
            GraphProposalError::IllegalEdge {
                reason: crate::dependency::DependencyError::Cycle {
                    from: ticket(2),
                    to: ticket(1)
                }
            },
            "a proposed cycle is refused before any human sees it (DR-DE-02)"
        );
        assert_eq!(
            recorded(
                vec![ticket(1), ticket(2)],
                vec![TicketDependency::new(ticket(1), ticket(1))],
            )
            .unwrap_err(),
            GraphProposalError::IllegalEdge {
                reason: crate::dependency::DependencyError::SelfEdge
            }
        );
        assert_eq!(
            recorded(
                vec![ticket(1), ticket(2)],
                vec![
                    TicketDependency::new(ticket(1), ticket(2)),
                    TicketDependency::new(ticket(1), ticket(2)),
                ],
            )
            .unwrap_err(),
            GraphProposalError::IllegalEdge {
                reason: crate::dependency::DependencyError::DuplicateEdge
            }
        );
    }

    #[test]
    fn a_recorded_proposal_holds_every_fact() {
        let proposal = proposal(
            vec![ticket(1), ticket(2)],
            vec![TicketDependency::new(ticket(1), ticket(2))],
        );

        assert_eq!(proposal.id(), GraphProposalId::new(7));
        assert_eq!(proposal.spec(), SpecId::new(1));
        assert_eq!(proposal.spec_version(), 1);
        assert_eq!(proposal.tickets(), [ticket(1), ticket(2)].as_slice());
        assert_eq!(
            proposal.edges(),
            [TicketDependency::new(ticket(1), ticket(2))].as_slice()
        );
        assert_eq!(proposal.state(), &GraphProposalState::Proposed);
        assert_eq!(proposal.version(), 1);
        assert_eq!(
            proposal,
            TicketGraphProposal::restore(
                GraphProposalId::new(7),
                SpecId::new(1),
                1,
                vec![ticket(1), ticket(2)],
                vec![TicketDependency::new(ticket(1), ticket(2))],
                GraphProposalState::Proposed,
                1,
            )
        );
        assert_eq!(
            GraphProposalState::parse("approved"),
            Some(GraphProposalState::Approved)
        );
        assert_eq!(GraphProposalState::parse("ghost"), None);
    }

    #[test]
    fn approval_moves_a_proposed_graph_once() {
        let mut proposal = proposal(vec![ticket(1)], Vec::new());

        proposal.approve().expect("a proposed graph approves");

        assert_eq!(proposal.state(), &GraphProposalState::Approved);
        assert_eq!(proposal.version(), 2);

        assert_eq!(
            proposal.approve().unwrap_err(),
            GraphProposalError::ApproveRequiresProposed,
            "an approved graph already passed the gate"
        );
        assert_eq!(proposal.version(), 2, "the refusal changed nothing");
    }

    #[test]
    fn the_gate_passes_a_complete_granular_verifiable_covered_graph() {
        let attached = [
            implementation(
                1,
                1,
                vec![
                    criterion(1, 1, "Graphs record completely."),
                    criterion(1, 2, "Slices stay granular."),
                ],
            ),
            bug(2, 1, true),
            task(3, 1),
        ];
        let proposal = proposal(vec![ticket(1), ticket(2), ticket(3)], Vec::new());

        enforce_approvable(&proposal, &scope(), &attached).expect("every gate holds (DR-PS-17)");
    }

    #[test]
    fn the_gate_refuses_a_graph_of_no_tickets() {
        // restore() rehydrates stored bytes without validating, which
        // is the only way to hold an empty set long enough to ask the
        // gate about it.
        let empty = TicketGraphProposal::restore(
            GraphProposalId::new(1),
            SpecId::new(1),
            1,
            Vec::new(),
            Vec::new(),
            GraphProposalState::Proposed,
            1,
        );

        assert_eq!(
            enforce_approvable(
                &empty,
                &scope(),
                &[implementation(
                    1,
                    1,
                    vec![criterion(1, 1, "Graphs record completely.")]
                )]
            )
            .unwrap_err(),
            GraphApprovalRefusal::Empty
        );
    }

    #[test]
    fn the_gate_refuses_a_ticket_the_spec_does_not_hold() {
        let attached = [implementation(
            1,
            1,
            vec![criterion(1, 1, "Graphs record completely.")],
        )];
        let proposal = proposal(vec![ticket(1), ticket(2)], Vec::new());

        assert_eq!(
            enforce_approvable(&proposal, &scope(), &attached).unwrap_err(),
            GraphApprovalRefusal::Detached { ticket: ticket(2) },
            "a Ticket of another Spec, or of none, is not this graph's to hold"
        );
    }

    #[test]
    fn the_gate_refuses_an_attached_ticket_left_outside() {
        let attached = [
            implementation(1, 1, vec![criterion(1, 1, "Graphs record completely.")]),
            implementation(2, 1, vec![criterion(1, 2, "Slices stay granular.")]),
        ];
        let proposal = proposal(vec![ticket(1)], Vec::new());

        assert_eq!(
            enforce_approvable(&proposal, &scope(), &attached).unwrap_err(),
            GraphApprovalRefusal::Incomplete {
                tickets: vec![ticket(2)]
            },
            "the graph is the Spec's complete graph or it is not approved"
        );
        assert_eq!(
            enforce_approvable(&proposal, &scope(), &attached)
                .unwrap_err()
                .to_string(),
            "the Ticket graph is not complete; Tickets 2 sit outside it"
        );
    }

    #[test]
    fn the_gate_refuses_a_slice_that_delivers_nothing_of_the_version() {
        // The Ticket is well created: its criteria claim Spec 1's
        // story nine. This version's scope claims one through three,
        // so the slice delivers nothing this version asks for.
        let attached = [
            implementation(1, 1, vec![criterion(1, 9, "A story this version drops.")]),
            implementation(2, 1, vec![criterion(1, 1, "Graphs record completely.")]),
            implementation(3, 1, vec![criterion(1, 2, "Slices stay granular.")]),
            implementation(4, 1, vec![criterion(1, 3, "Stories stay covered.")]),
        ];
        let proposal = proposal(vec![ticket(1), ticket(2), ticket(3), ticket(4)], Vec::new());

        assert_eq!(
            enforce_approvable(&proposal, &scope(), &attached).unwrap_err(),
            GraphApprovalRefusal::NotGranular { ticket: ticket(1) },
            "a slice of nothing this version claims is not granular (DR-TK-04)"
        );
    }

    #[test]
    fn the_gate_refuses_an_unqualified_bug() {
        let attached = [
            implementation(
                1,
                1,
                vec![
                    criterion(1, 1, "Graphs record completely."),
                    criterion(1, 2, "Slices stay granular."),
                ],
            ),
            bug(2, 1, false),
        ];
        let proposal = proposal(vec![ticket(1), ticket(2)], Vec::new());

        assert_eq!(
            enforce_approvable(&proposal, &scope(), &attached).unwrap_err(),
            GraphApprovalRefusal::NotVerifiable { ticket: ticket(2) },
            "an unqualified Bug carries no criteria and no Verification Steps (DR-TK-09)"
        );
    }

    #[test]
    fn the_gate_refuses_an_uncovered_story() {
        let attached = [implementation(
            1,
            1,
            vec![criterion(1, 1, "Graphs record completely.")],
        )];
        let proposal = proposal(vec![ticket(1)], Vec::new());

        let refusal = enforce_approvable(&proposal, &scope(), &attached).unwrap_err();

        assert_eq!(
            refusal,
            GraphApprovalRefusal::Uncovered {
                stories: vec![story(1, 2), story(1, 3)]
            },
            "every gap is listed in scope order (DR-PS-14)"
        );
        assert_eq!(refusal.uncovered(), [story(1, 2), story(1, 3)].as_slice());
        assert_eq!(
            refusal.to_string(),
            "the Ticket graph is not story-covered; User Stories S1-US2, S1-US3 stay uncovered"
        );
    }

    #[test]
    fn claims_accumulate_across_the_graphs_tickets() {
        let attached = [
            implementation(1, 1, vec![criterion(1, 1, "Graphs record completely.")]),
            implementation(2, 1, vec![criterion(1, 2, "Slices stay granular.")]),
            bug(3, 1, true),
            task(4, 1),
        ];
        let proposal = proposal(
            vec![ticket(1), ticket(2), ticket(3), ticket(4)],
            vec![TicketDependency::new(ticket(1), ticket(2))],
        );

        enforce_approvable(&proposal, &scope(), &attached)
            .expect("a story needs one claim from any Ticket in the graph (DR-PS-14)");
    }
}
