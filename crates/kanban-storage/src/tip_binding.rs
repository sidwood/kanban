//! Durable criterion evidence bindings.

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;
use kanban_app::tip_binding::CriterionBindingStore;
use kanban_domain::{CriterionBinding, CriterionKind, EvidenceReview};
use kanban_dto::ApiError;
use rusqlite::{OptionalExtension, params};

pub struct SqliteCriterionBindingStore {
    conn: ConnectionHandle,
}

impl SqliteCriterionBindingStore {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&error.to_string())
}

fn kind_wire(kind: CriterionKind) -> &'static str {
    match kind {
        CriterionKind::Acceptance => "acceptance",
        CriterionKind::Task => "task",
        CriterionKind::Walkthrough => "walkthrough",
    }
}

fn review_wire(review: EvidenceReview) -> &'static str {
    match review {
        EvidenceReview::Pending => "pending",
        EvidenceReview::Validated => "validated",
        EvidenceReview::Rejected => "rejected",
    }
}

fn decode(
    kind: String,
    criterion_index: u64,
    evidence_id: u64,
    tip: String,
    review: String,
    satisfied: i64,
    void: i64,
) -> Result<CriterionBinding, ApiError> {
    let kind = match kind.as_str() {
        "acceptance" => CriterionKind::Acceptance,
        "task" => CriterionKind::Task,
        "walkthrough" => CriterionKind::Walkthrough,
        _ => return Err(ApiError::internal("unknown criterion kind")),
    };
    let review = match review.as_str() {
        "pending" => EvidenceReview::Pending,
        "validated" => EvidenceReview::Validated,
        "rejected" => EvidenceReview::Rejected,
        _ => return Err(ApiError::internal("unknown evidence review")),
    };
    Ok(CriterionBinding::restore(
        kind,
        criterion_index,
        evidence_id,
        tip,
        review,
        satisfied != 0,
        void != 0,
    ))
}

impl CriterionBindingStore for SqliteCriterionBindingStore {
    fn save(
        &self,
        ticket_id: u64,
        binding: &CriterionBinding,
        envelope: kanban_app::TimelineEnvelope,
    ) -> Result<(), ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        refuse_satisfaction_without_review(&span, ticket_id, binding)?;
        span.execute(
            "INSERT INTO criterion_bindings(
                 ticket_id, criterion_index, kind, evidence_id, tip, review, satisfied, void
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(ticket_id, criterion_index) DO UPDATE SET
                 kind=excluded.kind,
                 evidence_id=excluded.evidence_id,
                 tip=excluded.tip,
                 review=excluded.review,
                 satisfied=excluded.satisfied,
                 void=excluded.void",
            params![
                ticket_id as i64,
                binding.criterion_index() as i64,
                kind_wire(binding.kind()),
                binding.evidence_id() as i64,
                binding.tip(),
                review_wire(binding.review()),
                if binding.satisfied() { 1 } else { 0 },
                if binding.void() { 1 } else { 0 },
            ],
        )
        .map_err(internal)?;
        insert_event(&span, &envelope).map_err(internal)?;
        span.commit().map_err(internal)
    }

    fn find(
        &self,
        ticket_id: u64,
        criterion_index: u64,
    ) -> Result<Option<CriterionBinding>, ApiError> {
        let conn = self.conn.lock();
        let mut statement = conn
            .prepare(
                "SELECT kind, criterion_index, evidence_id, tip, review, satisfied, void
                 FROM criterion_bindings WHERE ticket_id=?1 AND criterion_index=?2",
            )
            .map_err(internal)?;
        let mut rows = statement
            .query(params![ticket_id as i64, criterion_index as i64])
            .map_err(internal)?;
        match rows.next().map_err(internal)? {
            Some(row) => Ok(Some(decode(
                row.get(0).map_err(internal)?,
                row.get::<_, i64>(1).map_err(internal)? as u64,
                row.get::<_, i64>(2).map_err(internal)? as u64,
                row.get(3).map_err(internal)?,
                row.get(4).map_err(internal)?,
                row.get(5).map_err(internal)?,
                row.get(6).map_err(internal)?,
            )?)),
            None => Ok(None),
        }
    }

    fn list(&self, ticket_id: u64) -> Result<Vec<CriterionBinding>, ApiError> {
        let conn = self.conn.lock();
        let mut statement = conn
            .prepare(
                "SELECT kind, criterion_index, evidence_id, tip, review, satisfied, void
                 FROM criterion_bindings WHERE ticket_id=?1 ORDER BY criterion_index",
            )
            .map_err(internal)?;
        let rows = statement
            .query_map(params![ticket_id as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })
            .map_err(internal)?;
        let mut bindings = Vec::new();
        for row in rows {
            let (kind, index, evidence_id, tip, review, satisfied, void) = row.map_err(internal)?;
            bindings.push(decode(
                kind,
                index,
                evidence_id,
                tip,
                review,
                satisfied,
                void,
            )?);
        }
        Ok(bindings)
    }

    fn list_for_workspace(
        &self,
        workspace_id: u64,
    ) -> Result<Vec<(u64, CriterionBinding)>, ApiError> {
        let conn = self.conn.lock();
        let mut statement = conn
            .prepare(
                "SELECT cb.ticket_id, cb.kind, cb.criterion_index, cb.evidence_id,
                        cb.tip, cb.review, cb.satisfied, cb.void
                 FROM criterion_bindings cb
                 INNER JOIN lanes l ON l.ticket_id = cb.ticket_id
                 WHERE l.workspace_id = ?1
                 ORDER BY cb.ticket_id, cb.criterion_index",
            )
            .map_err(internal)?;
        let rows = statement
            .query_map(params![workspace_id as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, i64>(3)? as u64,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })
            .map_err(internal)?;
        let mut bindings = Vec::new();
        for row in rows {
            let (ticket_id, kind, index, evidence_id, tip, review, satisfied, void) =
                row.map_err(internal)?;
            bindings.push((
                ticket_id,
                decode(kind, index, evidence_id, tip, review, satisfied, void)?,
            ));
        }
        Ok(bindings)
    }

    fn record_voided_approval(
        &self,
        ticket_id: u64,
        review_id: u64,
        tip: &str,
    ) -> Result<(), ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        span.execute(
            "INSERT INTO voided_approvals(review_id, ticket_id, tip)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(review_id) DO NOTHING",
            params![review_id as i64, ticket_id as i64, tip],
        )
        .map_err(internal)?;
        span.commit().map_err(internal)
    }

    fn approval_is_voided(&self, review_id: u64) -> Result<bool, ApiError> {
        let conn = self.conn.lock();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM voided_approvals WHERE review_id=?1",
                params![review_id as i64],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        Ok(found.is_some())
    }
}

fn refuse_satisfaction_without_review(
    conn: &rusqlite::Connection,
    ticket_id: u64,
    binding: &CriterionBinding,
) -> Result<(), ApiError> {
    if !binding.satisfied() {
        return Ok(());
    }
    if binding.kind() == CriterionKind::Task && binding.tip().is_empty() {
        return Ok(());
    }
    let latest = conn
        .query_row(
            "SELECT status, tip, id FROM review_executions
             WHERE ticket_id = ?1
             ORDER BY id DESC
             LIMIT 1",
            params![ticket_id as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(internal)?;
    match latest {
        Some((status, tip, id)) if status == "approved" && tip == binding.tip() => {
            let voided: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM voided_approvals WHERE review_id=?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(internal)?;
            if voided.is_some() {
                return Err(ApiError::invalid_request(
                    "a content change voided the outstanding approval",
                ));
            }
            Ok(())
        }
        _ => Err(ApiError::invalid_request(
            "a criterion is satisfied only through a completed required-stage review at the bound tip",
        )),
    }
}

#[cfg(test)]
mod tests {
    use kanban_app::TimelineEnvelope;
    use kanban_app::tip_binding::CriterionBindingStore;
    use kanban_domain::{
        CriterionKind, EvidenceReview, attach_criterion_evidence, review_criterion_evidence,
        satisfy_at_approved_tip,
    };
    use kanban_dto::{ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind};
    use serde_json::json;

    use super::SqliteCriterionBindingStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    fn envelope() -> TimelineEnvelope {
        TimelineEnvelope::project(
            1,
            TimelineEventKind::Evidence,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Ticket,
                id: "1".to_owned(),
            }),
            json!({ "action": "satisfied" }),
        )
    }

    fn seed_task(database: &Database) {
        let conn = database.connection();
        conn.execute(
            "INSERT INTO projects
                 (code, name, repository, seed_workspace, default_branch,
                  herdr_workspace, herdr_session, archived, version)
             VALUES ('CORE', 'Control plane', '/repositories/kanban',
                     '/workspaces/kanban.seed', 'main', 'kanban.seed', 'kanban-main', 0, 1)",
            [],
        )
        .expect("the fixture Project lands");
        conn.execute(
            "INSERT INTO tickets
                 (project_id, number, kind, priority, state, title, criteria,
                  subtype, mode, completion, version)
             VALUES (1, 1, 'task', 'normal', 'ready', 'One slice', '[]',
                     'operational', 'agent', '[\"done\"]', 1)",
            [],
        )
        .expect("the fixture Ticket lands");
    }

    fn satisfied_acceptance() -> kanban_domain::CriterionBinding {
        let mut binding =
            attach_criterion_evidence(CriterionKind::Acceptance, 0, 7, "a".repeat(40))
                .expect("implementers attach evidence");
        review_criterion_evidence(&mut binding, EvidenceReview::Validated)
            .expect("reviewers validate attached evidence");
        satisfy_at_approved_tip(&mut binding, &"a".repeat(40))
            .expect("the domain marks satisfaction");
        binding
    }

    #[test]
    fn tip_binding_save_refuses_satisfaction_without_an_approved_review() {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        seed_task(&database);
        let store = SqliteCriterionBindingStore::new(&database);

        let error = store
            .save(1, &satisfied_acceptance(), envelope())
            .expect_err("storage must not persist satisfaction without a review join");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn tip_binding_records_a_voided_historical_approval() {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        seed_task(&database);
        let store = SqliteCriterionBindingStore::new(&database);

        store
            .record_voided_approval(1, 9, &"a".repeat(40))
            .expect("a voided approval is durable");

        assert!(
            store
                .approval_is_voided(9)
                .expect("the voided approval is readable"),
            "content invalidation must persist against the approval identity"
        );
        assert!(
            !store
                .approval_is_voided(8)
                .expect("an unknown approval is not voided"),
            "a different review stays live"
        );
    }
}
