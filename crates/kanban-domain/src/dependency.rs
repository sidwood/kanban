//! The Ticket dependency rules (CONTEXT.md, DR-DE-02, DR-DE-03,
//! DR-DE-04): dependency edges may cross Specs and registered
//! Projects, cycles are rejected, unregistered waiting work is
//! representable only as an explicit external blocker, and readiness
//! is a computed projection over those edges — it never mutates
//! state. Unlike a Plan's Spec edges, which are legal only inside one
//! Plan (DR-DE-01), a Ticket edge carries no membership rule of its
//! own here: an edge is legal between any two Tickets, and the
//! application layer registers it only between two Tickets that
//! exist, which is what makes a dependency "registered"
//! (DR-DE-02). Unregistered waiting work has no edge form at all —
//! the type names a Ticket — so it can only be recorded as an
//! [`ExternalBlocker`] (DR-DE-04).

use std::fmt;

use crate::ticket::{TicketId, TicketState};

/// Why a dependency rule was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyError {
    /// A Ticket never depends on itself.
    SelfEdge,
    /// The edge already exists.
    DuplicateEdge,
    /// Adding the edge would close a cycle, and cycles are never
    /// approved (DR-DE-02).
    Cycle {
        /// The endpoint that must land first.
        from: TicketId,
        /// The endpoint that waits.
        to: TicketId,
    },
    /// The edge does not exist.
    EdgeNotFound,
    /// An external blocker description held nothing but whitespace.
    BlankDescription,
}

impl fmt::Display for DependencyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelfEdge => write!(f, "a Ticket never depends on itself"),
            Self::DuplicateEdge => write!(f, "that Ticket dependency already exists"),
            Self::Cycle { from, to } => write!(
                f,
                "the dependency from Ticket {from} to Ticket {to} would close a cycle"
            ),
            Self::EdgeNotFound => write!(f, "that Ticket dependency does not exist"),
            Self::BlankDescription => {
                write!(f, "an external blocker description cannot be blank")
            }
        }
    }
}

impl std::error::Error for DependencyError {}

/// One dependency edge between Tickets: `from` must land before `to`
/// may begin. The edge carries no Project or Spec of its own — it is
/// legal across both (DR-DE-02) — so the identities alone name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TicketDependency {
    from: TicketId,
    to: TicketId,
}

impl TicketDependency {
    /// Join two Tickets; the graph validates direction when the edge
    /// is added.
    pub fn new(from: TicketId, to: TicketId) -> Self {
        Self { from, to }
    }

    /// The Ticket that must land first.
    pub fn from(self) -> TicketId {
        self.from
    }

    /// The Ticket that waits on `from`.
    pub fn to(self) -> TicketId {
        self.to
    }
}

/// The whole dependency graph over Tickets, as it rehydrates from
/// storage: the registered edges, with cycles rejected as they are
/// added so a stored graph is always acyclic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TicketDependencyGraph {
    edges: Vec<TicketDependency>,
}

impl TicketDependencyGraph {
    /// An empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Rehydrate a stored graph exactly as it was recorded. Storage
    /// only ever holds edges that passed `add`, so a cycle here is
    /// corruption the caller will hear about from the projection, not
    /// silent acceptance.
    pub fn restore(edges: Vec<TicketDependency>) -> Self {
        Self { edges }
    }

    /// Every registered edge, in insertion order.
    pub fn edges(&self) -> &[TicketDependency] {
        &self.edges
    }

    /// Add an edge: `from` must land before `to` may begin. A
    /// self-edge, a duplicate, or an edge that would close a cycle is
    /// refused (DR-DE-02).
    pub fn add(&mut self, from: TicketId, to: TicketId) -> Result<(), DependencyError> {
        if from == to {
            return Err(DependencyError::SelfEdge);
        }
        let edge = TicketDependency::new(from, to);
        if self.holds(edge) {
            return Err(DependencyError::DuplicateEdge);
        }
        // The new edge lets `from` reach everything `to` already
        // reaches, so it closes a cycle exactly when `to` already has
        // to land before `from`.
        if self.reaches(to, from) {
            return Err(DependencyError::Cycle { from, to });
        }
        self.edges.push(edge);
        Ok(())
    }

    /// Remove one edge, refusing an edge the graph does not hold.
    pub fn remove(&mut self, from: TicketId, to: TicketId) -> Result<(), DependencyError> {
        let edge = TicketDependency::new(from, to);
        self.edges
            .iter()
            .position(|held| *held == edge)
            .map(|position| self.edges.remove(position))
            .ok_or(DependencyError::EdgeNotFound)?;
        Ok(())
    }

    /// Whether the graph holds one edge.
    pub fn holds(&self, edge: TicketDependency) -> bool {
        self.edges.contains(&edge)
    }

    /// The edges one Ticket waits on: every edge whose `to` is that
    /// Ticket, in insertion order.
    pub fn required_by(&self, ticket: TicketId) -> Vec<TicketDependency> {
        self.edges
            .iter()
            .copied()
            .filter(|edge| edge.to() == ticket)
            .collect()
    }

    /// The registered edges running between two of `members`, in
    /// insertion order. An approved Ticket graph's install replaces
    /// exactly these edges; an edge crossing out of the membership
    /// stands.
    pub fn edges_within(&self, members: &[TicketId]) -> Vec<TicketDependency> {
        self.edges
            .iter()
            .copied()
            .filter(|edge| members.contains(&edge.from()) && members.contains(&edge.to()))
            .collect()
    }

    /// Whether `start` must land before `target` along some chain of
    /// edges — whether a path of dependencies leads from `start` to
    /// `target`.
    fn reaches(&self, start: TicketId, target: TicketId) -> bool {
        let mut pending = vec![start];
        let mut seen = Vec::new();
        while let Some(current) = pending.pop() {
            if current == target {
                return true;
            }
            if seen.contains(&current) {
                continue;
            }
            seen.push(current);
            pending.extend(
                self.edges
                    .iter()
                    .filter(|edge| edge.from() == current)
                    .map(|edge| edge.to()),
            );
        }
        false
    }
}

/// The identity of one recorded external blocker. Assigned once by
/// storage and immutable afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExternalBlockerId(u64);

impl ExternalBlockerId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ExternalBlockerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A validated external blocker description (DR-DE-04): what the
/// Ticket is waiting on, held by no registered Ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockerDescription(String);

impl BlockerDescription {
    /// Accept any description that holds at least one
    /// non-whitespace character, trimmed.
    pub fn new(raw: &str) -> Result<Self, DependencyError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DependencyError::BlankDescription);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The trimmed description.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One explicit external blocker: the unregistered work a Ticket
/// waits on, named in prose because no Ticket carries it (DR-DE-04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalBlocker {
    id: ExternalBlockerId,
    ticket: TicketId,
    description: BlockerDescription,
}

impl ExternalBlocker {
    /// Restore a stored blocker exactly as it was recorded.
    pub fn restore(
        id: ExternalBlockerId,
        ticket: TicketId,
        description: BlockerDescription,
    ) -> Self {
        Self {
            id,
            ticket,
            description,
        }
    }

    /// The immutable identity.
    pub fn id(&self) -> ExternalBlockerId {
        self.id
    }

    /// The Ticket waiting on the described work.
    pub fn ticket(&self) -> TicketId {
        self.ticket
    }

    /// The description of the unregistered work.
    pub fn description(&self) -> &BlockerDescription {
        &self.description
    }
}

/// Whether one dependency is satisfied: the blocking Ticket has
/// landed. `done` is the only state that lands work in full; a
/// cancelled or superseded blocker has landed nothing, so it holds
/// its waiter until the operator removes the edge.
pub fn dependency_satisfied(state: TicketState) -> bool {
    state == TicketState::Done
}

/// What one dependency edge waits on, paired with the state of the
/// blocking Ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DependencyState {
    /// The edge whose `to` waits.
    pub dependency: TicketDependency,
    /// The state of the Ticket the edge's `from` names.
    pub state: TicketState,
}

/// What still holds a Ticket back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadinessBlocker {
    /// A registered dependency whose blocker has not landed.
    Ticket {
        /// The edge and the blocker's state.
        waiting: DependencyState,
    },
    /// An explicit external blocker (DR-DE-04).
    External {
        /// The recorded blocker.
        blocker: ExternalBlocker,
    },
}

/// The computed readiness of one Ticket (DR-DE-03): the projection of
/// its dependencies and external blockers, and nothing else. It never
/// mutates state; the lifecycle slice decides what a Ticket may do
/// with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readiness {
    blocked_by: Vec<ReadinessBlocker>,
}

impl Readiness {
    /// Whether nothing holds the Ticket back.
    pub fn is_ready(&self) -> bool {
        self.blocked_by.is_empty()
    }

    /// What holds the Ticket back, dependencies in input order first,
    /// then external blockers in input order.
    pub fn blocked_by(&self) -> &[ReadinessBlocker] {
        &self.blocked_by
    }
}

/// The inputs one Ticket's readiness is computed from: the states of
/// its registered dependencies and its external blockers. Readiness
/// is a projection of exactly these — no other input exists.
#[derive(Debug, Clone, Copy)]
pub struct ReadinessInputs<'a> {
    /// Every dependency the Ticket waits on, each paired with the
    /// blocking Ticket's state.
    pub dependencies: &'a [DependencyState],
    /// The Ticket's external blockers.
    pub blockers: &'a [ExternalBlocker],
}

/// Compute one Ticket's readiness from its dependencies and external
/// blockers (DR-DE-03). A dependency blocks until its blocker has
/// landed; an external blocker blocks until it is removed; with
/// neither, the Ticket is ready.
pub fn compute_readiness(inputs: ReadinessInputs<'_>) -> Readiness {
    let mut blocked_by = Vec::new();
    for waiting in inputs.dependencies {
        if !dependency_satisfied(waiting.state) {
            blocked_by.push(ReadinessBlocker::Ticket { waiting: *waiting });
        }
    }
    for blocker in inputs.blockers {
        blocked_by.push(ReadinessBlocker::External {
            blocker: blocker.clone(),
        });
    }
    Readiness { blocked_by }
}

#[cfg(test)]
mod dependency_rules {
    use super::{
        BlockerDescription, DependencyError, ExternalBlocker, ExternalBlockerId, TicketDependency,
        TicketDependencyGraph,
    };
    use crate::ticket::{TicketId, TicketState};

    fn ticket(value: u64) -> TicketId {
        TicketId::new(value)
    }

    fn edge(from: u64, to: u64) -> TicketDependency {
        TicketDependency::new(ticket(from), ticket(to))
    }

    #[test]
    fn edges_join_tickets_across_any_spec_and_project() {
        // Tickets 1 and 4 belong to one Project, 7 to another, and the
        // edges name different Specs of each. A Ticket edge carries
        // no membership rule (DR-DE-02), unlike a Plan's Spec edges
        // (DR-DE-01): the graph accepts both directions across the
        // boundary.
        let mut graph = TicketDependencyGraph::new();

        graph
            .add(ticket(1), ticket(7))
            .expect("an edge crosses Projects");
        graph
            .add(ticket(4), ticket(1))
            .expect("an edge crosses Specs inside one Project");

        assert_eq!(graph.edges(), [edge(1, 7), edge(4, 1)].as_slice());
    }

    #[test]
    fn a_self_edge_is_refused() {
        let mut graph = TicketDependencyGraph::new();

        assert_eq!(
            graph.add(ticket(2), ticket(2)),
            Err(DependencyError::SelfEdge)
        );
        assert!(graph.edges().is_empty(), "the refusal changed nothing");
    }

    #[test]
    fn a_duplicate_edge_is_refused() {
        let mut graph = TicketDependencyGraph::new();
        graph
            .add(ticket(1), ticket(2))
            .expect("the fixture edge lands");

        assert_eq!(
            graph.add(ticket(1), ticket(2)),
            Err(DependencyError::DuplicateEdge)
        );
        assert_eq!(graph.edges().len(), 1, "the refusal changed nothing");
    }

    #[test]
    fn a_direct_cycle_is_refused() {
        let mut graph = TicketDependencyGraph::new();
        graph
            .add(ticket(1), ticket(2))
            .expect("the fixture edge lands");

        let refused = graph
            .add(ticket(2), ticket(1))
            .expect_err("a two-Ticket cycle is never approved");

        assert_eq!(
            refused,
            DependencyError::Cycle {
                from: ticket(2),
                to: ticket(1)
            }
        );
        assert_eq!(
            refused.to_string(),
            "the dependency from Ticket 2 to Ticket 1 would close a cycle"
        );
        assert_eq!(graph.edges().len(), 1, "the refusal changed nothing");
    }

    #[test]
    fn a_longer_cycle_is_refused() {
        let mut graph = TicketDependencyGraph::new();
        for (from, to) in [(1, 2), (2, 3), (3, 4)] {
            graph
                .add(ticket(from), ticket(to))
                .expect("the fixture chain lands");
        }

        assert_eq!(
            graph.add(ticket(4), ticket(1)),
            Err(DependencyError::Cycle {
                from: ticket(4),
                to: ticket(1)
            }),
            "the chain 1 → 2 → 3 → 4 closes on 4 → 1"
        );

        // The graph still holds a path, so an edge onto the chain's
        // head from a fresh Ticket is legal: not every edge near a
        // chain is a cycle.
        graph
            .add(ticket(9), ticket(1))
            .expect("a fresh edge onto the chain is not a cycle");
    }

    #[test]
    fn removing_an_edge_frees_the_pair() {
        let mut graph = TicketDependencyGraph::new();
        graph
            .add(ticket(1), ticket(2))
            .expect("the fixture edge lands");

        graph.remove(ticket(1), ticket(2)).expect("the edge leaves");

        assert!(graph.edges().is_empty());
        graph
            .add(ticket(2), ticket(1))
            .expect("the reverse direction is legal once the edge is gone");
    }

    #[test]
    fn removing_a_missing_edge_is_refused() {
        let mut graph = TicketDependencyGraph::new();

        assert_eq!(
            graph.remove(ticket(1), ticket(2)),
            Err(DependencyError::EdgeNotFound)
        );
    }

    #[test]
    fn required_by_lists_what_one_ticket_waits_on() {
        let mut graph = TicketDependencyGraph::new();
        for (from, to) in [(1, 2), (3, 2), (2, 9)] {
            graph
                .add(ticket(from), ticket(to))
                .expect("the fixture edge lands");
        }

        assert_eq!(graph.required_by(ticket(2)), vec![edge(1, 2), edge(3, 2)]);
        assert!(graph.required_by(ticket(1)).is_empty());
        assert!(graph.holds(edge(2, 9)));
        assert!(!graph.holds(edge(9, 2)));
    }

    #[test]
    fn a_blank_blocker_description_is_refused() {
        assert_eq!(
            BlockerDescription::new("   ").unwrap_err(),
            DependencyError::BlankDescription
        );
        assert_eq!(
            DependencyError::BlankDescription.to_string(),
            "an external blocker description cannot be blank"
        );

        let description = BlockerDescription::new("  Vendor SDK 4 upgrade  ")
            .expect("any non-blank description validates");
        assert_eq!(description.as_str(), "Vendor SDK 4 upgrade");
    }

    #[test]
    fn a_blocker_rehydrates_with_every_recorded_fact() {
        let blocker = ExternalBlocker::restore(
            ExternalBlockerId::new(5),
            ticket(3),
            BlockerDescription::new("The vendor SDK 4 upgrade")
                .expect("the fixture description validates"),
        );

        assert_eq!(blocker.id().value(), 5);
        assert_eq!(blocker.ticket(), ticket(3));
        assert_eq!(blocker.description().as_str(), "The vendor SDK 4 upgrade");
    }

    #[test]
    fn restore_rehydrates_the_recorded_graph() {
        let graph = TicketDependencyGraph::restore(vec![edge(1, 7), edge(4, 1)]);

        assert_eq!(
            graph.edges(),
            [edge(1, 7), edge(4, 1)].as_slice(),
            "the stored order survives"
        );
        assert_eq!(
            graph,
            TicketDependencyGraph::restore(vec![edge(1, 7), edge(4, 1)])
        );
        assert_eq!(
            TicketDependencyGraph::new(),
            TicketDependencyGraph::default()
        );
        assert_eq!(
            edge(1, 2),
            TicketDependency::new(TicketId::new(1), TicketId::new(2))
        );
        assert_ne!(edge(1, 2), edge(2, 1));
        let _state = TicketState::Done;
    }
}

#[cfg(test)]
mod readiness {
    use super::{
        BlockerDescription, DependencyState, ExternalBlocker, ExternalBlockerId, ReadinessBlocker,
        TicketDependency, compute_readiness, dependency_satisfied,
    };
    use crate::ticket::{TicketId, TicketState};

    fn ticket(value: u64) -> TicketId {
        TicketId::new(value)
    }

    fn edge(from: u64, to: u64) -> TicketDependency {
        TicketDependency::new(ticket(from), ticket(to))
    }

    fn waiting(from: u64, state: TicketState) -> DependencyState {
        DependencyState {
            dependency: edge(from, 2),
            state,
        }
    }

    fn blocker(id: u64, description: &str) -> ExternalBlocker {
        ExternalBlocker::restore(
            ExternalBlockerId::new(id),
            ticket(2),
            BlockerDescription::new(description).expect("the fixture description validates"),
        )
    }

    fn readiness_of<'a>(
        dependencies: &'a [DependencyState],
        blockers: &'a [ExternalBlocker],
    ) -> super::Readiness {
        compute_readiness(super::ReadinessInputs {
            dependencies,
            blockers,
        })
    }

    #[test]
    fn only_done_lands_work_for_a_dependency() {
        for state in TicketState::ALL {
            assert_eq!(
                dependency_satisfied(*state),
                *state == TicketState::Done,
                "`{}` must satisfy a dependency only when it is done",
                state.wire_name()
            );
        }
    }

    #[test]
    fn a_ticket_with_nothing_to_wait_on_is_ready() {
        let readiness = readiness_of(&[], &[]);

        assert!(readiness.is_ready());
        assert!(readiness.blocked_by().is_empty());
    }

    #[test]
    fn a_landed_dependency_stops_blocking() {
        let dependencies = [waiting(1, TicketState::Done)];

        assert!(readiness_of(&dependencies, &[]).is_ready());
    }

    #[test]
    fn an_unlanded_dependency_blocks_with_its_state() {
        let dependencies = [waiting(1, TicketState::Active)];

        let readiness = readiness_of(&dependencies, &[]);

        assert!(!readiness.is_ready());
        assert_eq!(
            readiness.blocked_by(),
            [ReadinessBlocker::Ticket {
                waiting: waiting(1, TicketState::Active)
            }]
            .as_slice()
        );
    }

    #[test]
    fn a_cancelled_blocker_still_holds_its_waiter() {
        // Cancelled and superseded land nothing, so the edge holds
        // until the operator removes it; readiness never edits the
        // graph on the Ticket's behalf.
        for state in [TicketState::Cancelled, TicketState::Superseded] {
            let dependencies = [waiting(1, state)];
            assert!(
                !readiness_of(&dependencies, &[]).is_ready(),
                "`{}` must not satisfy a dependency",
                state.wire_name()
            );
        }
    }

    #[test]
    fn an_external_blocker_blocks_until_it_is_removed() {
        let blockers = [blocker(4, "The vendor SDK 4 upgrade")];

        let blocked = readiness_of(&[], &blockers);
        assert!(!blocked.is_ready());
        assert_eq!(
            blocked.blocked_by(),
            [ReadinessBlocker::External {
                blocker: blocker(4, "The vendor SDK 4 upgrade")
            }]
            .as_slice()
        );

        // Removal is the operator's explicit action: with the blocker
        // gone from the inputs, nothing else holds the Ticket.
        assert!(readiness_of(&[], &[]).is_ready());
    }

    #[test]
    fn readiness_reports_every_waiter_and_blocker_together() {
        let dependencies = [
            waiting(1, TicketState::Done),
            waiting(3, TicketState::Draft),
            waiting(5, TicketState::Landing),
        ];
        let blockers = [
            blocker(7, "The vendor SDK 4 upgrade"),
            blocker(8, "Design sign-off"),
        ];

        let readiness = readiness_of(&dependencies, &blockers);

        assert!(!readiness.is_ready());
        assert_eq!(
            readiness.blocked_by(),
            [
                ReadinessBlocker::Ticket {
                    waiting: waiting(3, TicketState::Draft)
                },
                ReadinessBlocker::Ticket {
                    waiting: waiting(5, TicketState::Landing)
                },
                ReadinessBlocker::External {
                    blocker: blocker(7, "The vendor SDK 4 upgrade")
                },
                ReadinessBlocker::External {
                    blocker: blocker(8, "Design sign-off")
                },
            ]
            .as_slice(),
            "landed dependencies drop out; the rest report in input order"
        );
    }
}
