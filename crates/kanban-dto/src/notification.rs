//! Explicit per-Project notification and mirror preferences.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationSettingsQuery {
    pub project_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationSettingsRecord {
    pub project_id: u64,
    pub local_enabled: bool,
    pub mirror_role: Option<String>,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationSettingsUpdateRequest {
    pub mutation: crate::MutationContext,
    pub project_id: u64,
    pub local_enabled: bool,
    pub mirror_role: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationChannel {
    Local,
    HerdrMirror,
}
impl NotificationChannel {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::HerdrMirror => "herdr_mirror",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotificationTarget {
    Local {},
    HerdrMirror {
        session_name: Option<String>,
        product_workspace: String,
        herdr_workspace: String,
        role: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDeliveryStatus {
    Queued,
    Prepared,
    Submitted,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationDeliveryRecord {
    pub id: u64,
    pub project_id: u64,
    pub item_id: String,
    pub item_version: u64,
    pub channel: NotificationChannel,
    pub target: NotificationTarget,
    pub status: NotificationDeliveryStatus,
    pub receipt: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationPermissionState {
    NotDetermined,
    Denied,
    Granted,
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationPermissionRecord {
    pub state: NotificationPermissionState,
    pub reason: Option<String>,
    pub request_pending: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationPermissionQuery {}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationPermissionRequest {
    pub mutation: crate::MutationContext,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationPermissionResponse {
    pub accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationDeliveriesQuery {
    pub project_id: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationDeliveriesResponse {
    pub deliveries: Vec<NotificationDeliveryRecord>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationRetryRequest {
    pub mutation: crate::MutationContext,
    pub delivery_id: u64,
}
