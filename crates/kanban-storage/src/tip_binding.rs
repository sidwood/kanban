//! Durable criterion evidence bindings.

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;
use kanban_app::tip_binding::CriterionBindingStore;
use kanban_domain::{CriterionBinding, CriterionKind, EvidenceReview};
use kanban_dto::ApiError;
use rusqlite::params;

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
}
