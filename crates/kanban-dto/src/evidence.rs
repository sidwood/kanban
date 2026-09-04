//! Evidence payload definitions: attach and list commands and the
//! records every client sees (KAN-S2-US4).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether an evidence item is a managed file or repository reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKindDto {
    ManagedFile,
    Repository,
}

/// One evidence record as every client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRecord {
    pub id: u64,
    pub project_id: String,
    pub entity_kind: String,
    pub entity_id: String,
    pub evidence_kind: EvidenceKindDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_identity: Option<String>,
}

/// Request payload for the `evidence.attach` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceAttachRequest {
    pub mutation: super::MutationContext,
    pub project_id: String,
    pub entity_kind: String,
    pub entity_id: String,
    pub evidence_kind: EvidenceKindDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_identity: Option<String>,
}

/// Request payload for the `evidence.list` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceListRequest {
    pub mutation: super::MutationContext,
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
}

/// Response payload for the `evidence.list` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceListResponse {
    pub evidence: Vec<EvidenceRecord>,
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde_json::json;

    use super::{
        EvidenceAttachRequest, EvidenceKindDto, EvidenceListRequest, EvidenceListResponse,
        EvidenceRecord,
    };
    use crate::mutation::MutationContext;

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 0,
            idempotency_key: "key-1".to_owned(),
        }
    }

    #[test]
    fn a_managed_file_attach_request_round_trips() {
        let request = EvidenceAttachRequest {
            mutation: context(),
            project_id: "kan-p1".to_owned(),
            entity_kind: "ticket".to_owned(),
            entity_id: "kan-t10".to_owned(),
            evidence_kind: EvidenceKindDto::ManagedFile,
            content_base64: Some(STANDARD.encode(b"hello")),
            relative_path: None,
            commit_identity: None,
        };

        let encoded = serde_json::to_value(&request).expect("the request serialises");
        let decoded: EvidenceAttachRequest =
            serde_json::from_value(encoded).expect("the request deserialises");
        assert_eq!(decoded, request);
    }

    #[test]
    fn a_repository_attach_request_round_trips() {
        let request = EvidenceAttachRequest {
            mutation: context(),
            project_id: "kan-p1".to_owned(),
            entity_kind: "ticket".to_owned(),
            entity_id: "kan-t10".to_owned(),
            evidence_kind: EvidenceKindDto::Repository,
            content_base64: None,
            relative_path: Some("docs/spec.md".to_owned()),
            commit_identity: Some("deadbeef".to_owned()),
        };

        let encoded = serde_json::to_value(&request).expect("the request serialises");
        let decoded: EvidenceAttachRequest =
            serde_json::from_value(encoded).expect("the request deserialises");
        assert_eq!(decoded, request);
    }

    #[test]
    fn attach_and_list_requests_reject_unknown_fields() {
        let refused: Result<EvidenceAttachRequest, _> = serde_json::from_value(json!({
            "mutation": context(),
            "project_id": "kan-p1",
            "entity_kind": "ticket",
            "entity_id": "kan-t10",
            "evidence_kind": "managed_file",
            "content_base64": "aGVsbG8=",
            "surprise": true,
        }));
        assert!(refused.is_err(), "unknown fields are rejected");

        let refused: Result<EvidenceListRequest, _> = serde_json::from_value(json!({
            "mutation": context(),
            "project_id": "kan-p1",
            "all": true,
        }));
        assert!(refused.is_err(), "unknown fields are rejected");
    }

    #[test]
    fn a_list_response_carries_records() {
        let response = EvidenceListResponse {
            evidence: vec![EvidenceRecord {
                id: 1,
                project_id: "kan-p1".to_owned(),
                entity_kind: "ticket".to_owned(),
                entity_id: "kan-t10".to_owned(),
                evidence_kind: EvidenceKindDto::Repository,
                content_hash: None,
                relative_path: Some("docs/spec.md".to_owned()),
                commit_identity: Some("deadbeef".to_owned()),
            }],
        };

        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(encoded["evidence"].as_array().map(Vec::len), Some(1));
    }
}
