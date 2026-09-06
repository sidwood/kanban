//! Run-scoped capability payloads (KAN-S9-US4, DR-HB-17, DR-SS-14):
//! the minted token a dispatched run's agent holds, as every client
//! sees it. The permitted MCP operation set, the binding, and the
//! one-way status ride the record; the claim that mints it carries
//! it alongside the won Dispatch Request.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The run role a capability grants, on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRole {
    /// Executing an Implementation or Bug Ticket in a Lane.
    Implementer,
    /// Reviewing one Review Slot.
    Reviewer,
}

/// The closed capability status on the wire: live until run
/// settlement expires it. Settled is terminal; nothing renews.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    /// Live from the mint until the run settles.
    Active,
    /// Expired with run settlement. Terminal.
    Settled,
}

/// One minted, run-scoped capability as every client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRecord {
    /// The storage-assigned identity.
    pub id: u64,
    /// The Dispatch Request whose won claim minted this capability.
    pub dispatch_request_id: u64,
    /// The Ticket the run executes.
    pub ticket_id: u64,
    /// The Lane the run executes in.
    pub lane_id: u64,
    /// The run role granted.
    pub role: CapabilityRole,
    /// The Review Slot occupied, for a reviewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_slot_id: Option<u64>,
    /// The permitted MCP operations, canonical and ordered.
    pub operations: Vec<String>,
    /// Live or settled.
    pub status: CapabilityStatus,
    /// When the capability was minted, as unix seconds.
    pub minted_at: u64,
    /// When run settlement expired it, as unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::{CapabilityRecord, CapabilityRole, CapabilityStatus};

    #[test]
    fn a_capability_record_round_trips() {
        let record = CapabilityRecord {
            id: 3,
            dispatch_request_id: 9,
            ticket_id: 7,
            lane_id: 2,
            role: CapabilityRole::Reviewer,
            reviewer_slot_id: Some(5),
            operations: vec!["spec.get".to_owned(), "ticket.get".to_owned()],
            status: CapabilityStatus::Active,
            minted_at: 100,
            settled_at: None,
        };

        let encoded = serde_json::to_value(&record).expect("the record serialises");
        assert_eq!(
            serde_json::from_value::<CapabilityRecord>(encoded).expect("the record decodes"),
            record
        );
    }

    #[test]
    fn an_implementer_record_omits_its_absent_slot() {
        let record = CapabilityRecord {
            id: 4,
            dispatch_request_id: 10,
            ticket_id: 8,
            lane_id: 1,
            role: CapabilityRole::Implementer,
            reviewer_slot_id: None,
            operations: vec!["ticket.get".to_owned()],
            status: CapabilityStatus::Settled,
            minted_at: 100,
            settled_at: Some(160),
        };

        let encoded = serde_json::to_value(&record).expect("the record serialises");
        assert_eq!(
            encoded,
            serde_json::json!({
                "id": 4,
                "dispatch_request_id": 10,
                "ticket_id": 8,
                "lane_id": 1,
                "role": "implementer",
                "operations": ["ticket.get"],
                "status": "settled",
                "minted_at": 100,
                "settled_at": 160,
            }),
            "an absent slot and the wire vocabulary stay exact"
        );
        assert_eq!(
            serde_json::from_value::<CapabilityRecord>(encoded).expect("the record decodes"),
            record
        );
    }

    #[test]
    fn a_capability_record_rejects_unknown_fields() {
        let outcome = serde_json::from_value::<CapabilityRecord>(serde_json::json!({
            "id": 1,
            "dispatch_request_id": 1,
            "ticket_id": 1,
            "lane_id": 1,
            "role": "implementer",
            "operations": ["ticket.get"],
            "status": "active",
            "minted_at": 0,
            "renewed_at": 5,
        }));

        assert!(outcome.is_err(), "unknown fields are rejected");
    }
}
