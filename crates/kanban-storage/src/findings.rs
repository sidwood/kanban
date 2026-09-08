//! Read immutable finding custody from its authoritative review verdict.
use crate::db::{ConnectionHandle, Database, WriteSpan};
use kanban_app::findings::{FindingStore, blocks_approval, validate_findings};
use kanban_domain::finding::FindingIdentity;
use kanban_dto::{ApiError, FindingListQuery, FindingRecord, ReviewVerdictRecord};
use rusqlite::{Connection, OptionalExtension, params};

pub struct SqliteFindingStore {
    conn: ConnectionHandle,
}
impl SqliteFindingStore {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }
}
impl FindingStore for SqliteFindingStore {
    fn promotion(
        &self,
        project_id: u64,
        id: FindingIdentity,
    ) -> Result<Option<kanban_dto::DeferralPromotionRecord>, ApiError> {
        stored_promotion(&self.conn.lock(), project_id, &id.to_string())
    }
    fn record_promotion(
        &self,
        record: &kanban_dto::DeferralPromotionRecord,
        envelope: kanban_app::TimelineEnvelope,
    ) -> Result<(), ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        span.execute("INSERT INTO finding_promotions(project_id,finding_id,deferral_id,ticket_id) VALUES (?1,?2,?3,?4)",
            params![record.project_id as i64,record.finding_id,record.deferral_id as i64,record.ticket_id as i64]).map_err(|error| {
                if error.to_string().contains("finding already promoted") {ApiError::invalid_request("the finding is already promoted")} else {internal(error)}
            })?;
        crate::timeline::insert_event(&span, &envelope).map_err(internal)?;
        span.commit().map_err(internal)
    }

    fn list(&self, query: &FindingListQuery) -> Result<Vec<FindingRecord>, ApiError> {
        let conn = self.conn.lock();
        let mut statement = conn
            .prepare(
                "SELECT e.id,e.ticket_id,e.tip,s.id,v.record FROM review_executions e
            JOIN review_slots s ON s.review_id=e.id JOIN review_slot_verdicts v ON v.slot_id=s.id
            WHERE e.project_id=?1 AND (?2 IS NULL OR e.id=?2) ORDER BY e.id,s.id",
            )
            .map_err(internal)?;
        let rows = statement
            .query_map(
                params![query.project_id as i64, query.review_id.map(|id| id as i64)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)? as u64,
                        row.get::<_, i64>(1)? as u64,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)? as u64,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .map_err(internal)?;
        let mut records = Vec::new();
        for row in rows {
            let (review_id, ticket_id, tip, slot_id, json) = row.map_err(internal)?;
            let verdict: ReviewVerdictRecord = serde_json::from_str(&json).map_err(internal)?;
            validate_findings(&verdict.findings)?;
            for (index, finding) in verdict.findings.into_iter().enumerate() {
                let id =
                    FindingIdentity::new(review_id, slot_id, index as u64).map_err(internal)?;
                let blocking = blocks_approval(&finding);
                records.push(FindingRecord {
                    promotion: stored_promotion(&conn, query.project_id, &id.to_string())?,
                    id: id.to_string(),
                    project_id: query.project_id,
                    review_id,
                    ticket_id,
                    slot_id,
                    submission_id: verdict.submission_id,
                    tip: tip.clone(),
                    counts_for_resolution: verdict.counts_for_resolution,
                    finding,
                    blocking,
                });
            }
        }
        Ok(records)
    }
    fn find(
        &self,
        project_id: u64,
        id: FindingIdentity,
    ) -> Result<Option<FindingRecord>, ApiError> {
        Ok(self
            .list(&FindingListQuery {
                project_id,
                review_id: Some(id.review()),
            })?
            .into_iter()
            .find(|record| record.id == id.to_string()))
    }
}
fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&error.to_string())
}

fn stored_promotion(
    conn: &Connection,
    project_id: u64,
    id: &str,
) -> Result<Option<kanban_dto::DeferralPromotionRecord>, ApiError> {
    conn.query_row("SELECT deferral_id,ticket_id FROM finding_promotions WHERE project_id=?1 AND finding_id=?2",params![project_id as i64,id],|row| {
        Ok(kanban_dto::DeferralPromotionRecord{project_id,finding_id:id.to_owned(),deferral_id:row.get::<_,i64>(0)? as u64,ticket_id:row.get::<_,i64>(1)? as u64})
    }).optional().map_err(internal)
}
