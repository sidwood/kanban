use crate::MutationContext;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginLaunchQuery {}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginLaunchState {
    pub instance_id: String,
    pub version: u64,
    pub enabled: Option<bool>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginLaunchSetRequest {
    pub mutation: MutationContext,
    pub instance_id: String,
    pub enabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginLaunchSetResponse {
    pub status: LoginLaunchChangeStatus,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginLaunchChangeStatus {
    ChangeRequested,
}

use schemars::JsonSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceStopRequest {
    pub mutation: MutationContext,
    pub instance_id: String,
    pub warning_id: String,
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceStopResponse {
    pub instance_id: String,
    pub status: ServiceStopStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStopStatus {
    StopRequested,
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceStopWarningQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceStopWarning {
    pub instance_id: String,
    pub version: u64,
    pub warning_id: String,
    pub capabilities: Vec<String>,
}
