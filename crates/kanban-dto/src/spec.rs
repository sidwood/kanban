//! Spec payload definitions: the PRD content every client edits, the
//! content-version and execution vocabularies, and the author, edit,
//! approve, supersede, plan, and execution payloads (KAN-S3-US4,
//! KAN-S3-US5). Version rows are immutable once approved and stay
//! queryable superseded, so a Ticket's pin always resolves.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The closed content-version vocabulary on the wire (DR-PS-08).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpecContentState {
    Draft,
    Approved,
    Superseded,
}

impl SpecContentState {
    /// Every content state, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Draft, Self::Approved, Self::Superseded];

    /// The wire name, matching this state's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Approved => "approved",
            Self::Superseded => "superseded",
        }
    }

    /// The state `wire` names, or `None` outside the vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|state| state.as_str() == wire)
    }
}

/// The closed Spec execution vocabulary on the wire (DR-PS-12),
/// tracked separately from every content version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpecExecutionState {
    Unplanned,
    Planned,
    Blocked,
    Ready,
    Active,
    IntegrationReview,
    Complete,
    Cancelled,
}

impl SpecExecutionState {
    /// Every execution state, in vocabulary order.
    pub const ALL: &'static [Self] = &[
        Self::Unplanned,
        Self::Planned,
        Self::Blocked,
        Self::Ready,
        Self::Active,
        Self::IntegrationReview,
        Self::Complete,
        Self::Cancelled,
    ];

    /// The wire name, matching this state's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unplanned => "unplanned",
            Self::Planned => "planned",
            Self::Blocked => "blocked",
            Self::Ready => "ready",
            Self::Active => "active",
            Self::IntegrationReview => "integration_review",
            Self::Complete => "complete",
            Self::Cancelled => "cancelled",
        }
    }

    /// The state `wire` names, or `None` outside the vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|state| state.as_str() == wire)
    }
}

/// The nine PRD sections one Spec carries (DR-PS-07): the lightweight
/// product requirements document of CONTEXT.md. Every section is free
/// text; the name alone is required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecContent {
    /// The Spec's name.
    pub name: String,
    /// The one-line description.
    pub short_description: String,
    /// The problem being solved.
    pub problem_statement: String,
    /// The chosen solution.
    pub solution: String,
    /// The behaviour claims, one `US` bullet per line.
    pub user_stories: String,
    /// The settled implementation decisions.
    pub implementation_decisions: String,
    /// The settled testing decisions.
    pub testing_decisions: String,
    /// What this Spec deliberately does not deliver.
    pub out_of_scope: String,
    /// Anything else worth keeping beside the PRD.
    pub further_notes: String,
}

/// One Spec content version as every client sees it: the PRD exactly
/// as it stood when the version was minted or last drafted. Approved
/// and superseded versions never change again, and superseded ones
/// stay readable so Ticket pins resolve (DR-PS-11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecVersionRecord {
    /// The version's number; the first content is one and every
    /// material change mints the next.
    pub number: u64,
    /// The version's lifecycle state.
    pub state: SpecContentState,
    /// The PRD this version carries.
    pub content: SpecContent,
}

/// The Spec record as every client sees it: the Project it belongs
/// to, the number that Project minted, the execution tracked
/// separately from content, and the Plan it belongs to once planned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecRecord {
    /// The immutable, storage-assigned identity.
    pub id: u64,
    /// The Project this Spec belongs to.
    pub project_id: u64,
    /// The number this Project minted for this Spec; rendered with
    /// the Project's code, for example `CORE-S1`.
    pub number: u64,
    /// The Spec's name, from the current content.
    pub name: String,
    /// The execution state (DR-PS-12).
    pub execution: SpecExecutionState,
    /// The Plan this Spec belongs to once planned; `None` while
    /// unplanned (DR-PS-06).
    pub plan_id: Option<u64>,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// Request payload for the `spec.create` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecCreateRequest {
    pub mutation: super::MutationContext,
    /// The Project the new Spec belongs to.
    pub project_id: u64,
    /// The opening PRD content, minted as draft version one.
    pub content: SpecContent,
}

/// Request payload for the `spec.content.update` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecContentUpdateRequest {
    pub mutation: super::MutationContext,
    /// The Spec whose working content changes.
    pub spec_id: u64,
    /// The new content. A draft is edited in place; content that has
    /// moved on mints a new draft version (DR-PS-10).
    pub content: SpecContent,
}

/// Request payload for the `spec.version.approve` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecVersionApproveRequest {
    pub mutation: super::MutationContext,
    /// The Spec whose draft version becomes approved.
    pub spec_id: u64,
}

/// Request payload for the `spec.version.supersede` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecVersionSupersedeRequest {
    pub mutation: super::MutationContext,
    /// The Spec owning the version being superseded.
    pub spec_id: u64,
    /// The version being superseded, named explicitly (DR-PS-11).
    pub version: u64,
}

/// Request payload for the `spec.plan.join` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecPlanJoinRequest {
    pub mutation: super::MutationContext,
    /// The Spec joining a Plan.
    pub spec_id: u64,
    /// The Plan the Spec joins. Must hold the Spec's number and
    /// belong to the same Project.
    pub plan_id: u64,
}

/// Request payload for the `spec.execution.move` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecExecutionMoveRequest {
    pub mutation: super::MutationContext,
    /// The Spec whose execution moves.
    pub spec_id: u64,
    /// The target state, which must be a legal transition away
    /// (DR-PS-12). `planned` is reached by joining a Plan, never by
    /// this command.
    pub execution: SpecExecutionState,
}

/// Request payload for the `spec.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecListQuery {
    /// The Project whose Specs are listed, terminal execution states
    /// included.
    pub project_id: u64,
}

/// Response payload for the `spec.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecListResponse {
    /// Every Spec of the Project, oldest first, all execution states
    /// included.
    pub specs: Vec<SpecRecord>,
}

/// Request payload for the `spec.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecGetQuery {
    /// The Spec being read.
    pub spec_id: u64,
}

/// Response payload for the `spec.get` query: the Spec and every
/// content version beside it, so prior versions stay queryable for
/// diffing and for Ticket pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecGetResponse {
    /// The Spec's current record.
    pub spec: SpecRecord,
    /// Every content version, oldest first.
    pub versions: Vec<SpecVersionRecord>,
}

/// Request payload for the `spec.version.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecVersionGetQuery {
    /// The Spec owning the version.
    pub spec_id: u64,
    /// The version being read — the pin a Ticket resolves through,
    /// superseded versions included.
    pub number: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        SpecContent, SpecContentState, SpecContentUpdateRequest, SpecCreateRequest,
        SpecExecutionMoveRequest, SpecExecutionState, SpecGetQuery, SpecGetResponse, SpecListQuery,
        SpecListResponse, SpecPlanJoinRequest, SpecRecord, SpecVersionApproveRequest,
        SpecVersionGetQuery, SpecVersionRecord, SpecVersionSupersedeRequest,
    };
    use crate::mutation::MutationContext;
    use crate::schema_definitions;

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 3,
            idempotency_key: "key-1".to_owned(),
        }
    }

    fn content() -> SpecContent {
        SpecContent {
            name: "Plans and specifications".to_owned(),
            short_description: "Versioned Plan graphs of Specs".to_owned(),
            problem_statement: "Planning must survive change.".to_owned(),
            solution: "Freeze at activation.".to_owned(),
            user_stories: "KAN-S3-US4".to_owned(),
            implementation_decisions: "Edges are a separate relation.".to_owned(),
            testing_decisions: "Domain tests prove immutability.".to_owned(),
            out_of_scope: "The Ticket graph proposal.".to_owned(),
            further_notes: "None".to_owned(),
        }
    }

    fn record() -> SpecRecord {
        SpecRecord {
            id: 6,
            project_id: 2,
            number: 4,
            name: "Plans and specifications".to_owned(),
            execution: SpecExecutionState::IntegrationReview,
            plan_id: Some(5),
            version: 11,
        }
    }

    fn version(number: u64, state: SpecContentState) -> SpecVersionRecord {
        SpecVersionRecord {
            number,
            state,
            content: content(),
        }
    }

    #[test]
    fn content_states_round_trip_through_their_wire_names() {
        for state in SpecContentState::ALL {
            assert_eq!(
                SpecContentState::parse(state.as_str()),
                Some(*state),
                "`{}` must survive the round trip",
                state.as_str()
            );
            assert_eq!(
                serde_json::to_value(state).expect("the state encodes"),
                json!(state.as_str()),
                "the wire name and the serialised name must agree"
            );
        }
        assert_eq!(SpecContentState::parse("ghost"), None);
    }

    #[test]
    fn execution_states_round_trip_through_their_wire_names() {
        for state in SpecExecutionState::ALL {
            assert_eq!(
                SpecExecutionState::parse(state.as_str()),
                Some(*state),
                "`{}` must survive the round trip",
                state.as_str()
            );
            assert_eq!(
                serde_json::to_value(state).expect("the state encodes"),
                json!(state.as_str()),
                "the wire name and the serialised name must agree"
            );
        }
        assert_eq!(SpecExecutionState::parse("ghost"), None);
        assert_eq!(
            serde_json::to_value(SpecExecutionState::IntegrationReview).expect("the state encodes"),
            json!("integration_review")
        );
    }

    #[test]
    fn a_record_round_trips_with_its_relations() {
        let encoded = serde_json::to_value(record()).expect("the record serialises");

        assert_eq!(
            encoded,
            json!({
                "id": 6,
                "project_id": 2,
                "number": 4,
                "name": "Plans and specifications",
                "execution": "integration_review",
                "plan_id": 5,
                "version": 11,
            })
        );
        let decoded: SpecRecord = serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, record());

        let unplanned = SpecRecord {
            plan_id: None,
            execution: SpecExecutionState::Unplanned,
            ..record()
        };
        let encoded = serde_json::to_value(&unplanned).expect("the record serialises");
        assert_eq!(encoded["plan_id"], json!(null));
    }

    #[test]
    fn a_get_response_carries_the_spec_and_its_versions() {
        let response = SpecGetResponse {
            spec: record(),
            versions: vec![
                version(1, SpecContentState::Superseded),
                version(2, SpecContentState::Approved),
            ],
        };

        let encoded = serde_json::to_value(&response).expect("the response serialises");
        let decoded: SpecGetResponse =
            serde_json::from_value(encoded).expect("the response deserialises");

        assert_eq!(decoded, response);
        assert_eq!(decoded.versions[0].number, 1);
        assert_eq!(decoded.versions[0].state, SpecContentState::Superseded);
        assert_eq!(decoded.versions[1].state, SpecContentState::Approved);
    }

    #[test]
    fn every_request_round_trips_and_rejects_unknown_fields() {
        round_trips::<SpecCreateRequest>(json!({
            "mutation": context(),
            "project_id": 2,
            "content": content(),
        }));
        round_trips::<SpecContentUpdateRequest>(json!({
            "mutation": context(),
            "spec_id": 6,
            "content": content(),
        }));
        round_trips::<SpecVersionApproveRequest>(json!({
            "mutation": context(),
            "spec_id": 6,
        }));
        round_trips::<SpecVersionSupersedeRequest>(json!({
            "mutation": context(),
            "spec_id": 6,
            "version": 2,
        }));
        round_trips::<SpecPlanJoinRequest>(json!({
            "mutation": context(),
            "spec_id": 6,
            "plan_id": 5,
        }));
        round_trips::<SpecExecutionMoveRequest>(json!({
            "mutation": context(),
            "spec_id": 6,
            "execution": "ready",
        }));
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
    fn every_spec_schema_rejects_unknown_fields() {
        for name in [
            "SpecContent",
            "SpecContentUpdateRequest",
            "SpecCreateRequest",
            "SpecExecutionMoveRequest",
            "SpecGetQuery",
            "SpecGetResponse",
            "SpecListQuery",
            "SpecListResponse",
            "SpecPlanJoinRequest",
            "SpecRecord",
            "SpecVersionApproveRequest",
            "SpecVersionGetQuery",
            "SpecVersionRecord",
            "SpecVersionSupersedeRequest",
        ] {
            let schema = schema_of(name);
            let encoded = serde_json::to_string(&schema).expect("the schema serialises");
            assert!(
                encoded.contains("\"additionalProperties\":false"),
                "{name} should reject unknown fields"
            );
        }
    }

    #[test]
    fn the_queries_hold_their_identities() {
        let list: SpecListQuery =
            serde_json::from_value(json!({ "project_id": 2 })).expect("the list query decodes");
        assert_eq!(list, SpecListQuery { project_id: 2 });

        let get: SpecGetQuery =
            serde_json::from_value(json!({ "spec_id": 6 })).expect("the get query decodes");
        assert_eq!(get, SpecGetQuery { spec_id: 6 });

        let pinned: SpecVersionGetQuery = serde_json::from_value(json!({
            "spec_id": 6,
            "number": 1,
        }))
        .expect("the version query decodes");
        assert_eq!(
            pinned,
            SpecVersionGetQuery {
                spec_id: 6,
                number: 1,
            }
        );

        let response = SpecListResponse {
            specs: vec![record()],
        };
        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(encoded["specs"].as_array().map(Vec::len), Some(1));
    }
}
