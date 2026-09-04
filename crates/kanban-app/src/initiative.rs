//! Initiative commands and the query behind the management
//! surface: create, rename, archive, and list (KAN-S1-US3,
//! KAN-S1-US6). Every change appends a timeline event in the same
//! write, archived is terminal, and no delete exists.

use std::sync::Arc;

use kanban_domain::{Initiative, InitiativeId, InitiativeName};
use kanban_dto::{
    ApiError, InitiativeArchiveRequest, InitiativeCreateRequest, InitiativeListQuery,
    InitiativeListResponse, InitiativeRecord, InitiativeRenameRequest, TimelineEntityKind,
    TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::event_catalog::event_descriptor;
use crate::events::{EventSink, emit_catalogued};
use crate::mutation::{CommandHandler, ParsedCommand, parse_payload};
use crate::timeline::TimelineEnvelope;

/// The storage port Initiative commands call through. Implementations
/// insert the timeline envelope unchanged inside the same write as
/// the row.
pub trait InitiativeStore: Send + Sync {
    /// Insert a fresh Initiative. Storage assigns its identity and
    /// asks `envelope` for the timeline row that identity belongs in.
    fn create(
        &self,
        name: &InitiativeName,
        envelope: &dyn Fn(InitiativeId) -> TimelineEnvelope,
    ) -> Result<Initiative, ApiError>;
    /// Load one Initiative, if it exists.
    fn find(&self, id: InitiativeId) -> Result<Option<Initiative>, ApiError>;
    /// Persist an applied transition and its timeline envelope.
    fn save(&self, initiative: &Initiative, envelope: TimelineEnvelope) -> Result<(), ApiError>;
    /// Every Initiative in id order, archived included.
    fn list(&self) -> Result<Vec<Initiative>, ApiError>;
}

/// The timeline row for one Initiative transition. Initiatives sit
/// above every Project, so their history is global; `action` names
/// which transition it was inside the closed `transition` kind.
fn transition(id: InitiativeId, action: &str, facts: Value) -> TimelineEnvelope {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("Initiative transition facts are a JSON object");
    object.insert("action".to_owned(), Value::from(action));
    object.insert("id".to_owned(), Value::from(id.value()));
    TimelineEnvelope::global(
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Initiative,
            id: id.value().to_string(),
        }),
        detail,
    )
}

impl Core {
    /// Register the Initiative operations against `store`.
    pub fn register_initiatives(
        &mut self,
        store: Arc<dyn InitiativeStore>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "initiative.create",
            Arc::new(CreateInitiative {
                store: store.clone(),
            }),
        )?;
        self.register_command(
            "initiative.rename",
            Arc::new(RenameInitiative {
                store: store.clone(),
            }),
        )?;
        self.register_command(
            "initiative.archive",
            Arc::new(ArchiveInitiative {
                store: store.clone(),
            }),
        )?;
        self.register_query("initiative.list", Arc::new(ListInitiatives { store }))?;
        Ok(())
    }
}

/// Serves `initiative.create`.
struct CreateInitiative {
    store: Arc<dyn InitiativeStore>,
}

impl CommandHandler for CreateInitiative {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<InitiativeCreateRequest>(payload)?;
        ParsedCommand::lift("initiative", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        // A fresh aggregate is created at version 0.
        Ok(0)
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: InitiativeCreateRequest = parse_payload(&command.payload)?;
        let name = InitiativeName::new(&request.name)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let initiative = self.store.create(&name, &|id| {
            transition(id, "created", json!({ "name": name.as_str() }))
        })?;
        announce(events, event_descriptor("initiative.created"), &initiative);
        encode_record(&initiative)
    }
}

/// Serves `initiative.rename`.
struct RenameInitiative {
    store: Arc<dyn InitiativeStore>,
}

impl CommandHandler for RenameInitiative {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<InitiativeRenameRequest>(payload)?;
        ParsedCommand::lift("initiative", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: InitiativeRenameRequest = parse_payload(&command.payload)?;
        let initiative = load(&self.store, request.initiative_id)?;
        Ok(initiative.version())
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: InitiativeRenameRequest = parse_payload(&command.payload)?;
        let mut initiative = load(&self.store, request.initiative_id)?;
        let name = InitiativeName::new(&request.name)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let previous = initiative.name().to_owned();
        initiative
            .rename(name)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        self.store.save(
            &initiative,
            transition(
                initiative.id(),
                "renamed",
                json!({ "from": previous, "to": initiative.name() }),
            ),
        )?;
        announce(events, event_descriptor("initiative.renamed"), &initiative);
        encode_record(&initiative)
    }
}

/// Serves `initiative.archive`.
struct ArchiveInitiative {
    store: Arc<dyn InitiativeStore>,
}

impl CommandHandler for ArchiveInitiative {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<InitiativeArchiveRequest>(payload)?;
        ParsedCommand::lift("initiative", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: InitiativeArchiveRequest = parse_payload(&command.payload)?;
        let initiative = load(&self.store, request.initiative_id)?;
        Ok(initiative.version())
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: InitiativeArchiveRequest = parse_payload(&command.payload)?;
        let mut initiative = load(&self.store, request.initiative_id)?;
        initiative
            .archive()
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        self.store.save(
            &initiative,
            transition(initiative.id(), "archived", json!({})),
        )?;
        announce(events, event_descriptor("initiative.archived"), &initiative);
        encode_record(&initiative)
    }
}

/// Serves `initiative.list`.
struct ListInitiatives {
    store: Arc<dyn InitiativeStore>,
}

impl QueryHandler for ListInitiatives {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        parse_payload::<InitiativeListQuery>(payload)?;
        let response = InitiativeListResponse {
            initiatives: self.store.list()?.iter().map(record_of).collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// The Initiative a command addresses, or the stable not-found
/// refusal.
fn load(store: &Arc<dyn InitiativeStore>, id: u64) -> Result<Initiative, ApiError> {
    store
        .find(InitiativeId::new(id))?
        .ok_or_else(|| ApiError::not_found(&format!("initiative {id}")))
}

/// The DTO record for one Initiative.
fn record_of(initiative: &Initiative) -> InitiativeRecord {
    InitiativeRecord {
        id: initiative.id().value(),
        name: initiative.name().to_owned(),
        archived: initiative.is_archived(),
        version: initiative.version(),
    }
}

/// Encode a record for a command response.
fn encode_record(initiative: &Initiative) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(initiative))
        .map_err(|error| ApiError::internal(&error.to_string()))
}

/// Publish the change on the live event stream, matching the
/// durable timeline append.
fn announce(
    events: &dyn EventSink,
    event: &crate::event_catalog::EventDescriptor,
    initiative: &Initiative,
) {
    emit_catalogued(events, event, &record_of(initiative));
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use kanban_domain::{Initiative, InitiativeId, InitiativeName};
    use kanban_dto::{
        ApiError, ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
        TimelineScope,
    };
    use serde_json::{Value, json};

    use super::InitiativeStore;
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::EventSink;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::timeline::TimelineEnvelope;

    /// An in-memory store: rows by id, plus every timeline append
    /// it was asked to land, for assertions.
    #[derive(Default)]
    struct MemoryInitiativeStore {
        state: Mutex<MemoryState>,
    }

    #[derive(Default)]
    struct MemoryState {
        initiatives: Vec<Initiative>,
        next_id: u64,
        timeline: Vec<TimelineEnvelope>,
    }

    impl MemoryInitiativeStore {
        /// The stored rows and timeline envelopes, for assertions.
        fn snapshot(&self) -> (Vec<Initiative>, Vec<TimelineEnvelope>) {
            let state = self.state.lock().expect("the memory store lock is sound");
            (state.initiatives.clone(), state.timeline.clone())
        }
    }

    impl InitiativeStore for MemoryInitiativeStore {
        fn create(
            &self,
            name: &InitiativeName,
            envelope: &dyn Fn(InitiativeId) -> TimelineEnvelope,
        ) -> Result<Initiative, ApiError> {
            let mut state = self.state.lock().expect("the memory store lock is sound");
            state.next_id += 1;
            let id = InitiativeId::new(state.next_id);
            let initiative = Initiative::new(id, name.clone());
            state.initiatives.push(initiative.clone());
            state.timeline.push(envelope(id));
            Ok(initiative)
        }

        fn find(&self, id: InitiativeId) -> Result<Option<Initiative>, ApiError> {
            let state = self.state.lock().expect("the memory store lock is sound");
            Ok(state.initiatives.iter().find(|row| row.id() == id).cloned())
        }

        fn save(
            &self,
            initiative: &Initiative,
            envelope: TimelineEnvelope,
        ) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory store lock is sound");
            let id = initiative.id();
            if let Some(row) = state.initiatives.iter_mut().find(|row| row.id() == id) {
                *row = initiative.clone();
            }
            state.timeline.push(envelope);
            Ok(())
        }

        fn list(&self) -> Result<Vec<Initiative>, ApiError> {
            let state = self.state.lock().expect("the memory store lock is sound");
            Ok(state.initiatives.clone())
        }
    }

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

    fn initiative_core(store: Arc<MemoryInitiativeStore>, events: Arc<dyn EventSink>) -> Core {
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            events,
        );
        core.register_initiatives(store)
            .expect("the initiative operations register");
        core
    }

    fn create(name: &str, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "name": name,
        })
    }

    fn rename(id: u64, name: &str, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "initiative_id": id,
            "name": name,
        })
    }

    fn archive(id: u64, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "initiative_id": id,
        })
    }

    #[test]
    fn creating_returns_the_active_record_at_version_one() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store.clone(), Arc::new(crate::events::NoopEventSink));

        let response = core
            .command("initiative.create", &create("Reliability", "key-1", 0))
            .expect("the create applies");

        assert_eq!(
            response,
            json!({ "id": 1, "name": "Reliability", "archived": false, "version": 1 })
        );
    }

    #[test]
    fn creating_refuses_a_blank_name_without_recording_anything() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store.clone(), Arc::new(crate::events::NoopEventSink));

        let error = core
            .command("initiative.create", &create("   ", "key-1", 0))
            .expect_err("a blank name is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        let (rows, timeline) = store.snapshot();
        assert!(rows.is_empty(), "no row may be written");
        assert!(timeline.is_empty(), "no timeline event may be appended");
    }

    #[test]
    fn creating_rejects_unknown_fields() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store, Arc::new(crate::events::NoopEventSink));

        let error = core
            .command(
                "initiative.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                    "name": "Alpha",
                    "surprise": true,
                }),
            )
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }

    #[test]
    fn creating_replays_a_retry_without_reapplying() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store.clone(), Arc::new(crate::events::NoopEventSink));
        let request = create("Alpha", "key-1", 0);

        let first = core
            .command("initiative.create", &request)
            .expect("the first attempt applies");
        let replay = core
            .command("initiative.create", &request)
            .expect("the retry replays");

        assert_eq!(first, replay);
        let (rows, timeline) = store.snapshot();
        assert_eq!(rows.len(), 1, "the retry must not have applied again");
        assert_eq!(timeline.len(), 1);
    }

    #[test]
    fn renaming_changes_the_name_and_bumps_the_version() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store, Arc::new(crate::events::NoopEventSink));
        core.command("initiative.create", &create("Alpha", "key-1", 0))
            .expect("the create applies");

        let response = core
            .command("initiative.rename", &rename(1, "Beta", "key-2", 1))
            .expect("the rename applies");

        assert_eq!(
            response,
            json!({ "id": 1, "name": "Beta", "archived": false, "version": 2 })
        );
    }

    #[test]
    fn renaming_an_unknown_initiative_is_not_found() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store, Arc::new(crate::events::NoopEventSink));

        let error = core
            .command("initiative.rename", &rename(9, "Beta", "key-1", 0))
            .expect_err("the unknown initiative is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(error.message.contains("initiative 9"));
    }

    #[test]
    fn renaming_with_a_stale_version_is_rejected_with_the_current_one() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store, Arc::new(crate::events::NoopEventSink));
        core.command("initiative.create", &create("Alpha", "key-1", 0))
            .expect("the create applies");

        let error = core
            .command("initiative.rename", &rename(1, "Beta", "key-2", 0))
            .expect_err("the stale version is rejected");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(1));
    }

    #[test]
    fn renaming_after_archive_is_refused() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store.clone(), Arc::new(crate::events::NoopEventSink));
        core.command("initiative.create", &create("Alpha", "key-1", 0))
            .expect("the create applies");
        core.command("initiative.archive", &archive(1, "key-2", 1))
            .expect("the archive applies");

        let error = core
            .command("initiative.rename", &rename(1, "Beta", "key-3", 2))
            .expect_err("archived is terminal");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        let (rows, timeline) = store.snapshot();
        assert_eq!(rows[0].name(), "Alpha", "the refusal changed nothing");
        assert_eq!(timeline.len(), 2, "no third append may have landed");
    }

    #[test]
    fn archiving_is_terminal_and_the_list_keeps_every_fact() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store, Arc::new(crate::events::NoopEventSink));
        core.command("initiative.create", &create("Alpha", "key-1", 0))
            .expect("the create applies");

        let response = core
            .command("initiative.archive", &archive(1, "key-2", 1))
            .expect("the archive applies");

        assert_eq!(
            response,
            json!({ "id": 1, "name": "Alpha", "archived": true, "version": 2 })
        );
        let listed = core
            .query("initiative.list", &json!({}))
            .expect("the list serves");
        assert_eq!(
            listed,
            json!({
                "initiatives": [
                    { "id": 1, "name": "Alpha", "archived": true, "version": 2 }
                ]
            }),
            "an archived Initiative stays listed with every fact"
        );
    }

    #[test]
    fn archiving_twice_is_refused() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store, Arc::new(crate::events::NoopEventSink));
        core.command("initiative.create", &create("Alpha", "key-1", 0))
            .expect("the create applies");
        core.command("initiative.archive", &archive(1, "key-2", 1))
            .expect("the first archive applies");

        let error = core
            .command("initiative.archive", &archive(1, "key-3", 2))
            .expect_err("the second archive is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn archiving_an_unknown_initiative_is_not_found() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store, Arc::new(crate::events::NoopEventSink));

        let error = core
            .command("initiative.archive", &archive(9, "key-1", 0))
            .expect_err("the unknown initiative is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn listing_returns_every_initiative_in_id_order() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store, Arc::new(crate::events::NoopEventSink));
        core.command("initiative.create", &create("Alpha", "key-1", 0))
            .expect("the first create applies");
        core.command("initiative.create", &create("Beta", "key-2", 0))
            .expect("the second create applies");
        core.command("initiative.archive", &archive(1, "key-3", 1))
            .expect("the archive applies");

        let listed = core
            .query("initiative.list", &json!({}))
            .expect("the list serves");

        assert_eq!(
            listed,
            json!({
                "initiatives": [
                    { "id": 1, "name": "Alpha", "archived": true, "version": 2 },
                    { "id": 2, "name": "Beta", "archived": false, "version": 1 },
                ]
            })
        );
    }

    #[test]
    fn listing_rejects_unknown_fields() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store, Arc::new(crate::events::NoopEventSink));

        let error = core
            .query("initiative.list", &json!({ "include_archived": true }))
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
    }

    #[test]
    fn every_initiative_change_records_a_global_transition() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let core = initiative_core(store.clone(), Arc::new(crate::events::NoopEventSink));
        core.command("initiative.create", &create("Alpha", "key-1", 0))
            .expect("the create applies");
        core.command("initiative.rename", &rename(1, "Beta", "key-2", 1))
            .expect("the rename applies");
        core.command("initiative.archive", &archive(1, "key-3", 2))
            .expect("the archive applies");

        let (_, timeline) = store.snapshot();
        let entity = TimelineEntityRef {
            kind: TimelineEntityKind::Initiative,
            id: "1".to_owned(),
        };
        assert_eq!(
            timeline
                .iter()
                .map(|envelope| (
                    envelope.scope().clone(),
                    envelope.kind(),
                    envelope.entity().cloned(),
                    envelope.detail().clone(),
                ))
                .collect::<Vec<_>>(),
            vec![
                (
                    TimelineScope::Global,
                    TimelineEventKind::Transition,
                    Some(entity.clone()),
                    json!({ "name": "Alpha", "action": "created", "id": 1 }),
                ),
                (
                    TimelineScope::Global,
                    TimelineEventKind::Transition,
                    Some(entity.clone()),
                    json!({ "from": "Alpha", "to": "Beta", "action": "renamed", "id": 1 }),
                ),
                (
                    TimelineScope::Global,
                    TimelineEventKind::Transition,
                    Some(entity),
                    json!({ "action": "archived", "id": 1 }),
                ),
            ],
            "Initiatives sit above every Project, and the action names the transition"
        );
    }

    #[test]
    fn initiative_changes_publish_on_the_event_stream() {
        let store = Arc::new(MemoryInitiativeStore::default());
        let sink = Arc::new(RecordingSink::default());
        let core = initiative_core(store, sink.clone());
        core.command("initiative.create", &create("Alpha", "key-1", 0))
            .expect("the create applies");

        let events = sink.events.lock().expect("the recorder lock is sound");
        assert_eq!(
            *events,
            vec![(
                "initiative.created".to_owned(),
                json!({
                    "id": 1,
                    "name": "Alpha",
                    "archived": false,
                    "version": 1,
                })
            )],
            "the applied change announces itself live"
        );
    }

    #[test]
    fn no_initiative_delete_operation_is_catalogued() {
        for operation in exposed_operations() {
            assert!(
                !operation.name.contains("delete") && !operation.name.contains("remove"),
                "`{}` must not exist; Initiatives are archived, never deleted",
                operation.name
            );
        }
    }
}
