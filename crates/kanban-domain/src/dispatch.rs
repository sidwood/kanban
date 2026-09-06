//! Dispatch Requests: the durable, atomic claim on execution
//! (CONTEXT.md, DR-EP-08, DR-HB-14). A request is queued while
//! capacity is unavailable, claimed by exactly one concurrent
//! claimant, and ordered deterministically by priority, readiness,
//! and age. Capacity evaluation is KAN-T37's; this module only
//! decides what a claim does with that answer. Nothing here launches
//! an implementation agent.

use std::cmp::Ordering;
use std::fmt;

use crate::capacity::CapacityRefusal;
use crate::project::ProjectId;
use crate::ticket::{Priority, TicketId};

/// The identity of one Dispatch Request. Assigned once by storage
/// and immutable afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DispatchRequestId(u64);

impl DispatchRequestId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for DispatchRequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a Dispatch Request rule was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    /// A Ticket with no Execution Profile cannot occupy a capacity
    /// dimension, so it cannot enter the queue.
    UnassignedProfile,
    /// A Ticket already has an open (queued or claimed) Dispatch
    /// Request; a second one would be duplicate dispatch.
    DuplicateOpen { ticket: TicketId },
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnassignedProfile => write!(
                f,
                "a Dispatch Request requires an assigned Execution Profile"
            ),
            Self::DuplicateOpen { ticket } => {
                write!(f, "Ticket {ticket} already has an open Dispatch Request")
            }
        }
    }
}

impl std::error::Error for DispatchError {}

/// The closed Dispatch Request status vocabulary: queued while it
/// waits for a claimant and capacity, claimed once exactly one
/// claimant has won.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DispatchStatus {
    /// Waiting for a claimant, or waiting because capacity is
    /// exhausted.
    Queued,
    /// Held by the one claimant that won.
    Claimed,
}

impl DispatchStatus {
    /// The stored and wire name of this status.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
        }
    }

    /// The status a stored row names, or `None` outside the
    /// vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        match stored {
            "queued" => Some(Self::Queued),
            "claimed" => Some(Self::Claimed),
            _ => None,
        }
    }

    /// Whether this status still occupies the Ticket's open slot.
    pub fn is_open(self) -> bool {
        matches!(self, Self::Queued | Self::Claimed)
    }
}

/// One durable Dispatch Request. The profile families are snapshotted
/// at enqueue so a later catalogue change cannot rewrite the capacity
/// dimensions a queued request will draw (DR-EP-05). Priority and
/// readiness are snapshotted too, so queue order stays the order the
/// request entered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchRequest {
    id: DispatchRequestId,
    project: ProjectId,
    ticket: TicketId,
    status: DispatchStatus,
    priority: Priority,
    ready: bool,
    harness: String,
    model: String,
    usage_pool: String,
    created_at: u64,
    version: u64,
}

impl DispatchRequest {
    /// Enqueue a fresh request at version 1. The Ticket must already
    /// name a profile; the caller supplies the snapshotted families.
    #[allow(clippy::too_many_arguments)]
    pub fn enqueue(
        id: DispatchRequestId,
        project: ProjectId,
        ticket: TicketId,
        priority: Priority,
        ready: bool,
        harness: impl Into<String>,
        model: impl Into<String>,
        usage_pool: impl Into<String>,
        created_at: u64,
    ) -> Result<Self, DispatchError> {
        let harness = harness.into();
        let model = model.into();
        let usage_pool = usage_pool.into();
        if harness.trim().is_empty() || model.trim().is_empty() || usage_pool.trim().is_empty() {
            return Err(DispatchError::UnassignedProfile);
        }
        Ok(Self {
            id,
            project,
            ticket,
            status: DispatchStatus::Queued,
            priority,
            ready,
            harness,
            model,
            usage_pool,
            created_at,
            version: 1,
        })
    }

    /// Rehydrate a stored request exactly as it was recorded.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        id: DispatchRequestId,
        project: ProjectId,
        ticket: TicketId,
        status: DispatchStatus,
        priority: Priority,
        ready: bool,
        harness: String,
        model: String,
        usage_pool: String,
        created_at: u64,
        version: u64,
    ) -> Self {
        Self {
            id,
            project,
            ticket,
            status,
            priority,
            ready,
            harness,
            model,
            usage_pool,
            created_at,
            version,
        }
    }

    /// The immutable identity.
    pub fn id(&self) -> DispatchRequestId {
        self.id
    }

    /// The Project the request belongs to.
    pub fn project(&self) -> ProjectId {
        self.project
    }

    /// The Ticket the request dispatches.
    pub fn ticket(&self) -> TicketId {
        self.ticket
    }

    /// Queued or claimed.
    pub fn status(&self) -> DispatchStatus {
        self.status
    }

    /// The priority snapshotted at enqueue.
    pub fn priority(&self) -> Priority {
        self.priority
    }

    /// Whether the Ticket was ready at enqueue.
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// The snapshotted harness family.
    pub fn harness(&self) -> &str {
        &self.harness
    }

    /// The snapshotted model family.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The snapshotted usage pool.
    pub fn usage_pool(&self) -> &str {
        &self.usage_pool
    }

    /// When the request entered the queue, as unix seconds. Equal
    /// timestamps fall through to identity so age stays total.
    pub fn created_at(&self) -> u64 {
        self.created_at
    }

    /// The aggregate version for optimistic checks.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Apply a claim decision that already ran against this request's
    /// current status. Claiming bumps the version; remaining queued
    /// changes nothing, so a capacity miss is not a mutation.
    pub fn apply_claim(&mut self, decision: ClaimDecision) -> Result<(), DispatchError> {
        match decision {
            ClaimDecision::Claim => {
                self.status = DispatchStatus::Claimed;
                self.version += 1;
                Ok(())
            }
            ClaimDecision::AlreadyClaimed | ClaimDecision::RemainQueued(_) => Ok(()),
        }
    }
}

/// What one claim attempt does with a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimDecision {
    /// The request is queued and capacity fits: this claimant wins.
    Claim,
    /// Another claimant already holds the request.
    AlreadyClaimed,
    /// The request stays queued because capacity is exhausted.
    RemainQueued(CapacityRefusal),
}

/// Decide a claim from the request's status and one capacity
/// evaluation. Concurrent claimants racing the same queued request
/// see exactly one `Claim`; a request that is already claimed stays
/// claimed, and a capacity miss leaves it queued.
pub fn decide_claim(
    status: DispatchStatus,
    capacity: Result<(), CapacityRefusal>,
) -> ClaimDecision {
    match status {
        DispatchStatus::Claimed => ClaimDecision::AlreadyClaimed,
        DispatchStatus::Queued => match capacity {
            Ok(()) => ClaimDecision::Claim,
            Err(refusal) => ClaimDecision::RemainQueued(refusal),
        },
    }
}

/// Refuse a second open request for a Ticket that already has one.
pub fn refuse_duplicate_open(
    ticket: TicketId,
    existing: Option<DispatchStatus>,
) -> Result<(), DispatchError> {
    match existing {
        Some(status) if status.is_open() => Err(DispatchError::DuplicateOpen { ticket }),
        _ => Ok(()),
    }
}

/// Deterministic queue order: priority (urgent first), then
/// readiness (ready before blocked), then age (older first), then
/// identity so equal ages stay total.
pub fn compare_queue(left: &DispatchRequest, right: &DispatchRequest) -> Ordering {
    left.priority()
        .queue_rank()
        .cmp(&right.priority().queue_rank())
        .then_with(|| right.ready().cmp(&left.ready()))
        .then_with(|| left.created_at().cmp(&right.created_at()))
        .then_with(|| left.id().cmp(&right.id()))
}

/// Sort `requests` into queue order, in place.
pub fn sort_queue(requests: &mut [DispatchRequest]) {
    requests.sort_by(compare_queue);
}

#[cfg(test)]
mod dispatch_request_rules {
    use super::{
        ClaimDecision, DispatchError, DispatchRequest, DispatchRequestId, DispatchStatus,
        compare_queue, decide_claim, refuse_duplicate_open, sort_queue,
    };
    use crate::capacity::CapacityRefusal;
    use crate::project::ProjectId;
    use crate::ticket::{Priority, TicketId};

    fn queued(id: u64, priority: Priority, ready: bool, created_at: u64) -> DispatchRequest {
        DispatchRequest::enqueue(
            DispatchRequestId::new(id),
            ProjectId::new(1),
            TicketId::new(id),
            priority,
            ready,
            "claude-code",
            "opus",
            "operator",
            created_at,
        )
        .expect("snapshotted families enqueue")
    }

    #[test]
    fn a_fresh_request_starts_queued_at_version_one() {
        let request = queued(1, Priority::Normal, true, 10);

        assert_eq!(request.status(), DispatchStatus::Queued);
        assert_eq!(request.version(), 1);
        assert_eq!(request.harness(), "claude-code");
        assert_eq!(request.model(), "opus");
        assert_eq!(request.usage_pool(), "operator");
    }

    #[test]
    fn blank_profile_families_are_refused() {
        let outcome = DispatchRequest::enqueue(
            DispatchRequestId::new(1),
            ProjectId::new(1),
            TicketId::new(1),
            Priority::Normal,
            true,
            "  ",
            "opus",
            "operator",
            10,
        );
        assert_eq!(outcome, Err(DispatchError::UnassignedProfile));
    }

    #[test]
    fn a_second_open_request_for_the_same_ticket_is_refused() {
        assert_eq!(
            refuse_duplicate_open(TicketId::new(4), Some(DispatchStatus::Queued)),
            Err(DispatchError::DuplicateOpen {
                ticket: TicketId::new(4)
            })
        );
        assert_eq!(
            refuse_duplicate_open(TicketId::new(4), Some(DispatchStatus::Claimed)),
            Err(DispatchError::DuplicateOpen {
                ticket: TicketId::new(4)
            })
        );
        assert_eq!(refuse_duplicate_open(TicketId::new(4), None), Ok(()));
    }

    #[test]
    fn decide_claim_lets_exactly_one_queued_winner_through() {
        assert_eq!(
            decide_claim(DispatchStatus::Queued, Ok(())),
            ClaimDecision::Claim
        );
        assert_eq!(
            decide_claim(DispatchStatus::Claimed, Ok(())),
            ClaimDecision::AlreadyClaimed
        );
        assert_eq!(
            decide_claim(
                DispatchStatus::Queued,
                Err(CapacityRefusal::HarnessExhausted {
                    harness: "claude-code".to_owned(),
                    active: 2,
                    cap: 2,
                })
            ),
            ClaimDecision::RemainQueued(CapacityRefusal::HarnessExhausted {
                harness: "claude-code".to_owned(),
                active: 2,
                cap: 2,
            })
        );
    }

    #[test]
    fn applying_a_claim_marks_the_winner_and_leaves_queued_losers() {
        let mut winner = queued(1, Priority::Normal, true, 10);
        winner
            .apply_claim(ClaimDecision::Claim)
            .expect("a claim applies");
        assert_eq!(winner.status(), DispatchStatus::Claimed);
        assert_eq!(winner.version(), 2);

        let mut queued_loser = queued(2, Priority::Normal, true, 11);
        queued_loser
            .apply_claim(ClaimDecision::RemainQueued(
                CapacityRefusal::HarnessExhausted {
                    harness: "claude-code".to_owned(),
                    active: 1,
                    cap: 1,
                },
            ))
            .expect("a capacity miss is not a mutation");
        assert_eq!(queued_loser.status(), DispatchStatus::Queued);
        assert_eq!(queued_loser.version(), 1);

        let mut claimed = winner.clone();
        claimed
            .apply_claim(ClaimDecision::AlreadyClaimed)
            .expect("a late claimant changes nothing");
        assert_eq!(claimed.status(), DispatchStatus::Claimed);
        assert_eq!(claimed.version(), 2);
    }

    #[test]
    fn queue_order_is_priority_then_readiness_then_age() {
        let urgent_blocked = queued(1, Priority::Urgent, false, 40);
        let high_ready_newer = queued(2, Priority::High, true, 30);
        let high_ready_older = queued(3, Priority::High, true, 10);
        let high_blocked = queued(4, Priority::High, false, 5);
        let normal_ready = queued(5, Priority::Normal, true, 1);

        let mut requests = vec![
            normal_ready.clone(),
            high_blocked.clone(),
            high_ready_newer.clone(),
            urgent_blocked.clone(),
            high_ready_older.clone(),
        ];
        sort_queue(&mut requests);

        let ids: Vec<_> = requests
            .iter()
            .map(|request| request.id().value())
            .collect();
        assert_eq!(
            ids,
            vec![1, 3, 2, 4, 5],
            "urgent before high before normal; ready before blocked; older before newer"
        );
        assert_eq!(
            compare_queue(&high_ready_older, &high_ready_newer),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn equal_age_falls_through_to_identity() {
        let earlier_id = queued(2, Priority::Normal, true, 10);
        let later_id = queued(7, Priority::Normal, true, 10);
        assert_eq!(
            compare_queue(&earlier_id, &later_id),
            std::cmp::Ordering::Less
        );
    }
}
