//! Capacity payload definitions: the global defaults that constrain
//! active runs by harness, model family, and usage pool, the
//! stricter per-Project caps and maximum active Lane count
//! (KAN-S7-US3, DR-EP-06, DR-EP-07), and the queries and commands
//! that read and replace them. A Project cap field that a request
//! omits is a cap the Project does not set: the global default
//! stands on that dimension.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The global capacity defaults as every client sees them: the
/// maximum active runs one harness, model family, or usage pool may
/// carry across every Project (DR-EP-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapacityGlobalDefaults {
    /// The most active runs one harness family may carry.
    pub max_active_per_harness: u64,
    /// The most active runs one model family may carry.
    pub max_active_per_model: u64,
    /// The most active runs one usage pool may carry.
    pub max_active_per_usage_pool: u64,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// The caps one Project imposes (DR-EP-07): stricter ceilings on the
/// global dimensions plus a maximum active Lane count. A `null` cap
/// constrains nothing; a set cap never relaxes the global default
/// on its dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapacityProjectCaps {
    /// The Project's harness ceiling, when it imposes one.
    pub max_active_per_harness: Option<u64>,
    /// The Project's model family ceiling, when it imposes one.
    pub max_active_per_model: Option<u64>,
    /// The Project's usage pool ceiling, when it imposes one.
    pub max_active_per_usage_pool: Option<u64>,
    /// The Project's maximum active Lane count, when it imposes one.
    pub max_active_lanes: Option<u64>,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// Request payload for the `capacity.defaults.get` query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapacityDefaultsGetQuery {}

/// Response payload for the `capacity.defaults.get` query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapacityDefaultsGetResponse {
    /// The global capacity defaults.
    pub defaults: CapacityGlobalDefaults,
}

/// Request payload for the `capacity.defaults.update` command: the
/// three global limits, replaced wholesale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapacityDefaultsUpdateRequest {
    pub mutation: super::MutationContext,
    /// The most active runs one harness family may carry.
    pub max_active_per_harness: u64,
    /// The most active runs one model family may carry.
    pub max_active_per_model: u64,
    /// The most active runs one usage pool may carry.
    pub max_active_per_usage_pool: u64,
}

/// Request payload for the `capacity.settings.get` query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapacitySettingsGetQuery {
    /// The Project whose caps are being read.
    pub project_id: u64,
}

/// Response payload for the `capacity.settings.get` query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapacitySettingsGetResponse {
    /// The Project the caps belong to.
    pub project_id: u64,
    /// The Project's caps; every field null when it imposes none.
    pub caps: CapacityProjectCaps,
}

/// Request payload for the `capacity.settings.update` command: the
/// Project's caps, replaced wholesale. An omitted cap field clears
/// that cap, so the global default stands on the dimension again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapacitySettingsUpdateRequest {
    pub mutation: super::MutationContext,
    /// The Project whose caps are being replaced.
    pub project_id: u64,
    /// The Project's harness ceiling; omitted imposes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_active_per_harness: Option<u64>,
    /// The Project's model family ceiling; omitted imposes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_active_per_model: Option<u64>,
    /// The Project's usage pool ceiling; omitted imposes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_active_per_usage_pool: Option<u64>,
    /// The Project's maximum active Lane count; omitted imposes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_active_lanes: Option<u64>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        CapacityDefaultsGetResponse, CapacityDefaultsUpdateRequest, CapacityProjectCaps,
        CapacitySettingsGetResponse, CapacitySettingsUpdateRequest,
    };
    use crate::mutation::MutationContext;
    use crate::schema_definitions;

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 3,
            idempotency_key: "key-1".to_owned(),
        }
    }

    fn caps() -> CapacityProjectCaps {
        CapacityProjectCaps {
            max_active_per_harness: Some(2),
            max_active_per_model: None,
            max_active_per_usage_pool: Some(4),
            max_active_lanes: Some(3),
            version: 5,
        }
    }

    #[test]
    fn caps_round_trip_with_null_for_every_unset_dimension() {
        let encoded = serde_json::to_value(caps()).expect("the caps serialise");

        assert_eq!(
            encoded,
            json!({
                "max_active_per_harness": 2,
                "max_active_per_model": null,
                "max_active_per_usage_pool": 4,
                "max_active_lanes": 3,
                "version": 5,
            })
        );
        let decoded: CapacityProjectCaps =
            serde_json::from_value(encoded).expect("the caps deserialise");
        assert_eq!(decoded, caps());
    }

    #[test]
    fn a_settings_update_round_trips_and_omits_unset_caps() {
        round_trips::<CapacitySettingsUpdateRequest>(json!({
            "mutation": context(),
            "project_id": 1,
            "max_active_per_harness": 2,
            "max_active_lanes": 3,
        }));
        round_trips::<CapacityDefaultsUpdateRequest>(json!({
            "mutation": context(),
            "max_active_per_harness": 2,
            "max_active_per_model": 3,
            "max_active_per_usage_pool": 4,
        }));
    }

    #[test]
    fn the_responses_carry_their_records() {
        let defaults = CapacityDefaultsGetResponse {
            defaults: super::CapacityGlobalDefaults {
                max_active_per_harness: 2,
                max_active_per_model: 3,
                max_active_per_usage_pool: 4,
                version: 1,
            },
        };
        let encoded = serde_json::to_value(defaults).expect("the response serialises");
        assert_eq!(encoded["defaults"]["max_active_per_model"], json!(3));

        let settings = CapacitySettingsGetResponse {
            project_id: 1,
            caps: caps(),
        };
        let encoded = serde_json::to_value(settings).expect("the response serialises");
        assert_eq!(encoded["project_id"], json!(1));
        assert_eq!(encoded["caps"]["max_active_per_model"], json!(null));
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
    fn every_capacity_schema_rejects_unknown_fields() {
        for name in [
            "CapacityDefaultsGetQuery",
            "CapacityDefaultsGetResponse",
            "CapacityDefaultsUpdateRequest",
            "CapacityGlobalDefaults",
            "CapacityProjectCaps",
            "CapacitySettingsGetQuery",
            "CapacitySettingsGetResponse",
            "CapacitySettingsUpdateRequest",
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
