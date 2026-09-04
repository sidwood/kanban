//! Authoritative command, query, event, and error payload definitions
//! with schema derivation. Depends on nothing internal.

pub mod error;
pub mod event;
pub mod health;
pub mod mutation;
pub mod schema;

pub use error::{ApiError, ErrorCode};
pub use event::EventEnvelope;
pub use health::{HealthQuery, HealthResponse};
pub use mutation::MutationContext;
pub use schema::schema_definitions;

#[cfg(test)]
mod tests {
    use schemars::schema_for;

    use super::health::{HealthQuery, HealthResponse};
    use super::schema_definitions;

    #[test]
    fn health_query_derives_json_schema() {
        let schema = schema_for!(HealthQuery);
        let json = serde_json::to_value(schema).expect("schema serialises");

        assert_eq!(
            json.get("title").and_then(|title| title.as_str()),
            Some("HealthQuery")
        );
        let encoded = serde_json::to_string(&json).expect("schema encodes");
        assert!(
            encoded.contains("\"additionalProperties\":false"),
            "HealthQuery should reject unknown fields"
        );
    }

    #[test]
    fn health_response_schema_includes_service_version() {
        let schema = schema_for!(HealthResponse);
        let json = serde_json::to_value(schema).expect("schema serialises");
        let properties = json
            .get("properties")
            .and_then(|value| value.as_object())
            .expect("HealthResponse exposes properties");

        assert!(properties.contains_key("service_version"));
        assert!(properties.contains_key("connected"));
    }

    #[test]
    fn schema_registry_lists_every_exported_dto() {
        let names: Vec<_> = schema_definitions()
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        assert_eq!(
            names,
            vec![
                "ApiError",
                "ErrorCode",
                "EventEnvelope",
                "HealthQuery",
                "HealthResponse",
                "MutationContext",
            ]
        );
    }
}
