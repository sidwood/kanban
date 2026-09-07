//! Staged review payload definitions (KAN-S10-US1): the wire shape of
//! one Ticket's review configuration — ordered stages of parallel
//! slots, each slot required or optional and occupied by a human or
//! by an Execution Profile named by reference — with the configure
//! command that replaces it whole and the read-back query that serves
//! it (DR-EP-09). Separation is the core's rule at configuration
//! time (DR-EP-13 to DR-EP-15); the wire carries the shape alone.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The closed slot requirement vocabulary on the wire (DR-EP-09):
/// every required slot in a stage must finish before the stage
/// resolves; an optional slot never blocks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketReviewSlotRequirement {
    /// The stage cannot resolve without this slot finishing.
    Required,
    /// The slot never blocks the stage it sits in.
    Optional,
}

impl TicketReviewSlotRequirement {
    /// Every requirement, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Required, Self::Optional];

    /// The wire name of this requirement.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Optional => "optional",
        }
    }

    /// The requirement `wire` names, or `None` outside the vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|requirement| requirement.as_str() == wire)
    }
}

/// One slot's occupant on the wire: a human reviewer, or an agent
/// reviewer under a named Execution Profile. A human slot carries no
/// profile and is exempt from separation (DR-EP-15); a profile slot
/// names its entry by reference, never inlined values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TicketReviewOccupant {
    /// A human reviews the slot.
    Human,
    /// An agent reviews under the profile `name` references.
    Profile {
        /// The Execution Profile the slot's assignment names, by its
        /// catalogue name.
        name: String,
    },
}

/// One parallel review slot on the wire (DR-EP-09): its occupant and
/// its requirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketReviewSlot {
    /// The slot's occupant.
    pub occupant: TicketReviewOccupant,
    /// Whether the stage cannot resolve without this slot finishing.
    pub requirement: TicketReviewSlotRequirement,
}

/// One ordered stage on the wire (DR-EP-09): parallel slots that
/// resolve together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketReviewStage {
    /// The parallel slots, in configured order.
    pub slots: Vec<TicketReviewSlot>,
}

/// Request payload for the `ticket.review.configure` command: the
/// Ticket's whole review configuration, replaced in one act. The
/// stages are ordered; each stage holds its parallel slots whole.
/// Separation against the Ticket's implementer assignment is
/// validated at configuration time (DR-EP-13 to DR-EP-15).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketReviewConfigureRequest {
    pub mutation: super::MutationContext,
    /// The Ticket whose review is configured.
    pub ticket_id: u64,
    /// The ordered stages, replaced whole.
    pub stages: Vec<TicketReviewStage>,
}

/// The review configuration record as every client sees it: the
/// Ticket it configures, the ordered stages it holds, and the
/// aggregate version for optimistic mutation checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketReviewConfigRecord {
    /// The Ticket this configuration reviews.
    pub ticket_id: u64,
    /// The ordered stages, in configured order.
    pub stages: Vec<TicketReviewStage>,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// Request payload for the `ticket.review.config` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketReviewConfigQuery {
    /// The Ticket whose configuration is read.
    pub ticket_id: u64,
}

/// Response payload for the `ticket.review.config` query: the stored
/// configuration, or nothing while the Ticket carries none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketReviewConfigResponse {
    /// The Ticket's stored configuration, if one exists.
    pub config: Option<TicketReviewConfigRecord>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        TicketReviewConfigQuery, TicketReviewConfigRecord, TicketReviewConfigResponse,
        TicketReviewConfigureRequest, TicketReviewOccupant, TicketReviewSlot,
        TicketReviewSlotRequirement, TicketReviewStage,
    };
    use crate::mutation::MutationContext;
    use crate::schema_definitions;

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 3,
            idempotency_key: "key-1".to_owned(),
        }
    }

    /// One varied configuration: two stages, the first holding a
    /// required profile slot and an optional human slot in parallel,
    /// the second holding one required profile slot.
    fn stages() -> Vec<TicketReviewStage> {
        vec![
            TicketReviewStage {
                slots: vec![
                    TicketReviewSlot {
                        occupant: TicketReviewOccupant::Profile {
                            name: "outsider".to_owned(),
                        },
                        requirement: TicketReviewSlotRequirement::Required,
                    },
                    TicketReviewSlot {
                        occupant: TicketReviewOccupant::Human,
                        requirement: TicketReviewSlotRequirement::Optional,
                    },
                ],
            },
            TicketReviewStage {
                slots: vec![TicketReviewSlot {
                    occupant: TicketReviewOccupant::Profile {
                        name: "same-harness".to_owned(),
                    },
                    requirement: TicketReviewSlotRequirement::Required,
                }],
            },
        ]
    }

    #[test]
    fn the_requirement_vocabulary_round_trips() {
        assert_eq!(TicketReviewSlotRequirement::ALL.len(), 2);
        for requirement in TicketReviewSlotRequirement::ALL {
            assert_eq!(
                TicketReviewSlotRequirement::parse(requirement.as_str()),
                Some(*requirement),
                "`{}` must survive the round trip",
                requirement.as_str()
            );
        }
        assert_eq!(TicketReviewSlotRequirement::parse("ghost"), None);
    }

    #[test]
    fn a_configuration_round_trips_with_tagged_occupants() {
        let request = TicketReviewConfigureRequest {
            mutation: context(),
            ticket_id: 4,
            stages: stages(),
        };

        let encoded = serde_json::to_value(&request).expect("the request serialises");
        assert_eq!(
            encoded,
            json!({
                "mutation": context(),
                "ticket_id": 4,
                "stages": [
                    {
                        "slots": [
                            {
                                "occupant": { "kind": "profile", "name": "outsider" },
                                "requirement": "required",
                            },
                            {
                                "occupant": { "kind": "human" },
                                "requirement": "optional",
                            },
                        ],
                    },
                    {
                        "slots": [
                            {
                                "occupant": { "kind": "profile", "name": "same-harness" },
                                "requirement": "required",
                            },
                        ],
                    },
                ],
            })
        );
        let decoded: TicketReviewConfigureRequest =
            serde_json::from_value(encoded).expect("the request deserialises");
        assert_eq!(decoded, request);

        // An occupant outside the closed vocabulary refuses the
        // payload, and so does an unknown field.
        let mut ghost = serde_json::to_value(&request).expect("the request serialises");
        ghost["stages"][0]["slots"][0]["occupant"] = json!({ "kind": "robot" });
        assert!(
            serde_json::from_value::<TicketReviewConfigureRequest>(ghost).is_err(),
            "an occupant outside the closed vocabulary is rejected"
        );
        let mut surprised = serde_json::to_value(&request).expect("the request serialises");
        surprised["surprise"] = json!(true);
        assert!(
            serde_json::from_value::<TicketReviewConfigureRequest>(surprised).is_err(),
            "unknown fields are rejected"
        );
    }

    #[test]
    fn the_record_and_query_round_trip() {
        let record = TicketReviewConfigRecord {
            ticket_id: 4,
            stages: stages(),
            version: 2,
        };
        let encoded = serde_json::to_value(&record).expect("the record serialises");
        assert_eq!(encoded["ticket_id"], json!(4));
        assert_eq!(encoded["version"], json!(2));
        assert_eq!(
            encoded["stages"][0]["slots"].as_array().map(Vec::len),
            Some(2)
        );
        let decoded: TicketReviewConfigRecord =
            serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, record);

        let query: TicketReviewConfigQuery =
            serde_json::from_value(json!({ "ticket_id": 4 })).expect("the query decodes");
        assert_eq!(query, TicketReviewConfigQuery { ticket_id: 4 });

        let absent = TicketReviewConfigResponse { config: None };
        assert_eq!(
            serde_json::to_value(&absent).expect("the response serialises"),
            json!({ "config": null })
        );
        let present = TicketReviewConfigResponse {
            config: Some(record),
        };
        let encoded = serde_json::to_value(&present).expect("the response serialises");
        assert_eq!(encoded["config"]["version"], json!(2));
        let decoded: TicketReviewConfigResponse =
            serde_json::from_value(encoded).expect("the response deserialises");
        assert_eq!(decoded, present);
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
    fn every_review_schema_is_registered_and_closed() {
        for name in [
            "TicketReviewConfigQuery",
            "TicketReviewConfigRecord",
            "TicketReviewConfigResponse",
            "TicketReviewConfigureRequest",
            "TicketReviewOccupant",
            "TicketReviewSlot",
            "TicketReviewSlotRequirement",
            "TicketReviewStage",
        ] {
            let schema = schema_of(name);
            let encoded = serde_json::to_string(&schema).expect("the schema serialises");
            // An enum is a closed vocabulary, not a struct with
            // fields: a plain enum denies by its own set, and an
            // internally tagged enum carries its discriminator as
            // data, so neither can also deny unknown fields. Every
            // payload struct must.
            if !matches!(name, "TicketReviewOccupant" | "TicketReviewSlotRequirement") {
                assert!(
                    encoded.contains("\"additionalProperties\":false"),
                    "{name} should reject unknown fields"
                );
            }
        }
    }
}
