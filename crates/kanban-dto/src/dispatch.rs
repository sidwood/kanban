//! Dispatch Request payloads: the durable queue and atomic claim on
//! execution (KAN-S9-US1, DR-EP-08, DR-HB-14). A request is queued
//! while capacity is unavailable; exactly one concurrent claimant
//! wins, and losers remain queued.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::capability::CapabilityRecord;
use super::mutation::MutationContext;
use super::ticket::TicketPriority;

/// The closed Dispatch Request status on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DispatchStatus {
    /// Waiting for a claimant, or waiting because capacity is
    /// exhausted.
    Queued,
    /// Held by the one claimant that won.
    Claimed,
}

/// One Dispatch Request as every client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DispatchRequestRecord {
    /// The storage-assigned identity.
    pub id: u64,
    /// The Project the request belongs to.
    pub project_id: u64,
    /// The Ticket the request dispatches.
    pub ticket_id: u64,
    /// Queued or claimed.
    pub status: DispatchStatus,
    /// The priority snapshotted at enqueue.
    pub priority: TicketPriority,
    /// Whether the Ticket was ready at enqueue.
    pub ready: bool,
    /// The snapshotted harness family.
    pub harness: String,
    /// The snapshotted model family.
    pub model: String,
    /// The snapshotted usage pool.
    pub usage_pool: String,
    /// When the request entered the queue, as unix seconds.
    pub created_at: u64,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// Request payload for the `dispatch.request` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DispatchRequestCreateRequest {
    pub mutation: MutationContext,
    /// The Ticket to put on the dispatch queue.
    pub ticket_id: u64,
}

/// Request payload for the `dispatch.claim` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DispatchClaimRequest {
    pub mutation: MutationContext,
    /// The Dispatch Request to claim.
    pub dispatch_request_id: u64,
}

/// Response payload for the `dispatch.claim` command: the request as
/// it stands after the attempt, whether this claimant won, the
/// capacity refusal that kept it queued when this claimant lost, and
/// the run-scoped capability a win minted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DispatchClaimResponse {
    /// The request after the claim attempt.
    pub request: DispatchRequestRecord,
    /// Whether this claimant won.
    pub claimed: bool,
    /// Why the request stayed queued, when it did.
    pub capacity_refusal: Option<String>,
    /// The capability minted with the win: the authority the run's
    /// agent holds, expiring with run settlement. A request still
    /// queued has granted none.
    pub capability: Option<CapabilityRecord>,
}

/// Request payload for the `dispatch.queue` query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DispatchQueueQuery {
    /// The Project whose queued requests are listed.
    pub project_id: u64,
}

/// Response payload for the `dispatch.queue` query: queued requests
/// in deterministic priority, readiness, age order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DispatchQueueResponse {
    /// The Project whose queue this is.
    pub project_id: u64,
    /// Queued requests, urgent and ready and older first.
    pub requests: Vec<DispatchRequestRecord>,
}
