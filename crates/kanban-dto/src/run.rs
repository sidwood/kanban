//! Run payload definitions: the frozen requested and effective profile
//! snapshots every client sees (KAN-S9-US3, DR-EP-04). A run belongs to
//! exactly one claimed Dispatch Request; its snapshots and fallback
//! path are recorded at the mint and never rewritten.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::mutation::MutationContext;

/// The closed run status on the wire. A run mints executing;
/// settlement vocabulary arrives with the submissions that own it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Minted from a claimed request and occupying its execution.
    Executing,
}

/// One frozen profile snapshot as every client sees it: the entry's
/// name and its five decisions as they stood at the run's mint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileSnapshotRecord {
    /// The snapshotted entry name.
    pub name: String,
    /// The snapshotted harness family.
    pub harness: String,
    /// The snapshotted model family.
    pub model: String,
    /// The snapshotted effort.
    pub effort: String,
    /// The snapshotted usage pool.
    pub usage_pool: String,
}

/// One run as every client sees it: the claimed Dispatch Request it
/// executes, the Ticket it works, and the requested and effective
/// profile snapshots frozen at its mint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    /// The storage-assigned identity.
    pub id: u64,
    /// The Project the run belongs to.
    pub project_id: u64,
    /// The Ticket the run executes.
    pub ticket_id: u64,
    /// The claimed Dispatch Request this run executes.
    pub dispatch_request_id: u64,
    /// Executing.
    pub status: RunStatus,
    /// The requested profile snapshot: what the assignment named.
    pub requested: ProfileSnapshotRecord,
    /// The effective profile snapshot: what actually runs after the
    /// fallback policy.
    pub effective: ProfileSnapshotRecord,
    /// Whether the effective profile is not the requested one.
    pub fallback: bool,
    /// The names the fallback walk touched, requested first; empty
    /// when no fallback happened.
    pub fallback_path: Vec<String>,
    /// When the run minted, as unix seconds.
    pub created_at: u64,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// Request payload for the `run.acknowledge` command: mint the run of
/// one claimed Dispatch Request with its profile snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunAcknowledgeRequest {
    pub mutation: MutationContext,
    /// The claimed Dispatch Request the run executes.
    pub dispatch_request_id: u64,
}

/// Request payload for the `run.list` query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunListQuery {
    /// The Project whose runs are listed.
    pub project_id: u64,
}

/// Response payload for the `run.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunListResponse {
    /// The Project whose runs these are.
    pub project_id: u64,
    /// Every run of the Project, newest last.
    pub runs: Vec<RunRecord>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::mutation::MutationContext;
    use crate::run::{
        ProfileSnapshotRecord, RunAcknowledgeRequest, RunListQuery, RunListResponse, RunRecord,
        RunStatus,
    };
    use crate::schema_definitions;

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 2,
            idempotency_key: "key-1".to_owned(),
        }
    }

    fn snapshot(name: &str, model: &str) -> ProfileSnapshotRecord {
        ProfileSnapshotRecord {
            name: name.to_owned(),
            harness: "claude-code".to_owned(),
            model: model.to_owned(),
            effort: "high".to_owned(),
            usage_pool: "operator".to_owned(),
        }
    }

    fn record() -> RunRecord {
        RunRecord {
            id: 3,
            project_id: 1,
            ticket_id: 9,
            dispatch_request_id: 4,
            status: RunStatus::Executing,
            requested: snapshot("nightly", "opus"),
            effective: snapshot("standard", "sonnet"),
            fallback: true,
            fallback_path: vec!["nightly".to_owned(), "standard".to_owned()],
            created_at: 20,
            version: 1,
        }
    }

    #[test]
    fn a_record_round_trips_with_its_snapshots() {
        let encoded = serde_json::to_value(record()).expect("the record serialises");

        assert_eq!(
            encoded,
            json!({
                "id": 3,
                "project_id": 1,
                "ticket_id": 9,
                "dispatch_request_id": 4,
                "status": "executing",
                "requested": {
                    "name": "nightly",
                    "harness": "claude-code",
                    "model": "opus",
                    "effort": "high",
                    "usage_pool": "operator",
                },
                "effective": {
                    "name": "standard",
                    "harness": "claude-code",
                    "model": "sonnet",
                    "effort": "high",
                    "usage_pool": "operator",
                },
                "fallback": true,
                "fallback_path": ["nightly", "standard"],
                "created_at": 20,
                "version": 1,
            })
        );
        let decoded: RunRecord = serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, record());
    }

    #[test]
    fn a_run_without_fallback_carries_an_empty_path() {
        let without = RunRecord {
            status: RunStatus::Executing,
            requested: snapshot("standard", "opus"),
            effective: snapshot("standard", "opus"),
            fallback: false,
            fallback_path: Vec::new(),
            ..record()
        };
        let encoded = serde_json::to_value(&without).expect("the record serialises");
        assert_eq!(encoded["fallback"], json!(false));
        assert_eq!(encoded["fallback_path"], json!([]));
    }

    #[test]
    fn every_request_round_trips_and_rejects_unknown_fields() {
        round_trips::<RunAcknowledgeRequest>(json!({
            "mutation": context(),
            "dispatch_request_id": 4,
        }));

        let list: RunListQuery =
            serde_json::from_value(json!({ "project_id": 1 })).expect("the query decodes");
        assert_eq!(list, RunListQuery { project_id: 1 });

        let response = RunListResponse {
            project_id: 1,
            runs: vec![record()],
        };
        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(encoded["runs"].as_array().map(Vec::len), Some(1));
    }

    /// One request wire form decodes typed, re-encodes identically,
    /// and refuses an unknown field.
    fn round_trips<Request>(wire: serde_json::Value)
    where
        Request: serde::de::DeserializeOwned + serde::Serialize,
    {
        let decoded: Request =
            serde_json::from_value(wire.clone()).expect("the request decodes typed");
        let encoded = serde_json::to_value(&decoded).expect("the request re-encodes");
        assert_eq!(encoded, wire, "the wire form round trips");

        let mut refused = wire;
        refused["surprise"] = json!(true);
        assert!(
            serde_json::from_value::<Request>(refused).is_err(),
            "unknown fields are rejected"
        );
    }

    /// The schema of one registered DTO, proving registration.
    fn schema_of(name: &str) -> serde_json::Value {
        let (_, schema) = schema_definitions()
            .into_iter()
            .find(|(schema_name, _)| *schema_name == name)
            .unwrap_or_else(|| panic!("{name} is registered"));
        serde_json::to_value(schema).expect("the schema serialises")
    }

    #[test]
    fn every_run_schema_rejects_unknown_fields() {
        // RunStatus is a closed string vocabulary: it carries no
        // fields at all, so it appears in no unknown-field list.
        for name in [
            "ProfileSnapshotRecord",
            "RunAcknowledgeRequest",
            "RunListQuery",
            "RunListResponse",
            "RunRecord",
        ] {
            let schema = schema_of(name);
            let encoded = serde_json::to_string(&schema).expect("the schema serialises");
            assert!(
                encoded.contains("\"additionalProperties\":false"),
                "{name} should reject unknown fields"
            );
        }
    }
}
