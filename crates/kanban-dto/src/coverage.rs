//! Story coverage payload definitions: the executable gate's
//! evaluation surface (KAN-S3-US6). One Spec version's story scope is
//! checked against proposed story-linked criteria, the rules the graph
//! approval gate enforces (DR-PS-13 to DR-PS-15, T23).

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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        CoverageCriterionProposal, CriterionRefusal, RefusedCriterion, SpecCoverageCheckQuery,
        SpecCoverageCheckResponse,
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
    fn every_coverage_schema_rejects_unknown_fields() {
        for name in [
            "CoverageCriterionProposal",
            "CriterionRefusal",
            "RefusedCriterion",
            "SpecCoverageCheckQuery",
            "SpecCoverageCheckResponse",
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
