//! Comment commands and the revision history query (KAN-S2-US2).
//! Editing appends a revision; the current text resolves to the
//! latest revision and history stays queryable.

use std::sync::Arc;

use kanban_domain::{Comment, CommentId, CommentTarget, CommentText, IssueKey, TextError};
use kanban_dto::{
    ApiError, CommentCreateRequest, CommentEditRequest, CommentListItem, CommentListQuery,
    CommentListResponse, CommentRecord, CommentResolveRequest, CommentResolveResponse,
    CommentRevisionsQuery, CommentRevisionsResponse, ErrorCode, LiveEventName, TimelineEntityKind,
    TimelineEntityRef,
};
use serde_json::Value;

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::events::emit_catalogued;
use crate::issue::IssueStore;
use crate::mutation::{
    CommandEffects, CommandHandler, ParsedCommand, parse_issue_payload, parse_payload,
};
use crate::project::{ProjectStore, resolve_project};

/// The issue lookups Comment commands use to resolve a ticket target,
/// the same way the issue tools resolve their references: an issue
/// key or a numeric ticket id, answered with the canonical numeric
/// identity every stored comment shares.
pub trait CommentIssueTargets: Send + Sync {
    /// Resolve `reference` to the issue's key and numeric identity,
    /// or `None` when no such issue exists.
    fn resolve_issue_reference(&self, reference: &str)
    -> Result<Option<(IssueKey, u64)>, ApiError>;

    /// The project that owns the resolved issue, or `None` when no
    /// such issue exists.
    fn issue_project(&self, ticket_id: u64) -> Result<Option<u64>, ApiError>;

    /// Whether a non-ticket target names a stored entity of its kind.
    fn comment_target_exists(&self, kind: &str, id: &str) -> Result<bool, ApiError>;

    /// The project that owns a non-ticket target of its kind, or
    /// `None` when the kind carries no owning project. Streams
    /// outside the issue model keep their raw ids, so ownership
    /// follows the target's own stored project reference.
    fn comment_target_project(&self, kind: &str, id: &str) -> Result<Option<u64>, ApiError>;

    /// Advance the issue's update stamp without an edit, so a
    /// comment or reply keeps the issue current for incremental sync.
    fn touch_issue(&self, ticket_id: u64) -> Result<(), ApiError>;
}

/// [`CommentIssueTargets`] over the Linear issue store.
pub struct IssueStoreCommentTargets {
    issues: Arc<dyn IssueStore>,
}

impl IssueStoreCommentTargets {
    pub fn new(issues: Arc<dyn IssueStore>) -> Self {
        Self { issues }
    }
}

impl CommentIssueTargets for IssueStoreCommentTargets {
    fn resolve_issue_reference(
        &self,
        reference: &str,
    ) -> Result<Option<(IssueKey, u64)>, ApiError> {
        if let Ok(id) = reference.parse::<u64>() {
            if i64::try_from(id).is_err() {
                return Ok(None);
            }
            // A numeric reference addresses the ticket directly; the
            // key lookup doubles as the existence check.
            return match self.issues.load_issue_key(id as i64) {
                Ok(key) => Ok(Some((key, id))),
                Err(error) if error.code == ErrorCode::NotFound => Ok(None),
                Err(error) => Err(error),
            };
        }
        Ok(self
            .issues
            .resolve_identifier(reference)?
            .map(|(key, ticket_id)| (key, ticket_id as u64)))
    }

    fn issue_project(&self, ticket_id: u64) -> Result<Option<u64>, ApiError> {
        self.issues.issue_project(ticket_id as i64)
    }

    fn comment_target_exists(&self, kind: &str, id: &str) -> Result<bool, ApiError> {
        self.issues.comment_target_exists(kind, id)
    }

    fn comment_target_project(&self, kind: &str, id: &str) -> Result<Option<u64>, ApiError> {
        self.issues.comment_target_project(kind, id)
    }

    fn touch_issue(&self, ticket_id: u64) -> Result<(), ApiError> {
        self.issues.touch_updated_at(ticket_id as i64)
    }
}

/// The storage port Comment commands call through.
pub trait CommentStore: Send + Sync {
    /// Insert a fresh Comment with its first revision, under the
    /// resolved Project's numeric identity.
    fn create(
        &self,
        project_id: u64,
        target: &CommentTarget,
        text: &CommentText,
        author: &str,
        parent_comment_id: Option<u64>,
    ) -> Result<Comment, ApiError>;

    /// Record a resolution change and answer with the new version;
    /// resolving and unresolving each bump the aggregate like an edit.
    fn set_resolved(&self, id: CommentId, resolved: bool) -> Result<u64, ApiError>;

    /// Load one Comment, if it exists.
    fn find(&self, id: CommentId) -> Result<Option<Comment>, ApiError>;

    /// Persist an applied edit and its new revision.
    fn save(&self, comment: &Comment) -> Result<(), ApiError>;

    /// Return the current record and every revision.
    fn revisions(
        &self,
        id: CommentId,
    ) -> Result<(CommentRecord, Vec<kanban_dto::CommentRevisionRecord>), ApiError>;

    /// List comments on one target, oldest first, with cursor paging.
    /// Rows carry the target's canonical address, so the stream
    /// filters by kind and id in storage.
    fn list_for_target(
        &self,
        target: &CommentTarget,
        limit: u32,
        cursor: Option<u64>,
    ) -> Result<(Vec<CommentListItem>, Option<String>), ApiError>;

    /// The target and parent of a stored Comment, so reply
    /// validation can check stream membership and nesting depth.
    fn thread_anchor(
        &self,
        id: CommentId,
    ) -> Result<Option<(CommentTarget, Option<u64>)>, ApiError>;
}

impl Core {
    /// Register the Comment operations against `store`, resolving
    /// their Project through `projects` and their ticket targets
    /// through `targets`.
    pub fn register_comments(
        &mut self,
        store: Arc<dyn CommentStore>,
        projects: Arc<dyn ProjectStore>,
        targets: Arc<dyn CommentIssueTargets>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "comment.create",
            Arc::new(CreateComment {
                store: store.clone(),
                projects,
                targets: targets.clone(),
            }),
        )?;
        self.register_command(
            "comment.edit",
            Arc::new(EditComment {
                store: store.clone(),
            }),
        )?;
        self.register_query(
            "comment.revisions",
            Arc::new(RevisionsQuery {
                store: store.clone(),
            }),
        )?;
        self.register_query(
            "comment.list",
            Arc::new(ListCommentsQuery {
                store: store.clone(),
                targets,
            }),
        )?;
        self.register_command("comment.resolve", Arc::new(ResolveComment { store }))?;
        Ok(())
    }
}

/// Serves `comment.create`.
struct CreateComment {
    store: Arc<dyn CommentStore>,
    projects: Arc<dyn ProjectStore>,
    targets: Arc<dyn CommentIssueTargets>,
}

impl CommandHandler for CreateComment {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_issue_payload::<CommentCreateRequest>(payload)?;
        ParsedCommand::lift("comment", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: CommentCreateRequest = parse_payload(&command.payload)?;
        let project = resolve_project(self.projects.as_ref(), request.project_id)?;
        let text = parse_stored_text(&request.text)?;
        let author = require_author(&request.author)?;
        let target = resolve_target(self.targets.as_ref(), &request.target)?;
        if target.kind() == TimelineEntityKind::Ticket.as_str() {
            let ticket_id: u64 = target.id().parse().map_err(|_| {
                ApiError::internal("a resolved ticket target carries its numeric id")
            })?;
            let owner = self
                .targets
                .issue_project(ticket_id)?
                .ok_or_else(|| ApiError::not_found(&format!("issue {}", target.id())))?;
            if owner != project.id().value() {
                return Err(ApiError::invalid_request_field(
                    "project_id",
                    "project_id does not own the target issue",
                ));
            }
        } else if !self
            .targets
            .comment_target_exists(target.kind(), target.id())?
        {
            return Err(ApiError::not_found(&format!(
                "{} {}",
                target.kind(),
                target.id()
            )));
        } else if let Some(owner) = self
            .targets
            .comment_target_project(target.kind(), target.id())?
            && owner != project.id().value()
        {
            return Err(ApiError::invalid_request_field(
                "project_id",
                &format!(
                    "project_id does not own the target {} {}",
                    target.kind(),
                    target.id()
                ),
            ));
        }
        let parent_comment_id = validate_parent(
            self.store.as_ref(),
            self.targets.as_ref(),
            request.parent_comment_id,
            &target,
        )?;
        let comment = self.store.create(
            project.id().value(),
            &target,
            &text,
            author,
            parent_comment_id,
        )?;
        if target.kind() == TimelineEntityKind::Ticket.as_str() {
            let ticket_id: u64 = target.id().parse().map_err(|_| {
                ApiError::internal("a resolved ticket target carries its numeric id")
            })?;
            self.targets.touch_issue(ticket_id)?;
        }
        announce(effects, LiveEventName::CommentCreated, &comment);
        encode_record(&comment)
    }
}

/// Serves `comment.edit`.
struct EditComment {
    store: Arc<dyn CommentStore>,
}

impl CommandHandler for EditComment {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_issue_payload::<CommentEditRequest>(payload)?;
        ParsedCommand::lift("comment", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: CommentEditRequest = parse_payload(&command.payload)?;
        let comment = load(&self.store, request.comment_id)?;
        Ok(comment.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: CommentEditRequest = parse_payload(&command.payload)?;
        let text = parse_stored_text(&request.text)?;
        let mut comment = load(&self.store, request.comment_id)?;
        comment
            .edit(text)
            .map_err(|error| ApiError::invalid_request_field("text", &error.to_string()))?;
        self.store.save(&comment)?;
        announce(effects, LiveEventName::CommentEdited, &comment);
        encode_record(&comment)
    }
}

/// Serves `comment.revisions`.
struct RevisionsQuery {
    store: Arc<dyn CommentStore>,
}

impl QueryHandler for RevisionsQuery {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: CommentRevisionsQuery = parse_issue_payload(payload)?;
        let (comment, revisions) = self.store.revisions(CommentId::new(query.comment_id))?;
        let response = CommentRevisionsResponse { comment, revisions };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `comment.list`.
struct ListCommentsQuery {
    store: Arc<dyn CommentStore>,
    targets: Arc<dyn CommentIssueTargets>,
}

impl QueryHandler for ListCommentsQuery {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: CommentListQuery = parse_issue_payload(payload)?;
        let target = resolve_target(self.targets.as_ref(), &query.target)?;
        if target.kind() != TimelineEntityKind::Ticket.as_str()
            && !self
                .targets
                .comment_target_exists(target.kind(), target.id())?
        {
            return Err(ApiError::not_found(&format!(
                "{} {}",
                target.kind(),
                target.id()
            )));
        }
        let limit = if query.limit == 0 {
            50
        } else {
            query.limit.clamp(1, 100)
        };
        let cursor = query
            .cursor
            .as_deref()
            .map(str::parse::<u64>)
            .transpose()
            .map_err(|_| {
                ApiError::invalid_request_field("cursor", "cursor must be a comment id")
            })?;
        let (comments, next_cursor) = self.store.list_for_target(&target, limit, cursor)?;
        let response = CommentListResponse {
            comments,
            next_cursor,
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

struct ResolveComment {
    store: Arc<dyn CommentStore>,
}

impl CommandHandler for ResolveComment {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_issue_payload::<CommentResolveRequest>(payload)?;
        ParsedCommand::lift("comment", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: CommentResolveRequest = parse_payload(&command.payload)?;
        let comment = load(&self.store, request.comment_id)?;
        Ok(comment.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: CommentResolveRequest = parse_payload(&command.payload)?;
        let version = self
            .store
            .set_resolved(CommentId::new(request.comment_id), request.resolved)?;
        let response = CommentResolveResponse {
            comment_id: request.comment_id,
            resolved: request.resolved,
            version,
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

fn load(store: &Arc<dyn CommentStore>, id: u64) -> Result<Comment, ApiError> {
    store
        .find(CommentId::new(id))?
        .ok_or_else(|| ApiError::not_found(&format!("comment {id}")))
}

/// Every comment carries the identity it speaks for. The core holds
/// no operator identity to default from, so the caller must name one.
fn require_author(author: &str) -> Result<&str, ApiError> {
    if author.trim().is_empty() {
        return Err(ApiError::invalid_request_field(
            "author",
            "author is required; the core holds no operator identity to default from",
        ));
    }
    Ok(author)
}

fn parse_stored_text(raw: &str) -> Result<CommentText, ApiError> {
    CommentText::new_stored(raw)
        .map_err(|error: TextError| ApiError::invalid_request_field("text", &error.to_string()))
}

fn parse_target(target: &TimelineEntityRef) -> Result<CommentTarget, ApiError> {
    CommentTarget::new(target.kind.as_str(), &target.id)
        .map_err(|error| ApiError::invalid_request_field("target", &error.to_string()))
}

/// Resolve the target a command addressed. Ticket targets resolve
/// the way issue tools resolve their references, so an issue key and
/// the issue's numeric id address one comment stream and every new
/// write stores the numeric id. Other entity kinds keep the raw id.
fn resolve_target(
    targets: &dyn CommentIssueTargets,
    raw: &TimelineEntityRef,
) -> Result<CommentTarget, ApiError> {
    let target = parse_target(raw)?;
    if target.kind() != TimelineEntityKind::Ticket.as_str() {
        return Ok(target);
    }
    let (_, ticket_id) = targets
        .resolve_issue_reference(target.id())?
        .ok_or_else(|| ApiError::not_found(&format!("issue {}", target.id())))?;
    let canonical = ticket_id.to_string();
    CommentTarget::new(target.kind(), &canonical)
        .map_err(|error| ApiError::invalid_request_field("target", &error.to_string()))
}

/// Replies join the stream they answer and nest one level, the way
/// Linear threads comments. The parent must live on the same target
/// and must itself be a top-level comment.
fn validate_parent(
    store: &dyn CommentStore,
    targets: &dyn CommentIssueTargets,
    parent_comment_id: Option<u64>,
    target: &CommentTarget,
) -> Result<Option<u64>, ApiError> {
    let Some(parent_id) = parent_comment_id else {
        return Ok(None);
    };
    let (parent_target, grandparent) = store
        .thread_anchor(CommentId::new(parent_id))?
        .ok_or_else(|| ApiError::not_found(&format!("comment {parent_id}")))?;
    if !same_stream(targets, &parent_target, target)? {
        return Err(ApiError::invalid_request_field(
            "parent_comment_id",
            "parent_comment_id names a comment on another target",
        ));
    }
    if grandparent.is_some() {
        return Err(ApiError::invalid_request_field(
            "parent_comment_id",
            "parent_comment_id names a reply; replies nest one level",
        ));
    }
    Ok(Some(parent_id))
}

/// Whether a stored target and a resolved target address one entity.
/// Legacy key-form rows resolve through the issue lookups.
fn same_stream(
    targets: &dyn CommentIssueTargets,
    stored: &CommentTarget,
    requested: &CommentTarget,
) -> Result<bool, ApiError> {
    if stored.kind() != requested.kind() {
        return Ok(false);
    }
    // Only ticket identities resolve case-insensitively, through the
    // issue lookups; every other kind matches its id exactly, so a
    // case variant is a different stream rather than the same one.
    if stored.kind() == TimelineEntityKind::Ticket.as_str() {
        if stored.id().eq_ignore_ascii_case(requested.id()) {
            return Ok(true);
        }
        if let Some((_, stored_id)) = targets.resolve_issue_reference(stored.id())? {
            return Ok(stored_id.to_string() == requested.id());
        }
    }
    Ok(stored.id() == requested.id())
}

fn record_of(comment: &Comment) -> CommentRecord {
    CommentRecord {
        id: comment.id().value(),
        project_id: comment.project_id(),
        target: TimelineEntityRef {
            // The target passed the vocabulary check on the way in;
            // anything else is corruption, not a value to carry.
            kind: TimelineEntityKind::parse(comment.target().kind())
                .expect("a stored Comment target names a known entity kind"),
            id: comment.target().id().to_owned(),
        },
        text: comment.current_text().as_str().to_owned(),
        version: comment.version(),
    }
}

fn encode_record(comment: &Comment) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(comment)).map_err(|error| ApiError::internal(&error.to_string()))
}

fn announce(effects: &dyn CommandEffects, event: LiveEventName, comment: &Comment) {
    emit_catalogued(effects, event, &record_of(comment));
}

#[cfg(test)]
mod comments {
    use std::sync::{Arc, Mutex};

    use kanban_domain::{
        Comment, CommentId, CommentTarget, CommentText, IssueKey, ProjectCode, TicketNumber,
    };
    use kanban_dto::{CommentListItem, ErrorCode, TimelineEntityKind, TimelineEntityRef};
    use serde_json::{Value, json};

    use super::CommentIssueTargets;
    use super::CommentStore;
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::EventSink;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::project::testing::MemoryProjectStore;

    #[derive(Default)]
    struct MemoryCommentStore {
        state: Mutex<MemoryState>,
    }

    #[derive(Default)]
    struct MemoryState {
        comments: Vec<Comment>,
        parents: Vec<Option<u64>>,
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
            project_id: u64,
            target: &CommentTarget,
            text: &CommentText,
            _author: &str,
            parent_comment_id: Option<u64>,
        ) -> Result<Comment, kanban_dto::ApiError> {
            let mut state = self.state.lock().expect("the memory store lock is sound");
            state.next_id += 1;
            let id = state.next_id;
            let comment =
                Comment::create(CommentId::new(id), project_id, target.clone(), text.clone());
            state.comments.push(comment.clone());
            state.parents.push(parent_comment_id);
            state
                .revision_stamps
                .push(format!("2026-09-04T12:00:{id:02}Z"));
            Ok(comment)
        }

        fn thread_anchor(
            &self,
            id: CommentId,
        ) -> Result<Option<(CommentTarget, Option<u64>)>, kanban_dto::ApiError> {
            let state = self.state.lock().expect("the memory store lock is sound");
            let index = state.comments.iter().position(|row| row.id() == id);
            Ok(index.map(|at| {
                (
                    state.comments[at].target().clone(),
                    state.parents.get(at).copied().flatten(),
                )
            }))
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

        fn list_for_target(
            &self,
            target: &CommentTarget,
            limit: u32,
            cursor: Option<u64>,
        ) -> Result<(Vec<CommentListItem>, Option<String>), kanban_dto::ApiError> {
            let state = self.state.lock().expect("the memory store lock is sound");
            let after = cursor.unwrap_or(0);
            let mut items: Vec<CommentListItem> = Vec::new();
            for comment in &state.comments {
                if items.len() > limit as usize {
                    break;
                }
                if comment.target().kind() != target.kind()
                    || comment.target().id() != target.id()
                    || comment.id().value() <= after
                {
                    continue;
                }
                let id = comment.id().value();
                items.push(CommentListItem {
                    id,
                    author: String::new(),
                    body: comment.current_text().as_str().to_owned(),
                    created_at: state
                        .revision_stamps
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "2026-09-04T12:00:00Z".to_owned()),
                    // Edit metadata follows the body revisions, not
                    // the aggregate version, so a resolution change
                    // never reports an edit.
                    edited_at: (comment.revisions().len() > 1).then(|| {
                        state
                            .revision_stamps
                            .last()
                            .cloned()
                            .unwrap_or_else(|| "2026-09-04T12:00:00Z".to_owned())
                    }),
                    parent_comment_id: state
                        .parents
                        .get(
                            state
                                .comments
                                .iter()
                                .position(|row| row.id() == comment.id())
                                .expect("the row is indexed"),
                        )
                        .copied()
                        .flatten(),
                    resolved_at: None,
                    version: comment.version(),
                });
            }
            let next_cursor = if items.len() > limit as usize {
                Some(items[(limit as usize).saturating_sub(1)].id.to_string())
            } else {
                None
            };
            items.truncate(limit as usize);
            Ok((items, next_cursor))
        }

        fn set_resolved(
            &self,
            id: CommentId,
            _resolved: bool,
        ) -> Result<u64, kanban_dto::ApiError> {
            let mut state = self.state.lock().expect("the memory store lock is sound");
            let Some(row) = state.comments.iter_mut().find(|row| row.id() == id) else {
                return Err(kanban_dto::ApiError::not_found(&format!("comment {id}")));
            };
            *row = Comment::restore(
                row.id(),
                row.project_id(),
                row.target().clone(),
                row.revisions().to_vec(),
                row.version() + 1,
            );
            Ok(row.version())
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
            id: "11".to_owned(),
        }
    }

    fn issue_key(number: u64) -> IssueKey {
        IssueKey::new(
            ProjectCode::new("TUS").expect("the project code validates"),
            TicketNumber::new(number).expect("the ticket number validates"),
        )
    }

    /// Numeric references address the ticket they name; key
    /// references resolve to the number they carry.
    struct MemoryIssueTargets;

    impl CommentIssueTargets for MemoryIssueTargets {
        fn touch_issue(&self, _ticket_id: u64) -> Result<(), kanban_dto::ApiError> {
            Ok(())
        }
        fn resolve_issue_reference(
            &self,
            reference: &str,
        ) -> Result<Option<(IssueKey, u64)>, kanban_dto::ApiError> {
            if let Ok(id) = reference.parse::<u64>() {
                return Ok(Some((issue_key(id), id)));
            }
            match IssueKey::parse(reference) {
                Some(key) => Ok(Some((key.clone(), key.number().value()))),
                None => Ok(None),
            }
        }

        fn comment_target_project(
            &self,
            _kind: &str,
            _id: &str,
        ) -> Result<Option<u64>, kanban_dto::ApiError> {
            Ok(None)
        }

        fn issue_project(&self, _ticket_id: u64) -> Result<Option<u64>, kanban_dto::ApiError> {
            Ok(Some(1))
        }

        fn comment_target_exists(
            &self,
            _kind: &str,
            _id: &str,
        ) -> Result<bool, kanban_dto::ApiError> {
            Ok(true)
        }
    }

    fn comment_core(
        store: Arc<MemoryCommentStore>,
        projects: Arc<MemoryProjectStore>,
        events: Arc<dyn EventSink>,
    ) -> Core {
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            events,
        );
        core.register_comments(store, projects, Arc::new(MemoryIssueTargets))
            .expect("the comment operations register");
        core
    }

    /// A core with one stored Project (identity 1) and a comment
    /// store, the fixture every comment test shares.
    fn fixture(events: Arc<dyn EventSink>) -> (Arc<MemoryCommentStore>, Core) {
        let store = Arc::new(MemoryCommentStore::default());
        let projects = Arc::new(MemoryProjectStore::default());
        projects.seed(crate::project::testing::stored_project(
            1,
            "CORE",
            "kanban-main",
        ));
        let core = comment_core(store.clone(), projects, events);
        (store, core)
    }

    fn create(text: &str, key: &str, version: u64) -> Value {
        create_for(1, text, key, version)
    }

    fn create_for(project_id: u64, text: &str, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "project_id": project_id,
            "target": target(),
            "text": text,
            "author": "tester",
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
        let (store, core) = fixture(Arc::new(crate::events::NoopEventSink));

        let response = core
            .command("comment.create", &create("First thought", "key-1", 0))
            .expect("the create applies");

        assert_eq!(
            response,
            json!({
                "id": 1,
                "project_id": 1,
                "target": target(),
                "text": "First thought",
                "version": 1,
            })
        );
        assert_eq!(store.snapshot()[0].project_id(), 1);
    }

    #[test]
    fn creating_for_an_unknown_project_is_not_found_without_recording_anything() {
        let (store, core) = fixture(Arc::new(crate::events::NoopEventSink));

        let error = core
            .command("comment.create", &create_for(9, "Ghost", "key-1", 0))
            .expect_err("the unknown Project is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(error.message.contains("project 9"));
        assert!(store.snapshot().is_empty(), "no row may be written");
    }

    #[test]
    fn a_key_addressed_comment_joins_the_issue_id_stream() {
        let (_, core) = fixture(Arc::new(crate::events::NoopEventSink));

        let response = core
            .command(
                "comment.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                    "project_id": 1,
                    "target": { "kind": "ticket", "id": "core-7" },
                    "text": "Keyed",
                "author": "tester",
                }),
            )
            .expect("the create applies");

        assert_eq!(
            response["target"]["id"],
            json!("7"),
            "the stored target is the issue id, not the key"
        );
        for raw in ["7", "core-7", "CORE-7"] {
            let listed = core
                .query(
                    "comment.list",
                    &json!({ "target": { "kind": "ticket", "id": raw }, "limit": 10 }),
                )
                .expect("the list serves");
            assert_eq!(
                listed["comments"].as_array().unwrap().len(),
                1,
                "listing by {raw} reads the same stream"
            );
        }
    }

    #[test]
    fn historical_reference_spellings_list_with_the_resolved_stream() {
        let (store, core) = fixture(Arc::new(crate::events::NoopEventSink));
        for (id, body) in [
            ("11", "Canonical"),
            ("011", "Zero padded"),
            ("+11", "Signed"),
            ("tus-11", "Lowercase key"),
            ("TUS-011", "Padded key"),
        ] {
            store
                .create(
                    1,
                    &CommentTarget::new("ticket", id).expect("the stored target validates"),
                    &CommentText::new_stored(body).expect("the body validates"),
                    "legacy",
                    None,
                )
                .expect("the pre-change row lands");
        }

        for raw in ["11", "TUS-11", "011", "+11", "TUS-011"] {
            let listed = core
                .query(
                    "comment.list",
                    &json!({ "target": { "kind": "ticket", "id": raw }, "limit": 10 }),
                )
                .expect("the list serves");
            let bodies: Vec<_> = listed["comments"]
                .as_array()
                .unwrap()
                .iter()
                .map(|comment| comment["body"].clone())
                .collect();
            assert_eq!(
                bodies,
                vec![
                    json!("Canonical"),
                    json!("Zero padded"),
                    json!("Signed"),
                    json!("Lowercase key"),
                    json!("Padded key"),
                ],
                "listing by {raw} reads every stored spelling of one issue"
            );
        }
    }

    #[test]
    fn historical_reference_spellings_do_not_mix_issue_streams() {
        let (store, core) = fixture(Arc::new(crate::events::NoopEventSink));
        for (id, body) in [
            ("11", "On eleven"),
            ("012", "Twelve, zero padded"),
            ("tus-12", "Twelve, keyed"),
            ("garbage", "Resolves to nothing"),
        ] {
            store
                .create(
                    1,
                    &CommentTarget::new("ticket", id).expect("the stored target validates"),
                    &CommentText::new_stored(body).expect("the body validates"),
                    "legacy",
                    None,
                )
                .expect("the pre-change row lands");
        }

        let eleven = core
            .query(
                "comment.list",
                &json!({ "target": { "kind": "ticket", "id": "11" }, "limit": 10 }),
            )
            .expect("the list serves");
        assert_eq!(
            eleven["comments"].as_array().unwrap().len(),
            1,
            "another issue's rows never join this stream"
        );

        let twelve = core
            .query(
                "comment.list",
                &json!({ "target": { "kind": "ticket", "id": "12" }, "limit": 10 }),
            )
            .expect("the list serves");
        let bodies: Vec<_> = twelve["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|comment| comment["body"].clone())
            .collect();
        assert_eq!(
            bodies,
            vec![json!("Twelve, zero padded"), json!("Twelve, keyed")],
            "issue twelve's own spellings list together"
        );
    }

    #[test]
    fn a_legacy_spelled_root_still_accepts_replies() {
        let (store, core) = fixture(Arc::new(crate::events::NoopEventSink));
        let root = store
            .create(
                1,
                &CommentTarget::new("ticket", "011").expect("the stored target validates"),
                &CommentText::new_stored("Legacy root").expect("the body validates"),
                "legacy",
                None,
            )
            .expect("the pre-change row lands");

        let reply = core
            .command(
                "comment.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                    "project_id": 1,
                    "target": target(),
                    "text": "Reply from the canonical form",
                    "author": "lin",
                    "parent_comment_id": root.id().value(),
                }),
            )
            .expect("a reply joins the legacy root's resolved stream");

        assert_eq!(
            reply["target"]["id"],
            json!("11"),
            "the reply stores the canonical issue id"
        );
    }

    #[test]
    fn an_unresolvable_ticket_target_is_not_found_without_recording_anything() {
        let (store, core) = fixture(Arc::new(crate::events::NoopEventSink));

        let error = core
            .command(
                "comment.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                    "project_id": 1,
                    "target": { "kind": "ticket", "id": "kan-t11" },
                    "text": "Ghost",
                "author": "tester",
                }),
            )
            .expect_err("a target no issue answers to is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(error.message.contains("kan-t11"));
        assert!(store.snapshot().is_empty(), "no row may be written");
    }

    #[test]
    fn a_non_ticket_target_keeps_its_raw_identity() {
        let (_, core) = fixture(Arc::new(crate::events::NoopEventSink));

        let response = core
            .command(
                "comment.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                    "project_id": 1,
                    "target": { "kind": "project", "id": "kanban-main" },
                    "text": "On the project",
                "author": "tester",
                }),
            )
            .expect("the create applies");

        assert_eq!(response["target"]["id"], json!("kanban-main"));
    }
    #[test]
    fn a_non_ticket_stream_matches_ids_exactly() {
        let (store, core) = fixture(Arc::new(crate::events::NoopEventSink));
        let root = core
            .command(
                "comment.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
                    "project_id": 1,
                    "target": { "kind": "project", "id": "Alpha" },
                    "text": "On Alpha",
                "author": "tester",
                }),
            )
            .expect("the create applies");
        let root_id = root["id"].as_u64().unwrap();

        let listed = core
            .query(
                "comment.list",
                &json!({
                    "target": { "kind": "project", "id": "alpha" },
                    "limit": 10,
                }),
            )
            .expect("the list serves");
        assert_eq!(
            listed["comments"].as_array().unwrap().len(),
            0,
            "an id differing only in case is another stream"
        );

        let cross = core
            .command(
                "comment.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-2" },
                    "project_id": 1,
                    "target": { "kind": "project", "id": "alpha" },
                    "text": "Wrong stream",
                "author": "tester",
                    "parent_comment_id": root_id,
                }),
            )
            .expect_err("a parent on an exact-id mismatch is refused");
        assert_eq!(cross.code, ErrorCode::InvalidRequest);
        assert_eq!(cross.field.as_deref(), Some("parent_comment_id"));
        assert_eq!(store.snapshot().len(), 1, "only the Alpha row exists");
    }

    #[test]
    fn editing_appends_a_revision_and_updates_current_text() {
        let (store, core) = fixture(Arc::new(crate::events::NoopEventSink));
        core.command("comment.create", &create("First thought", "key-1", 0))
            .expect("the create applies");

        let response = core
            .command("comment.edit", &edit(1, "Corrected thought", "key-2", 1))
            .expect("the edit applies");

        assert_eq!(
            response,
            json!({
                "id": 1,
                "project_id": 1,
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
        let (_, core) = fixture(Arc::new(crate::events::NoopEventSink));
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
                    "project_id": 1,
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
        let (_, core) = fixture(Arc::new(crate::events::NoopEventSink));
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
        let (_, core) = fixture(Arc::new(crate::events::NoopEventSink));

        let error = core
            .command("comment.edit", &edit(9, "Ghost", "key-1", 0))
            .expect_err("the unknown comment is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(error.message.contains("comment 9"));
    }

    #[test]
    fn blank_text_is_refused_on_create_and_edit_without_recording_anything() {
        let (store, core) = fixture(Arc::new(crate::events::NoopEventSink));

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
    fn a_reply_joins_its_parents_stream_and_nests_one_level() {
        let (_, core) = fixture(Arc::new(crate::events::NoopEventSink));
        let root = core
            .command("comment.create", &create("Root", "key-1", 0))
            .expect("the root applies");
        let root_id = root["id"].as_u64().unwrap();
        let reply = core
            .command(
                "comment.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-2" },
                    "project_id": 1,
                    "target": target(),
                    "text": "Reply",
                "author": "tester",
                    "parent_comment_id": root_id,
                }),
            )
            .expect("the reply applies");
        let reply_id = reply["id"].as_u64().unwrap();

        core.command(
            "comment.create",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "key-3" },
                "project_id": 1,
                "target": { "kind": "ticket", "id": "12" },
                "text": "Elsewhere",
                "author": "tester",
            }),
        )
        .expect("the other-stream create applies");

        let cross = core
            .command(
                "comment.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-4" },
                    "project_id": 1,
                    "target": { "kind": "ticket", "id": "12" },
                    "text": "Wrong stream",
                "author": "tester",
                    "parent_comment_id": root_id,
                }),
            )
            .expect_err("a parent from another stream is refused");
        assert_eq!(cross.code, ErrorCode::InvalidRequest);
        assert_eq!(cross.field.as_deref(), Some("parent_comment_id"));

        let nested = core
            .command(
                "comment.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-5" },
                    "project_id": 1,
                    "target": target(),
                    "text": "Reply to the reply",
                "author": "tester",
                    "parent_comment_id": reply_id,
                }),
            )
            .expect_err("a reply to a reply is refused");
        assert_eq!(nested.code, ErrorCode::InvalidRequest);
        assert_eq!(nested.field.as_deref(), Some("parent_comment_id"));

        let ghost = core
            .command(
                "comment.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-6" },
                    "project_id": 1,
                    "target": target(),
                    "text": "Ghost parent",
                "author": "tester",
                    "parent_comment_id": 99,
                }),
            )
            .expect_err("an unknown parent is refused");
        assert_eq!(ghost.code, ErrorCode::NotFound);
    }

    #[test]
    fn blank_author_is_refused_without_recording_anything() {
        let (store, core) = fixture(Arc::new(crate::events::NoopEventSink));

        for author in [None, Some(""), Some("   ")] {
            let mut payload = create("First thought", "key-1", 0);
            match author {
                None => {
                    payload
                        .as_object_mut()
                        .expect("the payload is an object")
                        .remove("author");
                }
                Some(author) => payload["author"] = json!(author),
            }
            let error = core
                .command("comment.create", &payload)
                .expect_err("a blank author is refused");
            assert_eq!(error.code, ErrorCode::InvalidRequest);
            assert_eq!(error.field.as_deref(), Some("author"));
        }
        assert!(store.snapshot().is_empty(), "no row may be written");
    }

    #[test]
    fn comment_changes_publish_on_the_event_stream() {
        let sink = Arc::new(RecordingSink::default());
        let (_, core) = fixture(sink.clone());
        core.command("comment.create", &create("First thought", "key-1", 0))
            .expect("the create applies");

        let events = sink.events.lock().expect("the recorder lock is sound");
        assert_eq!(
            *events,
            vec![(
                "comment.created".to_owned(),
                json!({
                    "id": 1,
                    "project_id": 1,
                    "target": target(),
                    "text": "First thought",
                    "version": 1,
                })
            )]
        );
    }

    #[test]
    fn revisions_query_rejects_unknown_fields() {
        let (_, core) = fixture(Arc::new(crate::events::NoopEventSink));

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
            vec![
                "comment.create",
                "comment.edit",
                "comment.revisions",
                "comment.list",
                "comment.resolve",
            ]
        );
    }

    #[test]
    fn list_query_returns_comments_oldest_first_with_cursor() {
        let (_, core) = fixture(Arc::new(crate::events::NoopEventSink));
        core.command("comment.create", &create("First thought", "key-1", 0))
            .expect("the create applies");
        core.command("comment.create", &create("Second thought", "key-2", 0))
            .expect("the second create applies");

        let page = core
            .query("comment.list", &json!({ "target": target(), "limit": 1 }))
            .expect("the list query serves");
        assert_eq!(page["comments"].as_array().unwrap().len(), 1);
        assert_eq!(page["comments"][0]["body"], "First thought");
        assert_eq!(page["next_cursor"].as_str(), Some("1"));

        let rest = core
            .query(
                "comment.list",
                &json!({ "target": target(), "limit": 10, "cursor": "1" }),
            )
            .expect("the second page serves");
        assert_eq!(rest["comments"].as_array().unwrap().len(), 1);
        assert_eq!(rest["comments"][0]["body"], "Second thought");
        assert!(rest["next_cursor"].is_null());
    }
    #[test]
    fn resolution_changes_never_mark_the_body_edited() {
        let (_, core) = fixture(Arc::new(crate::events::NoopEventSink));
        core.command("comment.create", &create("First thought", "key-1", 0))
            .expect("the create applies");

        let listed = || {
            core.query("comment.list", &json!({ "target": target(), "limit": 10 }))
                .expect("the list serves")
        };

        assert!(
            listed()["comments"][0]["edited_at"].is_null(),
            "the original revision is not an edit"
        );

        for (version, resolved) in [(1u64, true), (2, false), (3, true)] {
            let response = core
                .command(
                    "comment.resolve",
                    &json!({
                        "mutation": {
                            "optimistic_version": version,
                            "idempotency_key": format!("resolve-{version}"),
                        },
                        "comment_id": 1,
                        "resolved": resolved,
                    }),
                )
                .expect("the resolution change applies");
            assert_eq!(response["version"], json!(version + 1));
            let row = listed();
            assert_eq!(row["comments"][0]["version"], json!(version + 1));
            assert!(
                row["comments"][0]["edited_at"].is_null(),
                "a resolution change is not a body edit"
            );
        }

        core.command("comment.edit", &edit(1, "Corrected thought", "key-edit", 4))
            .expect("the edit applies");
        let edited = listed();
        assert!(
            edited["comments"][0]["edited_at"].is_string(),
            "an actual edit reports the latest revision's stamp"
        );

        core.command(
            "comment.resolve",
            &json!({
                "mutation": {
                    "optimistic_version": 5,
                    "idempotency_key": "resolve-after-edit",
                },
                "comment_id": 1,
                "resolved": false,
            }),
        )
        .expect("the unresolve applies");
        let after = listed();
        assert_eq!(
            after["comments"][0]["edited_at"], edited["comments"][0]["edited_at"],
            "resolution leaves the edit stamp alone"
        );
        assert_eq!(after["comments"][0]["version"], json!(6));
    }
}
