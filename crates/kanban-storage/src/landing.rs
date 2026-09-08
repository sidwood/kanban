//! Durable Spec integration ownership and landing records.

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;
use kanban_app::landing::{LandingDraft, LandingStore};
use kanban_dto::{ApiError, LandingRecord, SpecIntegrationRecord};
use rusqlite::params;

pub struct SqliteLandingStore {
    conn: ConnectionHandle,
}

impl SqliteLandingStore {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
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
        envelope: kanban_app::TimelineEnvelope,
    ) -> Result<(), ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let conflict: bool = span
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM landing_intents WHERE idempotency_key = ?1
                OR (completed = 0 AND (from_path IN (?2, ?3) OR into_path IN (?2, ?3))))",
                params![key, draft.from_path, draft.into_path],
                |row| row.get(0),
            )
            .map_err(internal)?;
        if conflict {
            return Err(ApiError::invalid_request(
                "a prior landing requires explicit recovery; no Git operation was repeated",
            ));
        }
        span.execute("INSERT INTO landing_intents(idempotency_key, project_id, from_path, into_path, draft) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![key, draft.project_id as i64, draft.from_path, draft.into_path, serde_json::to_string(draft).map_err(internal)?],
        ).map_err(internal)?;
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
        let changed = span.execute("UPDATE landing_intents SET completed = 1 WHERE idempotency_key = ?1 AND completed = 0", [key]).map_err(internal)?;
        if changed != 1 {
            return Err(ApiError::invalid_request("landing has no pending intent"));
        }
        insert_event(&span, &envelope).map_err(internal)?;
        span.commit().map_err(internal)?;
        Ok(LandingRecord {
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
        })
    }
}
