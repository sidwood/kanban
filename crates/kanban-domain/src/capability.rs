//! Run-scoped capabilities (CONTEXT.md, DR-HB-17, DR-SS-14): the
//! minted token that bounds what one dispatched run's agent may do.
//! A capability is minted with a won Dispatch Request claim, binds
//! the Ticket, Lane, role, reviewer slot, and the permitted MCP
//! operations of that run, and is narrower than operator authority.
//! Expiry rides run settlement: settling is one-way, nothing renews,
//! and a settled capability permits nothing.

use std::collections::BTreeSet;
use std::fmt;

use crate::dispatch::DispatchRequestId;
use crate::lane::LaneId;
use crate::ticket::TicketId;

/// The identity of one minted capability. Assigned once by storage
/// and immutable afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CapabilityId(u64);

impl CapabilityId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The identity of one Review Slot, the reviewer position a reviewer
/// capability binds. Opaque here; review stages are KAN-S10's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReviewerSlotId(u64);

impl ReviewerSlotId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ReviewerSlotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The run role a capability grants: an implementer executes the
/// Ticket; a reviewer occupies one Review Slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityRole {
    /// Executing an Implementation or Bug Ticket in a Lane.
    Implementer,
    /// Reviewing one Review Slot.
    Reviewer,
}

impl CapabilityRole {
    /// The stored and wire name of this role.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Implementer => "implementer",
            Self::Reviewer => "reviewer",
        }
    }

    /// The role a stored row names, or `None` outside the
    /// vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        match stored {
            "implementer" => Some(Self::Implementer),
            "reviewer" => Some(Self::Reviewer),
            _ => None,
        }
    }
}

impl fmt::Display for CapabilityRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.wire_name())
    }
}

/// The closed capability status vocabulary: live from the mint until
/// run settlement expires it, settled from then on. Settled is
/// one-way; capabilities are never renewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityStatus {
    /// Live from the mint until the run settles.
    Active,
    /// Expired with run settlement. Terminal.
    Settled,
}

impl CapabilityStatus {
    /// The stored and wire name of this status.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Settled => "settled",
        }
    }

    /// The status a stored row names, or `None` outside the
    /// vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        match stored {
            "active" => Some(Self::Active),
            "settled" => Some(Self::Settled),
            _ => None,
        }
    }

    /// Whether this status still authorises anything.
    pub fn is_live(self) -> bool {
        matches!(self, Self::Active)
    }
}

/// Why a capability rule was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityError {
    /// A reviewer capability binds the Review Slot it occupies.
    ReviewerRoleRequiresSlot,
    /// An implementer capability binds no Review Slot.
    SlotRequiresReviewerRole,
    /// A capability with no permitted operations authorises
    /// nothing, so it is never minted.
    NoOperations,
    /// An operation name that is blank cannot be granted.
    BlankOperation { operation: String },
    /// The named operation sits outside the surface any capability
    /// may ever name, so minting it would widen agent authority past
    /// what the operator allows.
    OperationOutsideSurface { operation: String },
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReviewerRoleRequiresSlot => {
                write!(f, "a reviewer capability binds the Review Slot it occupies")
            }
            Self::SlotRequiresReviewerRole => {
                write!(f, "an implementer capability binds no Review Slot")
            }
            Self::NoOperations => {
                write!(
                    f,
                    "a capability with no permitted operations authorises nothing"
                )
            }
            Self::BlankOperation { operation } => {
                write!(f, "an operation name may not be blank: `{operation}`")
            }
            Self::OperationOutsideSurface { operation } => write!(
                f,
                "the operation `{operation}` is outside the surface any capability may name"
            ),
        }
    }
}

impl std::error::Error for CapabilityError {}

/// Why one operation was refused against a capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityRefusal {
    /// The capability expired with run settlement; it authorises
    /// nothing, not even operations it once permitted.
    Settled { capability: CapabilityId },
    /// The operation is outside the minted set.
    NotPermitted { operation: String },
}

impl fmt::Display for CapabilityRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Settled { capability } => write!(
                f,
                "the capability {capability} expired with run settlement and permits nothing"
            ),
            Self::NotPermitted { operation } => {
                write!(f, "the operation `{operation}` is outside the minted set")
            }
        }
    }
}

impl std::error::Error for CapabilityRefusal {}

/// What a capability binds (DR-HB-17): the Ticket, the Lane the run
/// executes in, the run role, and — for a reviewer — the Review Slot
/// occupied. Immutable from the mint on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityScope {
    ticket: TicketId,
    lane: LaneId,
    role: CapabilityRole,
    reviewer_slot: Option<ReviewerSlotId>,
}

impl CapabilityScope {
    /// Bind a fresh scope. A reviewer names the slot occupied; an
    /// implementer names none.
    pub fn new(
        ticket: TicketId,
        lane: LaneId,
        role: CapabilityRole,
        reviewer_slot: Option<ReviewerSlotId>,
    ) -> Result<Self, CapabilityError> {
        match (role, reviewer_slot) {
            (CapabilityRole::Reviewer, None) => Err(CapabilityError::ReviewerRoleRequiresSlot),
            (CapabilityRole::Implementer, Some(_)) => {
                Err(CapabilityError::SlotRequiresReviewerRole)
            }
            _ => Ok(Self {
                ticket,
                lane,
                role,
                reviewer_slot,
            }),
        }
    }

    /// Rehydrate a stored scope exactly as it was recorded.
    pub fn restore(
        ticket: TicketId,
        lane: LaneId,
        role: CapabilityRole,
        reviewer_slot: Option<ReviewerSlotId>,
    ) -> Self {
        Self {
            ticket,
            lane,
            role,
            reviewer_slot,
        }
    }

    /// The Ticket the run executes.
    pub fn ticket(&self) -> TicketId {
        self.ticket
    }

    /// The Lane the run executes in.
    pub fn lane(&self) -> LaneId {
        self.lane
    }

    /// The run role granted.
    pub fn role(&self) -> CapabilityRole {
        self.role
    }

    /// The Review Slot occupied, for a reviewer.
    pub fn reviewer_slot(&self) -> Option<ReviewerSlotId> {
        self.reviewer_slot
    }
}

/// The permitted MCP operation set one capability grants: canonical
/// (trimmed, deduplicated, ordered) so a minted grant has exactly one
/// stored shape and verification answers by membership alone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpOperations(BTreeSet<String>);

impl McpOperations {
    /// Grant `names`, refusing blanks and an empty set. The stored
    /// grant is the canonical form: trimmed, deduplicated, ordered.
    pub fn new(
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, CapabilityError> {
        let mut granted = BTreeSet::new();
        for name in names {
            let name = name.into();
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return Err(CapabilityError::BlankOperation { operation: name });
            }
            granted.insert(trimmed.to_owned());
        }
        if granted.is_empty() {
            return Err(CapabilityError::NoOperations);
        }
        Ok(Self(granted))
    }

    /// Rehydrate a stored grant exactly as it was recorded.
    pub fn restore(names: Vec<String>) -> Self {
        Self(names.into_iter().collect())
    }

    /// Whether `operation` is inside the grant.
    pub fn contains(&self, operation: &str) -> bool {
        self.0.contains(operation)
    }

    /// The canonical operation names, ordered.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    /// How many operations the grant carries.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the grant carries nothing.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// One minted, run-scoped capability (DR-SS-14): narrower than
/// operator authority, live until run settlement settles it, and
/// never renewed — a later run mints its own fresh capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    id: CapabilityId,
    dispatch: DispatchRequestId,
    scope: CapabilityScope,
    operations: McpOperations,
    status: CapabilityStatus,
}

impl Capability {
    /// Mint a live capability bound to `dispatch`'s run. The
    /// operations must already sit inside the agent surface
    /// ([`enforce_within_surface`]); nothing here can widen
    /// authority past what the caller allows.
    pub fn mint(
        id: CapabilityId,
        dispatch: DispatchRequestId,
        scope: CapabilityScope,
        operations: McpOperations,
    ) -> Result<Self, CapabilityError> {
        if operations.is_empty() {
            return Err(CapabilityError::NoOperations);
        }
        Ok(Self {
            id,
            dispatch,
            scope,
            operations,
            status: CapabilityStatus::Active,
        })
    }

    /// Rehydrate a stored capability exactly as it was recorded.
    pub fn restore(
        id: CapabilityId,
        dispatch: DispatchRequestId,
        scope: CapabilityScope,
        operations: McpOperations,
        status: CapabilityStatus,
    ) -> Self {
        Self {
            id,
            dispatch,
            scope,
            operations,
            status,
        }
    }

    /// The immutable identity.
    pub fn id(&self) -> CapabilityId {
        self.id
    }

    /// The Dispatch Request whose won claim minted this capability.
    pub fn dispatch(&self) -> DispatchRequestId {
        self.dispatch
    }

    /// What the capability binds.
    pub fn scope(&self) -> &CapabilityScope {
        &self.scope
    }

    /// The permitted MCP operation set.
    pub fn operations(&self) -> &McpOperations {
        &self.operations
    }

    /// Live or settled.
    pub fn status(&self) -> CapabilityStatus {
        self.status
    }

    /// Whether `operation` is permitted: the capability must still be
    /// live and the operation inside the minted set. A settled
    /// capability refuses everything (DR-SS-14).
    pub fn permits(&self, operation: &str) -> Result<(), CapabilityRefusal> {
        if !self.status.is_live() {
            return Err(CapabilityRefusal::Settled {
                capability: self.id,
            });
        }
        if !self.operations.contains(operation) {
            return Err(CapabilityRefusal::NotPermitted {
                operation: operation.to_owned(),
            });
        }
        Ok(())
    }

    /// Expire with run settlement. One-way and idempotent: settling
    /// twice changes nothing, and no path exists back to active —
    /// renewal is refused by there being nothing to renew.
    pub fn settle(&mut self) {
        self.status = CapabilityStatus::Settled;
    }
}

/// Refuse operations outside `surface`, the closed set of MCP
/// operations any capability may ever name. This is the
/// narrower-than-operator-authority rule: what sits outside the
/// surface is operator authority, and no mint may grant it.
pub fn enforce_within_surface(
    operations: &McpOperations,
    surface: &McpOperations,
) -> Result<(), CapabilityError> {
    for operation in operations.iter() {
        if !surface.contains(operation) {
            return Err(CapabilityError::OperationOutsideSurface {
                operation: operation.to_owned(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod capability_scope {
    use super::{
        Capability, CapabilityError, CapabilityId, CapabilityRefusal, CapabilityRole,
        CapabilityScope, CapabilityStatus, McpOperations, ReviewerSlotId, enforce_within_surface,
    };
    use crate::dispatch::DispatchRequestId;
    use crate::lane::LaneId;
    use crate::ticket::TicketId;

    fn implementer_scope() -> CapabilityScope {
        CapabilityScope::new(
            TicketId::new(7),
            LaneId::new(2),
            CapabilityRole::Implementer,
            None,
        )
        .expect("an implementer scope binds no slot")
    }

    fn surface() -> McpOperations {
        McpOperations::new(["ticket.get", "spec.get", "timeline.query"])
            .expect("the fixture surface validates")
    }

    fn minted() -> Capability {
        Capability::mint(
            CapabilityId::new(11),
            DispatchRequestId::new(4),
            implementer_scope(),
            McpOperations::new(["timeline.query", "ticket.get", "spec.get", "ticket.get"])
                .expect("the grant validates"),
        )
        .expect("the capability mints")
    }

    #[test]
    fn mint_binds_ticket_lane_role_slot_and_operations() {
        let scope = CapabilityScope::new(
            TicketId::new(9),
            LaneId::new(3),
            CapabilityRole::Reviewer,
            Some(ReviewerSlotId::new(5)),
        )
        .expect("a reviewer scope binds its slot");
        let capability = Capability::mint(
            CapabilityId::new(21),
            DispatchRequestId::new(6),
            scope.clone(),
            McpOperations::new(["ticket.get"]).expect("the grant validates"),
        )
        .expect("the capability mints");

        assert_eq!(capability.id(), CapabilityId::new(21));
        assert_eq!(capability.dispatch(), DispatchRequestId::new(6));
        assert_eq!(scope.ticket(), TicketId::new(9));
        assert_eq!(scope.lane(), LaneId::new(3));
        assert_eq!(scope.role(), CapabilityRole::Reviewer);
        assert_eq!(scope.reviewer_slot(), Some(ReviewerSlotId::new(5)));
        assert_eq!(capability.scope(), &scope);
        assert_eq!(capability.status(), CapabilityStatus::Active);
    }

    #[test]
    fn a_reviewer_scope_requires_its_slot() {
        assert_eq!(
            CapabilityScope::new(
                TicketId::new(1),
                LaneId::new(1),
                CapabilityRole::Reviewer,
                None
            ),
            Err(CapabilityError::ReviewerRoleRequiresSlot)
        );
    }

    #[test]
    fn an_implementer_scope_refuses_a_slot() {
        assert_eq!(
            CapabilityScope::new(
                TicketId::new(1),
                LaneId::new(1),
                CapabilityRole::Implementer,
                Some(ReviewerSlotId::new(2))
            ),
            Err(CapabilityError::SlotRequiresReviewerRole)
        );
    }

    #[test]
    fn a_grant_with_no_operations_is_refused() {
        assert_eq!(
            McpOperations::new(Vec::<String>::new()),
            Err(CapabilityError::NoOperations)
        );
        assert_eq!(
            Capability::mint(
                CapabilityId::new(1),
                DispatchRequestId::new(1),
                implementer_scope(),
                McpOperations::restore(Vec::new()),
            ),
            Err(CapabilityError::NoOperations)
        );
    }

    #[test]
    fn a_blank_operation_is_refused() {
        assert_eq!(
            McpOperations::new(["ticket.get", "  "]),
            Err(CapabilityError::BlankOperation {
                operation: "  ".to_owned()
            })
        );
    }

    #[test]
    fn a_grant_canonicalises_duplicates_and_order() {
        let grant = McpOperations::new(["timeline.query", "ticket.get", "spec.get", "ticket.get"])
            .expect("the grant validates");

        assert_eq!(
            grant.iter().collect::<Vec<_>>(),
            vec!["spec.get", "ticket.get", "timeline.query"],
            "the stored grant is deduplicated and ordered"
        );
        assert_eq!(grant.len(), 3);
        assert!(!grant.is_empty());
        assert!(grant.contains("ticket.get"));
        assert!(!grant.contains("ticket.create"));
    }

    #[test]
    fn operations_outside_the_agent_surface_are_refused() {
        let grant = McpOperations::new(["ticket.get", "capacity.defaults.update"])
            .expect("the grant validates");

        assert_eq!(
            enforce_within_surface(&grant, &surface()),
            Err(CapabilityError::OperationOutsideSurface {
                operation: "capacity.defaults.update".to_owned()
            })
        );
        assert_eq!(
            enforce_within_surface(&surface(), &surface()),
            Ok(()),
            "the surface itself stays inside itself"
        );
    }

    #[test]
    fn permits_operations_inside_the_minted_set() {
        let capability = minted();

        assert_eq!(capability.permits("ticket.get"), Ok(()));
        assert_eq!(capability.permits("spec.get"), Ok(()));
        assert_eq!(capability.permits("timeline.query"), Ok(()));
    }

    #[test]
    fn permits_rejects_operations_outside_the_minted_set() {
        let capability = minted();

        assert_eq!(
            capability.permits("ticket.create"),
            Err(CapabilityRefusal::NotPermitted {
                operation: "ticket.create".to_owned()
            })
        );
        assert_eq!(
            capability.permits("dispatch.claim"),
            Err(CapabilityRefusal::NotPermitted {
                operation: "dispatch.claim".to_owned()
            })
        );
    }

    #[test]
    fn settlement_expires_every_operation() {
        let mut capability = minted();
        capability.settle();

        assert_eq!(capability.status(), CapabilityStatus::Settled);
        assert!(!capability.status().is_live());
        for operation in ["ticket.get", "spec.get", "timeline.query"] {
            assert_eq!(
                capability.permits(operation),
                Err(CapabilityRefusal::Settled {
                    capability: CapabilityId::new(11)
                }),
                "a settled capability permits nothing, not even {operation}"
            );
        }
    }

    #[test]
    fn settlement_is_one_way_and_never_renews() {
        let mut expired = minted();
        expired.settle();
        expired.settle();
        assert_eq!(
            expired.status(),
            CapabilityStatus::Settled,
            "settling twice is the same settlement"
        );
        assert!(expired.permits("ticket.get").is_err());

        // The only path back to authority is a fresh mint with a
        // fresh identity; the settled capability stays settled.
        let successor = Capability::mint(
            CapabilityId::new(12),
            DispatchRequestId::new(5),
            implementer_scope(),
            McpOperations::new(["ticket.get"]).expect("the grant validates"),
        )
        .expect("a new run mints its own capability");
        assert_eq!(successor.status(), CapabilityStatus::Active);
        assert_eq!(successor.id(), CapabilityId::new(12));
        assert_eq!(
            expired.status(),
            CapabilityStatus::Settled,
            "the successor renews nothing"
        );
    }

    #[test]
    fn the_role_and_status_vocabularies_round_trip() {
        for role in [CapabilityRole::Implementer, CapabilityRole::Reviewer] {
            assert_eq!(CapabilityRole::parse(role.wire_name()), Some(role));
        }
        assert_eq!(CapabilityRole::parse("coordinator"), None);

        for status in [CapabilityStatus::Active, CapabilityStatus::Settled] {
            assert_eq!(CapabilityStatus::parse(status.wire_name()), Some(status));
            assert_eq!(
                CapabilityStatus::parse(status.wire_name()).map(|s| s.is_live()),
                Some(status == CapabilityStatus::Active)
            );
        }
        assert_eq!(CapabilityStatus::parse("revoked"), None);
    }
}
