//! Board payload definitions: the global board's filter and the
//! projection it returns (DR-BP-01). The filter carries one set per
//! axis — Initiative, Project, Plan, Spec, kind, state, priority,
//! Lane, execution profile, and attention state — with an absent set
//! constraining nothing, and the response carries the filtered
//! projection whole: every card already placed in its fixed group and
//! already in the deterministic order, beside the values each
//! reference axis currently offers so a client renders the filter
//! surface without a second round trip.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ticket::{TicketKind, TicketPriority, TicketRecord, TicketState};

/// Why one Ticket demands operator attention: the closed Attention
/// Item classes from CONTEXT.md. The per-Ticket projection that
/// raises them lands with the attention inbox (KAN-S11); the
/// vocabulary is fixed here so the filter surface is complete first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionState {
    /// A registered dependency or external blocker holds the work.
    Blocker,
    /// A settled run finished without its required submission.
    MissingResult,
    /// The work waits on a human decision.
    HumanDecision,
    /// A review asks the operator in.
    ReviewRequest,
    /// A schedule failed to activate or mint its occurrence.
    FailedSchedule,
    /// An approval no longer validates.
    InvalidApproval,
    /// The session observing the work disconnected.
    DisconnectedSession,
    /// A run passed its stall deadline.
    StaleRun,
}

impl AttentionState {
    /// Every class, in vocabulary order.
    pub const ALL: &'static [Self] = &[
        Self::Blocker,
        Self::MissingResult,
        Self::HumanDecision,
        Self::ReviewRequest,
        Self::FailedSchedule,
        Self::InvalidApproval,
        Self::DisconnectedSession,
        Self::StaleRun,
    ];

    /// The stored and wire name of this class.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Blocker => "blocker",
            Self::MissingResult => "missing_result",
            Self::HumanDecision => "human_decision",
            Self::ReviewRequest => "review_request",
            Self::FailedSchedule => "failed_schedule",
            Self::InvalidApproval => "invalid_approval",
            Self::DisconnectedSession => "disconnected_session",
            Self::StaleRun => "stale_run",
        }
    }

    /// The class a stored row names, or `None` outside the vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|class| class.wire_name() == stored)
    }
}

/// The fixed Board Groups as the wire carries them (DR-LC-03): the
/// group a card's lifecycle state projects onto, never a state
/// itself. The terminal states reach no group and appear on no board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BoardGroup {
    /// Captured work that has not qualified or circulated.
    Draft,
    /// Parked, blocked, scheduled, and ready work (DR-LC-04).
    Backlog,
    /// Work executing in a Lane.
    Current,
    /// Work under review.
    Review,
    /// Approved and landing work (DR-LC-05).
    Staged,
    /// Landed work.
    Done,
}

impl BoardGroup {
    /// Every group, in the fixed board order.
    pub const ALL: &'static [Self] = &[
        Self::Draft,
        Self::Backlog,
        Self::Current,
        Self::Review,
        Self::Staged,
        Self::Done,
    ];

    /// The stored and wire name of this group.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Backlog => "backlog",
            Self::Current => "current",
            Self::Review => "review",
            Self::Staged => "staged",
            Self::Done => "done",
        }
    }

    /// The group a stored row names, or `None` outside the vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|group| group.wire_name() == stored)
    }
}

/// The global board's filter: one set per axis (DR-BP-01). A set a
/// request leaves absent constrains nothing; a set with values admits
/// a card holding any of them; and the axes compose as one
/// intersection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardFilter {
    /// The Initiatives whose Projects' work shows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initiatives: Vec<u64>,
    /// The Projects whose work shows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<u64>,
    /// The Plans whose Specs' work shows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plans: Vec<u64>,
    /// The Specs whose work shows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub specs: Vec<u64>,
    /// The kinds that show.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<TicketKind>,
    /// The lifecycle states that show.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<TicketState>,
    /// The priorities that show.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub priorities: Vec<TicketPriority>,
    /// The Lanes whose held work shows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lanes: Vec<u64>,
    /// The Execution Profiles whose assigned work shows, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<String>,
    /// The attention classes whose raised work shows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attention: Vec<AttentionState>,
}

/// Request payload for the `board.global` query. An absent filter is
/// the whole board.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardGlobalQuery {
    /// The filter the projection applies.
    #[serde(default)]
    pub filter: BoardFilter,
}

/// One card of the filtered projection: the Ticket record every other
/// surface sees, the Project code its global number renders with, the
/// minted number of the Spec it attaches to and the Lane holding it
/// when the board resolves either, and the fixed group its lifecycle
/// state projects onto — the mapping and the deterministic order are
/// the core's projection, never a client's own recompute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardGlobalCard {
    /// The Ticket as every other surface sees it.
    pub ticket: TicketRecord,
    /// The Project's code, so global numbers render `CORE-T17`.
    pub project_code: String,
    /// The minted number of the Spec the Ticket attaches to, when it
    /// attaches to one.
    pub spec_number: Option<u64>,
    /// The Lane holding the Ticket as its active occupant, when one
    /// does.
    pub lane_id: Option<u64>,
    /// The fixed group the Ticket's lifecycle state projects onto.
    pub group: BoardGroup,
}

/// One selectable value of a reference axis: its identity and the
/// label the operator knows it by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardFilterOption {
    /// The value the filter carries when selected.
    pub id: u64,
    /// The identity the operator reads.
    pub label: String,
}

/// The values each reference axis currently offers, as the query
/// reads them: every Initiative, Project, Plan, Spec, Lane, and
/// Execution Profile that exists, and the closed attention
/// vocabulary. A client renders its filter surface from this alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardFilterOptions {
    /// Every Initiative, by name.
    pub initiatives: Vec<BoardFilterOption>,
    /// Every Project, by code and name.
    pub projects: Vec<BoardFilterOption>,
    /// Every Plan, by rendered number.
    pub plans: Vec<BoardFilterOption>,
    /// Every Spec, by rendered number and name.
    pub specs: Vec<BoardFilterOption>,
    /// Every Lane, by its Project and identity; the values populate
    /// as the Lane surface lands.
    pub lanes: Vec<BoardFilterOption>,
    /// Every Execution Profile, by name.
    pub profiles: Vec<BoardFilterOption>,
    /// The closed attention vocabulary, whole.
    pub attention: Vec<AttentionState>,
}

/// Response payload for the `board.global` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardGlobalResponse {
    /// The filtered projection, in its deterministic order, terminal
    /// states absent.
    pub cards: Vec<BoardGlobalCard>,
    /// The values each reference axis currently offers.
    pub options: BoardFilterOptions,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        AttentionState, BoardFilter, BoardFilterOption, BoardFilterOptions, BoardGlobalCard,
        BoardGlobalQuery, BoardGlobalResponse, BoardGroup,
    };
    use crate::schema_definitions;
    use crate::ticket::{TicketKind, TicketPriority, TicketRecord, TicketState};

    /// One minimal Ticket record a card carries.
    fn ticket() -> TicketRecord {
        TicketRecord {
            id: 7,
            project_id: 1,
            number: 12,
            kind: TicketKind::Task,
            priority: TicketPriority::Normal,
            state: TicketState::Ready,
            spec_id: None,
            title: Some("Archive the old exports".to_owned()),
            slice: None,
            criteria: vec![],
            bug: None,
            subtype: Some(crate::ticket::TaskSubtype::Operational),
            mode: Some(crate::ticket::TaskMode::Human),
            completion: vec!["The old exports are archived.".to_owned()],
            scheduled_for: None,
            due: None,
            profile: None,
            pinned_spec_version: None,
            predecessor_id: None,
            version: 3,
        }
    }

    #[test]
    fn an_absent_filter_is_the_whole_board() {
        let empty = serde_json::to_value(BoardGlobalQuery::default()).expect("the query encodes");
        assert_eq!(empty, json!({ "filter": {} }));
        let decoded: BoardGlobalQuery =
            serde_json::from_value(json!({})).expect("an absent filter decodes");
        assert_eq!(decoded.filter, BoardFilter::default());
    }

    #[test]
    fn a_filter_round_trips_every_axis() {
        let filter = BoardFilter {
            initiatives: vec![2],
            projects: vec![1, 3],
            plans: vec![7],
            specs: vec![4],
            kinds: vec![TicketKind::Task, TicketKind::Bug],
            states: vec![TicketState::Active],
            priorities: vec![TicketPriority::Urgent],
            lanes: vec![5],
            profiles: vec!["standard".to_owned()],
            attention: vec![AttentionState::StaleRun],
        };
        let query = BoardGlobalQuery {
            filter: filter.clone(),
        };

        let encoded = serde_json::to_value(&query).expect("the query encodes");
        assert_eq!(
            encoded,
            json!({
                "filter": {
                    "initiatives": [2],
                    "projects": [1, 3],
                    "plans": [7],
                    "specs": [4],
                    "kinds": ["task", "bug"],
                    "states": ["active"],
                    "priorities": ["urgent"],
                    "lanes": [5],
                    "profiles": ["standard"],
                    "attention": ["stale_run"],
                }
            })
        );
        let decoded: BoardGlobalQuery = serde_json::from_value(encoded).expect("the query decodes");
        assert_eq!(decoded.filter, filter);
    }

    #[test]
    fn filter_payloads_reject_unknown_fields_and_values() {
        let surprise = serde_json::from_value::<BoardFilter>(json!({
            "projects": [1],
            "epics": [2],
        }))
        .expect_err("an axis outside the ten is refused");
        assert!(surprise.to_string().contains("unknown field"));

        let wandering = serde_json::from_value::<BoardFilter>(json!({
            "attention": ["needs_input"],
        }))
        .expect_err("a class outside the vocabulary is refused");
        assert!(wandering.to_string().contains("unknown variant"));

        let query = serde_json::from_value::<BoardGlobalQuery>(json!({
            "filter": {},
            "sort": "manual",
        }))
        .expect_err("the query carries its filter and nothing else");
        assert!(query.to_string().contains("unknown field"));
    }

    #[test]
    fn one_card_round_trips_its_projection() {
        let card = BoardGlobalCard {
            ticket: ticket(),
            project_code: "CORE".to_owned(),
            spec_number: Some(9),
            lane_id: Some(4),
            group: BoardGroup::Backlog,
        };
        let response = BoardGlobalResponse {
            cards: vec![card],
            options: BoardFilterOptions {
                initiatives: vec![BoardFilterOption {
                    id: 2,
                    label: "Personal tooling".to_owned(),
                }],
                projects: vec![BoardFilterOption {
                    id: 1,
                    label: "CORE — Core service".to_owned(),
                }],
                plans: vec![BoardFilterOption {
                    id: 3,
                    label: "CORE-P1".to_owned(),
                }],
                specs: vec![BoardFilterOption {
                    id: 4,
                    label: "CORE-S9 · Serve the board".to_owned(),
                }],
                lanes: vec![BoardFilterOption {
                    id: 5,
                    label: "CORE lane 5".to_owned(),
                }],
                profiles: vec![BoardFilterOption {
                    id: 1,
                    label: "standard".to_owned(),
                }],
                attention: AttentionState::ALL.to_vec(),
            },
        };

        let encoded = serde_json::to_value(&response).expect("the response encodes");
        assert_eq!(encoded["cards"][0]["group"], json!("backlog"));
        assert_eq!(encoded["cards"][0]["project_code"], json!("CORE"));
        assert_eq!(
            encoded["options"]["attention"].as_array().map(Vec::len),
            Some(8)
        );
        let decoded: BoardGlobalResponse =
            serde_json::from_value(encoded).expect("the response decodes");
        assert_eq!(decoded, response);
    }

    #[test]
    fn attention_and_group_wire_names_round_trip() {
        for class in AttentionState::ALL {
            assert_eq!(AttentionState::parse(class.wire_name()), Some(*class));
        }
        assert_eq!(AttentionState::parse("loud"), None);
        for group in BoardGroup::ALL {
            assert_eq!(BoardGroup::parse(group.wire_name()), Some(*group));
        }
        assert_eq!(BoardGroup::parse("archived"), None);
        let names: Vec<_> = BoardGroup::ALL
            .iter()
            .map(|group| group.wire_name())
            .collect();
        assert_eq!(
            names,
            ["draft", "backlog", "current", "review", "staged", "done"]
        );
    }

    #[test]
    fn board_payloads_are_in_the_schema_registry() {
        let names: Vec<_> = schema_definitions()
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        for expected in [
            "AttentionState",
            "BoardFilter",
            "BoardFilterOption",
            "BoardFilterOptions",
            "BoardGlobalCard",
            "BoardGlobalQuery",
            "BoardGlobalResponse",
            "BoardGroup",
        ] {
            assert!(names.contains(&expected), "{expected} must be registered");
        }
    }
}
