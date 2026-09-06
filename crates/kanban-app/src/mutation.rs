//! The mutation guard's inputs: payload validation, the required
//! mutation context, and the durable record of spent idempotency
//! keys (DR-SS-03).

#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "test-support"))]
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

/// The prefix opening every operation-aware fingerprint. A recorded
/// fingerprint without it predates operation awareness and carries
/// only the aggregate and the body.
const FINGERPRINT_SCHEME_PREFIX: &str = "v2:";

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
    /// The serialised payload without its mutation context: the body
    /// every fingerprint projection shares.
    body: String,
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
        let body = serde_json::to_string(&Value::Object(body)).expect("a JSON object serialises");
        Ok(Self {
            aggregate: aggregate.to_owned(),
            payload: payload.clone(),
            optimistic_version: context.optimistic_version,
            idempotency_key: context.idempotency_key,
            body,
        })
    }

    /// The request's identity for idempotency under `operation`: the
    /// dispatched operation, the aggregate, and the payload without
    /// its mutation context, so a corrected retry with the same key
    /// is a replay, never a conflict — while an operation pair that
    /// shares an aggregate and a body shape, like an edge add and
    /// its remove, stays two different requests.
    pub fn fingerprint(&self, operation: &str) -> String {
        format!(
            "{FINGERPRINT_SCHEME_PREFIX}{operation}:{}:{}",
            self.aggregate, self.body
        )
    }

    /// The operation-blind projection outcomes recorded before the
    /// fingerprint named its operation. A spent key whose outcome
    /// predates the scheme still replays through it (see
    /// [`RecordedOutcome::replays`]).
    pub fn legacy_fingerprint(&self) -> String {
        format!("{}:{}", self.aggregate, self.body)
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

impl RecordedOutcome {
    /// Whether this outcome is the recorded answer to the request
    /// whose operation-aware fingerprint is `fingerprint`. An outcome
    /// recorded before operation awareness names no operation, so it
    /// cannot prove which operation spent the key: it replays any
    /// request whose legacy projection `legacy_projection` matches
    /// the aggregate and body those rows recorded, and retention
    /// bounds how long such an outcome can answer at all.
    pub fn replays(&self, fingerprint: &str, legacy_projection: &str) -> bool {
        self.fingerprint == fingerprint
            || (!self.fingerprint.starts_with(FINGERPRINT_SCHEME_PREFIX)
                && self.fingerprint == legacy_projection)
    }
}

/// The record of spent idempotency keys and the durable span each
/// mutation shares with its outcome. The service wires the durable
/// store; the in-memory store serves tests.
pub trait IdempotencyStore: Send + Sync {
    /// The outcome recorded for `key`, if it was spent.
    fn recorded(&self, key: &str) -> Result<Option<RecordedOutcome>, ApiError>;
    /// Open the span the mutation and its outcome share. Everything
    /// the mutation writes belongs to the span, so dropping it
    /// without committing discards the mutation too.
    fn begin(&self) -> Result<Box<dyn MutationSpan + '_>, ApiError>;
}

/// The durable span one mutation shares with the outcome that
/// replays it.
pub trait MutationSpan {
    /// Record `outcome` against `key` and commit the span. The
    /// mutation's writes and the outcome a retry replays land in one
    /// commit, so no crash boundary can leave either without the
    /// other.
    fn commit(self: Box<Self>, key: &str, outcome: RecordedOutcome) -> Result<(), ApiError>;
}

/// An in-memory [`IdempotencyStore`] for tests. It records nothing
/// durably, so it replays only within one process: the production
/// core wires the SQLite store instead. Reaching it from another
/// crate needs the `test-support` feature, which only a
/// dev-dependency may enable.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Default)]
pub struct MemoryIdempotencyStore {
    outcomes: Mutex<HashMap<String, RecordedOutcome>>,
}

#[cfg(any(test, feature = "test-support"))]
impl MemoryIdempotencyStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl IdempotencyStore for MemoryIdempotencyStore {
    fn recorded(&self, key: &str) -> Result<Option<RecordedOutcome>, ApiError> {
        Ok(self
            .outcomes
            .lock()
            .expect("the idempotency index is sound")
            .get(key)
            .cloned())
    }

    fn begin(&self) -> Result<Box<dyn MutationSpan + '_>, ApiError> {
        Ok(Box::new(MemoryMutationSpan { store: self }))
    }
}

/// The in-memory store's span. Memory has no crash boundary to
/// straddle, so committing is only the record itself.
#[cfg(any(test, feature = "test-support"))]
struct MemoryMutationSpan<'a> {
    store: &'a MemoryIdempotencyStore,
}

#[cfg(any(test, feature = "test-support"))]
impl MutationSpan for MemoryMutationSpan<'_> {
    fn commit(self: Box<Self>, key: &str, outcome: RecordedOutcome) -> Result<(), ApiError> {
        self.store
            .outcomes
            .lock()
            .expect("the idempotency index is sound")
            .insert(key.to_owned(), outcome);
        Ok(())
    }
}

/// One effect a command defers until its mutation commits.
pub type PostCommitEffect = Box<dyn FnOnce() + Send>;

/// One write a refused command still owes after its mutation is
/// discarded: the durable record of the refusal (DR-LW-07).
pub type PostDiscardWrite = Box<dyn FnOnce() + Send>;

/// What `apply` reports alongside its response: the events the
/// applied change announces, plus effects that must not run while the
/// mutation's durable span is still open. A span that never commits
/// discards the effects with it, so a failed commit leaves nothing
/// they would have started. A discarded write runs the other way
/// round: it exists precisely because the span rolled back, so it
/// must not run until the discard has finished.
pub trait CommandEffects: EventSink {
    /// Run `effect` once the command's mutation has committed.
    fn after_commit(&self, effect: PostCommitEffect);

    /// Run `write` once the command's failed mutation has been
    /// discarded. The span is closed by then, so the write opens its
    /// own and lands alone: nothing the rejected mutation wrote can
    /// commit with it, and nothing of it survives to be announced.
    fn after_discard(&self, write: PostDiscardWrite);
}

/// [`CommandEffects`] that discards everything, for exercising
/// handlers directly without the dispatcher.
#[derive(Debug, Default)]
pub struct NoopCommandEffects;

impl EventSink for NoopCommandEffects {
    fn emit(&self, _event_type: &str, _payload: Value) {}
}

impl CommandEffects for NoopCommandEffects {
    fn after_commit(&self, _effect: PostCommitEffect) {}

    fn after_discard(&self, _write: PostDiscardWrite) {}
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
    /// Everything that must outlive the write span — events and
    /// post-commit effects alike — is reported through `effects`.
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError>;
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
            first.fingerprint("counter.bump"),
            second.fingerprint("counter.bump"),
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
            first.fingerprint("counter.bump"),
            second.fingerprint("counter.bump"),
            "a different body is a different request"
        );
    }

    #[test]
    fn the_fingerprint_names_the_operation() {
        let command = ParsedCommand::lift(
            "counter",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                "step": 1,
            }),
        )
        .expect("the command lifts");

        let bump = command.fingerprint("counter.bump");
        let reset = command.fingerprint("counter.reset");

        assert_ne!(
            bump, reset,
            "the same body under two operations is two different requests"
        );
        assert!(
            bump.starts_with("v2:"),
            "an operation-aware fingerprint opens with its scheme: {bump}"
        );
    }

    #[test]
    fn the_legacy_projection_keeps_the_recorded_shape() {
        let command = ParsedCommand::lift(
            "counter",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                "step": 1,
            }),
        )
        .expect("the command lifts");

        assert_eq!(
            command.legacy_fingerprint(),
            "counter:{\"step\":1}",
            "the projection stays byte-identical to the rows recorded before the scheme"
        );
    }

    #[test]
    fn a_recorded_outcome_replays_its_own_fingerprint_and_legacy_twins_only() {
        let aware = RecordedOutcome {
            fingerprint: "v2:counter.bump:counter:{\"step\":1}".to_owned(),
            response: json!({ "value": 1 }),
        };
        assert!(
            aware.replays(
                "v2:counter.bump:counter:{\"step\":1}",
                "counter:{\"step\":1}"
            ),
            "the outcome replays the request that spent the key"
        );
        assert!(
            !aware.replays(
                "v2:counter.reset:counter:{\"step\":1}",
                "counter:{\"step\":1}"
            ),
            "an operation-aware outcome never answers through the legacy projection"
        );

        let legacy = RecordedOutcome {
            fingerprint: "counter:{\"step\":1}".to_owned(),
            response: json!({ "value": 1 }),
        };
        assert!(
            legacy.replays(
                "v2:counter.bump:counter:{\"step\":1}",
                "counter:{\"step\":1}"
            ),
            "a pre-scheme outcome replays the request shape it recorded"
        );
        assert!(
            !legacy.replays(
                "v2:counter.bump:counter:{\"step\":2}",
                "counter:{\"step\":2}"
            ),
            "a pre-scheme outcome still refuses a different body"
        );
    }

    #[test]
    fn the_memory_store_round_trips_outcomes() {
        let store = MemoryIdempotencyStore::new();

        assert!(
            store
                .recorded("key-1")
                .expect("the lookup serves")
                .is_none(),
            "an unspent key has no outcome"
        );

        store
            .begin()
            .expect("the span opens")
            .commit(
                "key-1",
                RecordedOutcome {
                    fingerprint: "counter:{\"step\":1}".to_owned(),
                    response: json!({ "value": 1 }),
                },
            )
            .expect("the span commits");

        let recorded = store
            .recorded("key-1")
            .expect("the lookup serves")
            .expect("the spent key is recorded");
        assert_eq!(recorded.fingerprint, "counter:{\"step\":1}");
        assert_eq!(recorded.response, json!({ "value": 1 }));
    }

    #[test]
    fn a_span_dropped_without_committing_spends_no_key() {
        let store = MemoryIdempotencyStore::new();

        drop(store.begin().expect("the span opens"));

        assert!(
            store
                .recorded("key-1")
                .expect("the lookup serves")
                .is_none(),
            "an uncommitted span records nothing"
        );
    }
}
