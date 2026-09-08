//! Durable attention projection; source reconciliation never acknowledges.
use crate::db::{ConnectionHandle, Database};
use kanban_app::attention::{AttentionSource, AttentionStore, InboxSignal};
use kanban_dto::{
    ApiError, AttentionItemRecord, AttentionListQuery, AttentionListResponse, AttentionState,
    AttentionSubjectKind,
};
use rusqlite::params;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

const ITEM_COLUMNS: &str = "id, project_id, kind, subject_kind, subject_id, summary, detail, version, active, acknowledged_by, acknowledged_at, first_seen_at, last_seen_at";

pub struct SqliteAttentionSource {
    conn: ConnectionHandle,
    submissions: crate::SqliteSubmissionStore,
    runtime: Option<std::sync::Arc<dyn kanban_app::attention::RuntimeAttentionFeed>>,
}
impl SqliteAttentionSource {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
            submissions: crate::SqliteSubmissionStore::new(database),
            runtime: None,
        }
    }
    pub fn with_runtime(
        mut self,
        runtime: std::sync::Arc<dyn kanban_app::attention::RuntimeAttentionFeed>,
    ) -> Self {
        self.runtime = Some(runtime);
        self
    }
}
impl SqliteAttentionSource {
    fn database_snapshot(&self) -> Result<Vec<InboxSignal>, ApiError> {
        let conn = self.conn.lock();
        let mut statement = conn
            .prepare(
                "SELECT b.id,t.project_id,t.id,b.description,t.state
            FROM ticket_blockers b JOIN tickets t ON t.id=b.ticket_id
            JOIN projects p ON p.id=t.project_id WHERE p.archived=0 ORDER BY b.id",
            )
            .map_err(internal)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    number(row, 0)?,
                    number(row, 1)?,
                    number(row, 2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        let mut signals = Vec::new();
        for (id, project_id, ticket, description, state) in rows {
            let state = kanban_domain::TicketState::parse(&state)
                .ok_or_else(|| ApiError::internal("stored Ticket state is invalid"))?;
            if matches!(
                state,
                kanban_domain::TicketState::Done
                    | kanban_domain::TicketState::Cancelled
                    | kanban_domain::TicketState::Superseded
            ) {
                continue;
            }
            signals.push(InboxSignal {project_id,kind:AttentionState::Blocker,subject_kind:AttentionSubjectKind::Ticket,
                subject_id:ticket.to_string(),revision:format!("blocker:{id}"),summary:description.clone(),
                detail:json!({"source":"external_blocker","blocker_id":id,"ticket_id":ticket,"description":description})});
        }
        let mut query = conn
            .prepare(
                "SELECT d.from_ticket,d.to_ticket,t.project_id,rp.code,r.number,r.state,t.state
            FROM ticket_dependencies d JOIN tickets t ON t.id=d.to_ticket
            JOIN tickets r ON r.id=d.from_ticket JOIN projects rp ON rp.id=r.project_id
            JOIN projects p ON p.id=t.project_id WHERE p.archived=0
            ORDER BY d.to_ticket,d.from_ticket",
            )
            .map_err(internal)?;
        let rows = query
            .query_map([], |r| {
                Ok((
                    number(r, 0)?,
                    number(r, 1)?,
                    number(r, 2)?,
                    r.get::<_, String>(3)?,
                    number(r, 4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        for (from, to, project_id, code, ticket_number, required_state, target_state) in rows {
            use kanban_domain::{
                DependencyState, ReadinessInputs, TicketDependency, TicketId, TicketState,
                compute_readiness,
            };
            let required = TicketState::parse(&required_state)
                .ok_or_else(|| ApiError::internal("stored dependency state is invalid"))?;
            let target = TicketState::parse(&target_state)
                .ok_or_else(|| ApiError::internal("stored Ticket state is invalid"))?;
            if matches!(
                target,
                TicketState::Done | TicketState::Cancelled | TicketState::Superseded
            ) {
                continue;
            }
            let dependency = DependencyState {
                dependency: TicketDependency::new(TicketId::new(from), TicketId::new(to)),
                state: required,
            };
            if compute_readiness(ReadinessInputs {
                dependencies: &[dependency],
                blockers: &[],
            })
            .is_ready()
            {
                continue;
            }
            signals.push(InboxSignal {project_id,kind:AttentionState::Blocker,subject_kind:AttentionSubjectKind::Ticket,
                subject_id:to.to_string(),revision:format!("dependency:{from}:{to}"),
                summary:format!("Waiting for prerequisite {code}-T{ticket_number}."),
                detail:json!({"source":"dependency","ticket_id":to,"required_ticket_id":from,"required_state":required_state})});
        }
        let mut query=conn.prepare("SELECT a.id,a.project_id,a.schedule_id,a.template_ticket_id,a.reason,a.first_window,a.last_window,a.next_activation
            FROM schedule_attention a JOIN projects p ON p.id=a.project_id
            WHERE p.archived=0 ORDER BY a.id").map_err(internal)?;
        let rows = query
            .query_map([], |r| {
                Ok((
                    number(r, 0)?,
                    number(r, 1)?,
                    number(r, 2)?,
                    number(r, 3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                ))
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        for (id, project_id, schedule_id, ticket_id, reason, first, last, next) in rows {
            let summary = match reason.as_str() {
                "missed_window" => "Missed schedule windows need attention.",
                "overlap" => "Scheduled work was skipped to prevent overlapping occurrences.",
                "blocked" => "Scheduled work was held by its blockers.",
                _ => {
                    return Err(ApiError::internal(
                        "stored schedule attention reason is invalid",
                    ));
                }
            };
            signals.push(InboxSignal {project_id,kind:AttentionState::FailedSchedule,subject_kind:AttentionSubjectKind::Schedule,
                subject_id:schedule_id.to_string(),revision:format!("schedule:{id}:{last}"),summary:summary.to_owned(),
                detail:json!({"source":"schedule_window","schedule_id":schedule_id,"ticket_id":ticket_id,
                    "reason":reason,"first_window":first,"last_window":last,"next_activation":next})});
        }
        let mut query = conn
            .prepare(
                "SELECT t.project_id,b.ticket_id,b.criterion_index,b.tip
            FROM criterion_bindings b JOIN tickets t ON t.id=b.ticket_id
            JOIN projects p ON p.id=t.project_id WHERE p.archived=0 AND b.void=1
            ORDER BY b.ticket_id,b.criterion_index",
            )
            .map_err(internal)?;
        let rows = query
            .query_map([], |r| {
                Ok((
                    number(r, 0)?,
                    number(r, 1)?,
                    number(r, 2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        for (project_id, ticket_id, criterion, tip) in rows {
            signals.push(InboxSignal {project_id,kind:AttentionState::InvalidApproval,subject_kind:AttentionSubjectKind::Ticket,
                subject_id:ticket_id.to_string(),revision:format!("binding:{criterion}:{tip}"),
                summary:"Criterion evidence no longer validates for the reviewed code.".to_owned(),
                detail:json!({"source":"criterion_binding","ticket_id":ticket_id,"criterion_index":criterion,"reviewed_tip":tip})});
        }
        let mut query=conn.prepare("SELECT p.id,d.id,d.finding_id,d.reason FROM deferrals d
            JOIN projects p ON d.project_id=CAST(p.id AS TEXT) WHERE p.archived=0
            AND NOT EXISTS(SELECT 1 FROM deferrals s WHERE s.supersedes_id=d.id AND s.project_id=d.project_id)
            AND NOT EXISTS(SELECT 1 FROM finding_promotions f WHERE f.project_id=p.id AND f.finding_id=d.finding_id)
            ORDER BY d.id").map_err(internal)?;
        let rows = query
            .query_map([], |r| {
                Ok((
                    number(r, 0)?,
                    number(r, 1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        for (project_id, id, finding_id, reason) in rows {
            signals.push(InboxSignal {project_id,kind:AttentionState::HumanDecision,subject_kind:AttentionSubjectKind::Deferral,
                subject_id:id.to_string(),revision:format!("deferral:{id}"),
                summary:"Deferred finding needs a follow-up decision.".to_owned(),
                detail:json!({"source":"deferral","deferral_id":id,"finding_id":finding_id,"reason":reason})});
        }
        let mut query = conn
            .prepare(
                "SELECT r.id FROM review_executions r JOIN projects p ON p.id=r.project_id
            WHERE p.archived=0 AND r.status='in_progress' ORDER BY r.id",
            )
            .map_err(internal)?;
        let ids = query
            .query_map([], |r| number(r, 0))
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        for id in ids {
            let review = crate::review_execution::load(&conn, id)?.ok_or_else(|| {
                ApiError::internal("review disappeared during attention collection")
            })?;
            let (_, active) = kanban_app::review_execution::review_progress(&review);
            let Some(stage) = active.and_then(|index| review.stages.get(index)) else {
                continue;
            };
            for slot in &stage.slots {
                if slot.verdict.is_some()
                    || slot.requirement != kanban_dto::TicketReviewSlotRequirement::Required
                    || !matches!(&slot.occupant, kanban_dto::TicketReviewOccupant::Human {})
                {
                    continue;
                }
                signals.push(InboxSignal {project_id:review.project_id,kind:AttentionState::ReviewRequest,
                    subject_kind:AttentionSubjectKind::Ticket,subject_id:review.ticket_id.to_string(),
                    revision:format!("review:{id}:slot:{}:{}",slot.id,review.tip),
                    summary:"Human review is requested for the current review stage.".to_owned(),
                    detail:json!({"source":"human_review_slot","ticket_id":review.ticket_id,"review_id":id,
                        "slot_id":slot.id,"stage_index":stage.index,"requirement":slot.requirement,"reviewed_tip":review.tip})});
            }
        }
        let mut query=conn.prepare("SELECT f.schedule_id,f.project_id,f.ticket_id,f.error_code,f.failed_at
            FROM schedule_failures f JOIN projects p ON p.id=f.project_id WHERE p.archived=0 ORDER BY f.schedule_id").map_err(internal)?;
        let rows = query
            .query_map([], |r| {
                Ok((
                    number(r, 0)?,
                    number(r, 1)?,
                    number(r, 2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        for (schedule_id, project_id, ticket_id, error_code, failed_at) in rows {
            signals.push(InboxSignal {project_id,kind:AttentionState::FailedSchedule,subject_kind:AttentionSubjectKind::Schedule,
                subject_id:schedule_id.to_string(),revision:format!("schedule_failure:{schedule_id}:{failed_at}:{error_code}"),
                summary:"A Schedule could not advance; inspect its configuration and service diagnostics.".to_owned(),
                detail:json!({"source":"schedule_failure","schedule_id":schedule_id,"ticket_id":ticket_id,
                    "error_code":error_code,"failed_at":failed_at})});
        }
        let mut query = conn
            .prepare(
                "SELECT t.project_id,g.ticket_id,g.latest_review_id FROM review_gates g
            JOIN tickets t ON t.id=g.ticket_id JOIN projects p ON p.id=t.project_id
            WHERE p.archived=0 AND g.needs_revalidation=1 ORDER BY g.ticket_id",
            )
            .map_err(internal)?;
        let rows = query
            .query_map([], |r| {
                Ok((number(r, 0)?, number(r, 1)?, r.get::<_, Option<i64>>(2)?))
            })
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        for (project_id, ticket_id, review_id) in rows {
            signals.push(InboxSignal {
                project_id,
                kind: AttentionState::InvalidApproval,
                subject_kind: AttentionSubjectKind::Ticket,
                subject_id: ticket_id.to_string(),
                revision: format!("review_gate:{ticket_id}:{review_id:?}"),
                summary: "A review gate needs explicit revalidation.".to_owned(),
                detail: json!({"source":"review_gate","ticket_id":ticket_id,"review_id":review_id}),
            });
        }
        Ok(signals)
    }
}

impl AttentionSource for SqliteAttentionSource {
    fn snapshot(&self) -> Result<Vec<InboxSignal>, ApiError> {
        let mut signals = self.database_snapshot()?;
        let projects = {
            let conn = self.conn.lock();
            let mut query = conn
                .prepare(&format!(
                    "SELECT {} FROM projects WHERE archived=0 ORDER BY id",
                    crate::projects::PROJECT_COLUMNS
                ))
                .map_err(internal)?;
            query
                .query_map([], |row| crate::projects::decode_row_at(row, 0))
                .map_err(internal)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(internal)?
        };
        for project in projects {
            let registration = project.registration();
            // Observation happens without holding the database writer.
            let mut observed = match &self.runtime {
                Some(runtime) => runtime.snapshot(&project)?,
                None => kanban_app::attention::RuntimeAttentionSnapshot {
                    diagnostics: kanban_dto::HerdrConnectionDiagnostics {
                        session_name: registration.herdr_session().map(str::to_owned),
                        product_workspace: registration.seed_workspace().to_owned(),
                        herdr_workspace: registration.herdr_workspace().to_owned(),
                        connected: false,
                        last_snapshot_at: None,
                        last_error: None,
                    },
                    signals: Vec::new(),
                },
            };
            {
                use rusqlite::OptionalExtension;
                let conn = self.conn.lock();
                let span = rusqlite::Transaction::new_unchecked(
                    &conn,
                    rusqlite::TransactionBehavior::Immediate,
                )
                .map_err(internal)?;
                for signal in &observed.signals {
                    if signal.project_id != project.id().value()
                        || !matches!(
                            signal.reason.as_str(),
                            "missing_submission"
                                | "missing_result_deadline_breached"
                                | "stall_deadline_breached"
                        )
                    {
                        continue;
                    }
                    let key = serde_json::to_string(&(
                        signal.project_id,
                        &signal.reason,
                        signal.detail.get("run_id"),
                        signal.detail.get("role"),
                        signal.detail.get("last_activity_unix_secs"),
                        signal.detail.get("settled_unix_secs"),
                    ))
                    .map_err(internal)?;
                    span.execute("INSERT OR IGNORE INTO observed_attention_signals(project_id,source_key,reason,detail) VALUES (?1,?2,?3,?4)",
                        params![signal.project_id as i64,key,signal.reason,serde_json::to_string(&signal.detail).map_err(internal)?]).map_err(internal)?;
                }
                let binding = serde_json::to_string(&(
                    registration.herdr_session(),
                    registration.seed_workspace(),
                    registration.herdr_workspace(),
                ))
                .map_err(internal)?;
                if observed.diagnostics.connected || observed.diagnostics.last_error.is_some() || observed.diagnostics.last_snapshot_at.is_some() {
                    span.execute("INSERT INTO observed_attention_connections(project_id,binding,diagnostics) VALUES (?1,?2,?3)
                        ON CONFLICT(project_id) DO UPDATE SET binding=excluded.binding,diagnostics=excluded.diagnostics",
                        params![project.id().value() as i64,binding,serde_json::to_string(&observed.diagnostics).map_err(internal)?]).map_err(internal)?;
                } else if let Some(saved)=span.query_row("SELECT diagnostics FROM observed_attention_connections WHERE project_id=?1 AND binding=?2",
                    params![project.id().value() as i64,binding],|r|r.get::<_,String>(0)).optional().map_err(internal)? {
                    observed.diagnostics=serde_json::from_str(&saved).map_err(internal)?;
                }
                observed.signals = {
                    let mut query=span.prepare("SELECT reason,detail FROM observed_attention_signals WHERE project_id=?1 ORDER BY id").map_err(internal)?;
                    let rows = query
                        .query_map([project.id().value() as i64], |r| {
                            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                        })
                        .map_err(internal)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(internal)?;
                    rows.into_iter()
                        .map(|(reason, detail)| {
                            Ok(kanban_app::AttentionSignal {
                                project_id: project.id().value(),
                                reason,
                                detail: serde_json::from_str(&detail).map_err(internal)?,
                            })
                        })
                        .collect::<Result<Vec<_>, ApiError>>()?
                };
                span.commit().map_err(internal)?;
            }
            signals.extend(kanban_app::attention::runtime_attention(
                &project,
                &observed,
                &self.submissions,
            )?);
        }
        Ok(signals)
    }
}

pub struct SqliteAttentionStore {
    conn: ConnectionHandle,
}
impl SqliteAttentionStore {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }
}
impl AttentionStore for SqliteAttentionStore {
    fn get(&self, id: &str) -> Result<AttentionItemRecord, ApiError> {
        load_item(&self.conn.lock(), id)
    }
    fn acknowledge(
        &self,
        request: &kanban_dto::AttentionAcknowledgeRequest,
        envelope: &dyn Fn(&AttentionItemRecord) -> kanban_app::TimelineEnvelope,
    ) -> Result<AttentionItemRecord, ApiError> {
        let conn = self.conn.lock();
        let span = crate::db::WriteSpan::begin(&conn).map_err(internal)?;
        let current = load_item(&span, &request.item_id)?;
        if current.version != request.mutation.optimistic_version {
            return Err(ApiError::stale_version(
                request.mutation.optimistic_version,
                current.version,
            ));
        }
        if current.acknowledged_by.is_some() {
            return Err(ApiError::invalid_request(
                "this attention item is already acknowledged",
            ));
        }
        if request.who.trim().is_empty() {
            return Err(ApiError::invalid_request(
                "name the operator acknowledging this item",
            ));
        }
        let at: String = span
            .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |row| {
                row.get(0)
            })
            .map_err(internal)?;
        span.execute("INSERT INTO attention_acknowledgements (item_id,item_version,who,acknowledged_at,snapshot)
            VALUES (?1,?2,?3,?4,?5)",params![current.id,current.version as i64,request.who,at,
                serde_json::to_string(&current).map_err(internal)?]).map_err(internal)?;
        span.execute("UPDATE attention_items SET acknowledged_by=?2,acknowledged_at=?3,version=version+1 WHERE id=?1",
            params![current.id,request.who,at]).map_err(internal)?;
        let acknowledged = load_item(&span, &current.id)?;
        crate::timeline::insert_event(&span, &envelope(&acknowledged)).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(acknowledged)
    }

    fn reconcile(&self, signals: &[InboxSignal], observed_at: &str) -> Result<(), ApiError> {
        let conn = self.conn.lock();
        let span =
            rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)
                .map_err(internal)?;
        let existing = {
            let mut query = span
                .prepare("SELECT id,fingerprint,active FROM attention_items")
                .map_err(internal)?;
            query
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        (r.get::<_, String>(1)?, r.get::<_, bool>(2)?),
                    ))
                })
                .map_err(internal)?
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map_err(internal)?
        };
        let mut seen = BTreeSet::new();
        for signal in signals {
            let key = signal.key()?;
            seen.insert(key.clone());
            let detail = serde_json::to_string(&signal.detail).map_err(internal)?;
            match existing.get(&key) {
                None => {
                    span.execute("INSERT INTO attention_items
                    (id,project_id,kind,subject_kind,subject_id,summary,detail,fingerprint,version,active,
                     acknowledged_by,acknowledged_at,first_seen_at,last_seen_at)
                    VALUES (?1,?2,?3,?4,?5,?6,?7,?8,1,1,NULL,NULL,?9,?9)",
                    params![key,signal.project_id as i64,signal.kind.wire_name(),signal.subject_kind.wire_name(),
                        signal.subject_id,signal.summary,detail,signal.revision,observed_at]).map_err(internal)?;
                }
                Some((fingerprint, active)) => {
                    let changed = fingerprint != &signal.revision || !*active;
                    span.execute("UPDATE attention_items SET summary=?2,detail=?3,fingerprint=?4,active=1,last_seen_at=?5,
                        version=version+?6,
                        acknowledged_by=CASE WHEN ?6=1 THEN NULL ELSE acknowledged_by END,
                        acknowledged_at=CASE WHEN ?6=1 THEN NULL ELSE acknowledged_at END WHERE id=?1",
                        params![key,signal.summary,detail,signal.revision,observed_at,i64::from(changed)]).map_err(internal)?;
                }
            }
        }
        for (id, (_, active)) in existing {
            if active && !seen.contains(&id) {
                span.execute(
                    "UPDATE attention_items SET active=0,version=version+1 WHERE id=?1",
                    [id],
                )
                .map_err(internal)?;
            }
        }
        span.commit().map_err(internal)
    }
    fn list(&self, query: &AttentionListQuery) -> Result<AttentionListResponse, ApiError> {
        let conn = self.conn.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {ITEM_COLUMNS} FROM attention_items
            WHERE (?1=1 OR acknowledged_by IS NULL) AND (?2=1 OR active=1)
            ORDER BY first_seen_at,id"
            ))
            .map_err(internal)?;
        let items = statement
            .query_map(
                params![
                    i64::from(query.include_acknowledged),
                    i64::from(query.include_inactive)
                ],
                decode_item,
            )
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        Ok(AttentionListResponse { items })
    }
}
fn decode_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttentionItemRecord> {
    Ok(AttentionItemRecord {
        id: row.get(0)?,
        project_id: number(row, 1)?,
        kind: AttentionState::parse(&row.get::<_, String>(2)?).ok_or_else(|| corrupt(2))?,
        subject_kind: AttentionSubjectKind::parse(&row.get::<_, String>(3)?)
            .ok_or_else(|| corrupt(3))?,
        subject_id: row.get(4)?,
        summary: row.get(5)?,
        detail: serde_json::from_str(&row.get::<_, String>(6)?).map_err(|_| corrupt(6))?,
        version: number(row, 7)?,
        active: row.get(8)?,
        acknowledged_by: row.get(9)?,
        acknowledged_at: row.get(10)?,
        first_seen_at: row.get(11)?,
        last_seen_at: row.get(12)?,
    })
}
fn number(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    u64::try_from(row.get::<_, i64>(index)?).map_err(|_| corrupt(index))
}
fn corrupt(column: usize) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid stored attention item",
        )),
    )
}
fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&error.to_string())
}

fn load_item(conn: &rusqlite::Connection, id: &str) -> Result<AttentionItemRecord, ApiError> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        &format!("SELECT {ITEM_COLUMNS} FROM attention_items WHERE id=?1"),
        [id],
        decode_item,
    )
    .optional()
    .map_err(internal)?
    .ok_or_else(|| ApiError::not_found("attention item"))
}
