//! Evidence commands: attach managed files or repository references
//! and list items, appending timeline events for each (KAN-S2-US4).

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kanban_domain::{CommitIdentity, EvidenceItem, EvidenceKind, RelativePath, is_entity_kind};
use kanban_dto::{
    ApiError, EvidenceAttachRequest, EvidenceKindDto, EvidenceListRequest, EvidenceListResponse,
    EvidenceRecord, TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::dispatch::{Core, RegistrationError};
use crate::events::EventSink;
use crate::mutation::{CommandHandler, ParsedCommand, parse_payload};
use crate::timeline::{TimelineEnvelope, TimelineFacts};

/// Filter for listing evidence within one Project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvidenceFilter {
    pub project_id: String,
    pub entity_kind: Option<String>,
    pub entity_id: Option<String>,
}

/// The storage port evidence commands call through. Implementations
/// append the timeline entry inside the same write as the row.
pub trait EvidenceStore: Send + Sync {
    /// Store managed-file bytes and metadata.
    fn attach_managed_file(
        &self,
        project_id: &str,
        entity_kind: &str,
        entity_id: &str,
        content_base64: &str,
        facts: TimelineFacts,
    ) -> Result<EvidenceItem, ApiError>;

    /// Record repository evidence without copying content.
    fn attach_repository(
        &self,
        project_id: &str,
        entity_kind: &str,
        entity_id: &str,
        relative_path: &RelativePath,
        commit_identity: &CommitIdentity,
        facts: TimelineFacts,
    ) -> Result<EvidenceItem, ApiError>;

    /// List evidence for `filter`, appending the list timeline event.
    fn list(
        &self,
        filter: &EvidenceFilter,
        envelope: TimelineEnvelope,
    ) -> Result<Vec<EvidenceItem>, ApiError>;
}

impl Core {
    /// Register the evidence operations against `store`.
    pub fn register_evidence(
        &mut self,
        store: Arc<dyn EvidenceStore>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "evidence.attach",
            Arc::new(AttachEvidence {
                store: store.clone(),
            }),
        )?;
        self.register_command("evidence.list", Arc::new(ListEvidence { store }))?;
        Ok(())
    }
}

/// Serves `evidence.attach`.
struct AttachEvidence {
    store: Arc<dyn EvidenceStore>,
}

impl CommandHandler for AttachEvidence {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<EvidenceAttachRequest>(payload)?;
        ParsedCommand::lift("evidence", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: EvidenceAttachRequest = parse_payload(&command.payload)?;
        validate_entity_kind(&request.entity_kind)?;
        let item = match request.evidence_kind {
            EvidenceKindDto::ManagedFile => {
                let content_base64 = request.content_base64.as_deref().ok_or_else(|| {
                    ApiError::invalid_request("managed-file evidence requires content_base64")
                })?;
                let content = STANDARD
                    .decode(content_base64)
                    .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
                let hash = content_hash(&content);
                self.store.attach_managed_file(
                    &request.project_id,
                    &request.entity_kind,
                    &request.entity_id,
                    content_base64,
                    TimelineFacts {
                        kind: TimelineEventKind::Evidence,
                        facts: json!({
                            "action": "attached",
                            "evidence_kind": "managed_file",
                            "content_hash": hash,
                            "entity_kind": request.entity_kind,
                            "entity_id": request.entity_id,
                        }),
                    },
                )?
            }
            EvidenceKindDto::Repository => {
                let relative_path = request.relative_path.as_deref().ok_or_else(|| {
                    ApiError::invalid_request(
                        "repository evidence requires relative_path and commit_identity",
                    )
                })?;
                let commit_identity = request.commit_identity.as_deref().ok_or_else(|| {
                    ApiError::invalid_request(
                        "repository evidence requires relative_path and commit_identity",
                    )
                })?;
                let path = RelativePath::new(relative_path)
                    .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
                let commit = CommitIdentity::new(commit_identity)
                    .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
                self.store.attach_repository(
                    &request.project_id,
                    &request.entity_kind,
                    &request.entity_id,
                    &path,
                    &commit,
                    TimelineFacts {
                        kind: TimelineEventKind::Evidence,
                        facts: json!({
                            "action": "attached",
                            "evidence_kind": "repository",
                            "relative_path": relative_path,
                            "commit_identity": commit_identity,
                            "entity_kind": request.entity_kind,
                            "entity_id": request.entity_id,
                        }),
                    },
                )?
            }
        };
        announce(events, "evidence.attached", &item);
        encode_record(&item)
    }
}

/// Serves `evidence.list`.
struct ListEvidence {
    store: Arc<dyn EvidenceStore>,
}

impl CommandHandler for ListEvidence {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<EvidenceListRequest>(payload)?;
        ParsedCommand::lift("evidence", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: EvidenceListRequest = parse_payload(&command.payload)?;
        if let Some(entity_kind) = &request.entity_kind {
            validate_entity_kind(entity_kind)?;
        }
        let filter = EvidenceFilter {
            project_id: request.project_id.clone(),
            entity_kind: request.entity_kind.clone(),
            entity_id: request.entity_id.clone(),
        };
        let entity = request
            .entity_kind
            .as_deref()
            .zip(request.entity_id.as_deref())
            .map(|(kind, id)| TimelineEntityRef {
                kind: TimelineEntityKind::parse(kind)
                    .expect("validated timeline entity kind has a DTO variant"),
                id: id.to_owned(),
            });
        let envelope = TimelineEnvelope::project(
            &request.project_id,
            TimelineEventKind::Evidence,
            entity,
            json!({
                "action": "listed",
                "entity_kind": request.entity_kind,
                "entity_id": request.entity_id,
            }),
        )?;
        let items = self.store.list(&filter, envelope)?;
        announce_list(events, &request.project_id, items.len());
        let response = EvidenceListResponse {
            evidence: items.iter().map(record_of).collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

fn validate_entity_kind(entity_kind: &str) -> Result<(), ApiError> {
    if is_entity_kind(entity_kind) {
        Ok(())
    } else {
        Err(ApiError::invalid_request(&format!(
            "unknown entity kind `{entity_kind}`"
        )))
    }
}

fn record_of(item: &EvidenceItem) -> EvidenceRecord {
    EvidenceRecord {
        id: item.id().value(),
        project_id: item.project_id().to_owned(),
        entity_kind: item.entity_kind().to_owned(),
        entity_id: item.entity_id().to_owned(),
        evidence_kind: match item.kind() {
            EvidenceKind::ManagedFile => EvidenceKindDto::ManagedFile,
            EvidenceKind::Repository => EvidenceKindDto::Repository,
        },
        content_hash: item.content_hash().map(|hash| hash.as_str().to_owned()),
        relative_path: item.relative_path().map(|path| path.as_str().to_owned()),
        commit_identity: item
            .commit_identity()
            .map(|commit| commit.as_str().to_owned()),
    }
}

fn encode_record(item: &EvidenceItem) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(item)).map_err(|error| ApiError::internal(&error.to_string()))
}

fn announce(events: &dyn EventSink, kind: &str, item: &EvidenceItem) {
    events.emit(kind, json!(record_of(item)));
}

fn announce_list(events: &dyn EventSink, project_id: &str, count: usize) {
    events.emit(
        "evidence.listed",
        json!({ "project_id": project_id, "count": count }),
    );
}

fn content_hash(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

#[cfg(test)]
mod evidence_attach {
    use std::sync::{Arc, Mutex};

    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use kanban_domain::{
        CommitIdentity, EvidenceId, EvidenceItem, EvidenceKind, EvidenceShape, RelativePath,
    };
    use kanban_dto::{ApiError, ErrorCode};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    use super::{EvidenceFilter, EvidenceStore, TimelineEnvelope, TimelineFacts};
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::EventSink;
    use crate::mutation::MemoryIdempotencyStore;

    #[derive(Default)]
    struct MemoryEvidenceStore {
        state: Mutex<MemoryState>,
    }

    #[derive(Default)]
    struct MemoryState {
        items: Vec<EvidenceItem>,
        next_id: u64,
        timeline: Vec<(String, Value)>,
    }

    impl MemoryEvidenceStore {
        fn snapshot(&self) -> (Vec<EvidenceItem>, Vec<(String, Value)>) {
            let state = self.state.lock().expect("the memory store lock is sound");
            (state.items.clone(), state.timeline.clone())
        }
    }

    impl EvidenceStore for MemoryEvidenceStore {
        fn attach_managed_file(
            &self,
            project_id: &str,
            entity_kind: &str,
            entity_id: &str,
            content_base64: &str,
            append: TimelineFacts,
        ) -> Result<EvidenceItem, ApiError> {
            let content = STANDARD
                .decode(content_base64)
                .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
            let hash = format!("{:x}", Sha256::digest(&content));
            let mut state = self.state.lock().expect("the memory store lock is sound");
            state.next_id += 1;
            let item = EvidenceItem::restore(
                EvidenceId::new(state.next_id),
                EvidenceShape {
                    project_id: project_id.to_owned(),
                    entity_kind: entity_kind.to_owned(),
                    entity_id: entity_id.to_owned(),
                    kind: EvidenceKind::ManagedFile,
                    content_hash: Some(
                        kanban_domain::ContentHash::new(&hash)
                            .map_err(|error| ApiError::invalid_request(&error.to_string()))?,
                    ),
                    relative_path: None,
                    commit_identity: None,
                },
            )
            .map_err(|error| ApiError::internal(&error.to_string()))?;
            state.items.push(item.clone());
            state.timeline.push((
                append.kind.as_str().to_owned(),
                with_id(&append.facts, item.id()),
            ));
            Ok(item)
        }

        fn attach_repository(
            &self,
            project_id: &str,
            entity_kind: &str,
            entity_id: &str,
            relative_path: &RelativePath,
            commit_identity: &CommitIdentity,
            append: TimelineFacts,
        ) -> Result<EvidenceItem, ApiError> {
            let mut state = self.state.lock().expect("the memory store lock is sound");
            state.next_id += 1;
            let item = EvidenceItem::restore(
                EvidenceId::new(state.next_id),
                EvidenceShape {
                    project_id: project_id.to_owned(),
                    entity_kind: entity_kind.to_owned(),
                    entity_id: entity_id.to_owned(),
                    kind: EvidenceKind::Repository,
                    content_hash: None,
                    relative_path: Some(relative_path.clone()),
                    commit_identity: Some(commit_identity.clone()),
                },
            )
            .map_err(|error| ApiError::internal(&error.to_string()))?;
            state.items.push(item.clone());
            state.timeline.push((
                append.kind.as_str().to_owned(),
                with_id(&append.facts, item.id()),
            ));
            Ok(item)
        }

        fn list(
            &self,
            filter: &EvidenceFilter,
            append: TimelineEnvelope,
        ) -> Result<Vec<EvidenceItem>, ApiError> {
            let mut state = self.state.lock().expect("the memory store lock is sound");
            let items = state
                .items
                .iter()
                .filter(|item| item.project_id() == filter.project_id)
                .filter(|item| {
                    filter
                        .entity_kind
                        .as_ref()
                        .map(|kind| item.entity_kind() == kind)
                        .unwrap_or(true)
                })
                .filter(|item| {
                    filter
                        .entity_id
                        .as_ref()
                        .map(|id| item.entity_id() == id)
                        .unwrap_or(true)
                })
                .cloned()
                .collect();
            state
                .timeline
                .push((append.kind().as_str().to_owned(), append.detail().clone()));
            Ok(items)
        }
    }

    fn with_id(facts: &Value, id: EvidenceId) -> Value {
        let mut detail = facts.clone();
        detail
            .as_object_mut()
            .expect("the facts are a JSON object")
            .insert("id".to_owned(), json!(id.value()));
        detail
    }

    fn evidence_core(store: Arc<MemoryEvidenceStore>, events: Arc<dyn EventSink>) -> Core {
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            events,
        );
        core.register_evidence(store)
            .expect("the evidence operations register");
        core
    }

    fn attach_managed(key: &str) -> Value {
        json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": "kan-p1",
            "entity_kind": "ticket",
            "entity_id": "kan-t10",
            "evidence_kind": "managed_file",
            "content_base64": STANDARD.encode(b"proof"),
        })
    }

    fn attach_repository(key: &str) -> Value {
        json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": "kan-p1",
            "entity_kind": "ticket",
            "entity_id": "kan-t10",
            "evidence_kind": "repository",
            "relative_path": "docs/spec.md",
            "commit_identity": "deadbeef",
        })
    }

    fn list(key: &str) -> Value {
        json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": "kan-p1",
            "entity_kind": "ticket",
            "entity_id": "kan-t10",
        })
    }

    #[test]
    fn attaching_managed_file_appends_a_timeline_event() {
        let store = Arc::new(MemoryEvidenceStore::default());
        let core = evidence_core(store.clone(), Arc::new(crate::events::NoopEventSink));

        core.command("evidence.attach", &attach_managed("key-1"))
            .expect("the attach applies");

        let (_, timeline) = store.snapshot();
        let (_, facts) = timeline.last().cloned().expect("timeline appended");
        assert_eq!(facts["action"], json!("attached"));
        assert_eq!(facts["evidence_kind"], json!("managed_file"));
        assert_eq!(facts["entity_kind"], json!("ticket"));
        assert_eq!(facts["entity_id"], json!("kan-t10"));
        assert_eq!(facts["id"], json!(1));
        assert!(facts.get("content_hash").is_some());
    }

    #[test]
    fn attaching_repository_evidence_appends_a_timeline_event() {
        let store = Arc::new(MemoryEvidenceStore::default());
        let core = evidence_core(store.clone(), Arc::new(crate::events::NoopEventSink));

        core.command("evidence.attach", &attach_repository("key-1"))
            .expect("the attach applies");

        let (_, timeline) = store.snapshot();
        assert_eq!(
            timeline.last().cloned().expect("timeline appended"),
            (
                "evidence".to_owned(),
                json!({
                    "action": "attached",
                    "evidence_kind": "repository",
                    "relative_path": "docs/spec.md",
                    "commit_identity": "deadbeef",
                    "entity_kind": "ticket",
                    "entity_id": "kan-t10",
                    "id": 1,
                })
            )
        );
    }

    #[test]
    fn listing_evidence_appends_a_timeline_event() {
        let store = Arc::new(MemoryEvidenceStore::default());
        let core = evidence_core(store.clone(), Arc::new(crate::events::NoopEventSink));
        core.command("evidence.attach", &attach_managed("key-1"))
            .expect("the attach applies");

        core.command("evidence.list", &list("key-2"))
            .expect("the list applies");

        let (_, timeline) = store.snapshot();
        assert_eq!(
            timeline.last().cloned().expect("timeline appended"),
            (
                "evidence".to_owned(),
                json!({
                    "action": "listed",
                    "entity_kind": "ticket",
                    "entity_id": "kan-t10",
                })
            )
        );
    }

    #[test]
    fn attaching_rejects_unknown_fields() {
        let store = Arc::new(MemoryEvidenceStore::default());
        let core = evidence_core(store, Arc::new(crate::events::NoopEventSink));

        let error = core
            .command(
                "evidence.attach",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                    "project_id": "kan-p1",
                    "entity_kind": "ticket",
                    "entity_id": "kan-t10",
                    "evidence_kind": "managed_file",
                    "content_base64": STANDARD.encode(b"proof"),
                    "surprise": true,
                }),
            )
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
    }

    #[test]
    fn attaching_managed_file_without_content_is_refused() {
        let store = Arc::new(MemoryEvidenceStore::default());
        let core = evidence_core(store.clone(), Arc::new(crate::events::NoopEventSink));

        let error = core
            .command(
                "evidence.attach",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                    "project_id": "kan-p1",
                    "entity_kind": "ticket",
                    "entity_id": "kan-t10",
                    "evidence_kind": "managed_file",
                }),
            )
            .expect_err("missing content is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        let (items, timeline) = store.snapshot();
        assert!(items.is_empty());
        assert!(timeline.is_empty());
    }
}
