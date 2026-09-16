//! Durable Spec integration ownership and landing records.

use crate::SqliteRulingStore;
use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;
use kanban_app::landing::{LandingDraft, LandingStore};
use kanban_app::{RulingStore, TimelineFacts};
use kanban_domain::{Ruling, RulingEntityRef, RulingSummary};
use kanban_dto::{ApiError, LandingRecord, SpecIntegrationRecord, TimelineEventKind};
use rusqlite::{OptionalExtension, params};
use serde_json::json;

pub struct SqliteLandingStore {
    conn: ConnectionHandle,
    rulings: SqliteRulingStore,
}

impl SqliteLandingStore {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
            rulings: SqliteRulingStore::new(database),
        }
    }

    fn persist_landing(
        &self,
        key: &str,
        record: &LandingDraft,
        landed_tip: &str,
        envelope: kanban_app::TimelineEnvelope,
        replay_original: bool,
    ) -> Result<LandingRecord, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        span.execute(
            "INSERT INTO landings(project_id, kind, from_path, into_path, from_branch, into_branch, spec_id, ticket_id, from_tip, into_tip, landed_tip)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                record.project_id as i64,
                record.kind,
                record.from_path,
                record.into_path,
                record.from_branch,
                record.into_branch,
                record.spec_id.map(|id| id as i64),
                record.ticket_id.map(|id| id as i64),
                record.from_tip,
                record.into_tip,
                landed_tip,
            ],
        )
        .map_err(internal)?;
        let id = span.last_insert_rowid() as u64;
        let changed = span
            .execute(
                "UPDATE landing_intents SET completed = 1 WHERE idempotency_key = ?1 AND completed = 0",
                [key],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(ApiError::invalid_request("landing has no pending intent"));
        }
        let landing = LandingRecord {
            id,
            project_id: record.project_id,
            kind: record.kind.to_owned(),
            from_path: record.from_path.clone(),
            into_path: record.into_path.clone(),
            from_branch: record.from_branch.clone(),
            into_branch: record.into_branch.clone(),
            from_tip: record.from_tip.clone(),
            into_tip: record.into_tip.clone(),
            landed_tip: landed_tip.to_owned(),
            spec_id: record.spec_id,
            ticket_id: record.ticket_id,
        };
        if replay_original {
            let fingerprint: String = span
                .query_row(
                    "SELECT command_fingerprint FROM landing_intents WHERE idempotency_key = ?1",
                    [key],
                    |row| row.get(0),
                )
                .map_err(internal)?;
            // Schema 50 intents keep command_fingerprint = ''. Do not
            // invent a v2 fingerprint from the draft: it would not match
            // the original request body, so the completed intent is the
            // original-key resolution until a retry records the real one.
            if !fingerprint.is_empty() {
                let response = serde_json::to_string(&landing).map_err(internal)?;
                span.execute(
                    "INSERT INTO idempotency_outcomes (idempotency_key, fingerprint, response)
                     VALUES (?1, ?2, ?3)",
                    params![key, fingerprint, response],
                )
                .map_err(internal)?;
            }
        }
        insert_event(&span, &envelope).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(landing)
    }
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&error.to_string())
}

impl LandingStore for SqliteLandingStore {
    fn prepare_landing(
        &self,
        key: &str,
        draft: &LandingDraft,
        fingerprint: &str,
        envelope: kanban_app::TimelineEnvelope,
    ) -> Result<(), ApiError> {
        let conn = self.conn.lock();
        let completed: Option<i64> = conn
            .query_row(
                "SELECT completed FROM landing_intents WHERE idempotency_key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        if completed == Some(1) {
            return Ok(());
        }
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let conflict: bool = span
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM landing_intents WHERE completed = 0 AND (
                    idempotency_key = ?1 OR from_path IN (?2, ?3) OR into_path IN (?2, ?3)
                ))",
                params![key, draft.from_path, draft.into_path],
                |row| row.get(0),
            )
            .map_err(internal)?;
        if conflict {
            return Err(ApiError::invalid_request(
                "a prior landing requires explicit recovery; no Git operation was repeated",
            ));
        }
        span.execute(
            "INSERT INTO landing_intents(
                idempotency_key, project_id, from_path, into_path, draft, command_fingerprint
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                key,
                draft.project_id as i64,
                draft.from_path,
                draft.into_path,
                serde_json::to_string(draft).map_err(internal)?,
                fingerprint,
            ],
        )
        .map_err(internal)?;
        insert_event(&span, &envelope).map_err(internal)?;
        span.commit().map_err(internal)
    }

    fn pending_landing(&self, key: &str) -> Result<LandingDraft, ApiError> {
        let conn = self.conn.lock();
        let draft: String = conn
            .query_row(
                "SELECT draft FROM landing_intents WHERE idempotency_key = ?1 AND completed = 0",
                [key],
                |row| row.get(0),
            )
            .map_err(internal)?;
        serde_json::from_str(&draft).map_err(internal)
    }

    fn claim_integration(
        &self,
        record: &SpecIntegrationRecord,
        envelope: kanban_app::TimelineEnvelope,
    ) -> Result<SpecIntegrationRecord, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let owned: bool = span
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM spec_integrations WHERE workspace_id = ?1
                OR (branch = ?2 AND project_id = (SELECT project_id FROM specs WHERE id = ?3)))",
                params![
                    record.workspace_id.map(|id| id as i64),
                    record.branch,
                    record.spec_id as i64
                ],
                |row| row.get(0),
            )
            .map_err(internal)?;
        if owned {
            return Err(ApiError::invalid_request(
                "another Spec already owns this integration Workspace or branch",
            ));
        }
        span.execute(
            "INSERT INTO spec_integrations(spec_id, project_id, branch, workspace_path, workspace_id, review_approved, base_tip)
             VALUES (?1, (SELECT project_id FROM specs WHERE id = ?1), ?2, ?3, ?4, 0, ?5)",
            params![
                record.spec_id as i64,
                record.branch,
                record.workspace_path,
                record.workspace_id.map(|id| id as i64),
                record.base_tip,
            ],
        )
        .map_err(internal)?;
        insert_event(&span, &envelope).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(record.clone())
    }

    fn integration_for(&self, spec_id: u64) -> Result<Option<SpecIntegrationRecord>, ApiError> {
        let conn = self.conn.lock();
        let mut statement = conn
            .prepare(
                "SELECT spec_id, branch, workspace_path, workspace_id, review_approved, approved_tip, base_tip
                 FROM spec_integrations WHERE spec_id = ?1",
            )
            .map_err(internal)?;
        let mut rows = statement.query(params![spec_id as i64]).map_err(internal)?;
        match rows.next().map_err(internal)? {
            Some(row) => Ok(Some(SpecIntegrationRecord {
                spec_id: row.get::<_, i64>(0).map_err(internal)? as u64,
                branch: row.get(1).map_err(internal)?,
                workspace_path: row.get(2).map_err(internal)?,
                workspace_id: row
                    .get::<_, Option<i64>>(3)
                    .map_err(internal)?
                    .map(|id| id as u64),
                review_approved: row.get::<_, i64>(4).map_err(internal)? != 0,
                approved_tip: row.get(5).map_err(internal)?,
                base_tip: row.get(6).map_err(internal)?,
            })),
            None => Ok(None),
        }
    }

    fn approve_integration(
        &self,
        spec_id: u64,
        tip: &str,
        envelope: kanban_app::TimelineEnvelope,
    ) -> Result<SpecIntegrationRecord, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let changed = span
            .execute(
                "UPDATE spec_integrations SET review_approved = 1, approved_tip = ?2 WHERE spec_id = ?1",
                params![spec_id as i64, tip],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(ApiError::not_found("spec integration"));
        }
        insert_event(&span, &envelope).map_err(internal)?;
        span.commit().map_err(internal)?;
        self.integration_for(spec_id)?
            .ok_or_else(|| ApiError::not_found("spec integration"))
    }

    fn record_landing(
        &self,
        key: &str,
        record: &LandingDraft,
        landed_tip: &str,
        envelope: kanban_app::TimelineEnvelope,
    ) -> Result<LandingRecord, ApiError> {
        self.persist_landing(key, record, landed_tip, envelope, false)
    }

    fn incomplete_intent(&self, key: &str) -> Result<Option<LandingDraft>, ApiError> {
        let conn = self.conn.lock();
        let draft: Option<String> = conn
            .query_row(
                "SELECT draft FROM landing_intents WHERE idempotency_key = ?1 AND completed = 0",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        draft
            .map(|draft| serde_json::from_str(&draft).map_err(internal))
            .transpose()
    }

    fn completed_landing(&self, key: &str) -> Result<Option<LandingRecord>, ApiError> {
        let conn = self.conn.lock();
        let draft: Option<String> = conn
            .query_row(
                "SELECT draft FROM landing_intents WHERE idempotency_key = ?1 AND completed = 1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        let Some(draft) = draft else {
            return Ok(None);
        };
        let draft: LandingDraft = serde_json::from_str(&draft).map_err(internal)?;
        conn.query_row(
            "SELECT id, project_id, kind, from_path, into_path, from_branch, into_branch,
                    from_tip, into_tip, landed_tip, spec_id, ticket_id
             FROM landings
             WHERE project_id = ?1 AND kind = ?2 AND from_path = ?3 AND into_path = ?4
               AND from_tip = ?5 AND into_tip = ?6
             ORDER BY id DESC LIMIT 1",
            params![
                draft.project_id as i64,
                draft.kind,
                draft.from_path,
                draft.into_path,
                draft.from_tip,
                draft.into_tip,
            ],
            |row| {
                Ok(LandingRecord {
                    id: row.get::<_, i64>(0)? as u64,
                    project_id: row.get::<_, i64>(1)? as u64,
                    kind: row.get(2)?,
                    from_path: row.get(3)?,
                    into_path: row.get(4)?,
                    from_branch: row.get(5)?,
                    into_branch: row.get(6)?,
                    from_tip: row.get(7)?,
                    into_tip: row.get(8)?,
                    landed_tip: row.get(9)?,
                    spec_id: row.get::<_, Option<i64>>(10)?.map(|id| id as u64),
                    ticket_id: row.get::<_, Option<i64>>(11)?.map(|id| id as u64),
                })
            },
        )
        .optional()
        .map_err(internal)
    }

    fn recover_landing(
        &self,
        key: &str,
        draft: &LandingDraft,
        landed_tip: Option<&str>,
        policy_name: &str,
        landing_envelope: Option<kanban_app::TimelineEnvelope>,
        reconcile_envelope: kanban_app::TimelineEnvelope,
    ) -> Result<(Option<LandingRecord>, u64), ApiError> {
        let summary = RulingSummary::new(policy_name)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let ruling = self.rulings.insert(
            &Ruling::record(
                draft.project_id,
                summary,
                Some(RulingEntityRef {
                    kind: "project".to_owned(),
                    id: draft.project_id.to_string(),
                }),
            ),
            TimelineFacts {
                kind: TimelineEventKind::Ruling,
                facts: json!({
                    "summary": policy_name,
                    "intent_key": key,
                    "policy": policy_name,
                }),
            },
        )?;
        let landing = match landed_tip {
            Some(tip) => {
                let envelope = landing_envelope.ok_or_else(|| {
                    ApiError::internal("completed recovery requires a landing envelope")
                })?;
                Some(self.persist_landing(key, draft, tip, envelope, true)?)
            }
            None => {
                let conn = self.conn.lock();
                let span = WriteSpan::begin(&conn).map_err(internal)?;
                let changed = span
                    .execute(
                        "DELETE FROM landing_intents WHERE idempotency_key = ?1 AND completed = 0",
                        [key],
                    )
                    .map_err(internal)?;
                if changed != 1 {
                    return Err(ApiError::invalid_request("landing has no pending intent"));
                }
                span.commit().map_err(internal)?;
                None
            }
        };
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        insert_event(&span, &reconcile_envelope).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok((landing, ruling.id().value()))
    }
}
