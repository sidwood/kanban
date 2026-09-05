//! Export payload definitions: deterministic Markdown rendering into
//! a configured directory within the Seed, and the drift report
//! between those exports and the current planning state (KAN-S6-US5).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Request payload for the `export.render` command. `directory` names
/// the directory within the Project's Seed Workspace the Markdown
/// lands in; a directory that would leave the Seed is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportRenderRequest {
    pub mutation: super::MutationContext,
    pub project_id: u64,
    pub directory: String,
}

/// Response payload for the `export.render` command: the files
/// written, each relative to the configured directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportRenderResponse {
    pub project_id: u64,
    pub directory: String,
    pub files: Vec<String>,
}

/// Request payload for the `export.drift` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportDriftQuery {
    pub project_id: u64,
    pub directory: String,
}

/// The closed drift vocabulary: how one file on disk stands against
/// the current planning state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportDriftStatus {
    /// The current state renders a file the directory does not hold.
    Missing,
    /// The file on disk is not the bytes the current state renders.
    Differs,
    /// The directory holds a file the current state does not render.
    Unmatched,
}

impl ExportDriftStatus {
    /// The wire name, matching this variant's serialised form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Differs => "differs",
            Self::Unmatched => "unmatched",
        }
    }
}

/// One drifted file: its path relative to the configured directory
/// and how it stands against the current planning state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportDriftEntry {
    pub path: String,
    pub status: ExportDriftStatus,
}

/// Response payload for the `export.drift` query: every drifted
/// file, in path order, with the verdict for the whole directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportDriftResponse {
    pub project_id: u64,
    pub directory: String,
    /// Whether any entry drifted; a clean export renders this false
    /// with no entries.
    pub in_drift: bool,
    pub entries: Vec<ExportDriftEntry>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        ExportDriftEntry, ExportDriftQuery, ExportDriftResponse, ExportDriftStatus,
        ExportRenderRequest, ExportRenderResponse,
    };
    use crate::mutation::MutationContext;
    use crate::schema_definitions;

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 0,
            idempotency_key: "key-1".to_owned(),
        }
    }

    #[test]
    fn export_render_request_rejects_unknown_fields() {
        let payload = json!({
            "mutation": context(),
            "project_id": 1,
            "directory": "temp/project-management/docs",
            "surprise": true,
        });

        let error = serde_json::from_value::<ExportRenderRequest>(payload)
            .expect_err("unknown fields are rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn export_drift_query_rejects_unknown_fields() {
        let payload = json!({
            "project_id": 1,
            "directory": "temp/project-management/docs",
            "include_clean": true,
        });

        let error = serde_json::from_value::<ExportDriftQuery>(payload)
            .expect_err("unknown fields are rejected");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn export_render_response_round_trips() {
        let response = ExportRenderResponse {
            project_id: 1,
            directory: "temp/project-management/docs".to_owned(),
            files: vec![
                "plans/CORE-P1.md".to_owned(),
                "specs/CORE-S6.md".to_owned(),
                "tickets/CORE-T35.md".to_owned(),
            ],
        };

        let encoded = serde_json::to_value(&response).expect("the response encodes");
        let decoded: ExportRenderResponse =
            serde_json::from_value(encoded).expect("the response decodes");

        assert_eq!(decoded, response);
    }

    #[test]
    fn export_drift_status_uses_its_wire_names() {
        for (status, wire) in [
            (ExportDriftStatus::Missing, "missing"),
            (ExportDriftStatus::Differs, "differs"),
            (ExportDriftStatus::Unmatched, "unmatched"),
        ] {
            assert_eq!(
                serde_json::to_value(status).expect("the status encodes"),
                json!(wire)
            );
            let decoded: ExportDriftStatus =
                serde_json::from_value(json!(wire)).expect("the status decodes");
            assert_eq!(decoded, status);
            assert_eq!(status.as_str(), wire);
        }
    }

    #[test]
    fn export_drift_response_round_trips() {
        let response = ExportDriftResponse {
            project_id: 1,
            directory: "temp/project-management/docs".to_owned(),
            in_drift: true,
            entries: vec![
                ExportDriftEntry {
                    path: "tickets/CORE-T35.md".to_owned(),
                    status: ExportDriftStatus::Differs,
                },
                ExportDriftEntry {
                    path: "plans/CORE-P2.md".to_owned(),
                    status: ExportDriftStatus::Missing,
                },
                ExportDriftEntry {
                    path: "specs/CORE-S9.md".to_owned(),
                    status: ExportDriftStatus::Unmatched,
                },
            ],
        };

        let encoded = serde_json::to_value(&response).expect("the response encodes");
        let decoded: ExportDriftResponse =
            serde_json::from_value(encoded).expect("the response decodes");

        assert_eq!(decoded, response);
    }

    #[test]
    fn export_payloads_are_in_the_schema_registry() {
        let names: Vec<_> = schema_definitions()
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        for name in [
            "ExportRenderRequest",
            "ExportRenderResponse",
            "ExportDriftQuery",
            "ExportDriftResponse",
            "ExportDriftEntry",
            "ExportDriftStatus",
        ] {
            assert!(names.contains(&name), "`{name}` must be registered");
        }
    }
}
