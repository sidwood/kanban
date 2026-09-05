//! Execution Profile payload definitions: the catalogue's closed
//! schema, the commands that change it, and the record every client
//! sees (KAN-S7-US1). A profile carries exactly harness, model,
//! effort, usage pool, and fallback policy under a name that is
//! unique and immutable per entry; assignments and fallbacks name
//! entries by reference, never inlined values (DR-EP-01, DR-EP-02).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Request payload for the `profile.define` command: one new named
/// entry carrying the closed schema's five decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileDefineRequest {
    pub mutation: super::MutationContext,
    /// The name the entry is defined under; unique and immutable.
    pub name: String,
    /// The harness family.
    pub harness: String,
    /// The model family.
    pub model: String,
    /// The effort.
    pub effort: String,
    /// The usage pool.
    pub usage_pool: String,
    /// The fallback policy, as the profile another entry names; a
    /// profile with no fallback policy omits the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

/// Request payload for the `profile.update` command: the definition
/// of one named entry, replaced wholesale under the same name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileUpdateRequest {
    pub mutation: super::MutationContext,
    /// The entry being updated; names are never renamed.
    pub name: String,
    /// The harness family.
    pub harness: String,
    /// The model family.
    pub model: String,
    /// The effort.
    pub effort: String,
    /// The usage pool.
    pub usage_pool: String,
    /// The fallback policy, as the profile another entry names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

/// Request payload for the `profile.retire` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileRetireRequest {
    pub mutation: super::MutationContext,
    /// The entry being retired. Retirement is terminal and preserves
    /// every recorded fact.
    pub name: String,
}

/// The profile record as every client sees it: the closed schema's
/// five decisions under the entry's immutable name, with the entry's
/// lifecycle state for the assignable surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileRecord {
    /// The entry's immutable identity.
    pub name: String,
    /// The harness family.
    pub harness: String,
    /// The model family.
    pub model: String,
    /// The effort.
    pub effort: String,
    /// The usage pool.
    pub usage_pool: String,
    /// The fallback policy, as the profile another entry names, if
    /// the entry carries one.
    pub fallback: Option<String>,
    /// Whether the entry is retired: terminal, preserved, and no
    /// longer assignable.
    pub retired: bool,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// Request payload for the `profile.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileListQuery {}

/// Response payload for the `profile.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileListResponse {
    /// Every catalogue entry, retired ones included.
    pub profiles: Vec<ProfileRecord>,
}

/// Request payload for the `profile.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileGetQuery {
    /// The entry being read.
    pub name: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        ProfileDefineRequest, ProfileGetQuery, ProfileListQuery, ProfileListResponse,
        ProfileRecord, ProfileRetireRequest, ProfileUpdateRequest,
    };
    use crate::mutation::MutationContext;
    use crate::schema_definitions;

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 3,
            idempotency_key: "key-1".to_owned(),
        }
    }

    fn record() -> ProfileRecord {
        ProfileRecord {
            name: "standard".to_owned(),
            harness: "claude-code".to_owned(),
            model: "opus".to_owned(),
            effort: "high".to_owned(),
            usage_pool: "operator".to_owned(),
            fallback: None,
            retired: false,
            version: 1,
        }
    }

    #[test]
    fn a_record_round_trips_with_the_closed_schema() {
        let encoded = serde_json::to_value(record()).expect("the record serialises");

        assert_eq!(
            encoded,
            json!({
                "name": "standard",
                "harness": "claude-code",
                "model": "opus",
                "effort": "high",
                "usage_pool": "operator",
                "fallback": null,
                "retired": false,
                "version": 1,
            })
        );
        let decoded: ProfileRecord =
            serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, record());

        let with_fallback = ProfileRecord {
            fallback: Some("nightly".to_owned()),
            retired: true,
            version: 2,
            ..record()
        };
        let encoded = serde_json::to_value(&with_fallback).expect("the record serialises");
        assert_eq!(encoded["fallback"], json!("nightly"));
        assert_eq!(encoded["retired"], json!(true));
    }

    #[test]
    fn every_request_round_trips_and_rejects_unknown_fields() {
        round_trips::<ProfileDefineRequest>(json!({
            "mutation": context(),
            "name": "nightly",
            "harness": "claude-code",
            "model": "haiku",
            "effort": "medium",
            "usage_pool": "operator",
            "fallback": "standard",
        }));
        // A profile with no fallback policy sends no fallback field:
        // absence carries no field at all.
        round_trips::<ProfileDefineRequest>(json!({
            "mutation": context(),
            "name": "standard",
            "harness": "claude-code",
            "model": "opus",
            "effort": "high",
            "usage_pool": "operator",
        }));
        round_trips::<ProfileUpdateRequest>(json!({
            "mutation": context(),
            "name": "standard",
            "harness": "shell-agent",
            "model": "sonnet",
            "effort": "medium",
            "usage_pool": "operator",
        }));
        round_trips::<ProfileRetireRequest>(json!({
            "mutation": context(),
            "name": "nightly",
        }));

        let list: ProfileListQuery =
            serde_json::from_value(json!({})).expect("the list query decodes");
        assert_eq!(list, ProfileListQuery {});

        let get: ProfileGetQuery =
            serde_json::from_value(json!({ "name": "standard" })).expect("the get query decodes");
        assert_eq!(
            get,
            ProfileGetQuery {
                name: "standard".to_owned()
            }
        );

        let response = ProfileListResponse {
            profiles: vec![record()],
        };
        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(encoded["profiles"].as_array().map(Vec::len), Some(1));
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
    fn every_profile_schema_rejects_unknown_fields() {
        for name in [
            "ProfileDefineRequest",
            "ProfileGetQuery",
            "ProfileListQuery",
            "ProfileListResponse",
            "ProfileRecord",
            "ProfileRetireRequest",
            "ProfileUpdateRequest",
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
