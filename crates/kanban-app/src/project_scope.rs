//! The unified Project timeline identity (KAN-T79-AC1, KAN-T79-AC2):
//! every project-scoped command resolves one numeric Project identity
//! through the Project store before any durable write, and every
//! writer derives the same canonical timeline scope from that
//! identity. These tests drive the whole application core the way the
//! service wires it, capturing what each store was asked to land.

use std::sync::{Arc, Mutex};

use kanban_domain::{
    Comment, CommentId, CommentTarget, CommentText, CommitIdentity, ContentHash, Deferral,
    DeferralDraft, DeferralId, EvidenceId, EvidenceItem, EvidenceKind, EvidenceShape, RelativePath,
    Ruling, RulingDraft, RulingId,
};
use kanban_dto::{
    ApiError, CommentRecord, CommentRevisionRecord, DeferralListQuery, RulingListQuery,
    TimelineEntityKind, TimelineEntityRef, TimelineScope,
};
use serde_json::{Value, json};

use crate::comment::CommentStore;
use crate::deferrals::DeferralStore;
use crate::dispatch::Core;
use crate::events::NoopEventSink;
use crate::evidence::{EvidenceFilter, EvidenceStore};
use crate::mutation::MemoryIdempotencyStore;
use crate::project::testing::{MemoryProjectStore, stored_project};
use crate::rulings::RulingStore;
use crate::timeline::{TimelineEnvelope, TimelineFacts};

/// Everything the project-scoped commands asked storage to land, so
/// the tests can see which Project identity each write carried.
#[derive(Default)]
struct Recorded {
    comments: Mutex<Vec<u64>>,
    rulings: Mutex<Vec<u64>>,
    deferrals: Mutex<Vec<u64>>,
    evidence: Mutex<Vec<u64>>,
    list_scopes: Mutex<Vec<TimelineScope>>,
}

/// The Comment port that records the Project identity of each write.
struct RecordingComments {
    recorded: Arc<Recorded>,
    rows: Mutex<Vec<Comment>>,
}

impl CommentStore for RecordingComments {
    fn create(
        &self,
        project_id: u64,
        _target: &CommentTarget,
        _text: &CommentText,
    ) -> Result<Comment, ApiError> {
        let id = self.rows.lock().expect("the rows lock is sound").len() as u64 + 1;
        let comment = Comment::create(
            CommentId::new(id),
            project_id,
            CommentTarget::new("ticket", "kan-t10").expect("the target validates"),
            CommentText::new("noted").expect("the text validates"),
        );
        self.rows
            .lock()
            .expect("the rows lock is sound")
            .push(comment.clone());
        self.recorded
            .comments
            .lock()
            .expect("the recorder lock is sound")
            .push(project_id);
        Ok(comment)
    }

    fn find(&self, id: CommentId) -> Result<Option<Comment>, ApiError> {
        Ok(self
            .rows
            .lock()
            .expect("the rows lock is sound")
            .iter()
            .find(|row| row.id() == id)
            .cloned())
    }

    fn save(&self, comment: &Comment) -> Result<(), ApiError> {
        self.rows
            .lock()
            .expect("the rows lock is sound")
            .push(comment.clone());
        Ok(())
    }

    fn revisions(
        &self,
        id: CommentId,
    ) -> Result<(CommentRecord, Vec<CommentRevisionRecord>), ApiError> {
        let comment = self
            .find(id)?
            .ok_or_else(|| ApiError::not_found(&format!("comment {}", id.value())))?;
        Ok((
            CommentRecord {
                id: comment.id().value(),
                project_id: comment.project_id(),
                target: TimelineEntityRef {
                    kind: TimelineEntityKind::Ticket,
                    id: "kan-t10".to_owned(),
                },
                text: comment.current_text().as_str().to_owned(),
                version: comment.version(),
            },
            Vec::new(),
        ))
    }
}

/// The Ruling port that records the Project identity of each insert.
struct RecordingRulings {
    recorded: Arc<Recorded>,
    rows: Mutex<Vec<Ruling>>,
}

impl RulingStore for RecordingRulings {
    fn insert(&self, draft: &RulingDraft, _facts: TimelineFacts) -> Result<Ruling, ApiError> {
        let mut rows = self.rows.lock().expect("the rows lock is sound");
        let id = rows.len() as u64 + 1;
        let ruling = Ruling::restore(
            RulingId::new(id),
            draft.project_id,
            draft.entity.clone(),
            draft.summary.clone(),
            draft.supersedes,
            format!("2026-09-05T12:00:{id:02}Z"),
        );
        rows.push(ruling.clone());
        self.recorded
            .rulings
            .lock()
            .expect("the recorder lock is sound")
            .push(draft.project_id);
        Ok(ruling)
    }

    fn find(&self, project_id: u64, id: RulingId) -> Result<Option<Ruling>, ApiError> {
        Ok(self
            .rows
            .lock()
            .expect("the rows lock is sound")
            .iter()
            .find(|row| row.project_id() == project_id && row.id() == id)
            .cloned())
    }

    fn list(&self, query: &RulingListQuery) -> Result<Vec<Ruling>, ApiError> {
        Ok(self
            .rows
            .lock()
            .expect("the rows lock is sound")
            .iter()
            .filter(|row| row.project_id() == query.project_id)
            .cloned()
            .collect())
    }
}

/// The Deferral port that records the Project identity of each
/// insert.
struct RecordingDeferrals {
    recorded: Arc<Recorded>,
    rows: Mutex<Vec<Deferral>>,
}

impl DeferralStore for RecordingDeferrals {
    fn insert(&self, draft: &DeferralDraft, _facts: TimelineFacts) -> Result<Deferral, ApiError> {
        let mut rows = self.rows.lock().expect("the rows lock is sound");
        let id = rows.len() as u64 + 1;
        let deferral = Deferral::restore(
            DeferralId::new(id),
            draft.project_id,
            draft.finding_id.clone(),
            draft.reason.clone(),
            draft.supersedes,
            format!("2026-09-05T12:00:{id:02}Z"),
        );
        rows.push(deferral.clone());
        self.recorded
            .deferrals
            .lock()
            .expect("the recorder lock is sound")
            .push(draft.project_id);
        Ok(deferral)
    }

    fn find(&self, project_id: u64, id: DeferralId) -> Result<Option<Deferral>, ApiError> {
        Ok(self
            .rows
            .lock()
            .expect("the rows lock is sound")
            .iter()
            .find(|row| row.project_id() == project_id && row.id() == id)
            .cloned())
    }

    fn list(&self, query: &DeferralListQuery) -> Result<Vec<Deferral>, ApiError> {
        Ok(self
            .rows
            .lock()
            .expect("the rows lock is sound")
            .iter()
            .filter(|row| row.project_id() == query.project_id)
            .cloned()
            .collect())
    }
}

/// The Evidence port that records the Project identity of each write
/// and the scope each list envelope carried.
struct RecordingEvidence {
    recorded: Arc<Recorded>,
    items: Mutex<Vec<EvidenceItem>>,
}

impl EvidenceStore for RecordingEvidence {
    fn attach_managed_file(
        &self,
        project_id: u64,
        _entity_kind: &str,
        _entity_id: &str,
        _content_base64: &str,
        _facts: TimelineFacts,
    ) -> Result<EvidenceItem, ApiError> {
        Ok(self.restore(project_id, EvidenceKind::ManagedFile))
    }

    fn attach_repository(
        &self,
        project_id: u64,
        _entity_kind: &str,
        _entity_id: &str,
        _relative_path: &RelativePath,
        _commit_identity: &CommitIdentity,
        _facts: TimelineFacts,
    ) -> Result<EvidenceItem, ApiError> {
        Ok(self.restore(project_id, EvidenceKind::Repository))
    }

    fn list(
        &self,
        filter: &EvidenceFilter,
        envelope: TimelineEnvelope,
    ) -> Result<Vec<EvidenceItem>, ApiError> {
        self.recorded
            .evidence
            .lock()
            .expect("the recorder lock is sound")
            .push(filter.project_id);
        self.recorded
            .list_scopes
            .lock()
            .expect("the recorder lock is sound")
            .push(*envelope.scope());
        Ok(self
            .items
            .lock()
            .expect("the items lock is sound")
            .iter()
            .filter(|item| item.project_id() == filter.project_id)
            .cloned()
            .collect())
    }
}

impl RecordingEvidence {
    /// Mint one stored item for `project_id` and record the write.
    fn restore(&self, project_id: u64, kind: EvidenceKind) -> EvidenceItem {
        let mut items = self.items.lock().expect("the items lock is sound");
        let id = items.len() as u64 + 1;
        let hash = ContentHash::new(&"a".repeat(64)).expect("the fixture hash validates");
        let shape = EvidenceShape {
            project_id,
            entity_kind: "ticket".to_owned(),
            entity_id: "kan-t10".to_owned(),
            kind,
            content_hash: (kind == EvidenceKind::ManagedFile).then_some(hash),
            relative_path: (kind == EvidenceKind::Repository)
                .then(|| RelativePath::new("docs/spec.md").expect("the fixture path validates")),
            commit_identity: (kind == EvidenceKind::Repository)
                .then(|| CommitIdentity::new("deadbeef").expect("the fixture commit validates")),
        };
        let item =
            EvidenceItem::restore(EvidenceId::new(id), shape).expect("the fixture shape validates");
        items.push(item.clone());
        self.recorded
            .evidence
            .lock()
            .expect("the recorder lock is sound")
            .push(project_id);
        item
    }
}

/// A core wired like the service wires it: one Project store shared
/// by every project-scoped operation, and recorders behind each
/// entity port.
fn harness() -> (Arc<Recorded>, Core) {
    let recorded = Arc::new(Recorded::default());
    let projects = Arc::new(MemoryProjectStore::default());
    projects.seed(stored_project(1, "CORE", "kanban-main"));
    let mut core = Core::new(
        crate::catalog::exposed_operations(),
        Arc::new(MemoryIdempotencyStore::new()),
        Arc::new(NoopEventSink),
    );
    core.register_comments(
        Arc::new(RecordingComments {
            recorded: recorded.clone(),
            rows: Mutex::new(Vec::new()),
        }),
        projects.clone(),
    )
    .expect("the comment operations register");
    core.register_rulings(
        Arc::new(RecordingRulings {
            recorded: recorded.clone(),
            rows: Mutex::new(Vec::new()),
        }),
        projects.clone(),
    )
    .expect("the ruling operations register");
    core.register_deferrals(
        Arc::new(RecordingDeferrals {
            recorded: recorded.clone(),
            rows: Mutex::new(Vec::new()),
        }),
        projects.clone(),
    )
    .expect("the deferral operations register");
    core.register_evidence(
        Arc::new(RecordingEvidence {
            recorded: recorded.clone(),
            items: Mutex::new(Vec::new()),
        }),
        projects,
    )
    .expect("the evidence operations register");
    (recorded, core)
}

/// The mutation header every command payload carries.
fn mutation(key: &str) -> Value {
    json!({ "optimistic_version": 0, "idempotency_key": key })
}

#[cfg(test)]
mod contract {
    use kanban_dto::ErrorCode;
    use serde_json::json;

    use super::{harness, mutation};

    #[test]
    fn every_command_lands_its_write_under_the_resolved_project() {
        let (recorded, core) = harness();

        core.command(
            "comment.create",
            &json!({
                "mutation": mutation("comment"),
                "project_id": 1,
                "target": { "kind": "ticket", "id": "kan-t10" },
                "text": "One scope",
            }),
        )
        .expect("the comment applies");
        core.command(
            "ruling.record",
            &json!({
                "mutation": mutation("ruling"),
                "project_id": 1,
                "summary": "Allow landing",
            }),
        )
        .expect("the ruling applies");
        core.command(
            "deferral.record",
            &json!({
                "mutation": mutation("deferral"),
                "project_id": 1,
                "finding_id": "finding-1",
                "reason": "Cosmetic only",
            }),
        )
        .expect("the deferral applies");
        core.command(
            "evidence.attach",
            &json!({
                "mutation": mutation("attach"),
                "project_id": 1,
                "entity_kind": "ticket",
                "entity_id": "kan-t10",
                "evidence_kind": "managed_file",
                "content_base64": "cHJvb2Y=",
            }),
        )
        .expect("the attach applies");
        core.command(
            "evidence.list",
            &json!({
                "mutation": mutation("list"),
                "project_id": 1,
            }),
        )
        .expect("the list applies");

        let guard = recorded;
        assert_eq!(
            *guard.comments.lock().expect("the recorder lock is sound"),
            vec![1],
            "the comment write carries the resolved identity"
        );
        assert_eq!(
            *guard.rulings.lock().expect("the recorder lock is sound"),
            vec![1],
            "the ruling write carries the resolved identity"
        );
        assert_eq!(
            *guard.deferrals.lock().expect("the recorder lock is sound"),
            vec![1],
            "the deferral write carries the resolved identity"
        );
        assert_eq!(
            *guard.evidence.lock().expect("the recorder lock is sound"),
            vec![1, 1],
            "attach and list both carry the resolved identity"
        );
        assert_eq!(
            *guard
                .list_scopes
                .lock()
                .expect("the recorder lock is sound"),
            vec![kanban_dto::TimelineScope::Project(1)],
            "the timeline scope derives from the resolved numeric identity"
        );
    }

    #[test]
    fn every_command_refuses_an_unresolved_project_without_writing() {
        let (recorded, core) = harness();

        for (operation, payload) in [
            (
                "comment.create",
                json!({
                    "mutation": mutation("comment"),
                    "project_id": 9,
                    "target": { "kind": "ticket", "id": "kan-t10" },
                    "text": "Ghost",
                }),
            ),
            (
                "ruling.record",
                json!({
                    "mutation": mutation("ruling"),
                    "project_id": 9,
                    "summary": "Hold",
                }),
            ),
            (
                "deferral.record",
                json!({
                    "mutation": mutation("deferral"),
                    "project_id": 9,
                    "finding_id": "finding-1",
                    "reason": "Cosmetic only",
                }),
            ),
            (
                "evidence.attach",
                json!({
                    "mutation": mutation("attach"),
                    "project_id": 9,
                    "entity_kind": "ticket",
                    "entity_id": "kan-t10",
                    "evidence_kind": "managed_file",
                    "content_base64": "cHJvb2Y=",
                }),
            ),
            (
                "evidence.list",
                json!({
                    "mutation": mutation("list"),
                    "project_id": 9,
                }),
            ),
        ] {
            let error = core
                .command(operation, &payload)
                .expect_err("an unresolved Project is refused");
            assert_eq!(error.code, ErrorCode::NotFound, "`{operation}` refuses");
            assert!(
                error.message.contains("project 9"),
                "`{operation}` names the Project: {}",
                error.message
            );
        }

        for (operation, payload) in [
            ("ruling.list", json!({ "project_id": 9 })),
            ("deferral.list", json!({ "project_id": 9 })),
        ] {
            let refusal = core
                .query(operation, &payload)
                .expect_err("an unresolved Project is refused");
            assert_eq!(refusal.code, ErrorCode::NotFound, "`{operation}` refuses");
            assert!(
                refusal.message.contains("project 9"),
                "`{operation}` names the Project: {}",
                refusal.message
            );
        }

        assert!(
            recorded
                .comments
                .lock()
                .expect("the recorder lock is sound")
                .is_empty()
        );
        assert!(
            recorded
                .rulings
                .lock()
                .expect("the recorder lock is sound")
                .is_empty()
        );
        assert!(
            recorded
                .deferrals
                .lock()
                .expect("the recorder lock is sound")
                .is_empty()
        );
        assert!(
            recorded
                .evidence
                .lock()
                .expect("the recorder lock is sound")
                .is_empty()
        );
        assert!(
            recorded
                .list_scopes
                .lock()
                .expect("the recorder lock is sound")
                .is_empty()
        );
    }

    #[test]
    fn the_wire_accepts_project_references_only_as_numbers() {
        let (_recorded, core) = harness();

        let error = core
            .command(
                "comment.create",
                &json!({
                    "mutation": mutation("comment"),
                    "project_id": "kan",
                    "target": { "kind": "ticket", "id": "kan-t10" },
                    "text": "String scope",
                }),
            )
            .expect_err("a string Project reference is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }
}
