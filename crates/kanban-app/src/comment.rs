//! Comment commands and the revision history query (KAN-S2-US2).
//! Editing appends a revision; the current text resolves to the
//! latest revision and history stays queryable.

use std::sync::Arc;

use kanban_domain::{Comment, CommentId, CommentTarget, CommentText, TextError};
use kanban_dto::{
    ApiError, CommentCreateRequest, CommentEditRequest, CommentRecord, CommentRevisionsQuery,
    CommentRevisionsResponse, TimelineEntityKind, TimelineEntityRef,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::events::EventSink;
use crate::mutation::{CommandHandler, ParsedCommand, parse_payload};

/// The storage port Comment commands call through.
pub trait CommentStore: Send + Sync {
    /// Insert a fresh Comment with its first revision.
    fn create(
        &self,
        project_id: &str,
        target: &CommentTarget,
        text: &CommentText,
    ) -> Result<Comment, ApiError>;

    /// Load one Comment, if it exists.
    fn find(&self, id: CommentId) -> Result<Option<Comment>, ApiError>;

    /// Persist an applied edit and its new revision.
    fn save(&self, comment: &Comment) -> Result<(), ApiError>;

    /// Return the current record and every revision.
    fn revisions(
        &self,
        id: CommentId,
    ) -> Result<(CommentRecord, Vec<kanban_dto::CommentRevisionRecord>), ApiError>;
}

impl Core {
    /// Register the Comment operations against `store`.
    pub fn register_comments(
        &mut self,
        store: Arc<dyn CommentStore>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "comment.create",
            Arc::new(CreateComment {
                store: store.clone(),
            }),
        )?;
        self.register_command(
            "comment.edit",
            Arc::new(EditComment {
                store: store.clone(),
            }),
        )?;
        self.register_query("comment.revisions", Arc::new(RevisionsQuery { store }))?;
        Ok(())
    }
}

/// Serves `comment.create`.
struct CreateComment {
    store: Arc<dyn CommentStore>,
}

impl CommandHandler for CreateComment {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<CommentCreateRequest>(payload)?;
        ParsedCommand::lift("comment", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: CommentCreateRequest = parse_payload(&command.payload)?;
        let text = parse_text(&request.text)?;
        let target = parse_target(&request.target)?;
        let comment = self.store.create(&request.project_id, &target, &text)?;
        announce(events, "comment.created", &comment);
        encode_record(&comment)
    }
}

/// Serves `comment.edit`.
struct EditComment {
    store: Arc<dyn CommentStore>,
}

impl CommandHandler for EditComment {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<CommentEditRequest>(payload)?;
        ParsedCommand::lift("comment", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: CommentEditRequest = parse_payload(&command.payload)?;
        let comment = load(&self.store, request.comment_id)?;
        Ok(comment.version())
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: CommentEditRequest = parse_payload(&command.payload)?;
        let text = parse_text(&request.text)?;
        let mut comment = load(&self.store, request.comment_id)?;
        comment
            .edit(text)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        self.store.save(&comment)?;
        announce(events, "comment.edited", &comment);
        encode_record(&comment)
    }
}

/// Serves `comment.revisions`.
struct RevisionsQuery {
    store: Arc<dyn CommentStore>,
}

impl QueryHandler for RevisionsQuery {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: CommentRevisionsQuery = parse_payload(payload)?;
        let (comment, revisions) = self.store.revisions(CommentId::new(query.comment_id))?;
        let response = CommentRevisionsResponse { comment, revisions };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

fn load(store: &Arc<dyn CommentStore>, id: u64) -> Result<Comment, ApiError> {
    store
        .find(CommentId::new(id))?
        .ok_or_else(|| ApiError::not_found(&format!("comment {id}")))
}

fn parse_text(raw: &str) -> Result<CommentText, ApiError> {
    CommentText::new(raw).map_err(|error: TextError| ApiError::invalid_request(&error.to_string()))
}

fn parse_target(target: &TimelineEntityRef) -> Result<CommentTarget, ApiError> {
    let kind = entity_kind_wire(target.kind);
    CommentTarget::new(&kind, &target.id)
        .map_err(|error| ApiError::invalid_request(&error.to_string()))
}

fn entity_kind_wire(kind: TimelineEntityKind) -> String {
    serde_json::to_string(&kind)
        .expect("entity kind encodes")
        .trim_matches('"')
        .to_owned()
}

fn record_of(comment: &Comment) -> CommentRecord {
    CommentRecord {
        id: comment.id().value(),
        project_id: comment.project_id().to_owned(),
        target: TimelineEntityRef {
            kind: dto_entity_kind(comment.target().kind()),
            id: comment.target().id().to_owned(),
        },
        text: comment.current_text().as_str().to_owned(),
        version: comment.version(),
    }
}

fn dto_entity_kind(kind: &str) -> TimelineEntityKind {
    match kind {
        "initiative" => TimelineEntityKind::Initiative,
        "project" => TimelineEntityKind::Project,
        "plan" => TimelineEntityKind::Plan,
        "spec" => TimelineEntityKind::Spec,
        "ticket" => TimelineEntityKind::Ticket,
        "run" => TimelineEntityKind::Run,
        "review" => TimelineEntityKind::Review,
        "finding" => TimelineEntityKind::Finding,
        "evidence" => TimelineEntityKind::Evidence,
        "comment" => TimelineEntityKind::Comment,
        other => panic!("unknown entity kind `{other}`"),
    }
}

fn encode_record(comment: &Comment) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(comment)).map_err(|error| ApiError::internal(&error.to_string()))
}

fn announce(events: &dyn EventSink, kind: &str, comment: &Comment) {
    events.emit(
        kind,
        json!({
            "id": comment.id().value(),
            "project_id": comment.project_id(),
            "target": {
                "kind": comment.target().kind(),
                "id": comment.target().id(),
            },
            "text": comment.current_text().as_str(),
            "version": comment.version(),
        }),
    );
}

#[cfg(test)]
mod comments {
    use std::sync::{Arc, Mutex};

    use kanban_domain::{Comment, CommentId, CommentTarget, CommentText};
    use kanban_dto::{ErrorCode, TimelineEntityKind, TimelineEntityRef};
    use serde_json::{Value, json};

    use super::CommentStore;
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::EventSink;
    use crate::mutation::MemoryIdempotencyStore;

    #[derive(Default)]
    struct MemoryCommentStore {
        state: Mutex<MemoryState>,
    }

    #[derive(Default)]
    struct MemoryState {
        comments: Vec<Comment>,
        next_id: u64,
        revision_stamps: Vec<String>,
    }

    impl MemoryCommentStore {
        fn snapshot(&self) -> Vec<Comment> {
            self.state
                .lock()
                .expect("the memory store lock is sound")
                .comments
                .clone()
        }
    }

    impl CommentStore for MemoryCommentStore {
        fn create(
            &self,
            project_id: &str,
            target: &CommentTarget,
            text: &CommentText,
        ) -> Result<Comment, kanban_dto::ApiError> {
            let mut state = self.state.lock().expect("the memory store lock is sound");
            state.next_id += 1;
            let id = state.next_id;
            let comment =
                Comment::create(CommentId::new(id), project_id, target.clone(), text.clone());
            state.comments.push(comment.clone());
            state
                .revision_stamps
                .push(format!("2026-09-04T12:00:{id:02}Z"));
            Ok(comment)
        }

        fn find(&self, id: CommentId) -> Result<Option<Comment>, kanban_dto::ApiError> {
            let state = self.state.lock().expect("the memory store lock is sound");
            Ok(state.comments.iter().find(|row| row.id() == id).cloned())
        }

        fn save(&self, comment: &Comment) -> Result<(), kanban_dto::ApiError> {
            let mut state = self.state.lock().expect("the memory store lock is sound");
            let id = comment.id();
            if let Some(row) = state.comments.iter_mut().find(|row| row.id() == id) {
                *row = comment.clone();
            } else {
                return Err(kanban_dto::ApiError::not_found(&format!("comment {id}")));
            }
            let revision = comment.revisions().len() as u64;
            state
                .revision_stamps
                .push(format!("2026-09-04T12:00:{revision:02}Z"));
            Ok(())
        }

        fn revisions(
            &self,
            id: CommentId,
        ) -> Result<
            (
                kanban_dto::CommentRecord,
                Vec<kanban_dto::CommentRevisionRecord>,
            ),
            kanban_dto::ApiError,
        > {
            let comment = self
                .find(id)?
                .ok_or_else(|| kanban_dto::ApiError::not_found(&format!("comment {id}")))?;
            let state = self.state.lock().expect("the memory store lock is sound");
            let revisions = comment
                .revisions()
                .iter()
                .enumerate()
                .map(|(index, revision)| kanban_dto::CommentRevisionRecord {
                    revision: revision.number(),
                    text: revision.text().as_str().to_owned(),
                    recorded_at: state
                        .revision_stamps
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| "2026-09-04T12:00:00Z".to_owned()),
                })
                .collect();
            Ok((super::record_of(&comment), revisions))
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

    fn target() -> TimelineEntityRef {
        TimelineEntityRef {
            kind: TimelineEntityKind::Ticket,
            id: "kan-t11".to_owned(),
        }
    }

    fn comment_core(store: Arc<MemoryCommentStore>, events: Arc<dyn EventSink>) -> Core {
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            events,
        );
        core.register_comments(store)
            .expect("the comment operations register");
        core
    }

    fn create(text: &str, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "project_id": "kan",
            "target": target(),
            "text": text,
        })
    }

    fn edit(id: u64, text: &str, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "comment_id": id,
            "text": text,
        })
    }

    #[test]
    fn creating_returns_the_comment_at_revision_one() {
        let store = Arc::new(MemoryCommentStore::default());
        let core = comment_core(store.clone(), Arc::new(crate::events::NoopEventSink));

        let response = core
            .command("comment.create", &create("First thought", "key-1", 0))
            .expect("the create applies");

        assert_eq!(
            response,
            json!({
                "id": 1,
                "project_id": "kan",
                "target": target(),
                "text": "First thought",
                "version": 1,
            })
        );
    }

    #[test]
    fn editing_appends_a_revision_and_updates_current_text() {
        let store = Arc::new(MemoryCommentStore::default());
        let core = comment_core(store.clone(), Arc::new(crate::events::NoopEventSink));
        core.command("comment.create", &create("First thought", "key-1", 0))
            .expect("the create applies");

        let response = core
            .command("comment.edit", &edit(1, "Corrected thought", "key-2", 1))
            .expect("the edit applies");

        assert_eq!(
            response,
            json!({
                "id": 1,
                "project_id": "kan",
                "target": target(),
                "text": "Corrected thought",
                "version": 2,
            })
        );
        let stored = store.snapshot();
        assert_eq!(stored[0].revisions().len(), 2);
        assert_eq!(
            stored[0].revisions()[0].text().as_str(),
            "First thought",
            "earlier revisions stay intact"
        );
    }

    #[test]
    fn revisions_query_returns_full_history_in_order() {
        let store = Arc::new(MemoryCommentStore::default());
        let core = comment_core(store, Arc::new(crate::events::NoopEventSink));
        core.command("comment.create", &create("First thought", "key-1", 0))
            .expect("the create applies");
        core.command("comment.edit", &edit(1, "Second thought", "key-2", 1))
            .expect("the first edit applies");
        core.command("comment.edit", &edit(1, "Latest thought", "key-3", 2))
            .expect("the second edit applies");

        let response = core
            .query("comment.revisions", &json!({ "comment_id": 1 }))
            .expect("the revisions query serves");

        assert_eq!(
            response,
            json!({
                "comment": {
                    "id": 1,
                    "project_id": "kan",
                    "target": target(),
                    "text": "Latest thought",
                    "version": 3,
                },
                "revisions": [
                    {
                        "revision": 1,
                        "text": "First thought",
                        "recorded_at": "2026-09-04T12:00:01Z",
                    },
                    {
                        "revision": 2,
                        "text": "Second thought",
                        "recorded_at": "2026-09-04T12:00:02Z",
                    },
                    {
                        "revision": 3,
                        "text": "Latest thought",
                        "recorded_at": "2026-09-04T12:00:03Z",
                    },
                ],
            })
        );
    }

    #[test]
    fn editing_with_a_stale_version_is_rejected_with_the_current_one() {
        let store = Arc::new(MemoryCommentStore::default());
        let core = comment_core(store, Arc::new(crate::events::NoopEventSink));
        core.command("comment.create", &create("First thought", "key-1", 0))
            .expect("the create applies");

        let error = core
            .command("comment.edit", &edit(1, "Corrected thought", "key-2", 0))
            .expect_err("the stale version is rejected");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(1));
    }

    #[test]
    fn editing_an_unknown_comment_is_not_found() {
        let store = Arc::new(MemoryCommentStore::default());
        let core = comment_core(store, Arc::new(crate::events::NoopEventSink));

        let error = core
            .command("comment.edit", &edit(9, "Ghost", "key-1", 0))
            .expect_err("the unknown comment is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(error.message.contains("comment 9"));
    }

    #[test]
    fn blank_text_is_refused_on_create_and_edit_without_recording_anything() {
        let store = Arc::new(MemoryCommentStore::default());
        let core = comment_core(store.clone(), Arc::new(crate::events::NoopEventSink));

        let create_error = core
            .command("comment.create", &create("   ", "key-1", 0))
            .expect_err("blank create text is refused");
        assert_eq!(create_error.code, ErrorCode::InvalidRequest);
        assert!(store.snapshot().is_empty(), "no row may be written");

        core.command("comment.create", &create("First thought", "key-1", 0))
            .expect("the create applies");
        let edit_error = core
            .command("comment.edit", &edit(1, "   ", "key-2", 1))
            .expect_err("blank edit text is refused");
        assert_eq!(edit_error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            store.snapshot()[0].current_text().as_str(),
            "First thought",
            "the refusal changed nothing"
        );
    }

    #[test]
    fn comment_changes_publish_on_the_event_stream() {
        let store = Arc::new(MemoryCommentStore::default());
        let sink = Arc::new(RecordingSink::default());
        let core = comment_core(store, sink.clone());
        core.command("comment.create", &create("First thought", "key-1", 0))
            .expect("the create applies");

        let events = sink.events.lock().expect("the recorder lock is sound");
        assert_eq!(
            *events,
            vec![(
                "comment.created".to_owned(),
                json!({
                    "id": 1,
                    "project_id": "kan",
                    "target": { "kind": "ticket", "id": "kan-t11" },
                    "text": "First thought",
                    "version": 1,
                })
            )]
        );
    }

    #[test]
    fn revisions_query_rejects_unknown_fields() {
        let store = Arc::new(MemoryCommentStore::default());
        let core = comment_core(store, Arc::new(crate::events::NoopEventSink));

        let error = core
            .query(
                "comment.revisions",
                &json!({ "comment_id": 1, "surprise": true }),
            )
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
    }

    #[test]
    fn comment_operations_are_catalogued_on_the_core() {
        let names: Vec<_> = exposed_operations()
            .iter()
            .filter(|operation| operation.name.starts_with("comment."))
            .map(|operation| operation.name)
            .collect();
        assert_eq!(
            names,
            vec!["comment.create", "comment.edit", "comment.revisions"]
        );
    }
}
