//! Routing named commands and queries to their handlers with the
//! mutation guard in front of every command (DR-SS-02, DR-SS-03).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use kanban_dto::{ApiError, HealthResponse};
use serde_json::Value;

use crate::catalog::{OperationDescriptor, OperationKind};
use crate::events::EventSink;
use crate::mutation::{CommandHandler, IdempotencyStore, RecordedOutcome};

/// A catalogued query's handler.
pub trait QueryHandler: Send + Sync {
    /// Serve the query payload.
    fn handle(&self, payload: &Value) -> Result<Value, ApiError>;
}

/// Why a handler could not be registered.
#[derive(Debug, PartialEq, Eq)]
pub enum RegistrationError {
    /// The name is not in the core's catalog.
    Uncatalogued(String),
    /// The name is catalogued for the other operation kind.
    WrongKind(String),
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uncatalogued(name) => {
                write!(f, "operation `{name}` is not in the catalog")
            }
            Self::WrongKind(name) => {
                write!(f, "operation `{name}` is catalogued for the other kind")
            }
        }
    }
}

impl std::error::Error for RegistrationError {}

/// The application core: serves the catalog's named operations, with
/// every command passing the mutation guard (optimistic versions,
/// idempotency keys, unknown-field rejection).
pub struct Core {
    catalog: &'static [OperationDescriptor],
    queries: HashMap<&'static str, Arc<dyn QueryHandler>>,
    commands: HashMap<&'static str, Arc<dyn CommandHandler>>,
    idempotency: Arc<dyn IdempotencyStore>,
    events: Arc<dyn EventSink>,
    /// Serialises the guard's check-and-record span so one idempotency
    /// key can never apply twice, even across transport threads.
    command_gate: Mutex<()>,
}

impl Core {
    /// An empty core serving operations from `catalog`.
    pub fn new(
        catalog: &'static [OperationDescriptor],
        idempotency: Arc<dyn IdempotencyStore>,
        events: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            catalog,
            queries: HashMap::new(),
            commands: HashMap::new(),
            idempotency,
            events,
            command_gate: Mutex::new(()),
        }
    }

    /// A core serving the exposed catalog; health is always wired.
    pub fn with_health(
        service_version: &str,
        idempotency: Arc<dyn IdempotencyStore>,
        events: Arc<dyn EventSink>,
    ) -> Result<Self, RegistrationError> {
        let mut core = Self::new(crate::catalog::exposed_operations(), idempotency, events);
        core.register_query(
            "health.get",
            Arc::new(HealthQueryHandler {
                service_version: service_version.to_owned(),
            }),
        )?;
        Ok(core)
    }

    /// Register a query handler. The name must be a catalogued query.
    pub fn register_query(
        &mut self,
        name: &'static str,
        handler: Arc<dyn QueryHandler>,
    ) -> Result<(), RegistrationError> {
        self.assert_catalogued(name, OperationKind::Query)?;
        self.queries.insert(name, handler);
        Ok(())
    }

    /// Register a command handler. The name must be a catalogued
    /// command.
    pub fn register_command(
        &mut self,
        name: &'static str,
        handler: Arc<dyn CommandHandler>,
    ) -> Result<(), RegistrationError> {
        self.assert_catalogued(name, OperationKind::Command)?;
        self.commands.insert(name, handler);
        Ok(())
    }

    /// The operations this core serves, for drift checks against the
    /// catalog.
    pub fn registered_operations(&self) -> Vec<(&'static str, OperationKind)> {
        let queries = self
            .queries
            .keys()
            .map(|name| (*name, OperationKind::Query));
        let commands = self
            .commands
            .keys()
            .map(|name| (*name, OperationKind::Command));
        queries.chain(commands).collect()
    }

    /// Serve a named query.
    pub fn query(&self, name: &str, payload: &Value) -> Result<Value, ApiError> {
        let handler = self
            .queries
            .get(name)
            .ok_or_else(|| ApiError::not_found(&format!("operation `{name}`")))?;
        handler.handle(payload)
    }

    /// Serve a named command through the mutation guard: idempotent
    /// replay first, then the optimistic version check, then one
    /// apply, then the outcome is recorded (DR-SS-03).
    pub fn command(&self, name: &str, payload: &Value) -> Result<Value, ApiError> {
        let handler = self
            .commands
            .get(name)
            .ok_or_else(|| ApiError::not_found(&format!("operation `{name}`")))?;
        let command = handler.parse(payload)?;

        // The gate spans check-and-record so one idempotency key can
        // never apply twice. A poisoned gate means a handler panicked
        // mid-command; the state it left is the aggregate's problem,
        // not a reason to stop serving every other command.
        let _gate = self
            .command_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(recorded) = self.idempotency.recorded(&command.idempotency_key) {
            if recorded.fingerprint == command.fingerprint {
                return Ok(recorded.response);
            }
            return Err(ApiError::duplicate_idempotency_key(
                &command.idempotency_key,
            ));
        }

        let current = handler.current_version(&command)?;
        if command.optimistic_version != current {
            return Err(ApiError::stale_version(command.optimistic_version, current));
        }

        let response = handler.apply(&command, self.events.as_ref())?;
        self.idempotency.record(
            &command.idempotency_key,
            RecordedOutcome {
                fingerprint: command.fingerprint,
                response: response.clone(),
            },
        );
        Ok(response)
    }

    /// Refuse names the catalog does not carry, or carries for the
    /// other kind.
    fn assert_catalogued(&self, name: &str, kind: OperationKind) -> Result<(), RegistrationError> {
        match self.catalog.iter().find(|operation| operation.name == name) {
            None => Err(RegistrationError::Uncatalogued(name.to_owned())),
            Some(operation) if operation.kind != kind => {
                Err(RegistrationError::WrongKind(name.to_owned()))
            }
            Some(_) => Ok(()),
        }
    }
}

/// Serves `health.get`: the core answering is the core connected.
struct HealthQueryHandler {
    service_version: String,
}

impl QueryHandler for HealthQueryHandler {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        crate::mutation::parse_payload::<kanban_dto::HealthQuery>(payload)?;
        let response = HealthResponse {
            connected: true,
            service_version: self.service_version.clone(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use kanban_dto::{ApiError, ErrorCode, MutationContext};
    use serde_json::{Value, json};

    use super::{Core, QueryHandler, RegistrationError};
    use crate::catalog::{OperationDescriptor, OperationKind, exposed_operations};
    use crate::events::{EventSink, NoopEventSink};
    use crate::mutation::{CommandHandler, MemoryIdempotencyStore, ParsedCommand, parse_payload};

    const TEST_CATALOG: &[OperationDescriptor] = &[OperationDescriptor {
        name: "counter.bump",
        kind: OperationKind::Command,
        request_schema: "MutationContext",
        response_schema: "HealthResponse",
        mcp_tool_name: "counter_bump",
        description: "Test fixture: bump a versioned counter.",
    }];

    #[derive(Debug, Default)]
    struct RecordingSink {
        events: Mutex<Vec<(String, Value)>>,
    }

    impl EventSink for RecordingSink {
        fn emit(&self, event_type: &str, payload: Value) {
            self.events
                .lock()
                .expect("the recorder lock is sound")
                .push((event_type.to_owned(), payload));
        }
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct BumpRequest {
        mutation: MutationContext,
        step: i64,
    }

    #[derive(Debug, Default)]
    struct CounterState {
        value: i64,
        version: u64,
        applies: u64,
    }

    /// A one-aggregate test command: bumps a versioned counter once
    /// per idempotency key.
    #[derive(Debug, Default)]
    struct Counter {
        state: Mutex<CounterState>,
    }

    impl CommandHandler for Counter {
        fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
            parse_payload::<BumpRequest>(payload)?;
            ParsedCommand::lift("counter", payload)
        }

        fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
            Ok(self
                .state
                .lock()
                .expect("the counter lock is sound")
                .version)
        }

        fn apply(
            &self,
            command: &ParsedCommand,
            events: &dyn EventSink,
        ) -> Result<Value, ApiError> {
            let request: BumpRequest = parse_payload(&command.payload)?;
            debug_assert_eq!(
                request.mutation.optimistic_version, command.optimistic_version,
                "the typed DTO and the lift agree on the mutation context"
            );
            let mut state = self.state.lock().expect("the counter lock is sound");
            state.value += request.step;
            state.version += 1;
            state.applies += 1;
            events.emit("counter.bumped", json!({ "to": state.value }));
            Ok(json!({ "value": state.value, "version": state.version }))
        }
    }

    fn counter_core() -> Core {
        counter_core_with_sink(Arc::new(NoopEventSink))
    }

    fn counter_core_with_sink(events: Arc<dyn EventSink>) -> Core {
        let mut core = Core::new(
            TEST_CATALOG,
            Arc::new(MemoryIdempotencyStore::new()),
            events,
        );
        core.register_command("counter.bump", Arc::new(Counter::default()))
            .expect("the test command registers");
        core
    }

    fn bump(step: i64, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "step": step,
        })
    }

    #[test]
    fn health_query_round_trips() {
        let core = Core::with_health(
            "0.1.0-test",
            Arc::new(MemoryIdempotencyStore::new()),
            Arc::new(NoopEventSink),
        )
        .expect("the default core wires");

        let response = core.query("health.get", &json!({})).expect("health serves");

        assert_eq!(
            response,
            json!({ "connected": true, "service_version": "0.1.0-test" })
        );
    }

    #[test]
    fn the_default_core_serves_catalogued_operations_only() {
        let core = Core::with_health(
            "0.1.0-test",
            Arc::new(MemoryIdempotencyStore::new()),
            Arc::new(NoopEventSink),
        )
        .expect("the default core wires");

        let catalogued: std::collections::HashMap<_, _> = exposed_operations()
            .iter()
            .map(|operation| (operation.name, operation.kind))
            .collect();

        // Every served operation is catalogued, and the health query
        // the default core wires is among them.
        for (name, kind) in core.registered_operations() {
            assert_eq!(
                catalogued.get(name),
                Some(&kind),
                "`{name}` is served as catalogued"
            );
        }
        assert_eq!(catalogued.get("health.get"), Some(&OperationKind::Query));
    }

    struct NoopQuery;

    impl QueryHandler for NoopQuery {
        fn handle(&self, _payload: &Value) -> Result<Value, ApiError> {
            Ok(json!({}))
        }
    }

    #[test]
    fn registering_an_uncatalogued_operation_is_refused() {
        let mut core = Core::new(
            TEST_CATALOG,
            Arc::new(MemoryIdempotencyStore::new()),
            Arc::new(NoopEventSink),
        );

        assert_eq!(
            core.register_query("ghost.get", Arc::new(NoopQuery)),
            Err(RegistrationError::Uncatalogued("ghost.get".to_owned()))
        );
    }

    #[test]
    fn registering_a_command_as_a_query_is_refused() {
        let mut core = Core::new(
            TEST_CATALOG,
            Arc::new(MemoryIdempotencyStore::new()),
            Arc::new(NoopEventSink),
        );

        assert_eq!(
            core.register_query("counter.bump", Arc::new(NoopQuery)),
            Err(RegistrationError::WrongKind("counter.bump".to_owned()))
        );
    }

    #[test]
    fn an_unknown_operation_is_not_found() {
        let core = counter_core();

        let query_error = core.query("ghost.get", &json!({})).expect_err("refused");
        let command_error = core
            .command("ghost.bump", &bump(1, "key-1", 0))
            .expect_err("refused");

        assert_eq!(query_error.code, ErrorCode::NotFound);
        assert_eq!(command_error.code, ErrorCode::NotFound);
    }

    #[test]
    fn a_query_rejects_unknown_fields() {
        let core = Core::with_health(
            "0.1.0-test",
            Arc::new(MemoryIdempotencyStore::new()),
            Arc::new(NoopEventSink),
        )
        .expect("the default core wires");

        let error = core
            .query("health.get", &json!({ "surprise": 1 }))
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }

    #[test]
    fn command_with_the_current_optimistic_version_applies() {
        let core = counter_core();

        let response = core
            .command("counter.bump", &bump(1, "key-1", 0))
            .expect("the first command applies");

        assert_eq!(response, json!({ "value": 1, "version": 1 }));

        let second = core
            .command("counter.bump", &bump(10, "key-2", 1))
            .expect("the second command applies");

        assert_eq!(second, json!({ "value": 11, "version": 2 }));
    }

    #[test]
    fn command_with_a_stale_optimistic_version_is_rejected_with_the_current_version() {
        let core = counter_core();
        core.command("counter.bump", &bump(1, "key-1", 0))
            .expect("the aggregate is created at version 0");

        let error = core
            .command("counter.bump", &bump(1, "key-2", 0))
            .expect_err("a stale version is rejected");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(1));
        assert_eq!(
            error.message,
            "optimistic version 0 is stale; the aggregate is at version 1"
        );
    }

    #[test]
    fn corrected_retry_after_a_stale_rejection_applies_with_the_same_key() {
        let core = counter_core();
        core.command("counter.bump", &bump(1, "key-1", 0))
            .expect("the aggregate is created at version 0");

        let stale = core
            .command("counter.bump", &bump(10, "key-2", 0))
            .expect_err("the stale attempt is rejected");
        assert_eq!(stale.current_version, Some(1));

        let corrected = core
            .command("counter.bump", &bump(10, "key-2", 1))
            .expect("the corrected retry applies");

        assert_eq!(corrected, json!({ "value": 11, "version": 2 }));
    }

    #[test]
    fn command_missing_an_optimistic_version_is_rejected() {
        let core = counter_core();

        let error = core
            .command(
                "counter.bump",
                &json!({
                    "mutation": { "idempotency_key": "key-1" },
                    "step": 1,
                }),
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
    fn command_without_a_mutation_context_is_rejected() {
        let core = counter_core();

        let error = core
            .command("counter.bump", &json!({ "step": 1 }))
            .expect_err("a command without a mutation context is rejected");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn command_with_an_unknown_field_is_rejected() {
        let core = counter_core();

        let error = core
            .command(
                "counter.bump",
                &json!({
                    "mutation": {
                        "optimistic_version": 0,
                        "idempotency_key": "key-1"
                    },
                    "step": 1,
                    "surprise": true,
                }),
            )
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }

    #[test]
    fn command_retry_with_the_same_idempotency_key_replays_without_reapplying() {
        let core = counter_core();
        let request = bump(1, "key-1", 0);

        let first = core
            .command("counter.bump", &request)
            .expect("the first attempt applies");
        let replay = core
            .command("counter.bump", &request)
            .expect("the retry replays the recorded outcome");

        assert_eq!(first, replay);

        let next = core
            .command("counter.bump", &bump(10, "key-2", 1))
            .expect("a fresh key applies against the live aggregate");

        assert_eq!(
            next,
            json!({ "value": 11, "version": 2 }),
            "the replay must not have applied a second time"
        );
    }

    #[test]
    fn command_reusing_an_idempotency_key_for_a_different_body_is_refused() {
        let core = counter_core();

        core.command("counter.bump", &bump(1, "key-1", 0))
            .expect("the first attempt applies");

        let error = core
            .command("counter.bump", &bump(2, "key-1", 1))
            .expect_err("a key cannot be spent on a different body");

        assert_eq!(error.code, ErrorCode::DuplicateIdempotencyKey);
        assert!(
            error.message.contains("key-1"),
            "the message should name the reused key: {}",
            error.message
        );
    }

    #[test]
    fn commands_emit_events_through_the_core_sink() {
        let sink = Arc::new(RecordingSink::default());
        let core = counter_core_with_sink(sink.clone());

        core.command("counter.bump", &bump(1, "key-1", 0))
            .expect("the command applies");

        let events = sink.events.lock().expect("the recorder lock is sound");
        assert_eq!(
            *events,
            vec![("counter.bumped".to_owned(), json!({ "to": 1 }))],
            "the applied command emits exactly its event"
        );
    }
}
