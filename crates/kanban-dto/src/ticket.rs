//! Ticket payload definitions: the kind, priority, and lifecycle
//! vocabularies, the per-kind creation payload, and the record every
//! client sees (KAN-S4-US1, KAN-S4-US2). Each kind sends exactly its
//! own fields on creation — an Implementation attaches to one Spec
//! and carries its slice and story-linked criteria; a Bug or Task
//! carries a title and an optional attachment — and fields a later
//! slice owns, like Bug qualification, arrive with that slice, never
//! here early.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The closed Ticket kind vocabulary on the wire (DR-TK-01).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketKind {
    /// A small vertical product slice of exactly one Spec.
    Implementation,
    /// A record of incorrect behaviour, with qualification evidence.
    Bug,
    /// Bounded operational, investigative, or administrative work.
    Task,
}

impl TicketKind {
    /// Every kind, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Implementation, Self::Bug, Self::Task];

    /// The wire name, matching this kind's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Implementation => "implementation",
            Self::Bug => "bug",
            Self::Task => "task",
        }
    }

    /// The kind `wire` names, or `None` outside the vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.as_str() == wire)
    }
}

/// The closed priority vocabulary on the wire (DR-LC-12): urgent,
/// high, normal, low.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketPriority {
    /// Ahead of everything else.
    Urgent,
    /// Ahead of normal work.
    High,
    /// The everyday order.
    Normal,
    /// Behind normal work.
    Low,
}

impl TicketPriority {
    /// Every priority, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Urgent, Self::High, Self::Normal, Self::Low];

    /// The wire name, matching this priority's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Urgent => "urgent",
            Self::High => "high",
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }

    /// The priority `wire` names, or `None` outside the vocabulary.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|priority| priority.as_str() == wire)
    }
}

/// The closed Ticket lifecycle vocabulary on the wire (DR-LC-01): the
/// canonical states in order, with the terminal states after them.
/// Every Ticket is created into draft; the transition rules are the
/// lifecycle slice's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketState {
    /// Newly created; not yet qualified or planned.
    Draft,
    /// Deliberately set aside.
    Parked,
    /// Waiting on something outside the Ticket.
    Blocked,
    /// Qualified but unavailable until activation.
    Scheduled,
    /// Free to execute when capacity allows.
    Ready,
    /// Executing.
    Active,
    /// Under review.
    InReview,
    /// Review approved, waiting to land.
    Approved,
    /// Landing through the Seed Workspace.
    Landing,
    /// Landed in full.
    Done,
    /// Terminal: will not execute.
    Cancelled,
    /// Terminal: replaced by a reassignment Ticket.
    Superseded,
}

impl TicketState {
    /// Every state, canonical order first, terminal states last.
    pub const ALL: &'static [Self] = &[
        Self::Draft,
        Self::Parked,
        Self::Blocked,
        Self::Scheduled,
        Self::Ready,
        Self::Active,
        Self::InReview,
        Self::Approved,
        Self::Landing,
        Self::Done,
        Self::Cancelled,
        Self::Superseded,
    ];

    /// The wire name, matching this state's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Parked => "parked",
            Self::Blocked => "blocked",
            Self::Scheduled => "scheduled",
            Self::Ready => "ready",
            Self::Active => "active",
            Self::InReview => "in_review",
            Self::Approved => "approved",
            Self::Landing => "landing",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
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

/// One story-linked criterion an Implementation Ticket claims: an
/// observable outcome and the User Stories it delivers, named like
/// `CORE-S3-US6` or `S3-US6` (DR-TK-04, DR-PS-13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketCriterion {
    /// The observable outcome the criterion states.
    pub outcome: String,
    /// The User Stories the criterion claims.
    pub stories: Vec<String>,
}

/// Request payload for the `ticket.create` command. Each kind sends
/// exactly its own fields: an Implementation names its Spec, slice,
/// and criteria; a Bug or Task names its title and may name a Spec.
/// Fields the kind does not carry are simply absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketCreateRequest {
    pub mutation: super::MutationContext,
    /// The Project the new Ticket belongs to.
    pub project_id: u64,
    /// The kind whose schema the Ticket carries.
    pub kind: TicketKind,
    /// The Ticket's priority (DR-LC-12).
    pub priority: TicketPriority,
    /// The Spec this Ticket attaches to. An Implementation must name
    /// exactly one (DR-TK-02); a Bug or Task may name one or none
    /// (DR-TK-03).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_id: Option<u64>,
    /// The Bug or Task title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The Implementation slice description, naming the behaviour
    /// delivered end to end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slice: Option<String>,
    /// The Implementation's story-linked criteria.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<Vec<TicketCriterion>>,
}

/// The Ticket record as every client sees it: the Project it belongs
/// to, the number that Project minted, the kind whose schema it
/// carries, and the kind-specific fields — a title for Bugs and
/// Tasks, a slice and criteria for Implementations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketRecord {
    /// The immutable, storage-assigned identity.
    pub id: u64,
    /// The Project this Ticket belongs to.
    pub project_id: u64,
    /// The number this Project minted for this Ticket; rendered with
    /// the Project's code, for example `CORE-T17`.
    pub number: u64,
    /// The kind whose schema this Ticket carries.
    pub kind: TicketKind,
    /// The Ticket's priority (DR-LC-12).
    pub priority: TicketPriority,
    /// The lifecycle state (DR-LC-01).
    pub state: TicketState,
    /// The Spec this Ticket attaches to, if it attaches to one.
    pub spec_id: Option<u64>,
    /// The Bug or Task title, if this Ticket carries one.
    pub title: Option<String>,
    /// The Implementation slice description, if this Ticket carries
    /// one.
    pub slice: Option<String>,
    /// The Implementation's story-linked criteria; empty for every
    /// other kind.
    pub criteria: Vec<TicketCriterion>,
    /// The aggregate version, for optimistic mutation checks.
    pub version: u64,
}

/// Request payload for the `ticket.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketListQuery {
    /// The Project whose Tickets are listed, terminal states
    /// included.
    pub project_id: u64,
}

/// Response payload for the `ticket.list` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketListResponse {
    /// Every Ticket of the Project, oldest first, all lifecycle
    /// states included.
    pub tickets: Vec<TicketRecord>,
}

/// Request payload for the `ticket.get` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketGetQuery {
    /// The Ticket being read.
    pub ticket_id: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        TicketCreateRequest, TicketCriterion, TicketGetQuery, TicketKind, TicketListQuery,
        TicketListResponse, TicketPriority, TicketRecord, TicketState,
    };
    use crate::mutation::MutationContext;
    use crate::schema_definitions;

    fn context() -> MutationContext {
        MutationContext {
            optimistic_version: 3,
            idempotency_key: "key-1".to_owned(),
        }
    }

    fn criterion() -> TicketCriterion {
        TicketCriterion {
            outcome: "Projects register with unique codes.".to_owned(),
            stories: vec!["CORE-S1-US1".to_owned(), "S1-US2".to_owned()],
        }
    }

    fn record() -> TicketRecord {
        TicketRecord {
            id: 6,
            project_id: 2,
            number: 17,
            kind: TicketKind::Implementation,
            priority: TicketPriority::High,
            state: TicketState::Draft,
            spec_id: Some(4),
            title: None,
            slice: Some("Registration creates Projects end to end".to_owned()),
            criteria: vec![criterion()],
            version: 1,
        }
    }

    #[test]
    fn kinds_priorities_and_states_round_trip_through_their_wire_names() {
        for kind in TicketKind::ALL {
            assert_eq!(
                TicketKind::parse(kind.as_str()),
                Some(*kind),
                "`{}` must survive the round trip",
                kind.as_str()
            );
            assert_eq!(
                serde_json::to_value(kind).expect("the kind encodes"),
                json!(kind.as_str()),
                "the wire name and the serialised name must agree"
            );
        }
        assert_eq!(TicketKind::parse("ghost"), None);

        for priority in TicketPriority::ALL {
            assert_eq!(
                TicketPriority::parse(priority.as_str()),
                Some(*priority),
                "`{}` must survive the round trip",
                priority.as_str()
            );
            assert_eq!(
                serde_json::to_value(priority).expect("the priority encodes"),
                json!(priority.as_str())
            );
        }
        assert_eq!(TicketPriority::parse("ghost"), None);

        for state in TicketState::ALL {
            assert_eq!(
                TicketState::parse(state.as_str()),
                Some(*state),
                "`{}` must survive the round trip",
                state.as_str()
            );
        }
        assert_eq!(TicketState::parse("ghost"), None);
        assert_eq!(
            serde_json::to_value(TicketState::InReview).expect("the state encodes"),
            json!("in_review")
        );
    }

    #[test]
    fn a_record_round_trips_with_its_kind_specific_fields() {
        let encoded = serde_json::to_value(record()).expect("the record serialises");

        assert_eq!(
            encoded,
            json!({
                "id": 6,
                "project_id": 2,
                "number": 17,
                "kind": "implementation",
                "priority": "high",
                "state": "draft",
                "spec_id": 4,
                "title": null,
                "slice": "Registration creates Projects end to end",
                "criteria": [
                    {
                        "outcome": "Projects register with unique codes.",
                        "stories": ["CORE-S1-US1", "S1-US2"],
                    }
                ],
                "version": 1,
            })
        );
        let decoded: TicketRecord =
            serde_json::from_value(encoded).expect("the record deserialises");
        assert_eq!(decoded, record());

        let bug = TicketRecord {
            kind: TicketKind::Bug,
            priority: TicketPriority::Urgent,
            spec_id: None,
            title: Some("Landing drops the integration branch".to_owned()),
            slice: None,
            criteria: Vec::new(),
            ..record()
        };
        let encoded = serde_json::to_value(&bug).expect("the record serialises");
        assert_eq!(
            encoded["title"],
            json!("Landing drops the integration branch")
        );
        assert_eq!(encoded["spec_id"], json!(null));
        assert_eq!(encoded["criteria"], json!([]));
    }

    #[test]
    fn every_request_round_trips_and_rejects_unknown_fields() {
        round_trips::<TicketCreateRequest>(json!({
            "mutation": context(),
            "project_id": 2,
            "kind": "implementation",
            "priority": "high",
            "spec_id": 4,
            "slice": "Registration creates Projects end to end",
            "criteria": [criterion()],
        }));
        // A Bug sends no slice and no criteria: absence carries no
        // fields at all.
        round_trips::<TicketCreateRequest>(json!({
            "mutation": context(),
            "project_id": 2,
            "kind": "bug",
            "priority": "normal",
            "title": "Landing drops the integration branch",
        }));

        let list: TicketListQuery =
            serde_json::from_value(json!({ "project_id": 2 })).expect("the list query decodes");
        assert_eq!(list, TicketListQuery { project_id: 2 });

        let get: TicketGetQuery =
            serde_json::from_value(json!({ "ticket_id": 6 })).expect("the get query decodes");
        assert_eq!(get, TicketGetQuery { ticket_id: 6 });

        let response = TicketListResponse {
            tickets: vec![record()],
        };
        let encoded = serde_json::to_value(&response).expect("the response serialises");
        assert_eq!(encoded["tickets"].as_array().map(Vec::len), Some(1));
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
    fn every_ticket_schema_rejects_unknown_fields() {
        for name in [
            "TicketCreateRequest",
            "TicketCriterion",
            "TicketGetQuery",
            "TicketKind",
            "TicketListQuery",
            "TicketListResponse",
            "TicketPriority",
            "TicketRecord",
            "TicketState",
        ] {
            let schema = schema_of(name);
            let encoded = serde_json::to_string(&schema).expect("the schema serialises");
            assert!(
                encoded.contains("\"additionalProperties\":false") || encoded.contains("\"enum\":"),
                "{name} should reject unknown fields or close its vocabulary"
            );
        }
    }
}
