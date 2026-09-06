//! Run-scoped capability minting and enforcement at the application
//! layer (KAN-S9-US4, DR-HB-17, DR-SS-14). A won Dispatch Request
//! claim mints the capability its run's agent will present: bound to
//! the Ticket, the Lane the run executes in, the implementer role,
//! and the permitted MCP operations — a set drawn from the closed
//! agent surface, which is strictly narrower than operator
//! authority. Minting rides the claim's transaction in storage; this
//! module owns the surface, the draft the claim carries, and the
//! check every transport enforces against.

use std::collections::BTreeSet;

use kanban_domain::{
    Capability, CapabilityId, CapabilityRefusal, CapabilityRole, CapabilityScope, CapabilityStatus,
    DispatchRequestId, LaneId, McpOperations, TicketId,
};
use kanban_dto::{
    ApiError, CapabilityRecord, CapabilityRole as WireRole, CapabilityStatus as WireStatus,
};

use crate::catalog::exposed_operations;
use crate::timeline::TimelineEnvelope;

/// Every MCP operation a run-scoped capability may ever name: the
/// closed agent surface (DR-SS-14). Anything outside it —
/// dispatching, capacity, settings, lifecycle drags — is operator
/// authority, and no mint may grant it. The implementing agent reads
/// its Ticket and pinned Spec, contributes comments and evidence,
/// and watches the timeline; reviewer grants join here with review
/// dispatch.
pub const AGENT_MCP_OPERATIONS: &[&str] = &[
    "comment.create",
    "evidence.attach",
    "evidence.list",
    "health.get",
    "spec.get",
    "spec.version.get",
    "ticket.get",
    "timeline.query",
];

/// The agent surface as a canonical grant. Every curated name must
/// answer to a live catalogued operation: a name no command serves
/// would grant authority over a ghost, so drift refuses the mint.
pub fn agent_surface() -> Result<McpOperations, ApiError> {
    let catalogued: BTreeSet<&str> = exposed_operations()
        .iter()
        .map(|operation| operation.name)
        .collect();
    for name in AGENT_MCP_OPERATIONS {
        if !catalogued.contains(name) {
            return Err(ApiError::internal(&format!(
                "the agent MCP operation `{name}` is not in the catalogue"
            )));
        }
    }
    McpOperations::new(AGENT_MCP_OPERATIONS.iter().map(|name| (*name).to_owned()))
        .map_err(|error| ApiError::internal(&error.to_string()))
}

/// The facts one won claim mints: the Dispatch Request whose run
/// this is, the validated binding, the permitted operations, and the
/// moment. Built before the claim runs, so a mint that cannot be
/// validated never claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityMintDraft {
    dispatch: DispatchRequestId,
    scope: CapabilityScope,
    operations: McpOperations,
    minted_at: u64,
}

impl CapabilityMintDraft {
    /// Assemble a draft from parts that already hold their
    /// invariants: a scope validated at its construction and a
    /// non-empty grant.
    pub fn new(
        dispatch: DispatchRequestId,
        scope: CapabilityScope,
        operations: McpOperations,
        minted_at: u64,
    ) -> Self {
        Self {
            dispatch,
            scope,
            operations,
            minted_at,
        }
    }

    /// Draft the implementer mint for the run `dispatch` won: the
    /// Ticket, the Lane it executes in, the implementer role, and
    /// the agent surface as the permitted set.
    pub fn implementer(
        dispatch: DispatchRequestId,
        ticket: TicketId,
        lane: LaneId,
        minted_at: u64,
    ) -> Result<Self, ApiError> {
        Ok(Self {
            dispatch,
            scope: CapabilityScope::new(ticket, lane, CapabilityRole::Implementer, None)
                .map_err(|error| ApiError::invalid_request(&error.to_string()))?,
            operations: agent_surface()?,
            minted_at,
        })
    }

    /// The Dispatch Request whose won claim mints.
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

    /// The mint moment, as unix seconds.
    pub fn minted_at(&self) -> u64 {
        self.minted_at
    }
}

/// The storage port capability reads and run settlement call
/// through. Minting itself rides the dispatch claim's transaction,
/// so it has no method here: there is no way to mint a capability
/// except by winning a claim.
pub trait CapabilityStore: Send + Sync {
    /// Load one capability, if it exists.
    fn find(&self, id: CapabilityId) -> Result<Option<Capability>, ApiError>;
    /// Expire `id` with run settlement, recording `envelope` on the
    /// timeline. Idempotent: settling a settled capability changes
    /// nothing, and nothing renews.
    fn settle(
        &self,
        id: CapabilityId,
        settled_at: u64,
        envelope: TimelineEnvelope,
    ) -> Result<Capability, ApiError>;
}

/// Why one operation check failed. The storage failure stays an
/// [`ApiError`]; the verdict is the capability model's own refusal.
#[derive(Debug, Clone, PartialEq)]
pub enum CapabilityCheckError {
    /// No capability with that identity exists.
    Unknown(CapabilityId),
    /// The capability exists but refuses the operation: it settled,
    /// or the operation is outside the minted set.
    Refused(CapabilityRefusal),
    /// The lookup itself failed.
    Storage(ApiError),
}

impl CapabilityCheckError {
    /// The message every client sees, whichever arm failed.
    pub fn message(&self) -> String {
        match self {
            Self::Unknown(id) => format!("capability {id} was not found"),
            Self::Refused(refusal) => refusal.to_string(),
            Self::Storage(error) => error.message.clone(),
        }
    }
}

/// Check one operation against one minted capability: the check every
/// transport enforces per operation, MCP included (DR-SS-14). A live
/// capability permits exactly the operations inside its minted set;
/// a settled capability permits nothing.
pub fn enforce_capability(
    store: &dyn CapabilityStore,
    id: CapabilityId,
    operation: &str,
) -> Result<(), CapabilityCheckError> {
    let capability = store
        .find(id)
        .map_err(CapabilityCheckError::Storage)?
        .ok_or(CapabilityCheckError::Unknown(id))?;
    capability
        .permits(operation)
        .map_err(CapabilityCheckError::Refused)
}

/// Encode one capability for the wire.
pub fn encode_capability(capability: &Capability) -> CapabilityRecord {
    CapabilityRecord {
        id: capability.id().value(),
        dispatch_request_id: capability.dispatch().value(),
        ticket_id: capability.scope().ticket().value(),
        lane_id: capability.scope().lane().value(),
        role: match capability.scope().role() {
            CapabilityRole::Implementer => WireRole::Implementer,
            CapabilityRole::Reviewer => WireRole::Reviewer,
        },
        reviewer_slot_id: capability.scope().reviewer_slot().map(|slot| slot.value()),
        operations: capability.operations().iter().map(str::to_owned).collect(),
        status: match capability.status() {
            CapabilityStatus::Active => WireStatus::Active,
            CapabilityStatus::Settled => WireStatus::Settled,
        },
        minted_at: capability.minted_at(),
        settled_at: capability.settled_at(),
    }
}
