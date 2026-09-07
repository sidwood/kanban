//! Global search payload definitions (DR-BP-17): one read-only query
//! over Initiatives, Projects, Plans, Specs, and Tickets by
//! identifier and text. The palette and search surface share these
//! types; neither carries a mutating command.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The kind of entity one search hit names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchHitKind {
    Initiative,
    Project,
    Plan,
    Spec,
    Ticket,
}

impl SearchHitKind {
    /// Every kind, in vocabulary order.
    pub const ALL: &'static [Self] = &[
        Self::Initiative,
        Self::Project,
        Self::Plan,
        Self::Spec,
        Self::Ticket,
    ];

    /// The wire name, matching this kind's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Initiative => "initiative",
            Self::Project => "project",
            Self::Plan => "plan",
            Self::Spec => "spec",
            Self::Ticket => "ticket",
        }
    }
}

/// One row the `search.global` query returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchGlobalHit {
    /// The kind of entity this hit names.
    pub kind: SearchHitKind,
    /// The immutable, storage-assigned identity.
    pub id: u64,
    /// The identifier the operator would quote: `CORE-T12`, `CORE`,
    /// or an Initiative name.
    pub identifier: String,
    /// The line the palette leads with: a title, a name, or a plan
    /// label.
    pub label: String,
    /// The Project this hit belongs to, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<u64>,
}

/// Request payload for the `search.global` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchGlobalQuery {
    /// The operator's search text; blank returns no hits.
    pub q: String,
}

/// Response payload for the `search.global` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchGlobalResponse {
    /// Every matching entity, in the domain's deterministic order.
    pub hits: Vec<SearchGlobalHit>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{SearchGlobalHit, SearchGlobalQuery, SearchGlobalResponse, SearchHitKind};
    use crate::schema_definitions;

    #[test]
    fn a_hit_round_trips() {
        let hit = SearchGlobalHit {
            kind: SearchHitKind::Ticket,
            id: 12,
            identifier: "CORE-T12".to_owned(),
            label: "Archive the register".to_owned(),
            project_id: Some(1),
        };

        let encoded = serde_json::to_value(&hit).expect("the hit serialises");
        assert_eq!(
            encoded,
            json!({
                "kind": "ticket",
                "id": 12,
                "identifier": "CORE-T12",
                "label": "Archive the register",
                "project_id": 1,
            })
        );
        let decoded: SearchGlobalHit =
            serde_json::from_value(encoded).expect("the hit deserialises");
        assert_eq!(decoded, hit);
    }

    #[test]
    fn a_query_round_trips_and_rejects_unknown_fields() {
        let query = SearchGlobalQuery {
            q: "core-t12".to_owned(),
        };

        let encoded = serde_json::to_value(&query).expect("the query serialises");
        let decoded: SearchGlobalQuery =
            serde_json::from_value(encoded).expect("the query deserialises");
        assert_eq!(decoded, query);

        let refused: Result<SearchGlobalQuery, _> =
            serde_json::from_value(json!({ "q": "core", "mutate": true }));
        assert!(refused.is_err(), "unknown fields are refused");
    }

    #[test]
    fn a_response_round_trips() {
        let response = SearchGlobalResponse {
            hits: vec![SearchGlobalHit {
                kind: SearchHitKind::Project,
                id: 1,
                identifier: "CORE".to_owned(),
                label: "Control plane".to_owned(),
                project_id: Some(1),
            }],
        };

        let encoded = serde_json::to_value(&response).expect("the response serialises");
        let decoded: SearchGlobalResponse =
            serde_json::from_value(encoded).expect("the response deserialises");
        assert_eq!(decoded, response);
    }

    #[test]
    fn schemas_are_registered() {
        let names = schema_definitions()
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"SearchGlobalQuery"));
        assert!(names.contains(&"SearchGlobalResponse"));
        assert!(names.contains(&"SearchGlobalHit"));
        assert!(names.contains(&"SearchHitKind"));
    }
}
