//! Plan payload definitions: the record every client sees, the graph
//! it carries, and the create, edit, lifecycle, and query payloads
//! (KAN-S3-US1, KAN-S3-US2, KAN-S3-US3). Display order and dependency
//! edges ride the record as the two separate relations they are, and
//! there is deliberately no delete payload.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The closed Plan lifecycle vocabulary on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanState {
    Draft,
    Active,
    Complete,
    Cancelled,
    Archived,
}

impl PlanState {
    /// Every lifecycle state, in vocabulary order.
    pub const ALL: &'static [Self] = &[
        Self::Draft,
        Self::Active,
        Self::Complete,
        Self::Cancelled,
        Self::Archived,
    ];

    /// The wire name, matching this state's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Complete => "complete",
            Self::Cancelled => "cancelled",
            Self::Archived => "archived",
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

/// One dependency edge of a Plan's graph: `from_spec` must land
/// before `to_spec` may begin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanEdge {
    /// The Spec that must land first.
    pub from_spec: u64,
    /// The Spec that waits on `from_spec`.
    pub to_spec: u64,
}

/// The Plan record as every client sees it: the lifecycle state, the
/// working display order, and the working dependency graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanRecord {
    /// The immutable, storage-assigned identity.
    pub id: u64,
    /// The Project this Plan belongs to.
    pub project_id: u64,
    /// The number this Project minted for this Plan; rendered with
    /// the Project's code, for example `CORE-P1`.
    pub number: u64,
    /// The lifecycle state.
    pub state: PlanState,
    /// The display order: the member Spec numbers as a per-Plan
    /// sequence.
    pub spec_numbers: Vec<u64>,
    /// The dependency edges, held separately from the display order.
    pub edges: Vec<PlanEdge>,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// One frozen Plan version as every client sees it: the Spec
/// membership, display order, and dependency graph exactly as they
/// stood at activation. Immutable once minted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanVersionRecord {
    /// The version's number; the first freeze is one.
    pub number: u64,
    /// The frozen display order.
    pub spec_numbers: Vec<u64>,
    /// The frozen dependency graph.
    pub edges: Vec<PlanEdge>,
}

/// Request payload for the `plan.create` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanCreateRequest {
    pub mutation: super::MutationContext,
    /// The Project the new Plan belongs to.
    pub project_id: u64,
}

/// Request payload for the `plan.spec.add` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanSpecAddRequest {
    pub mutation: super::MutationContext,
    /// The Plan gaining a member.
    pub plan_id: u64,
    /// The Spec number joining the Plan.
    pub spec_number: u64,
}

/// Request payload for the `plan.spec.remove` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanSpecRemoveRequest {
    pub mutation: super::MutationContext,
    /// The Plan losing a member.
    pub plan_id: u64,
    /// The Spec number leaving the Plan.
    pub spec_number: u64,
}

/// Request payload for the `plan.spec.move` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanSpecMoveRequest {
    pub mutation: super::MutationContext,
    /// The Plan whose display order changes.
    pub plan_id: u64,
    /// The Spec number moving.
    pub spec_number: u64,
    /// The position in the display order the Spec moves to.
    pub position: u64,
}

/// Request payload for the `plan.edge.add` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanEdgeAddRequest {
    pub mutation: super::MutationContext,
    /// The Plan whose graph gains an edge.
    pub plan_id: u64,
    /// The Spec that must land first.
    pub from_spec: u64,
    /// The Spec that waits on `from_spec`.
    pub to_spec: u64,
}

/// Request payload for the `plan.edge.remove` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanEdgeRemoveRequest {
    pub mutation: super::MutationContext,
    /// The Plan whose graph loses an edge.
    pub plan_id: u64,
    /// The Spec that must land first.
    pub from_spec: u64,
    /// The Spec that waits on `from_spec`.
    pub to_spec: u64,
}

/// Request payload for the `plan.activate` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanActivateRequest {
    pub mutation: super::MutationContext,
    /// The Plan freezing its shape into a version.
    pub plan_id: u64,
}

/// Request payload for the `plan.replan` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanReplanRequest {
    pub mutation: super::MutationContext,
    /// The Plan whose replacement version is reserved.
    pub plan_id: u64,
}

/// Request payload for the `plan.complete` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanCompleteRequest {
    pub mutation: super::MutationContext,
    /// The Plan completing.
    pub plan_id: u64,
}

/// Request payload for the `plan.cancel` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanCancelRequest {
    pub mutation: super::MutationContext,
    /// The Plan being cancelled.
    pub plan_id: u64,
}

/// Request payload for the `plan.archive` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanArchiveRequest {
    pub mutation: super::MutationContext,
    /// The Plan being archived. Archiving is terminal and preserves
    /// every recorded fact.
    pub plan_id: u64,
}

/// Request payload for the `plan.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanListQuery {
    /// The Project whose Plans are listed, terminal states included.
    pub project_id: u64,
}

/// Response payload for the `plan.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanListResponse {
    /// Every Plan of the Project, newest last, all states included.
    pub plans: Vec<PlanRecord>,
}

/// Request payload for the `plan.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanGetQuery {
    /// The Plan being read.
    pub plan_id: u64,
}

/// Response payload for the `plan.get` query: the Plan and every
/// frozen version beside it, so prior versions stay queryable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanGetResponse {
    /// The Plan's current record.
    pub plan: PlanRecord,
    /// Every frozen version, oldest first.
    pub versions: Vec<PlanVersionRecord>,
}

/// One dependency cycle the planning diagnostics report: a group of
/// member Specs mutually reachable through the Plan's dependency
/// edges, so no execution order exists among them (DR-PS-18).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanCycle {
    /// The ring's member Spec numbers, ascending.
    pub spec_numbers: Vec<u64>,
}

/// One member Spec's coverage gap against the Plan: the User Stories
/// its scope claims that no criterion claims yet (DR-PS-14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanCoverageGap {
    /// The member Spec whose stories stay uncovered.
    pub spec_number: u64,
    /// Every uncovered User Story, full identities in scope order.
    pub uncovered: Vec<String>,
    /// The version's story section names no User Story at all: an
    /// empty scope admits no coverage, so no Ticket graph could ever
    /// cover this Spec.
    pub claims_no_stories: bool,
}

/// One profile reference that resolves to no catalogue entry. This is
/// the invalid-profile diagnostics interface of DR-PS-18: the
/// execution profile catalogue feeds the entries (KAN-S7, T38), so
/// the list rides empty until the catalogue exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanInvalidProfile {
    /// The referenced profile identity, as written where it was
    /// referenced.
    pub reference: String,
}

/// Request payload for the `plan.diagnostics` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanDiagnosticsQuery {
    /// The Plan whose graph is diagnosed.
    pub plan_id: u64,
    /// The frozen version whose graph is diagnosed; when `null` the
    /// working shape on display is read instead.
    pub version: Option<u64>,
}

/// Response payload for the `plan.diagnostics` query: the blocking
/// diagnostics of one graph (DR-PS-18).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanDiagnosticsResponse {
    /// Every dependency cycle the diagnosed graph holds.
    pub cycles: Vec<PlanCycle>,
    /// One entry per member Spec whose stories stay uncovered; the
    /// story-to-criterion-to-Ticket coverage matrix completes the
    /// picture with graph approval (T23).
    pub coverage_gaps: Vec<PlanCoverageGap>,
    /// Profile references resolving to no catalogue entry; the
    /// catalogue feeds this list (KAN-S7, T38).
    pub invalid_profiles: Vec<PlanInvalidProfile>,
    /// Whether any diagnostic blocks the Plan's Ticket graph from
    /// becoming executable: a cycle, a coverage gap, or an invalid
    /// profile reference.
    pub blocking: bool,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        PlanActivateRequest, PlanArchiveRequest, PlanCancelRequest, PlanCompleteRequest,
        PlanCoverageGap, PlanCreateRequest, PlanCycle, PlanDiagnosticsQuery,
        PlanDiagnosticsResponse, PlanEdge, PlanEdgeAddRequest, PlanEdgeRemoveRequest, PlanGetQuery,
        PlanGetResponse, PlanInvalidProfile, PlanListQuery, PlanListResponse, PlanRecord,
        PlanReplanRequest, PlanSpecAddRequest, PlanSpecMoveRequest, PlanSpecRemoveRequest,
        PlanState, PlanVersionRecord,
    };
    use crate::mutation::MutationContext;
    use crate::schema_definitions;

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 3,
            idempotency_key: "key-1".to_owned(),
        }
    }

    fn edge(from: u64, to: u64) -> PlanEdge {
        PlanEdge {
            from_spec: from,
            to_spec: to,
        }
    }

    fn record() -> PlanRecord {
        PlanRecord {
            id: 5,
            project_id: 2,
            number: 1,
            state: PlanState::Active,
            spec_numbers: vec![1, 3, 2],
            edges: vec![edge(1, 2), edge(3, 2)],
            version: 9,
        }
    }

    fn version(number: u64) -> PlanVersionRecord {
        PlanVersionRecord {
            number,
            spec_numbers: vec![1, 3, 2],
            edges: vec![edge(1, 2), edge(3, 2)],
        }
    }

    #[test]
    fn lifecycle_states_round_trip_through_their_wire_names() {
        for state in PlanState::ALL {
            assert_eq!(
                PlanState::parse(state.as_str()),
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
        assert_eq!(PlanState::parse("ghost"), None);
    }

    #[test]
    fn a_record_round_trips_with_its_two_relations() {
        let encoded = serde_json::to_value(record()).expect("the record serialises");

        assert_eq!(
            encoded,
            json!({
                "id": 5,
                "project_id": 2,
                "number": 1,
                "state": "active",
                "spec_numbers": [1, 3, 2],
                "edges": [
                    { "from_spec": 1, "to_spec": 2 },
                    { "from_spec": 3, "to_spec": 2 },
                ],
                "version": 9,
            })
        );
        let decoded: PlanRecord = serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, record());
    }

    #[test]
    fn a_get_response_carries_the_plan_and_its_versions() {
        let response = PlanGetResponse {
            plan: record(),
            versions: vec![version(1), version(2)],
        };

        let encoded = serde_json::to_value(&response).expect("the response serialises");
        let decoded: PlanGetResponse =
            serde_json::from_value(encoded).expect("the response deserialises");

        assert_eq!(decoded, response);
        assert_eq!(decoded.versions[0].number, 1);
        assert_eq!(decoded.versions[1].number, 2);
    }

    #[test]
    fn every_request_round_trips_and_rejects_unknown_fields() {
        round_trips::<PlanCreateRequest>(json!({
            "mutation": context(),
            "project_id": 2,
        }));
        round_trips::<PlanSpecAddRequest>(json!({
            "mutation": context(),
            "plan_id": 5,
            "spec_number": 4,
        }));
        round_trips::<PlanSpecRemoveRequest>(json!({
            "mutation": context(),
            "plan_id": 5,
            "spec_number": 4,
        }));
        round_trips::<PlanSpecMoveRequest>(json!({
            "mutation": context(),
            "plan_id": 5,
            "spec_number": 2,
            "position": 0,
        }));
        round_trips::<PlanEdgeAddRequest>(json!({
            "mutation": context(),
            "plan_id": 5,
            "from_spec": 1,
            "to_spec": 2,
        }));
        round_trips::<PlanEdgeRemoveRequest>(json!({
            "mutation": context(),
            "plan_id": 5,
            "from_spec": 1,
            "to_spec": 2,
        }));
        round_trips::<PlanActivateRequest>(json!({ "mutation": context(), "plan_id": 5 }));
        round_trips::<PlanReplanRequest>(json!({ "mutation": context(), "plan_id": 5 }));
        round_trips::<PlanCompleteRequest>(json!({ "mutation": context(), "plan_id": 5 }));
        round_trips::<PlanCancelRequest>(json!({ "mutation": context(), "plan_id": 5 }));
        round_trips::<PlanArchiveRequest>(json!({ "mutation": context(), "plan_id": 5 }));
        round_trips::<PlanDiagnosticsQuery>(json!({ "plan_id": 5, "version": null }));
        round_trips::<PlanDiagnosticsQuery>(json!({ "plan_id": 5, "version": 3 }));
    }

    #[test]
    fn a_diagnostics_query_may_leave_the_version_out() {
        let decoded: PlanDiagnosticsQuery =
            serde_json::from_value(json!({ "plan_id": 5 })).expect("the query decodes");
        assert_eq!(
            decoded.version, None,
            "an absent version reads the working shape"
        );
    }

    #[test]
    fn a_diagnostics_response_round_trips_with_every_diagnostic() {
        let response = PlanDiagnosticsResponse {
            cycles: vec![PlanCycle {
                spec_numbers: vec![1, 2],
            }],
            coverage_gaps: vec![
                PlanCoverageGap {
                    spec_number: 1,
                    uncovered: vec!["CORE-S1-US1".to_owned(), "CORE-S1-US2".to_owned()],
                    claims_no_stories: false,
                },
                PlanCoverageGap {
                    spec_number: 3,
                    uncovered: Vec::new(),
                    claims_no_stories: true,
                },
            ],
            invalid_profiles: vec![PlanInvalidProfile {
                reference: "ghost-profile".to_owned(),
            }],
            blocking: true,
        };

        let encoded = serde_json::to_value(&response).expect("the response serialises");

        assert_eq!(
            encoded,
            json!({
                "cycles": [{ "spec_numbers": [1, 2] }],
                "coverage_gaps": [
                    {
                        "spec_number": 1,
                        "uncovered": ["CORE-S1-US1", "CORE-S1-US2"],
                        "claims_no_stories": false,
                    },
                    {
                        "spec_number": 3,
                        "uncovered": [],
                        "claims_no_stories": true,
                    },
                ],
                "invalid_profiles": [{ "reference": "ghost-profile" }],
                "blocking": true,
            })
        );
        let decoded: PlanDiagnosticsResponse =
            serde_json::from_value(encoded).expect("the response deserialises");
        assert_eq!(decoded, response);
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
    fn every_plan_schema_rejects_unknown_fields() {
        for name in [
            "PlanActivateRequest",
            "PlanArchiveRequest",
            "PlanCancelRequest",
            "PlanCompleteRequest",
            "PlanCoverageGap",
            "PlanCreateRequest",
            "PlanCycle",
            "PlanDiagnosticsQuery",
            "PlanDiagnosticsResponse",
            "PlanEdge",
            "PlanEdgeAddRequest",
            "PlanEdgeRemoveRequest",
            "PlanGetQuery",
            "PlanGetResponse",
            "PlanInvalidProfile",
            "PlanListQuery",
            "PlanListResponse",
            "PlanRecord",
            "PlanReplanRequest",
            "PlanSpecAddRequest",
            "PlanSpecMoveRequest",
            "PlanSpecRemoveRequest",
            "PlanVersionRecord",
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
        let list: PlanListQuery =
            serde_json::from_value(json!({ "project_id": 2 })).expect("the list query decodes");
        assert_eq!(list, PlanListQuery { project_id: 2 });

        let get: PlanGetQuery =
            serde_json::from_value(json!({ "plan_id": 5 })).expect("the get query decodes");
        assert_eq!(get, PlanGetQuery { plan_id: 5 });

        let response = PlanListResponse {
            plans: vec![record()],
        };
        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(encoded["plans"].as_array().map(Vec::len), Some(1));
    }
}
