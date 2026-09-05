use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Request payload for the `diagnostics.export` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsExportQuery {}

/// Response payload for the `diagnostics.export` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsExportResponse {
    /// The exported bundle directory under managed application data.
    pub bundle_dir: String,
}
