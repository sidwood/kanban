//! Herdr observation settings and connection diagnostics (KAN-S8).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Per-Project Herdr observation settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HerdrProjectSettings {
    /// How often reconciliation compares full session state.
    pub reconciliation_interval_secs: u64,
    /// Whether the ten-second whole-session polling fallback is on.
    pub polling_fallback_enabled: bool,
    /// The whole-session polling interval when fallback is enabled.
    pub polling_fallback_interval_secs: u64,
    /// After this many seconds a run is considered stale.
    pub stall_deadline_secs: u64,
    /// After this many seconds a settled session is result-missing.
    pub missing_result_deadline_secs: u64,
    /// The optimistic version for updates.
    pub version: u64,
}

/// Global defaults applied to new Projects and unset fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HerdrGlobalDefaults {
    pub reconciliation_interval_secs: u64,
    pub stall_deadline_secs: u64,
    pub missing_result_deadline_secs: u64,
    pub version: u64,
}

/// Connection diagnostics for one Project's Herdr session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HerdrConnectionDiagnostics {
    /// The named Herdr session, if the Project selected one; absence
    /// selects Herdr's default session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    /// The product Seed Workspace this session maps to.
    pub product_workspace: String,
    /// The required target Herdr workspace, resolved inside the
    /// effective session.
    pub herdr_workspace: String,
    /// Whether the per-session socket is connected.
    pub connected: bool,
    /// When the last full snapshot was captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_snapshot_at: Option<String>,
    /// The last connection or mapping error, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Request payload for the `herdr.settings.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HerdrSettingsGetQuery {
    pub project_id: u64,
}

/// Response payload for the `herdr.settings.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HerdrSettingsGetResponse {
    pub project_id: u64,
    pub settings: HerdrProjectSettings,
    pub diagnostics: HerdrConnectionDiagnostics,
}

/// Request payload for the `herdr.settings.update` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HerdrSettingsUpdateRequest {
    pub mutation: super::MutationContext,
    pub project_id: u64,
    pub reconciliation_interval_secs: u64,
    pub polling_fallback_enabled: bool,
    pub polling_fallback_interval_secs: u64,
    pub stall_deadline_secs: u64,
    pub missing_result_deadline_secs: u64,
}

/// Request payload for the `herdr.defaults.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HerdrDefaultsGetQuery {}

/// Response payload for the `herdr.defaults.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HerdrDefaultsGetResponse {
    pub defaults: HerdrGlobalDefaults,
}

/// Request payload for the `herdr.defaults.update` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HerdrDefaultsUpdateRequest {
    pub mutation: super::MutationContext,
    pub reconciliation_interval_secs: u64,
    pub stall_deadline_secs: u64,
    pub missing_result_deadline_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::{
        HerdrConnectionDiagnostics, HerdrDefaultsGetQuery, HerdrGlobalDefaults,
        HerdrProjectSettings, HerdrSettingsGetQuery, HerdrSettingsUpdateRequest,
    };

    #[test]
    fn settings_get_query_round_trips() {
        let query = HerdrSettingsGetQuery { project_id: 1 };
        let encoded = serde_json::to_string(&query).expect("the query encodes");
        let decoded: HerdrSettingsGetQuery =
            serde_json::from_str(&encoded).expect("the query decodes");
        assert_eq!(decoded, query);
    }

    #[test]
    fn settings_update_request_rejects_unknown_fields() {
        let raw = r#"{
            "mutation": { "optimistic_version": 1, "idempotency_key": "k" },
            "project_id": 1,
            "reconciliation_interval_secs": 300,
            "polling_fallback_enabled": false,
            "polling_fallback_interval_secs": 10,
            "stall_deadline_secs": 3600,
            "missing_result_deadline_secs": 7200,
            "surprise": true
        }"#;
        assert!(serde_json::from_str::<HerdrSettingsUpdateRequest>(raw).is_err());
    }

    #[test]
    fn diagnostics_omit_empty_optional_fields() {
        let diagnostics = HerdrConnectionDiagnostics {
            session_name: None,
            product_workspace: "/workspaces/kanban.seed".to_owned(),
            herdr_workspace: "kanban.seed".to_owned(),
            connected: false,
            last_snapshot_at: None,
            last_error: Some("disconnected".to_owned()),
        };
        let encoded = serde_json::to_value(&diagnostics).expect("diagnostics encode");
        assert!(
            encoded.get("session_name").is_none(),
            "an unnamed session reports nothing, not an empty name"
        );
        assert!(encoded.get("last_snapshot_at").is_none());
        assert_eq!(encoded["herdr_workspace"], "kanban.seed");
        assert_eq!(encoded["last_error"], "disconnected");
    }

    #[test]
    fn defaults_get_query_round_trips() {
        let query = HerdrDefaultsGetQuery {};
        let encoded = serde_json::to_string(&query).expect("the query encodes");
        let decoded: HerdrDefaultsGetQuery =
            serde_json::from_str(&encoded).expect("the query decodes");
        assert_eq!(decoded, query);
    }

    #[test]
    fn global_defaults_round_trip() {
        let defaults = HerdrGlobalDefaults {
            reconciliation_interval_secs: 300,
            stall_deadline_secs: 3600,
            missing_result_deadline_secs: 7200,
            version: 1,
        };
        let encoded = serde_json::to_string(&defaults).expect("defaults encode");
        let decoded: HerdrGlobalDefaults = serde_json::from_str(&encoded).expect("defaults decode");
        assert_eq!(decoded, defaults);
    }

    #[test]
    fn project_settings_round_trip() {
        let settings = HerdrProjectSettings {
            reconciliation_interval_secs: 300,
            polling_fallback_enabled: false,
            polling_fallback_interval_secs: 10,
            stall_deadline_secs: 3600,
            missing_result_deadline_secs: 7200,
            version: 1,
        };
        let encoded = serde_json::to_string(&settings).expect("settings encode");
        let decoded: HerdrProjectSettings =
            serde_json::from_str(&encoded).expect("settings decode");
        assert_eq!(decoded, settings);
    }
}
