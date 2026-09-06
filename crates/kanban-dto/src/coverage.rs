//! Story coverage payload definitions: the executable gate's
//! evaluation surface (KAN-S3-US6). One Spec version's story scope is
//! checked against proposed story-linked criteria, the rules the graph
//! approval gate enforces (DR-PS-13 to DR-PS-15, T23), and rendered
//! back as the story-to-criterion-to-Ticket coverage matrix the
//! planning UI exposes (DR-PS-18).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Why one proposed Acceptance Criterion was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CriterionRefusal {
    /// The outcome states nothing.
    NoOutcome,
    /// No User Story is linked (DR-PS-13).
    Unlinked,
    /// The outcome is a technical command (DR-PS-15).
    TechnicalCommand,
    /// A story link names no User Story of this form.
    MalformedStory,
    /// A story link names another Project's story.
    ForeignStory,
}

impl CriterionRefusal {
    /// Every refusal, in vocabulary order.
    pub const ALL: &'static [Self] = &[
        Self::NoOutcome,
        Self::Unlinked,
        Self::TechnicalCommand,
        Self::MalformedStory,
        Self::ForeignStory,
    ];

    /// The wire name, matching this refusal's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoOutcome => "no_outcome",
            Self::Unlinked => "unlinked",
            Self::TechnicalCommand => "technical_command",
            Self::MalformedStory => "malformed_story",
            Self::ForeignStory => "foreign_story",
        }
    }

    /// The refusal `wire` names, or `None` outside the vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|refusal| refusal.as_str() == wire)
    }
}

/// One proposed Acceptance Criterion: an observable outcome and the
/// User Stories it claims, named like `CORE-S3-US6`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CoverageCriterionProposal {
    /// The observable outcome the criterion states.
    pub outcome: String,
    /// The User Stories the criterion claims, one full identity each.
    pub stories: Vec<String>,
}

/// One refused proposal with the rule it broke.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RefusedCriterion {
    /// The refused proposal's outcome text.
    pub outcome: String,
    /// Why the domain rules refused it.
    pub reason: CriterionRefusal,
}

/// Request payload for the `spec.coverage.check` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecCoverageCheckQuery {
    /// The Spec whose pinned version defines the story scope.
    pub spec_id: u64,
    /// The content version the Ticket graph would deliver.
    pub version: u64,
    /// The proposed criteria of the Ticket graph, every ticket's
    /// criteria together.
    pub criteria: Vec<CoverageCriterionProposal>,
}

/// Response payload for the `spec.coverage.check` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecCoverageCheckResponse {
    /// Every User Story the pinned version claims, full identities in
    /// scope order.
    pub scope: Vec<String>,
    /// The stories no criterion claims, in scope order (DR-PS-14).
    pub uncovered: Vec<String>,
    /// The proposals the domain rules refused, in proposal order.
    pub refused: Vec<RefusedCriterion>,
    /// Whether the executable gate passes: no uncovered story and no
    /// refused proposal.
    pub executable: bool,
}

/// Request payload for the `spec.coverage.matrix` query: the
/// story-to-criterion-to-Ticket coverage matrix of one Spec version
/// (DR-PS-18). A null `version` reads the version the Spec's Tickets
/// answer to — the approved one when operative, otherwise the
/// working content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecCoverageMatrixQuery {
    /// The Spec whose coverage matrix is rendered.
    pub spec_id: u64,
    /// The content version the matrix reports; null reads the
    /// approved version when one is operative, else the current one.
    #[serde(default)]
    pub version: Option<u64>,
}

/// One claim inside the coverage matrix: a criterion of one Ticket
/// claiming the story of the row it sits in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecCoverageClaim {
    /// The Ticket whose criterion claims the story.
    pub ticket_id: u64,
    /// The number the Ticket's Project minted for it; rendered with
    /// the Project's code, for example `CORE-T17`.
    pub ticket_number: u64,
    /// The criterion's observable outcome.
    pub outcome: String,
}

/// One row of the coverage matrix: a User Story of the reported
/// version and every claim a Ticket's criterion makes on it, in
/// claim order. An empty claim list is the row's coverage gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecCoverageMatrixRow {
    /// The story, as its full identity `CORE-S3-US6`.
    pub story: String,
    /// The claims Tickets' criteria make on this story, in claim
    /// order; empty while the story stays uncovered (DR-PS-14).
    pub claims: Vec<SpecCoverageClaim>,
}

/// Response payload for the `spec.coverage.matrix` query: the
/// coverage matrix of one Spec version, one row per User Story in
/// scope order, completing the planning diagnostics (DR-PS-18).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecCoverageMatrixResponse {
    /// The Spec the matrix reports.
    pub spec_id: u64,
    /// The content version the matrix reports.
    pub version: u64,
    /// One row per User Story the version claims, in scope order.
    pub stories: Vec<SpecCoverageMatrixRow>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        CoverageCriterionProposal, CriterionRefusal, RefusedCriterion, SpecCoverageCheckQuery,
        SpecCoverageCheckResponse, SpecCoverageClaim, SpecCoverageMatrixQuery,
        SpecCoverageMatrixResponse, SpecCoverageMatrixRow,
    };
    use crate::schema_definitions;

    #[test]
    fn refusals_round_trip_through_their_wire_names() {
        for refusal in CriterionRefusal::ALL {
            assert_eq!(
                CriterionRefusal::parse(refusal.as_str()),
                Some(*refusal),
                "`{}` must survive the round trip",
                refusal.as_str()
            );
            assert_eq!(
                serde_json::to_value(refusal).expect("the refusal encodes"),
                json!(refusal.as_str()),
                "the wire name and the serialised name must agree"
            );
        }
        assert_eq!(CriterionRefusal::parse("ghost"), None);
    }

    #[test]
    fn the_query_round_trips_with_its_proposals() {
        let wire = json!({
            "spec_id": 3,
            "version": 2,
            "criteria": [
                {
                    "outcome": "Every criterion links to one or more User Stories.",
                    "stories": ["CORE-S3-US6", "S3-US7"],
                },
                { "outcome": "The gate refuses uncovered stories.", "stories": [] },
            ],
        });

        let decoded: SpecCoverageCheckQuery =
            serde_json::from_value(wire.clone()).expect("the query decodes typed");
        assert_eq!(
            decoded,
            SpecCoverageCheckQuery {
                spec_id: 3,
                version: 2,
                criteria: vec![
                    CoverageCriterionProposal {
                        outcome: "Every criterion links to one or more User Stories.".to_owned(),
                        stories: vec!["CORE-S3-US6".to_owned(), "S3-US7".to_owned()],
                    },
                    CoverageCriterionProposal {
                        outcome: "The gate refuses uncovered stories.".to_owned(),
                        stories: Vec::new(),
                    },
                ],
            }
        );
        let encoded = serde_json::to_value(&decoded).expect("the query re-encodes");
        assert_eq!(encoded, wire, "the wire form round trips");

        let mut refused = wire.clone();
        refused["surprise"] = json!(true);
        assert!(
            serde_json::from_value::<SpecCoverageCheckQuery>(refused).is_err(),
            "unknown fields are rejected"
        );
        let mut refused_proposal = wire;
        refused_proposal["criteria"][0]["surprise"] = json!(true);
        assert!(
            serde_json::from_value::<SpecCoverageCheckQuery>(refused_proposal).is_err(),
            "unknown fields are rejected inside a proposal"
        );
    }

    #[test]
    fn the_response_round_trips_with_its_refusals() {
        let response = SpecCoverageCheckResponse {
            scope: vec!["CORE-S3-US6".to_owned(), "CORE-S3-US7".to_owned()],
            uncovered: vec!["CORE-S3-US7".to_owned()],
            refused: vec![RefusedCriterion {
                outcome: "cargo test -p kanban-domain coverage".to_owned(),
                reason: CriterionRefusal::TechnicalCommand,
            }],
            executable: false,
        };

        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(
            encoded,
            json!({
                "scope": ["CORE-S3-US6", "CORE-S3-US7"],
                "uncovered": ["CORE-S3-US7"],
                "refused": [
                    {
                        "outcome": "cargo test -p kanban-domain coverage",
                        "reason": "technical_command",
                    }
                ],
                "executable": false,
            })
        );
        let decoded: SpecCoverageCheckResponse =
            serde_json::from_value(encoded).expect("the response deserialises");
        assert_eq!(decoded, response);
    }

    #[test]
    fn the_matrix_round_trips_row_by_row() {
        let response = SpecCoverageMatrixResponse {
            spec_id: 3,
            version: 2,
            stories: vec![
                SpecCoverageMatrixRow {
                    story: "CORE-S3-US6".to_owned(),
                    claims: vec![
                        SpecCoverageClaim {
                            ticket_id: 17,
                            ticket_number: 17,
                            outcome: "Every criterion links to one or more User Stories."
                                .to_owned(),
                        },
                        SpecCoverageClaim {
                            ticket_id: 19,
                            ticket_number: 19,
                            outcome: "Claims accumulate across Tickets.".to_owned(),
                        },
                    ],
                },
                SpecCoverageMatrixRow {
                    story: "CORE-S3-US7".to_owned(),
                    claims: Vec::new(),
                },
            ],
        };

        let encoded = serde_json::to_value(&response).expect("the matrix serialises");
        assert_eq!(
            encoded,
            json!({
                "spec_id": 3,
                "version": 2,
                "stories": [
                    {
                        "story": "CORE-S3-US6",
                        "claims": [
                            {
                                "ticket_id": 17,
                                "ticket_number": 17,
                                "outcome": "Every criterion links to one or more User Stories.",
                            },
                            {
                                "ticket_id": 19,
                                "ticket_number": 19,
                                "outcome": "Claims accumulate across Tickets.",
                            },
                        ],
                    },
                    { "story": "CORE-S3-US7", "claims": [] },
                ],
            })
        );
        let decoded: SpecCoverageMatrixResponse =
            serde_json::from_value(encoded).expect("the matrix deserialises");
        assert_eq!(decoded, response);

        let query = json!({ "spec_id": 3, "version": null });
        let decoded: SpecCoverageMatrixQuery =
            serde_json::from_value(query).expect("the query decodes");
        assert_eq!(
            decoded,
            SpecCoverageMatrixQuery {
                spec_id: 3,
                version: None,
            }
        );
        let mut refused = json!({ "spec_id": 3 });
        refused["surprise"] = json!(true);
        assert!(
            serde_json::from_value::<SpecCoverageMatrixQuery>(refused).is_err(),
            "unknown fields are rejected"
        );
    }

    #[test]
    fn every_coverage_schema_rejects_unknown_fields() {
        for name in [
            "CoverageCriterionProposal",
            "CriterionRefusal",
            "RefusedCriterion",
            "SpecCoverageCheckQuery",
            "SpecCoverageCheckResponse",
            "SpecCoverageClaim",
            "SpecCoverageMatrixQuery",
            "SpecCoverageMatrixResponse",
            "SpecCoverageMatrixRow",
        ] {
            let (_, schema) = schema_definitions()
                .into_iter()
                .find(|(schema_name, _)| *schema_name == name)
                .unwrap_or_else(|| panic!("{name} is registered"));
            let encoded = serde_json::to_string(&schema).expect("the schema serialises");
            assert!(
                encoded.contains("\"additionalProperties\":false") || encoded.contains("\"enum\":"),
                "{name} should reject unknown fields or close its vocabulary"
            );
        }
    }
}
