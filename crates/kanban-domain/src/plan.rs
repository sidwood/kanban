//! The Plan entity: a versioned, ordered dependency graph of Specs
//! (CONTEXT.md). Display order is a per-Plan sequence and dependency
//! edges are a separate relation; both are editable only in draft and
//! both freeze at activation into an immutable version. The lifecycle
//! is draft, active, complete, cancelled, archived, with the terminal
//! states off the active surface. Replanning reopens the draft and
//! reserves the next version while every earlier version stays
//! queryable, and dependency edges are legal only inside one Plan.

use std::fmt;

use crate::project::ProjectId;

/// The identity of one Plan. Assigned once by storage and immutable
/// afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlanId(u64);

impl PlanId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for PlanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a Spec number was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecNumberError {
    /// Spec numbers start at one; zero names no Spec.
    Zero,
}

impl fmt::Display for SpecNumberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a Spec number starts at one")
    }
}

impl std::error::Error for SpecNumberError {}

/// One Spec of one Project, named by the number that Project minted
/// for it, for example the `1` of `CORE-S1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpecNumber(u64);

impl SpecNumber {
    /// Accept any positive number.
    pub fn new(value: u64) -> Result<Self, SpecNumberError> {
        if value == 0 {
            return Err(SpecNumberError::Zero);
        }
        Ok(Self(value))
    }

    /// The minted number this Spec is named by.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SpecNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One dependency edge: `from` must land before `to` may begin. Edges
/// are a separate relation from display order and are legal only
/// between two Specs of the same Plan (DR-DE-01).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DependencyEdge {
    from: SpecNumber,
    to: SpecNumber,
}

impl DependencyEdge {
    /// Join two Specs; the domain validates membership and direction
    /// when the edge is added to a Plan.
    pub fn new(from: SpecNumber, to: SpecNumber) -> Self {
        Self { from, to }
    }

    /// The Spec that must land first.
    pub fn from(self) -> SpecNumber {
        self.from
    }

    /// The Spec that waits on `from`.
    pub fn to(self) -> SpecNumber {
        self.to
    }
}

/// The closed lifecycle vocabulary for a Plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanState {
    /// Shaped and editable; nothing is frozen yet.
    Draft,
    /// Frozen into a version and executing.
    Active,
    /// Terminal: the frozen version landed in full.
    Complete,
    /// Terminal: the Plan will not execute.
    Cancelled,
    /// Terminal: every recorded fact is preserved and no further
    /// change is legal.
    Archived,
}

impl PlanState {
    /// Whether this state accepts no further lifecycle movement. The
    /// terminal states sit off the active surface: they stay queryable
    /// but never rejoin the working set.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Cancelled | Self::Archived)
    }
}

/// Why a Plan refused an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanError {
    /// Shape edits and activation require the draft state.
    RequiresDraft,
    /// Completion and replanning require the active state.
    RequiresActive,
    /// Cancellation requires draft or active.
    RequiresOpen,
    /// The Plan is already archived.
    AlreadyArchived,
    /// Activation requires at least one Spec: a graph of nothing has
    /// no order to execute.
    EmptyMembership,
    /// The Spec number is already a member.
    DuplicateMember(SpecNumber),
    /// The Spec number is not a member of this Plan.
    NotAMember(SpecNumber),
    /// The Spec still carries dependency edges; remove them first.
    MemberCarriesEdges(SpecNumber),
    /// An edge must connect two distinct Specs.
    SelfEdge,
    /// The edge already exists.
    DuplicateEdge,
    /// The edge does not exist.
    EdgeNotFound,
    /// Dependency edges are legal only within one Plan; an endpoint
    /// of this edge is not a member here (DR-DE-01).
    EdgeOutsideSinglePlan {
        /// The endpoint that must land first.
        from: SpecNumber,
        /// The endpoint that waits.
        to: SpecNumber,
    },
    /// The position sits outside the display order.
    PositionOutOfRange {
        /// The refused position.
        position: usize,
    },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequiresDraft => {
                write!(f, "only a draft Plan accepts this change")
            }
            Self::RequiresActive => {
                write!(f, "only an active Plan accepts this change")
            }
            Self::RequiresOpen => {
                write!(f, "only a draft or active Plan can be cancelled")
            }
            Self::AlreadyArchived => write!(f, "the Plan is already archived"),
            Self::EmptyMembership => {
                write!(f, "a Plan needs at least one Spec before it can activate")
            }
            Self::DuplicateMember(spec) => {
                write!(f, "Spec {spec} is already a member of this Plan")
            }
            Self::NotAMember(spec) => write!(f, "Spec {spec} is not a member of this Plan"),
            Self::MemberCarriesEdges(spec) => write!(
                f,
                "Spec {spec} still carries dependency edges; remove them first"
            ),
            Self::SelfEdge => write!(f, "a Spec never depends on itself"),
            Self::DuplicateEdge => write!(f, "that dependency edge already exists"),
            Self::EdgeNotFound => write!(f, "that dependency edge does not exist"),
            Self::EdgeOutsideSinglePlan { from, to } => write!(
                f,
                "the dependency from Spec {from} to Spec {to} leaves this Plan; \
                 edges are legal only within one Plan"
            ),
            Self::PositionOutOfRange { position } => {
                write!(f, "position {position} sits outside the display order")
            }
        }
    }
}

impl std::error::Error for PlanError {}

/// The working shape of one Plan: its display order and dependency
/// edges together, as they rehydrate from storage. The two relations
/// stay separate inside it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanShape {
    order: Vec<SpecNumber>,
    edges: Vec<DependencyEdge>,
}

impl PlanShape {
    /// A shape carrying exactly the order and edges given.
    pub fn new(order: Vec<SpecNumber>, edges: Vec<DependencyEdge>) -> Self {
        Self { order, edges }
    }

    /// The display order.
    pub fn order(&self) -> &[SpecNumber] {
        &self.order
    }

    /// The dependency edges.
    pub fn edges(&self) -> &[DependencyEdge] {
        &self.edges
    }
}

/// One dependency cycle in a Plan's graph: a group of member Specs
/// mutually reachable through the edges, so no execution order exists
/// among them. The planning diagnostics expose each group as one
/// blocking finding (DR-PS-18).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DependencyCycle {
    specs: Vec<SpecNumber>,
}

impl DependencyCycle {
    /// Wrap one group, as [`cycles_in`] mints it.
    pub fn new(specs: Vec<SpecNumber>) -> Self {
        Self { specs }
    }

    /// The group's members, ascending.
    pub fn specs(&self) -> &[SpecNumber] {
        &self.specs
    }
}

/// Every dependency cycle a graph holds: each group of two or more
/// member Specs mutually reachable through `edges`, plus every
/// self-edge, reported with its members ascending and its groups
/// ordered by first member. Edges naming Specs outside `order` shape
/// no ring among the membership and are ignored. Pure and
/// deterministic, so the working shape and any frozen version read
/// the same rule.
pub fn cycles_in(order: &[SpecNumber], edges: &[DependencyEdge]) -> Vec<DependencyCycle> {
    let members: std::collections::BTreeSet<SpecNumber> = order.iter().copied().collect();
    let mut adjacency: std::collections::BTreeMap<SpecNumber, Vec<SpecNumber>> =
        std::collections::BTreeMap::new();
    let mut self_edges: std::collections::BTreeSet<SpecNumber> = std::collections::BTreeSet::new();
    for edge in edges {
        if !order.contains(&edge.from()) || !order.contains(&edge.to()) {
            continue;
        }
        if edge.from() == edge.to() {
            self_edges.insert(edge.from());
            continue;
        }
        adjacency.entry(edge.from()).or_default().push(edge.to());
    }

    let mut cycles: Vec<DependencyCycle> = strongly_connected_groups(&members, &adjacency)
        .into_iter()
        .filter(|group| group.len() > 1)
        .map(|mut group| {
            group.sort();
            DependencyCycle::new(group)
        })
        .collect();
    cycles.extend(
        self_edges
            .into_iter()
            .map(|spec| DependencyCycle::new(vec![spec])),
    );
    cycles.sort();
    cycles
}

/// The groups of Specs mutually reachable through `adjacency`, in
/// discovery order. An iterative depth-first walk keeps the walk
/// deterministic and free of recursion limits on large drafts.
fn strongly_connected_groups(
    members: &std::collections::BTreeSet<SpecNumber>,
    adjacency: &std::collections::BTreeMap<SpecNumber, Vec<SpecNumber>>,
) -> Vec<Vec<SpecNumber>> {
    let mut visited: std::collections::BTreeMap<SpecNumber, usize> =
        std::collections::BTreeMap::new();
    let mut lowest: std::collections::BTreeMap<SpecNumber, usize> =
        std::collections::BTreeMap::new();
    let mut on_stack = std::collections::BTreeSet::new();
    let mut walked: Vec<SpecNumber> = Vec::new();
    let mut groups = Vec::new();
    let mut next = 0usize;
    let low = |map: &std::collections::BTreeMap<SpecNumber, usize>, spec: SpecNumber| {
        map.get(&spec).copied().unwrap_or(usize::MAX)
    };

    for &root in members {
        if visited.contains_key(&root) {
            continue;
        }
        visited.insert(root, next);
        lowest.insert(root, next);
        next += 1;
        walked.push(root);
        on_stack.insert(root);
        let mut frames: Vec<(SpecNumber, usize)> = vec![(root, 0)];
        while let Some((spec, position)) = frames.pop() {
            let neighbours: &[SpecNumber] =
                adjacency.get(&spec).map(Vec::as_slice).unwrap_or_default();
            if position < neighbours.len() {
                let target = neighbours[position];
                frames.push((spec, position + 1));
                if let Some(&seen) = visited.get(&target) {
                    if on_stack.contains(&target) && seen < low(&lowest, spec) {
                        lowest.insert(spec, seen);
                    }
                    continue;
                }
                visited.insert(target, next);
                lowest.insert(target, next);
                next += 1;
                walked.push(target);
                on_stack.insert(target);
                frames.push((target, 0));
                continue;
            }
            // The Spec is exhausted: a lowest link equal to its own
            // visit order roots one group; everything stacked above
            // it belongs to that group.
            if low(&lowest, spec) == visited[&spec] {
                let mut group = Vec::new();
                while let Some(member) = walked.pop() {
                    on_stack.remove(&member);
                    group.push(member);
                    if member == spec {
                        break;
                    }
                }
                groups.push(group);
            }
            if let Some(&(parent, _)) = frames.last() {
                let reached = low(&lowest, spec);
                if reached < low(&lowest, parent) {
                    lowest.insert(parent, reached);
                }
            }
        }
    }
    groups
}

/// One immutable frozen Plan version: the Spec membership, display
/// order, and dependency graph exactly as they stood at activation.
/// Minted once, never edited; later versions stand beside it rather
/// than replacing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanVersion {
    number: u64,
    order: Vec<SpecNumber>,
    edges: Vec<DependencyEdge>,
}

impl PlanVersion {
    /// Rehydrate a stored version exactly as it was recorded.
    pub fn new(number: u64, order: Vec<SpecNumber>, edges: Vec<DependencyEdge>) -> Self {
        Self {
            number,
            order,
            edges,
        }
    }

    /// The version's number; the first freeze is one and every
    /// replan-and-reactivate mints the next.
    pub fn number(&self) -> u64 {
        self.number
    }

    /// The frozen display order.
    pub fn order(&self) -> &[SpecNumber] {
        &self.order
    }

    /// The frozen dependency graph.
    pub fn edges(&self) -> &[DependencyEdge] {
        &self.edges
    }

    /// Every dependency cycle the frozen graph holds, exactly as it
    /// froze: a version's rings stay diagnostics forever, because the
    /// frozen shape never changes (DR-PS-04, DR-PS-18).
    pub fn cycles(&self) -> Vec<DependencyCycle> {
        cycles_in(&self.order, &self.edges)
    }
}

/// One Plan aggregate: the working shape and every version frozen
/// from it. The version counts applied changes: creation lands at 1
/// and every legal edit or transition bumps it, so a stored version
/// is all a caller needs for optimistic checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    id: PlanId,
    project: ProjectId,
    number: u64,
    state: PlanState,
    order: Vec<SpecNumber>,
    edges: Vec<DependencyEdge>,
    versions: Vec<PlanVersion>,
    version: u64,
}

impl Plan {
    /// A fresh Plan: a draft holding an empty graph, at version 1.
    /// `number` is the number the Project minted for this Plan.
    pub fn new(id: PlanId, project: ProjectId, number: u64) -> Self {
        Self {
            id,
            project,
            number,
            state: PlanState::Draft,
            order: Vec::new(),
            edges: Vec::new(),
            versions: Vec::new(),
            version: 1,
        }
    }

    /// Rehydrate a stored Plan exactly as it was recorded.
    pub fn restore(
        id: PlanId,
        project: ProjectId,
        number: u64,
        state: PlanState,
        shape: PlanShape,
        versions: Vec<PlanVersion>,
        version: u64,
    ) -> Self {
        Self {
            id,
            project,
            number,
            state,
            order: shape.order,
            edges: shape.edges,
            versions,
            version,
        }
    }

    /// The immutable identity.
    pub fn id(&self) -> PlanId {
        self.id
    }

    /// The Project this Plan belongs to.
    pub fn project(&self) -> ProjectId {
        self.project
    }

    /// The number this Project minted for this Plan.
    pub fn number(&self) -> u64 {
        self.number
    }

    /// The lifecycle state.
    pub fn state(&self) -> PlanState {
        self.state
    }

    /// The working display order: the per-Plan sequence of member
    /// Specs, editable only in draft.
    pub fn order(&self) -> &[SpecNumber] {
        &self.order
    }

    /// The working dependency graph, editable only in draft.
    pub fn edges(&self) -> &[DependencyEdge] {
        &self.edges
    }

    /// Every dependency cycle the working graph holds: the rings the
    /// planning diagnostics expose as blocking, because a ring admits
    /// no execution order (DR-PS-18).
    pub fn cycles(&self) -> Vec<DependencyCycle> {
        cycles_in(&self.order, &self.edges)
    }

    /// Every frozen version, oldest first; each stays queryable
    /// beside its replacements.
    pub fn versions(&self) -> &[PlanVersion] {
        &self.versions
    }

    /// The number the next freeze will take. Versions are minted
    /// monotonically and never reused, so this is one past the
    /// highest number ever frozen — never one past the count, which
    /// a gap in the recorded numbers would send backwards into a
    /// collision.
    pub fn next_version_number(&self) -> u64 {
        self.versions
            .iter()
            .map(|version| version.number())
            .max()
            .map_or(1, |highest| highest + 1)
    }

    /// The number of applied changes, for optimistic version checks.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Add a Spec to the membership, appending it to the end of the
    /// display order. Refused outside draft and for a Spec already on
    /// the order.
    pub fn add_spec(&mut self, spec: SpecNumber) -> Result<(), PlanError> {
        self.require_draft()?;
        if self.order.contains(&spec) {
            return Err(PlanError::DuplicateMember(spec));
        }
        self.order.push(spec);
        self.applied();
        Ok(())
    }

    /// Remove a Spec from the membership and the display order. A Spec
    /// that still carries dependency edges is refused: the graph is
    /// edited separately and deliberately.
    pub fn remove_spec(&mut self, spec: SpecNumber) -> Result<(), PlanError> {
        self.require_draft()?;
        if self
            .edges
            .iter()
            .any(|edge| edge.from() == spec || edge.to() == spec)
        {
            return Err(PlanError::MemberCarriesEdges(spec));
        }
        self.order
            .iter()
            .position(|member| *member == spec)
            .map(|position| self.order.remove(position))
            .ok_or(PlanError::NotAMember(spec))?;
        self.applied();
        Ok(())
    }

    /// Move a Spec to `position` in the display order, leaving the
    /// dependency edges untouched.
    pub fn move_spec(&mut self, spec: SpecNumber, position: usize) -> Result<(), PlanError> {
        self.require_draft()?;
        let current = self
            .order
            .iter()
            .position(|member| *member == spec)
            .ok_or(PlanError::NotAMember(spec))?;
        if position >= self.order.len() {
            return Err(PlanError::PositionOutOfRange { position });
        }
        let member = self.order.remove(current);
        self.order.insert(position, member);
        self.applied();
        Ok(())
    }

    /// Add a dependency edge: `from` must land before `to`. Both
    /// endpoints must be members of this one Plan; an edge reaching
    /// outside it is rejected in the domain layer (DR-DE-01).
    pub fn add_edge(&mut self, from: SpecNumber, to: SpecNumber) -> Result<(), PlanError> {
        self.require_draft()?;
        if from == to {
            return Err(PlanError::SelfEdge);
        }
        if !self.order.contains(&from) || !self.order.contains(&to) {
            return Err(PlanError::EdgeOutsideSinglePlan { from, to });
        }
        let edge = DependencyEdge::new(from, to);
        if self.edges.contains(&edge) {
            return Err(PlanError::DuplicateEdge);
        }
        self.edges.push(edge);
        self.applied();
        Ok(())
    }

    /// Remove a dependency edge.
    pub fn remove_edge(&mut self, from: SpecNumber, to: SpecNumber) -> Result<(), PlanError> {
        self.require_draft()?;
        let edge = DependencyEdge::new(from, to);
        self.edges
            .iter()
            .position(|held| *held == edge)
            .map(|position| self.edges.remove(position))
            .ok_or(PlanError::EdgeNotFound)?;
        self.applied();
        Ok(())
    }

    /// Activate a draft: freeze the Spec membership, display order,
    /// and dependency graph into an immutable version, and move to
    /// the active state. The frozen shape is returned as it was
    /// minted.
    pub fn activate(&mut self) -> Result<PlanVersion, PlanError> {
        self.require_draft()?;
        if self.order.is_empty() {
            return Err(PlanError::EmptyMembership);
        }
        let frozen = PlanVersion::new(
            self.next_version_number(),
            self.order.clone(),
            self.edges.clone(),
        );
        self.versions.push(frozen.clone());
        self.state = PlanState::Active;
        self.applied();
        Ok(frozen)
    }

    /// Replan an active Plan: reopen the draft for editing from the
    /// frozen shape and reserve the number the replacement version
    /// will freeze under. Prior versions stay frozen and queryable.
    pub fn replan(&mut self) -> Result<u64, PlanError> {
        if self.state != PlanState::Active {
            return Err(PlanError::RequiresActive);
        }
        self.state = PlanState::Draft;
        self.applied();
        Ok(self.next_version_number())
    }

    /// Complete an active Plan.
    pub fn complete(&mut self) -> Result<(), PlanError> {
        if self.state != PlanState::Active {
            return Err(PlanError::RequiresActive);
        }
        self.state = PlanState::Complete;
        self.applied();
        Ok(())
    }

    /// Cancel a draft or active Plan.
    pub fn cancel(&mut self) -> Result<(), PlanError> {
        if matches!(
            self.state,
            PlanState::Complete | PlanState::Cancelled | PlanState::Archived
        ) {
            return Err(PlanError::RequiresOpen);
        }
        self.state = PlanState::Cancelled;
        self.applied();
        Ok(())
    }

    /// Archive a Plan from any non-archived state. Archived is
    /// terminal; every recorded fact, frozen versions included, is
    /// preserved.
    pub fn archive(&mut self) -> Result<(), PlanError> {
        if self.state == PlanState::Archived {
            return Err(PlanError::AlreadyArchived);
        }
        self.state = PlanState::Archived;
        self.applied();
        Ok(())
    }

    /// Refuse every shape edit outside draft.
    fn require_draft(&self) -> Result<(), PlanError> {
        if self.state == PlanState::Draft {
            return Ok(());
        }
        Err(PlanError::RequiresDraft)
    }

    /// Count one applied change.
    fn applied(&mut self) {
        self.version += 1;
    }
}

#[cfg(test)]
mod spec_number {
    use super::{SpecNumber, SpecNumberError};

    #[test]
    fn a_spec_number_starts_at_one() {
        assert_eq!(SpecNumber::new(0), Err(SpecNumberError::Zero));

        let first = SpecNumber::new(1).expect("one is the first Spec number");
        assert_eq!(first.value(), 1);
        assert_eq!(
            SpecNumber::new(41)
                .expect("any positive number is a Spec number")
                .value(),
            41
        );
    }
}

#[cfg(test)]
mod plan_graph {
    use super::{DependencyEdge, Plan, PlanError, PlanId, PlanState, ProjectId, SpecNumber};

    fn spec(number: u64) -> SpecNumber {
        SpecNumber::new(number).expect("the fixture number is positive")
    }

    fn edge(from: u64, to: u64) -> DependencyEdge {
        DependencyEdge::new(spec(from), spec(to))
    }

    /// A draft plan holding the three Specs the graph tests vary.
    fn draft() -> Plan {
        let mut plan = Plan::new(PlanId::new(1), ProjectId::new(7), 1);
        for number in [1, 2, 3] {
            plan.add_spec(spec(number))
                .expect("the fixture membership lands");
        }
        plan
    }

    #[test]
    fn a_fresh_plan_holds_an_empty_graph_at_version_one() {
        let plan = Plan::new(PlanId::new(4), ProjectId::new(7), 2);

        assert_eq!(plan.id(), PlanId::new(4));
        assert_eq!(plan.project(), ProjectId::new(7));
        assert_eq!(plan.number(), 2);
        assert_eq!(plan.state(), PlanState::Draft);
        assert!(plan.order().is_empty(), "nothing is on display yet");
        assert!(plan.edges().is_empty(), "no dependency exists yet");
        assert!(plan.versions().is_empty(), "nothing is frozen yet");
        assert_eq!(plan.version(), 1);
    }

    #[test]
    fn adding_a_spec_appends_to_the_display_order() {
        let mut plan = Plan::new(PlanId::new(1), ProjectId::new(7), 1);

        plan.add_spec(spec(2)).expect("the first Spec joins");
        plan.add_spec(spec(1)).expect("the second Spec joins");

        assert_eq!(plan.order(), [spec(2), spec(1)].as_slice());
        assert_eq!(plan.version(), 3, "every edit is an applied change");
    }

    #[test]
    fn adding_a_duplicate_spec_is_refused() {
        let mut plan = draft();
        let version = plan.version();

        assert_eq!(
            plan.add_spec(spec(2)),
            Err(PlanError::DuplicateMember(spec(2)))
        );
        assert_eq!(plan.version(), version, "the refusal changed nothing");
    }

    #[test]
    fn removing_a_spec_leaves_the_display_order() {
        let mut plan = draft();

        plan.remove_spec(spec(2))
            .expect("a member without edges leaves");

        assert_eq!(plan.order(), [spec(1), spec(3)].as_slice());
    }

    #[test]
    fn removing_a_spec_that_carries_edges_is_refused() {
        let mut plan = draft();
        plan.add_edge(spec(1), spec(2))
            .expect("the fixture edge lands");
        let version = plan.version();

        assert_eq!(
            plan.remove_spec(spec(2)),
            Err(PlanError::MemberCarriesEdges(spec(2)))
        );
        assert_eq!(plan.version(), version, "the refusal changed nothing");
    }

    #[test]
    fn removing_an_unknown_spec_is_refused() {
        let mut plan = draft();

        assert_eq!(
            plan.remove_spec(spec(9)),
            Err(PlanError::NotAMember(spec(9)))
        );
    }

    #[test]
    fn moving_a_spec_changes_only_the_display_order() {
        let mut plan = draft();
        plan.add_edge(spec(1), spec(3))
            .expect("the fixture edge lands");

        plan.move_spec(spec(3), 0).expect("the move applies");

        assert_eq!(
            plan.order(),
            [spec(3), spec(1), spec(2)].as_slice(),
            "the moved Spec heads the display order"
        );
        assert_eq!(
            plan.edges(),
            [edge(1, 3)].as_slice(),
            "the dependency edges are a separate relation and do not move"
        );
    }

    #[test]
    fn moving_a_spec_out_of_range_is_refused() {
        let mut plan = draft();
        let version = plan.version();

        assert_eq!(
            plan.move_spec(spec(1), 3),
            Err(PlanError::PositionOutOfRange { position: 3 })
        );
        assert_eq!(plan.version(), version, "the refusal changed nothing");
    }

    #[test]
    fn an_edge_joins_two_members() {
        let mut plan = draft();

        plan.add_edge(spec(1), spec(2))
            .expect("members may depend on each other");

        assert_eq!(plan.edges(), [edge(1, 2)].as_slice());
    }

    #[test]
    fn a_self_edge_is_refused() {
        let mut plan = draft();

        assert_eq!(
            plan.add_edge(spec(1), spec(1)),
            Err(PlanError::SelfEdge),
            "a Spec never depends on itself"
        );
    }

    #[test]
    fn a_duplicate_edge_is_refused() {
        let mut plan = draft();
        plan.add_edge(spec(1), spec(2))
            .expect("the fixture edge lands");

        assert_eq!(
            plan.add_edge(spec(1), spec(2)),
            Err(PlanError::DuplicateEdge)
        );
    }

    #[test]
    fn an_edge_outside_the_single_plan_is_refused() {
        let mut plan = draft();
        let version = plan.version();

        // Spec 9 belongs to another Plan, or to no Plan at all: either
        // way the edge leaves this one (DR-DE-01).
        assert_eq!(
            plan.add_edge(spec(1), spec(9)),
            Err(PlanError::EdgeOutsideSinglePlan {
                from: spec(1),
                to: spec(9),
            })
        );
        assert_eq!(
            plan.add_edge(spec(9), spec(1)),
            Err(PlanError::EdgeOutsideSinglePlan {
                from: spec(9),
                to: spec(1),
            })
        );
        assert_eq!(plan.version(), version, "the refusal changed nothing");
    }

    #[test]
    fn removing_an_edge_frees_both_endpoints() {
        let mut plan = draft();
        plan.add_edge(spec(1), spec(2))
            .expect("the fixture edge lands");

        plan.remove_edge(spec(1), spec(2)).expect("the edge leaves");
        plan.remove_spec(spec(2))
            .expect("the endpoint is free to leave");

        assert_eq!(plan.order(), [spec(1), spec(3)].as_slice());
        assert!(plan.edges().is_empty());
    }

    #[test]
    fn removing_a_missing_edge_is_refused() {
        let mut plan = draft();

        assert_eq!(
            plan.remove_edge(spec(1), spec(2)),
            Err(PlanError::EdgeNotFound)
        );
    }
}

#[cfg(test)]
mod cycle_detection {
    use super::{
        DependencyCycle, DependencyEdge, Plan, PlanId, PlanShape, PlanState, ProjectId, SpecNumber,
        cycles_in,
    };

    fn spec(number: u64) -> SpecNumber {
        SpecNumber::new(number).expect("the fixture number is positive")
    }

    fn edge(from: u64, to: u64) -> DependencyEdge {
        DependencyEdge::new(spec(from), spec(to))
    }

    fn cycle(numbers: &[u64]) -> DependencyCycle {
        DependencyCycle::new(numbers.iter().map(|number| spec(*number)).collect())
    }

    /// The cycles a graph holds, as the numbers its rings carry.
    fn rings(order: &[u64], edges: &[DependencyEdge]) -> Vec<Vec<u64>> {
        let members: Vec<SpecNumber> = order.iter().map(|number| spec(*number)).collect();
        cycles_in(&members, edges)
            .iter()
            .map(|found| {
                found
                    .specs()
                    .iter()
                    .map(|member| member.value())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn an_acyclic_graph_reports_no_cycle() {
        // A chain and a diamond: every edge runs one way.
        assert_eq!(
            rings(&[1, 2, 3], &[edge(1, 2), edge(2, 3), edge(1, 3)]),
            Vec::<Vec<u64>>::new()
        );
        assert_eq!(rings(&[1, 2], &[]), Vec::<Vec<u64>>::new());
    }

    #[test]
    fn a_ring_of_two_specs_is_one_cycle() {
        assert_eq!(rings(&[1, 2], &[edge(1, 2), edge(2, 1)]), [[1, 2]]);
    }

    #[test]
    fn a_ring_of_three_specs_reports_its_members_ascending() {
        assert_eq!(
            rings(&[3, 1, 2], &[edge(3, 1), edge(1, 2), edge(2, 3)]),
            [[1, 2, 3]],
            "the members are named ascending, whatever the display order"
        );
    }

    #[test]
    fn disjoint_rings_report_separately_in_member_order() {
        assert_eq!(
            rings(
                &[4, 2, 1, 3],
                &[edge(4, 3), edge(3, 4), edge(1, 2), edge(2, 1)]
            ),
            [[1, 2], [3, 4]],
            "each ring is one diagnostic, ordered by its first member"
        );
    }

    #[test]
    fn overlapping_rings_report_as_one_group() {
        // 1 ↔ 2 and 2 ↔ 3 share Spec 2: every member reaches every
        // other, so one group names the whole locked set.
        assert_eq!(
            rings(
                &[1, 2, 3],
                &[edge(1, 2), edge(2, 1), edge(2, 3), edge(3, 2)]
            ),
            [[1, 2, 3]]
        );
    }

    #[test]
    fn a_ring_reached_only_downstream_reports_too() {
        // The ring sits below Spec 4, not around it: 4 stays out.
        assert_eq!(
            rings(&[4, 1, 2], &[edge(4, 1), edge(1, 2), edge(2, 1)]),
            [[1, 2]]
        );
    }

    #[test]
    fn a_stored_self_edge_is_a_cycle_of_one() {
        // add_edge refuses a self edge; a rehydrated shape may still
        // hold one, and it names a Spec that can never land.
        assert_eq!(rings(&[1], &[edge(1, 1)]), [[1]]);
    }

    #[test]
    fn edges_leaving_the_membership_shape_no_cycle() {
        // Storage hands the membership and the edges as two rows; an
        // edge naming a non-member is not a ring among these Specs.
        assert_eq!(
            rings(&[1, 2], &[edge(1, 9), edge(9, 1)]),
            Vec::<Vec<u64>>::new()
        );
    }

    #[test]
    fn the_working_graph_and_frozen_versions_report_their_own_cycles() {
        let mut plan = Plan::new(PlanId::new(1), ProjectId::new(7), 1);
        for number in [1, 2] {
            plan.add_spec(spec(number))
                .expect("the fixture membership lands");
        }
        plan.add_edge(spec(1), spec(2))
            .expect("the fixture edge lands");
        plan.add_edge(spec(2), spec(1))
            .expect("the closing edge lands");

        assert_eq!(plan.cycles(), [cycle(&[1, 2])]);

        plan.activate().expect("the ring freezes");
        plan.replan().expect("the draft reopens");
        plan.remove_edge(spec(2), spec(1)).expect("the ring breaks");

        assert!(
            plan.cycles().is_empty(),
            "the working graph reports its own shape"
        );
        assert_eq!(
            plan.versions()[0].cycles(),
            [cycle(&[1, 2])],
            "the frozen version keeps the ring it froze"
        );
    }

    #[test]
    fn a_rehydrated_shape_reports_its_cycles() {
        let plan = Plan::restore(
            PlanId::new(1),
            ProjectId::new(7),
            1,
            PlanState::Active,
            PlanShape::new(vec![spec(1), spec(2)], vec![edge(1, 2), edge(2, 1)]),
            Vec::new(),
            5,
        );

        assert_eq!(plan.cycles(), [cycle(&[1, 2])]);
    }
}

#[cfg(test)]
mod plan_lifecycle {
    use super::{
        DependencyEdge, Plan, PlanError, PlanId, PlanShape, PlanState, PlanVersion, ProjectId,
        SpecNumber,
    };

    fn spec(number: u64) -> SpecNumber {
        SpecNumber::new(number).expect("the fixture number is positive")
    }

    fn edge(from: u64, to: u64) -> DependencyEdge {
        DependencyEdge::new(spec(from), spec(to))
    }

    /// A draft plan holding an order of 1, 3, 2 with 1 → 2 and 3 → 2.
    fn shaped() -> Plan {
        let mut plan = Plan::new(PlanId::new(1), ProjectId::new(7), 1);
        for number in [1, 3, 2] {
            plan.add_spec(spec(number))
                .expect("the fixture membership lands");
        }
        plan.add_edge(spec(1), spec(2))
            .expect("the fixture edge lands");
        plan.add_edge(spec(3), spec(2))
            .expect("the fixture edge lands");
        plan
    }

    /// The shape `shaped` freezes to at activation.
    fn frozen_shape(number: u64) -> PlanVersion {
        PlanVersion::new(
            number,
            vec![spec(1), spec(3), spec(2)],
            vec![edge(1, 2), edge(3, 2)],
        )
    }

    #[test]
    fn a_fresh_plan_is_a_draft() {
        let plan = Plan::new(PlanId::new(1), ProjectId::new(7), 1);

        assert_eq!(plan.state(), PlanState::Draft);
        assert!(!plan.state().is_terminal());
    }

    #[test]
    fn activating_freezes_membership_order_and_graph_into_a_version() {
        let mut plan = shaped();

        let frozen = plan.activate().expect("a shaped draft activates");

        assert_eq!(plan.state(), PlanState::Active);
        assert_eq!(frozen, frozen_shape(1), "the freeze is the whole shape");
        assert_eq!(plan.versions(), [frozen_shape(1)].as_slice());
        assert_eq!(plan.next_version_number(), 2);
    }

    #[test]
    fn activating_an_empty_plan_is_refused() {
        let mut plan = Plan::new(PlanId::new(1), ProjectId::new(7), 1);
        let version = plan.version();

        assert_eq!(plan.activate(), Err(PlanError::EmptyMembership));
        assert_eq!(plan.version(), version, "the refusal changed nothing");
        assert!(plan.versions().is_empty(), "nothing froze");
    }

    #[test]
    fn activating_twice_is_refused() {
        let mut plan = shaped();
        plan.activate().expect("the first activation freezes");
        let version = plan.version();

        assert_eq!(plan.activate().unwrap_err(), PlanError::RequiresDraft);
        assert_eq!(plan.version(), version, "the refusal changed nothing");
        assert_eq!(plan.versions().len(), 1, "no second freeze may land");
    }

    #[test]
    fn editing_a_frozen_plan_is_refused() {
        let mut plan = shaped();
        plan.activate().expect("the activation freezes");
        let version = plan.version();

        assert_eq!(
            plan.add_spec(spec(4)).unwrap_err(),
            PlanError::RequiresDraft
        );
        assert_eq!(
            plan.remove_spec(spec(1)).unwrap_err(),
            PlanError::RequiresDraft
        );
        assert_eq!(
            plan.move_spec(spec(1), 0).unwrap_err(),
            PlanError::RequiresDraft
        );
        assert_eq!(
            plan.add_edge(spec(1), spec(3)).unwrap_err(),
            PlanError::RequiresDraft
        );
        assert_eq!(
            plan.remove_edge(spec(1), spec(2)).unwrap_err(),
            PlanError::RequiresDraft
        );
        assert_eq!(plan.version(), version, "the refusals changed nothing");
        assert_eq!(plan.versions(), [frozen_shape(1)].as_slice());
    }

    #[test]
    fn the_frozen_version_survives_replanning_and_later_edits() {
        let mut plan = shaped();
        plan.activate().expect("the first activation freezes");

        plan.replan().expect("the draft reopens");
        plan.move_spec(spec(2), 0).expect("the shape changes");
        plan.remove_edge(spec(1), spec(2))
            .expect("the graph changes");
        plan.add_spec(spec(4)).expect("the membership changes");
        let second = plan.activate().expect("the replacement freezes");

        assert_eq!(
            plan.versions(),
            [frozen_shape(1), second.clone()].as_slice(),
            "the first version is unchanged beside its replacement"
        );
        assert_ne!(second, frozen_shape(1));
        assert_eq!(second.number(), 2);
        assert_eq!(
            second.order(),
            [spec(2), spec(1), spec(3), spec(4)].as_slice()
        );
        assert_eq!(second.edges(), [edge(3, 2)].as_slice());
    }

    #[test]
    fn replanning_reopens_the_draft_and_reserves_the_next_version() {
        let mut plan = shaped();
        plan.activate().expect("the activation freezes");
        let version = plan.version();

        let reserved = plan.replan().expect("an active Plan replans");

        assert_eq!(plan.state(), PlanState::Draft);
        assert_eq!(reserved, 2, "the replacement version is reserved");
        assert_eq!(plan.next_version_number(), 2);
        assert_eq!(
            plan.versions(),
            [frozen_shape(1)].as_slice(),
            "reserving freezes nothing"
        );
        assert_eq!(plan.version(), version + 1);
        assert_eq!(
            plan.order(),
            [spec(1), spec(3), spec(2)].as_slice(),
            "the reopened draft carries the frozen shape to edit from"
        );
    }

    #[test]
    fn replanning_is_refused_outside_the_active_state() {
        let mut fresh = Plan::new(PlanId::new(1), ProjectId::new(7), 1);
        assert_eq!(fresh.replan().unwrap_err(), PlanError::RequiresActive);

        let mut plan = shaped();
        plan.activate().expect("the activation freezes");
        plan.complete().expect("the Plan completes");
        assert_eq!(plan.replan().unwrap_err(), PlanError::RequiresActive);
    }

    #[test]
    fn a_gap_in_frozen_numbers_mints_one_past_the_highest() {
        // Versions one and three with no two: a count would mint
        // three again and collide with the recorded version.
        let mut plan = Plan::restore(
            PlanId::new(1),
            ProjectId::new(7),
            1,
            PlanState::Active,
            PlanShape::new(
                vec![spec(1), spec(3), spec(2)],
                vec![edge(1, 2), edge(3, 2)],
            ),
            vec![frozen_shape(1), frozen_shape(3)],
            9,
        );

        assert_eq!(plan.next_version_number(), 4);
        assert_eq!(
            plan.replan().expect("an active Plan replans"),
            4,
            "the replacement reserves one past the highest frozen number"
        );

        let replacement = plan.activate().expect("the replacement freezes");

        assert_eq!(replacement.number(), 4);
        assert_eq!(
            plan.versions(),
            [frozen_shape(1), frozen_shape(3), frozen_shape(4)].as_slice(),
            "the minted number never collides with a recorded one"
        );
    }

    #[test]
    fn completing_requires_the_active_state() {
        let mut plan = shaped();
        let version = plan.version();
        assert_eq!(plan.complete().unwrap_err(), PlanError::RequiresActive);
        assert_eq!(plan.version(), version, "the refusal changed nothing");

        plan.activate().expect("the activation freezes");
        plan.complete().expect("an active Plan completes");

        assert_eq!(plan.state(), PlanState::Complete);
        assert_eq!(plan.complete().unwrap_err(), PlanError::RequiresActive);
    }

    #[test]
    fn cancelling_leaves_draft_or_active() {
        let mut draft = shaped();
        draft.cancel().expect("a draft cancels");
        assert_eq!(draft.state(), PlanState::Cancelled);

        let mut plan = shaped();
        plan.activate().expect("the activation freezes");
        plan.cancel().expect("an active Plan cancels");
        assert_eq!(plan.state(), PlanState::Cancelled);

        let mut complete = shaped();
        complete.activate().expect("the activation freezes");
        complete.complete().expect("the Plan completes");
        assert_eq!(complete.cancel().unwrap_err(), PlanError::RequiresOpen);
    }

    #[test]
    fn archiving_is_legal_from_every_non_archived_state() {
        for state in [
            PlanState::Draft,
            PlanState::Active,
            PlanState::Complete,
            PlanState::Cancelled,
        ] {
            let mut plan = shaped();
            match state {
                PlanState::Active => {
                    plan.activate().expect("the fixture activates");
                }
                PlanState::Complete => {
                    plan.activate().expect("the fixture activates");
                    plan.complete().expect("the fixture completes");
                }
                PlanState::Cancelled => {
                    plan.cancel().expect("the fixture cancels");
                }
                PlanState::Draft => {}
                PlanState::Archived => unreachable!("the loop names non-archived states"),
            }

            plan.archive().expect("every open state archives");

            assert_eq!(plan.state(), PlanState::Archived);
            assert!(plan.state().is_terminal());
            assert_eq!(plan.archive().unwrap_err(), PlanError::AlreadyArchived);
        }
    }

    #[test]
    fn archived_is_terminal_for_every_operation() {
        let mut plan = shaped();
        plan.archive().expect("the Plan archives");
        let version = plan.version();

        assert_eq!(
            plan.add_spec(spec(4)).unwrap_err(),
            PlanError::RequiresDraft
        );
        assert_eq!(plan.activate().unwrap_err(), PlanError::RequiresDraft);
        assert_eq!(plan.replan().unwrap_err(), PlanError::RequiresActive);
        assert_eq!(plan.complete().unwrap_err(), PlanError::RequiresActive);
        assert_eq!(plan.cancel().unwrap_err(), PlanError::RequiresOpen);
        assert_eq!(plan.archive().unwrap_err(), PlanError::AlreadyArchived);
        assert_eq!(plan.version(), version, "the refusals changed nothing");
    }

    #[test]
    fn only_the_terminal_states_leave_the_active_surface() {
        assert!(!PlanState::Draft.is_terminal());
        assert!(!PlanState::Active.is_terminal());
        assert!(PlanState::Complete.is_terminal());
        assert!(PlanState::Cancelled.is_terminal());
        assert!(PlanState::Archived.is_terminal());
    }

    #[test]
    fn restore_rehydrates_every_recorded_fact() {
        let plan = Plan::restore(
            PlanId::new(9),
            ProjectId::new(4),
            3,
            PlanState::Active,
            PlanShape::new(vec![spec(2), spec(1)], vec![edge(2, 1)]),
            vec![PlanVersion::new(
                1,
                vec![spec(2), spec(1)],
                vec![edge(2, 1)],
            )],
            11,
        );

        assert_eq!(plan.id().value(), 9);
        assert_eq!(plan.project().value(), 4);
        assert_eq!(plan.number(), 3);
        assert_eq!(plan.state(), PlanState::Active);
        assert_eq!(plan.order(), [spec(2), spec(1)].as_slice());
        assert_eq!(plan.edges(), [edge(2, 1)].as_slice());
        assert_eq!(plan.versions().len(), 1);
        assert_eq!(plan.version(), 11);
        assert_eq!(plan.next_version_number(), 2);
    }
}
