//! The mutation guard's inputs: payload validation, the required
//! mutation context, and the idempotency store (DR-SS-03).

use std::collections::HashMap;
use std::sync::Mutex;

use kanban_dto::{ApiError, MutationContext};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::events::EventSink;

/// Deserialize a payload, mapping failures to the stable error
/// codes. Types must derive `deny_unknown_fields` for unknown-field
/// rejection to fire.
pub fn parse_payload<Request: DeserializeOwned>(payload: &Value) -> Result<Request, ApiError> {
    serde_json::from_value(payload.clone()).map_err(translate_error)
}

/// Map serde failures onto the stable codes, distinguishing unknown
/// fields from every other malformation. The message format is
/// serde's and has been stable for years.
fn translate_error(error: serde_json::Error) -> ApiError {
    let message = error.to_string();
    match message.strip_prefix("unknown field `") {
        Some(rest) => match rest.split('`').next() {
            Some(field) => ApiError::unknown_field(field),
            None => ApiError::invalid_request(&message),
        },
        None => ApiError::invalid_request(&message),
    }
}

/// The guard-relevant half of a validated command payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    /// The aggregate the command mutates.
    pub aggregate: String,
    /// The complete validated payload; handlers deserialize their
    /// typed DTO from it inside `current_version` and `apply`.
    pub payload: Value,
    /// The optimistic version the client expects the aggregate to be
    /// at.
    pub optimistic_version: u64,
    /// The client's idempotency key for this logical request.
    pub idempotency_key: String,
    /// The request's identity for idempotency: the aggregate plus the
    /// payload without its mutation context, so a corrected retry
    /// with the same key is a replay, never a conflict.
    pub fingerprint: String,
}

impl ParsedCommand {
    /// Lift the mutation context out of a validated payload. Handlers
    /// call this after `parse_payload` accepted the body, so the
    /// guard sees the context whatever the typed DTO looks like.
    pub fn lift(aggregate: &str, payload: &Value) -> Result<Self, ApiError> {
        let mutation = payload
            .get("mutation")
            .ok_or_else(|| ApiError::invalid_request("a command must carry a mutation context"))?;
        let context: MutationContext = parse_payload(mutation)?;
        let mut body = payload
            .as_object()
            .cloned()
            .ok_or_else(|| ApiError::invalid_request("a command payload must be a JSON object"))?;
        body.remove("mutation");
        let body = Value::Object(body);
        Ok(Self {
            aggregate: aggregate.to_owned(),
            payload: payload.clone(),
            optimistic_version: context.optimistic_version,
            idempotency_key: context.idempotency_key,
            fingerprint: format!(
                "{aggregate}:{}",
                serde_json::to_string(&body).expect("a JSON object serialises")
            ),
        })
    }
}

/// A spent idempotency key and the outcome replayed on retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedOutcome {
    /// The fingerprint the key was spent on.
    pub fingerprint: String,
    /// The recorded success response.
    pub response: Value,
}

/// The record of spent idempotency keys. The service wires the
/// durable store; the in-memory store serves the in-process core.
pub trait IdempotencyStore: Send + Sync {
    /// The outcome recorded for `key`, if it was spent.
    fn recorded(&self, key: &str) -> Option<RecordedOutcome>;
    /// Record a successful outcome against `key`.
    fn record(&self, key: &str, outcome: RecordedOutcome);
}

/// An in-memory [`IdempotencyStore`] for the in-process core and
/// tests.
#[derive(Debug, Default)]
pub struct MemoryIdempotencyStore {
    outcomes: Mutex<HashMap<String, RecordedOutcome>>,
}

impl MemoryIdempotencyStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl IdempotencyStore for MemoryIdempotencyStore {
    fn recorded(&self, key: &str) -> Option<RecordedOutcome> {
        self.outcomes
            .lock()
            .expect("the idempotency index is sound")
            .get(key)
            .cloned()
    }

    fn record(&self, key: &str, outcome: RecordedOutcome) {
        self.outcomes
            .lock()
            .expect("the idempotency index is sound")
            .insert(key.to_owned(), outcome);
    }
}

/// A catalogued command's handler. The dispatcher runs the guard
/// around `current_version` and `apply`; `parse` supplies the guard's
/// inputs and must validate the payload through `parse_payload` so
/// unknown fields never reach the guard.
pub trait CommandHandler: Send + Sync {
    /// Validate the raw payload and lift its mutation context.
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError>;
    /// The aggregate's current version, or [`ApiError::not_found`].
    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError>;
    /// Apply the mutation. Runs at most once per idempotency key.
    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError>;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{IdempotencyStore, MemoryIdempotencyStore, RecordedOutcome, parse_payload};
    use crate::mutation::ParsedCommand;
    use kanban_dto::ErrorCode;

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Sample {
        value: u64,
    }

    #[test]
    fn parse_payload_maps_unknown_fields_to_their_code() {
        let accepted: Sample =
            parse_payload(&json!({ "value": 1 })).expect("a well-formed payload parses");
        assert_eq!(accepted.value, 1);

        let error = parse_payload::<Sample>(&json!({ "value": 1, "surprise": 2 }))
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }

    #[test]
    fn parse_payload_maps_missing_fields_to_invalid_request() {
        let error = parse_payload::<Sample>(&json!({})).expect_err("missing fields are rejected");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("value"),
            "the message should name the missing field: {}",
            error.message
        );
    }

    #[test]
    fn lift_requires_a_mutation_context() {
        let error = ParsedCommand::lift("counter", &json!({ "step": 1 }))
            .expect_err("a command without a mutation context is rejected");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("mutation"),
            "the message should name the mutation context: {}",
            error.message
        );
    }

    #[test]
    fn lift_requires_an_optimistic_version() {
        let error = ParsedCommand::lift(
            "counter",
            &json!({ "mutation": { "idempotency_key": "key-1" }, "step": 1 }),
        )
        .expect_err("a mutation context without a version is rejected");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("optimistic_version"),
            "the message should name the missing field: {}",
            error.message
        );
    }

    #[test]
    fn lift_exposes_the_context_and_the_payload() {
        let command = ParsedCommand::lift(
            "counter",
            &json!({
                "mutation": { "optimistic_version": 3, "idempotency_key": "key-1" },
                "step": 1,
            }),
        )
        .expect("a well-formed command lifts");

        assert_eq!(command.aggregate, "counter");
        assert_eq!(command.optimistic_version, 3);
        assert_eq!(command.idempotency_key, "key-1");
        assert_eq!(
            command.payload,
            json!({
                "mutation": { "optimistic_version": 3, "idempotency_key": "key-1" },
                "step": 1,
            })
        );
    }

    #[test]
    fn the_fingerprint_ignores_the_mutation_context() {
        let first = ParsedCommand::lift(
            "counter",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                "step": 1,
            }),
        )
        .expect("the first command lifts");
        let second = ParsedCommand::lift(
            "counter",
            &json!({
                "mutation": { "optimistic_version": 7, "idempotency_key": "key-9" },
                "step": 1,
            }),
        )
        .expect("the second command lifts");

        assert_eq!(
            first.fingerprint, second.fingerprint,
            "a corrected retry with the same body is the same request"
        );
    }

    #[test]
    fn the_fingerprint_tracks_the_request_body() {
        let first = ParsedCommand::lift(
            "counter",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                "step": 1,
            }),
        )
        .expect("the first command lifts");
        let second = ParsedCommand::lift(
            "counter",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                "step": 2,
            }),
        )
        .expect("the second command lifts");

        assert_ne!(
            first.fingerprint, second.fingerprint,
            "a different body is a different request"
        );
    }

    #[test]
    fn the_memory_store_round_trips_outcomes() {
        let store = MemoryIdempotencyStore::new();

        assert!(
            store.recorded("key-1").is_none(),
            "an unspent key has no outcome"
        );

        store.record(
            "key-1",
            RecordedOutcome {
                fingerprint: "counter:{\"step\":1}".to_owned(),
                response: json!({ "value": 1 }),
            },
        );

        let recorded = store.recorded("key-1").expect("the spent key is recorded");
        assert_eq!(recorded.fingerprint, "counter:{\"step\":1}");
        assert_eq!(recorded.response, json!({ "value": 1 }));
    }
}
