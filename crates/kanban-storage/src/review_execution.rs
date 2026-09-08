//! Review persistence and atomic projection of authoritative verdicts.
use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;
use kanban_app::review_execution::{
    ReviewExecutionDraft, ReviewExecutionStore, ReviewSlotDraft, active_review_slot as active_slot,
    review_stage_status, review_transition, validate_review_tip as validate_tip,
};
use kanban_dto::*;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Clone)]
pub struct SqliteReviewExecutionStore {
    conn: ConnectionHandle,
}
impl SqliteReviewExecutionStore {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }
}

impl ReviewExecutionStore for SqliteReviewExecutionStore {
    fn start(&self, draft: &ReviewExecutionDraft) -> Result<ReviewExecutionRecord, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        span.execute("INSERT INTO review_executions (project_id,ticket_id,submission_id,tip,configuration_version,priority,version,status)
            VALUES (?1,?2,?3,?4,?5,?6,1,'in_progress')",params![draft.project_id as i64,draft.ticket_id as i64,draft.submission_id as i64,draft.tip,draft.configuration_version as i64,draft.priority]).map_err(internal)?;
        let id = span.last_insert_rowid() as u64;
        for (stage_index, stage) in draft.stages.iter().enumerate() {
            for (slot_index, slot) in stage.iter().enumerate() {
                span.execute("INSERT INTO review_slots (review_id,stage_index,slot_index,snapshot) VALUES (?1,?2,?3,?4)",
                    params![id as i64,stage_index as i64,slot_index as i64,serde_json::to_string(slot).map_err(internal)?]).map_err(internal)?;
            }
        }
        let record = advance(&span, id)?;
        insert_event(&span, &review_transition(&record, "review_started")).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(record)
    }
    fn find(&self, id: u64) -> Result<Option<ReviewExecutionRecord>, ApiError> {
        load(&self.conn.lock(), id)
    }
    fn latest_for_ticket(&self, ticket_id: u64) -> Result<Option<ReviewExecutionRecord>, ApiError> {
        let conn = self.conn.lock();
        let id = conn
            .query_row(
                "SELECT id FROM review_executions WHERE ticket_id=?1 ORDER BY id DESC LIMIT 1",
                params![ticket_id as i64],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(internal)?;
        id.map(|id| load(&conn, id as u64))
            .transpose()
            .map(|record| record.flatten())
    }
    fn human_verdict(
        &self,
        request: &ReviewHumanSubmitRequest,
    ) -> Result<ReviewExecutionRecord, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let review =
            load(&span, request.review_id)?.ok_or_else(|| ApiError::not_found("review"))?;
        let slot = active_slot(&review, request.slot_id)?;
        if slot.occupant != (TicketReviewOccupant::Human {}) {
            return Err(ApiError::invalid_request(
                "agent slots require their scoped run submission",
            ));
        }
        validate_tip(&review, &request.tip)?;
        kanban_app::findings::validate_review_verdict(
            request.approve,
            &request.findings,
            kanban_app::review_execution::counts_for_resolution(&review, request.slot_id),
        )?;
        insert_verdict(
            &span,
            request.slot_id,
            &ReviewVerdictRecord {
                counts_for_resolution: kanban_app::review_execution::counts_for_resolution(
                    &review,
                    request.slot_id,
                ),
                submission_id: None,
                tip: request.tip.clone(),
                approve: request.approve,
                summary: request.summary.clone(),
                findings: request.findings.clone(),
            },
        )?;
        let record = finish_verdict(&span, request.review_id)?;
        record_gate_outcome(&span, &record)?;
        span.commit().map_err(internal)?;
        Ok(record)
    }
    fn revalidate(&self, ticket_id: u64) -> Result<ReviewHistoryResponse, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let needed = span
            .query_row(
                "SELECT needs_revalidation FROM review_gates WHERE ticket_id=?1",
                params![ticket_id as i64],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(internal)?
            .unwrap_or(0);
        if needed == 0 {
            return Err(ApiError::invalid_request(
                "revalidation is only for failed or expired gates",
            ));
        }
        span.execute(
            "UPDATE review_gates SET needs_revalidation=0 WHERE ticket_id=?1",
            params![ticket_id as i64],
        )
        .map_err(internal)?;
        let history = load_history(&span, ticket_id)?;
        span.commit().map_err(internal)?;
        Ok(history)
    }
    fn expire(&self, review_id: u64) -> Result<ReviewExecutionRecord, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let review = load(&span, review_id)?.ok_or_else(|| ApiError::not_found("review"))?;
        if review.status != ReviewExecutionStatus::InProgress {
            return Err(ApiError::invalid_request(
                "only an in-progress review can expire",
            ));
        }
        span.execute(
            "UPDATE review_executions SET status='expired', version=version+1 WHERE id=?1",
            params![review_id as i64],
        )
        .map_err(internal)?;
        let record = load(&span, review_id)?.ok_or_else(|| ApiError::not_found("review"))?;
        record_expired_gate(&span, &record)?;
        insert_event(&span, &review_transition(&record, "review_expired")).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(record)
    }
    fn history(&self, ticket_id: u64) -> Result<ReviewHistoryResponse, ApiError> {
        load_history(&self.conn.lock(), ticket_id)
    }
    fn needs_revalidation(&self, ticket_id: u64) -> Result<bool, ApiError> {
        Ok(load_history(&self.conn.lock(), ticket_id)?.needs_revalidation)
    }
}

fn insert_verdict(
    conn: &Connection,
    slot: u64,
    verdict: &ReviewVerdictRecord,
) -> Result<(), ApiError> {
    conn.execute(
        "INSERT INTO review_slot_verdicts(slot_id,record) VALUES (?1,?2)",
        params![
            slot as i64,
            serde_json::to_string(verdict).map_err(internal)?
        ],
    )
    .map_err(internal)?;
    Ok(())
}
fn finish_verdict(conn: &Connection, id: u64) -> Result<ReviewExecutionRecord, ApiError> {
    let previous = load(conn, id)?
        .ok_or_else(|| ApiError::not_found("review"))?
        .status;
    conn.execute(
        "UPDATE review_executions SET version=version+1 WHERE id=?1",
        params![id as i64],
    )
    .map_err(internal)?;
    let review = advance(conn, id)?;
    insert_event(conn, &review_transition(&review, "review_slot_submitted")).map_err(internal)?;
    if previous != ReviewExecutionStatus::Rejected
        && review.status == ReviewExecutionStatus::Rejected
    {
        insert_event(conn, &review_transition(&review, "review_stage_bounced"))
            .map_err(internal)?;
    }
    Ok(review)
}

/// The submission, slot verdict, next-stage queue and audit share one span.
pub(crate) fn accept_submission(
    conn: &Connection,
    submission: &SubmissionRecord,
    reviewed: &kanban_app::review_execution::ReviewChangeObserver<'_>,
) -> Result<(), ApiError> {
    let SubmissionResult::Review {
        tip,
        approve,
        summary,
        findings,
    } = &submission.result
    else {
        return Ok(());
    };
    let assignment = conn
        .query_row(
            "SELECT s.review_id,s.id,s.dispatch_request_id,r.dispatch_request_id
        FROM capabilities c JOIN review_slots s ON s.id=c.reviewer_slot_id
        JOIN runs r ON r.id=?2 WHERE c.id=?1",
            params![submission.capability_id as i64, submission.run_id as i64],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(internal)?;
    let Some((review_id, slot_id, expected_request, actual_request)) = assignment else {
        return Ok(());
    };
    if expected_request != Some(actual_request) {
        return Err(ApiError::invalid_request(
            "review capability belongs to another slot run",
        ));
    }
    let review = load(conn, review_id)?.ok_or_else(|| ApiError::not_found("review"))?;
    let slot = active_slot(&review, slot_id)?;
    if matches!(slot.occupant, TicketReviewOccupant::Human {}) {
        return Err(ApiError::invalid_request(
            "human slots cannot be completed by agents",
        ));
    }
    validate_tip(&review, tip)?;
    kanban_app::findings::validate_review_verdict(
        *approve,
        findings,
        kanban_app::review_execution::counts_for_resolution(&review, slot_id),
    )?;
    insert_verdict(
        conn,
        slot_id,
        &ReviewVerdictRecord {
            counts_for_resolution: kanban_app::review_execution::counts_for_resolution(
                &review, slot_id,
            ),
            submission_id: Some(submission.id),
            tip: tip.clone(),
            approve: *approve,
            summary: summary.clone(),
            findings: findings.clone(),
        },
    )?;
    let updated = finish_verdict(conn, review_id)?;
    record_gate_outcome(conn, &updated)?;
    reviewed(&review, &updated)?;
    Ok(())
}

pub(crate) fn reviewer_for_request(
    conn: &Connection,
    id: u64,
) -> Result<Option<ReviewerDispatchRecord>, ApiError> {
    let row = conn
        .query_row(
            "SELECT s.id,s.review_id,e.tip,s.snapshot FROM review_slots s
        JOIN review_executions e ON e.id=s.review_id WHERE s.dispatch_request_id=?1",
            params![id as i64],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(internal)?;
    row.map(|(slot_id, review_id, tip, snapshot)| {
        let draft: ReviewSlotDraft = serde_json::from_str(&snapshot).map_err(internal)?;
        Ok(ReviewerDispatchRecord {
            slot_id,
            review_id,
            tip,
            requested: draft
                .requested
                .ok_or_else(|| ApiError::internal("review dispatch lacks requested profile"))?,
            effective: draft
                .effective
                .ok_or_else(|| ApiError::internal("review dispatch lacks effective profile"))?,
            fallback_path: draft.fallback_path,
        })
    })
    .transpose()
}

pub(crate) fn load(conn: &Connection, id: u64) -> Result<Option<ReviewExecutionRecord>, ApiError> {
    let row=conn.query_row("SELECT project_id,ticket_id,submission_id,tip,configuration_version,version,status FROM review_executions WHERE id=?1",params![id as i64],
        |r|Ok((r.get::<_,i64>(0)? as u64,r.get::<_,i64>(1)? as u64,r.get::<_,i64>(2)? as u64,r.get::<_,String>(3)?,r.get::<_,i64>(4)? as u64,r.get::<_,i64>(5)? as u64,r.get::<_,String>(6)?))).optional().map_err(internal)?;
    let Some((project_id, ticket_id, submission_id, tip, configuration_version, version, status)) =
        row
    else {
        return Ok(None);
    };
    let status = match status.as_str() {
        "in_progress" => ReviewExecutionStatus::InProgress,
        "approved" => ReviewExecutionStatus::Approved,
        "rejected" => ReviewExecutionStatus::Rejected,
        "expired" => ReviewExecutionStatus::Expired,
        _ => return Err(ApiError::internal("invalid stored review status")),
    };
    let mut statement = conn
        .prepare(
            "SELECT s.id,s.stage_index,s.snapshot,s.dispatch_request_id,v.record
        FROM review_slots s LEFT JOIN review_slot_verdicts v ON v.slot_id=s.id
        WHERE s.review_id=?1 ORDER BY s.stage_index,s.slot_index",
        )
        .map_err(internal)?;
    let rows = statement
        .query_map(params![id as i64], |r| {
            Ok((
                r.get::<_, i64>(0)? as u64,
                r.get::<_, i64>(1)? as usize,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?.map(|v| v as u64),
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(internal)?;
    let mut stages: Vec<ReviewStageRecord> = Vec::new();
    for row in rows {
        let (slot_id, index, snapshot, dispatch_request_id, verdict) = row.map_err(internal)?;
        while stages.len() <= index {
            stages.push(ReviewStageRecord {
                index: stages.len(),
                status: ReviewStageStatus::Waiting,
                slots: Vec::new(),
            });
        }
        let draft: ReviewSlotDraft = serde_json::from_str(&snapshot).map_err(internal)?;
        stages[index].slots.push(ReviewSlotRecord {
            id: slot_id,
            requirement: draft.requirement,
            occupant: draft.occupant,
            requested: draft.requested,
            effective: draft.effective,
            fallback_path: draft.fallback_path,
            dispatch_request_id,
            verdict: verdict
                .map(|s| serde_json::from_str(&s).map_err(internal))
                .transpose()?,
        });
    }
    for stage in &mut stages {
        stage.status = review_stage_status(&tip, &stage.slots);
    }
    let bounce = kanban_app::review_execution::review_bounce(&tip, &stages);
    Ok(Some(ReviewExecutionRecord {
        bounce,
        id,
        project_id,
        ticket_id,
        submission_id,
        tip,
        configuration_version,
        version,
        status,
        stages,
    }))
}

fn advance(conn: &Connection, id: u64) -> Result<ReviewExecutionRecord, ApiError> {
    let record = load(conn, id)?.ok_or_else(|| ApiError::not_found("review"))?;
    let (status, active) = kanban_app::review_execution::review_progress(&record);
    if let Some(index) = active {
        let stage = &record.stages[index];
        for slot in &stage.slots {
            if slot.dispatch_request_id.is_some() || slot.verdict.is_some() {
                continue;
            }
            if let Some(effective) = &slot.effective {
                conn.execute("INSERT INTO dispatch_requests(project_id,ticket_id,status,priority,ready,harness,model,usage_pool,created_at,version,reviewer_slot_id)
                            SELECT ?1,?2,'queued',e.priority,1,?3,?4,?5,unixepoch(),1,?6 FROM review_executions e WHERE e.id=?7",
                            params![record.project_id as i64,record.ticket_id as i64,effective.harness,effective.model,effective.usage_pool,slot.id as i64,id as i64]).map_err(internal)?;
                let dispatch = conn.last_insert_rowid();
                conn.execute(
                    "UPDATE review_slots SET dispatch_request_id=?2 WHERE id=?1",
                    params![slot.id as i64, dispatch],
                )
                .map_err(internal)?;
            }
        }
    }
    let wire = match status {
        ReviewExecutionStatus::InProgress => "in_progress",
        ReviewExecutionStatus::Approved => "approved",
        ReviewExecutionStatus::Rejected => "rejected",
        ReviewExecutionStatus::Expired => "expired",
    };
    conn.execute(
        "UPDATE review_executions SET status=?2 WHERE id=?1",
        params![id as i64, wire],
    )
    .map_err(internal)?;
    load(conn, id)?.ok_or_else(|| ApiError::not_found("review"))
}

fn record_gate_outcome(conn: &Connection, review: &ReviewExecutionRecord) -> Result<(), ApiError> {
    let outcome = match review.status {
        ReviewExecutionStatus::Rejected => "failed",
        ReviewExecutionStatus::Expired => "expired",
        ReviewExecutionStatus::Approved => "approved",
        ReviewExecutionStatus::InProgress => return Ok(()),
    };
    let needs_revalidation = matches!(
        review.status,
        ReviewExecutionStatus::Rejected | ReviewExecutionStatus::Expired
    ) as i64;
    conn.execute(
        "INSERT INTO review_gates(ticket_id, needs_revalidation, latest_review_id)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(ticket_id) DO UPDATE SET
            needs_revalidation = excluded.needs_revalidation,
            latest_review_id = excluded.latest_review_id",
        params![
            review.ticket_id as i64,
            needs_revalidation,
            review.id as i64
        ],
    )
    .map_err(internal)?;
    let attempt = conn
        .query_row(
            "SELECT COALESCE(MAX(attempt), 0) + 1 FROM review_execution_attempts WHERE ticket_id=?1",
            params![review.ticket_id as i64],
            |row| row.get::<_, i64>(0),
        )
        .map_err(internal)?;
    let verdicts: Vec<String> = review
        .stages
        .iter()
        .flat_map(|stage| stage.slots.iter())
        .filter_map(|slot| {
            slot.verdict.as_ref().map(|verdict| {
                if verdict.approve {
                    "approved".to_owned()
                } else {
                    "rejected".to_owned()
                }
            })
        })
        .collect();
    conn.execute(
        "INSERT INTO review_execution_attempts(ticket_id, review_id, attempt, outcome, verdicts, invalidations)
         VALUES (?1, ?2, ?3, ?4, ?5, '[]')",
        params![
            review.ticket_id as i64,
            review.id as i64,
            attempt,
            outcome,
            serde_json::to_string(&verdicts).map_err(internal)?
        ],
    )
    .map_err(internal)?;
    Ok(())
}

fn record_expired_gate(conn: &Connection, review: &ReviewExecutionRecord) -> Result<(), ApiError> {
    conn.execute(
        "INSERT INTO review_gates(ticket_id, needs_revalidation, latest_review_id)
         VALUES (?1, 1, ?2)
         ON CONFLICT(ticket_id) DO UPDATE SET
            needs_revalidation = 1,
            latest_review_id = excluded.latest_review_id",
        params![review.ticket_id as i64, review.id as i64],
    )
    .map_err(internal)?;
    let attempt = conn
        .query_row(
            "SELECT COALESCE(MAX(attempt), 0) + 1 FROM review_execution_attempts WHERE ticket_id=?1",
            params![review.ticket_id as i64],
            |row| row.get::<_, i64>(0),
        )
        .map_err(internal)?;
    conn.execute(
        "INSERT INTO review_execution_attempts(ticket_id, review_id, attempt, outcome, verdicts, invalidations)
         VALUES (?1, ?2, ?3, 'expired', '[]', '[\"gate expired\"]')",
        params![review.ticket_id as i64, review.id as i64, attempt],
    )
    .map_err(internal)?;
    Ok(())
}

fn load_history(conn: &Connection, ticket_id: u64) -> Result<ReviewHistoryResponse, ApiError> {
    let needs_revalidation = conn
        .query_row(
            "SELECT needs_revalidation FROM review_gates WHERE ticket_id=?1",
            params![ticket_id as i64],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(internal)?
        .unwrap_or(0)
        == 1;
    let mut statement = conn
        .prepare(
            "SELECT attempt, review_id, outcome, verdicts, invalidations
             FROM review_execution_attempts WHERE ticket_id=?1 ORDER BY attempt",
        )
        .map_err(internal)?;
    let rows = statement
        .query_map(params![ticket_id as i64], |row| {
            Ok((
                row.get::<_, i64>(0)? as u32,
                row.get::<_, Option<i64>>(1)?.map(|id| id as u64),
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(internal)?;
    let mut attempts = Vec::new();
    for row in rows {
        let (attempt, review_id, outcome, verdicts, invalidations) = row.map_err(internal)?;
        attempts.push(ReviewAttemptRecord {
            attempt,
            review_id,
            outcome,
            verdicts: serde_json::from_str(&verdicts).map_err(internal)?,
            invalidations: serde_json::from_str(&invalidations).map_err(internal)?,
        });
    }
    Ok(ReviewHistoryResponse {
        needs_revalidation,
        attempts,
    })
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&error.to_string())
}
