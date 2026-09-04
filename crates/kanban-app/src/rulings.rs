//! Ruling commands and the list query (KAN-S2-US3, DR-AE-03).
//! Rulings are immutable once recorded; supersession appends a new
//! record referencing the original.

use std::sync::Arc;

use kanban_domain::{Ruling, RulingDraft, RulingEntityRef, RulingId, RulingSummary};
use kanban_dto::{
    ApiError, RulingListQuery, RulingListResponse, RulingRecord, RulingRecordRequest,
    RulingSupersedeRequest, TimelineEntityRef,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::events::EventSink;
use crate::initiative::TimelineAppend;
use crate::mutation::{CommandHandler, ParsedCommand, parse_payload};

/// The storage port ruling commands call through.
pub trait RulingStore: Send + Sync {
    /// Insert one immutable ruling and its timeline append.
    fn insert(&self, draft: &RulingDraft, append: TimelineAppend) -> Result<Ruling, ApiError>;
    /// Load one ruling, if it exists in the project.
    fn find(&self, project_id: &str, id: RulingId) -> Result<Option<Ruling>, ApiError>;
    /// List every ruling for a project, superseded originals included.
    fn list(&self, query: &RulingListQuery) -> Result<Vec<Ruling>, ApiError>;
}

impl Core {
    /// Register the ruling operations against `store`.
    pub fn register_rulings(
        &mut self,
        store: Arc<dyn RulingStore>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "ruling.record",
            Arc::new(RecordRuling {
                store: store.clone(),
            }),
        )?;
        self.register_command(
            "ruling.supersede",
            Arc::new(SupersedeRuling {
                store: store.clone(),
            }),
        )?;
        self.register_query("ruling.list", Arc::new(ListRulings { store }))?;
        Ok(())
    }
}

struct RecordRuling {
    store: Arc<dyn RulingStore>,
}

impl CommandHandler for RecordRuling {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<RulingRecordRequest>(payload)?;
        ParsedCommand::lift("ruling", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: RulingRecordRequest = parse_payload(&command.payload)?;
        let summary = RulingSummary::new(&request.summary)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let entity = request.entity.as_ref().map(dto_entity);
        let draft = Ruling::record(&request.project_id, summary, entity)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let ruling = self.store.insert(
            &draft,
            TimelineAppend {
                kind: "ruling",
                facts: json!({ "summary": ruling_summary(&draft) }),
            },
        )?;
        announce(events, "ruling.recorded", &ruling);
        serde_json::to_value(encode_record(&ruling))
            .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

struct SupersedeRuling {
    store: Arc<dyn RulingStore>,
}

impl CommandHandler for SupersedeRuling {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<RulingSupersedeRequest>(payload)?;
        ParsedCommand::lift("ruling", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: RulingSupersedeRequest = parse_payload(&command.payload)?;
        let original = load(self.store.as_ref(), &request.project_id, request.ruling_id)?;
        let summary = RulingSummary::new(&request.summary)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let draft = original.supersede(summary);
        let ruling = self.store.insert(
            &draft,
            TimelineAppend {
                kind: "ruling",
                facts: json!({
                    "summary": ruling_summary(&draft),
                    "supersedes_id": original.id().value(),
                }),
            },
        )?;
        announce(events, "ruling.superseded", &ruling);
        serde_json::to_value(encode_record(&ruling))
            .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

struct ListRulings {
    store: Arc<dyn RulingStore>,
}

impl QueryHandler for ListRulings {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query = parse_payload::<RulingListQuery>(payload)?;
        let rulings = self.store.list(&query)?;
        let response = RulingListResponse {
            rulings: rulings.iter().map(encode_record).collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

fn load(store: &dyn RulingStore, project_id: &str, ruling_id: u64) -> Result<Ruling, ApiError> {
    store
        .find(project_id, RulingId::new(ruling_id))?
        .ok_or_else(|| ApiError::not_found(&format!("ruling {ruling_id}")))
}

fn dto_entity(entity: &TimelineEntityRef) -> RulingEntityRef {
    RulingEntityRef {
        kind: entity.kind.as_str().to_owned(),
        id: entity.id.clone(),
    }
}

fn ruling_summary(draft: &RulingDraft) -> &str {
    draft.summary.as_str()
}

fn encode_record(ruling: &Ruling) -> RulingRecord {
    RulingRecord {
        id: ruling.id().value(),
        project_id: ruling.project_id().to_owned(),
        entity: ruling.entity().map(|entity| TimelineEntityRef {
            // Stored entity kinds passed the vocabulary check on the
            // way in; anything else is corruption.
            kind: kanban_dto::TimelineEntityKind::parse(&entity.kind)
                .expect("a stored Ruling entity names a known entity kind"),
            id: entity.id.clone(),
        }),
        summary: ruling.summary().as_str().to_owned(),
        supersedes_id: ruling.supersedes().map(|id| id.value()),
        recorded_at: ruling.recorded_at().to_owned(),
    }
}

fn announce(events: &dyn EventSink, kind: &str, ruling: &Ruling) {
    events.emit(kind, json!({ "id": ruling.id().value() }));
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use kanban_dto::{ApiError, ErrorCode, RulingListQuery};
    use serde_json::json;

    use super::{ListRulings, RecordRuling, RulingStore, SupersedeRuling, TimelineAppend};
    use crate::dispatch::QueryHandler;
    use crate::events::NoopEventSink;
    use crate::mutation::{CommandHandler, ParsedCommand};
    use kanban_domain::{Ruling, RulingDraft, RulingId};

    #[derive(Default)]
    struct MemoryRulingStore {
        rulings: Mutex<Vec<Ruling>>,
        recorded_at: Mutex<u64>,
    }

    impl MemoryRulingStore {
        fn insert_draft(&self, draft: RulingDraft) -> Ruling {
            let mut rulings = self.rulings.lock().expect("lock is sound");
            let mut stamp = self.recorded_at.lock().expect("lock is sound");
            let id = rulings.len() as u64 + 1;
            *stamp += 1;
            let recorded_at = format!("2026-09-04T12:00:{id:02}Z");
            rulings.push(Ruling::restore(
                RulingId::new(id),
                draft.project_id,
                draft.entity,
                draft.summary,
                draft.supersedes,
                recorded_at,
            ));
            rulings.last().expect("just pushed").clone()
        }
    }

    impl RulingStore for MemoryRulingStore {
        fn insert(&self, draft: &RulingDraft, _append: TimelineAppend) -> Result<Ruling, ApiError> {
            Ok(self.insert_draft(draft.clone()))
        }

        fn find(&self, project_id: &str, id: RulingId) -> Result<Option<Ruling>, ApiError> {
            let rulings = self.rulings.lock().expect("lock is sound");
            Ok(rulings
                .iter()
                .find(|ruling| ruling.project_id() == project_id && ruling.id() == id)
                .cloned())
        }

        fn list(&self, query: &RulingListQuery) -> Result<Vec<Ruling>, ApiError> {
            let rulings = self.rulings.lock().expect("lock is sound");
            Ok(rulings
                .iter()
                .filter(|ruling| ruling.project_id() == query.project_id)
                .filter(|ruling| {
                    query.entity.as_ref().is_none_or(|entity| {
                        ruling.entity().is_some_and(|value| {
                            value.kind
                                == serde_json::to_string(&entity.kind)
                                    .expect("entity kind encodes")
                                    .trim_matches('"')
                                && value.id == entity.id
                        })
                    })
                })
                .cloned()
                .collect())
        }
    }

    fn mutation() -> serde_json::Value {
        json!({ "optimistic_version": 0, "idempotency_key": "key-1" })
    }

    #[test]
    fn recording_appends_an_immutable_ruling() {
        let store = Arc::new(MemoryRulingStore::default());
        let handler = RecordRuling {
            store: store.clone(),
        };
        let response = handler
            .apply(
                &ParsedCommand {
                    aggregate: "ruling".to_owned(),
                    payload: json!({
                        "mutation": mutation(),
                        "project_id": "kan",
                        "entity": { "kind": "ticket", "id": "kan-t12" },
                        "summary": "Allow landing",
                    }),
                    optimistic_version: 0,
                    idempotency_key: "key-1".to_owned(),
                    fingerprint: "ruling:{}".to_owned(),
                },
                &NoopEventSink,
            )
            .expect("recording succeeds");

        assert_eq!(response["summary"], json!("Allow landing"));
        assert_eq!(response["supersedes_id"], json!(null));
        let listed = ListRulings { store }
            .handle(&json!({ "project_id": "kan" }))
            .expect("list serves");
        assert_eq!(listed["rulings"].as_array().expect("array").len(), 1);
    }

    #[test]
    fn no_edit_command_exists_in_the_catalog() {
        let names: Vec<_> = crate::catalog::exposed_operations()
            .iter()
            .map(|operation| operation.name)
            .collect();
        assert!(names.contains(&"ruling.record"));
        assert!(names.contains(&"ruling.supersede"));
        assert!(names.contains(&"ruling.list"));
        assert!(
            !names.iter().any(|name| name.contains("ruling.edit")
                || name.contains("ruling.update")
                || name.contains("ruling.rename")),
            "no edit path exists: {names:?}"
        );
    }

    #[test]
    fn superseding_creates_a_new_record_referencing_the_original() {
        let store = Arc::new(MemoryRulingStore::default());
        let record = RecordRuling {
            store: store.clone(),
        };
        let supersede = SupersedeRuling {
            store: store.clone(),
        };
        let original = record
            .apply(
                &ParsedCommand {
                    aggregate: "ruling".to_owned(),
                    payload: json!({
                        "mutation": mutation(),
                        "project_id": "kan",
                        "summary": "Hold",
                    }),
                    optimistic_version: 0,
                    idempotency_key: "key-1".to_owned(),
                    fingerprint: "ruling:{}".to_owned(),
                },
                &NoopEventSink,
            )
            .expect("original lands");
        let replacement = supersede
            .apply(
                &ParsedCommand {
                    aggregate: "ruling".to_owned(),
                    payload: json!({
                        "mutation": mutation(),
                        "project_id": "kan",
                        "ruling_id": original["id"],
                        "summary": "Proceed",
                    }),
                    optimistic_version: 0,
                    idempotency_key: "key-2".to_owned(),
                    fingerprint: "ruling:{}".to_owned(),
                },
                &NoopEventSink,
            )
            .expect("supersession lands");

        assert_eq!(replacement["supersedes_id"], original["id"]);
        let listed = ListRulings { store }
            .handle(&json!({ "project_id": "kan" }))
            .expect("list serves");
        let rulings = listed["rulings"].as_array().expect("array");
        assert_eq!(rulings.len(), 2);
        assert_eq!(rulings[0]["summary"], json!("Hold"));
        assert_eq!(rulings[1]["summary"], json!("Proceed"));
    }

    #[test]
    fn superseding_an_unknown_ruling_is_not_found() {
        let store = Arc::new(MemoryRulingStore::default());
        let handler = SupersedeRuling { store };
        let error = handler
            .apply(
                &ParsedCommand {
                    aggregate: "ruling".to_owned(),
                    payload: json!({
                        "mutation": mutation(),
                        "project_id": "kan",
                        "ruling_id": 9,
                        "summary": "Proceed",
                    }),
                    optimistic_version: 0,
                    idempotency_key: "key-1".to_owned(),
                    fingerprint: "ruling:{}".to_owned(),
                },
                &NoopEventSink,
            )
            .expect_err("unknown rulings are refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn record_rejects_unknown_fields() {
        let store = Arc::new(MemoryRulingStore::default());
        let handler = RecordRuling { store };
        let error = handler
            .parse(&json!({
                "mutation": mutation(),
                "project_id": "kan",
                "summary": "Hold",
                "surprise": true,
            }))
            .expect_err("unknown fields are rejected");
        assert_eq!(error.code, ErrorCode::UnknownField);
    }
}
