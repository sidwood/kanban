//! SQLite notification preferences, with no acknowledgement operation.
use crate::db::{ConnectionHandle, Database, WriteSpan};
use kanban_app::{TimelineEnvelope, notifications::NotificationSettingsStore};
use kanban_dto::{ApiError, NotificationSettingsRecord, NotificationSettingsUpdateRequest};
use rusqlite::{OptionalExtension, params};

pub struct SqliteNotificationSettingsStore {
    conn: ConnectionHandle,
}
impl SqliteNotificationSettingsStore {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }
}
impl NotificationSettingsStore for SqliteNotificationSettingsStore {
    fn get(&self, project_id: u64) -> Result<NotificationSettingsRecord, ApiError> {
        settings(&self.conn.lock(), project_id)
    }
    fn update(
        &self,
        request: &NotificationSettingsUpdateRequest,
        envelope: TimelineEnvelope,
    ) -> Result<NotificationSettingsRecord, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let current = settings(&span, request.project_id)?;
        if current.version != request.mutation.optimistic_version {
            return Err(ApiError::stale_version(
                request.mutation.optimistic_version,
                current.version,
            ));
        }
        span.execute("INSERT INTO notification_settings (project_id,local_enabled,mirror_role,version) VALUES (?1,?2,?3,1)
            ON CONFLICT(project_id) DO UPDATE SET local_enabled=excluded.local_enabled,mirror_role=excluded.mirror_role,version=notification_settings.version+1",
            params![request.project_id as i64,i64::from(request.local_enabled),request.mirror_role]).map_err(internal)?;
        crate::timeline::insert_event(&span, &envelope).map_err(internal)?;
        let record = settings(&span, request.project_id)?;
        span.commit().map_err(internal)?;
        Ok(record)
    }
}
fn settings(
    conn: &rusqlite::Connection,
    project_id: u64,
) -> Result<NotificationSettingsRecord, ApiError> {
    Ok(conn.query_row("SELECT local_enabled,mirror_role,version FROM notification_settings WHERE project_id=?1",[project_id as i64],|row| {
        Ok(NotificationSettingsRecord {project_id,local_enabled:row.get(0)?,mirror_role:row.get(1)?,version:row.get::<_,i64>(2)? as u64})
    }).optional().map_err(internal)?.unwrap_or(NotificationSettingsRecord {project_id,local_enabled:false,mirror_role:None,version:0}))
}
fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&error.to_string())
}

pub struct SqliteNotificationDeliveryStore {
    conn: ConnectionHandle,
}
impl SqliteNotificationDeliveryStore {
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }
}
impl kanban_app::notifications::NotificationDeliveryStore for SqliteNotificationDeliveryStore {
    fn get(&self, id: u64) -> Result<kanban_dto::NotificationDeliveryRecord, ApiError> {
        load_delivery(&self.conn.lock(), id)
    }
    fn list(
        &self,
        project_id: u64,
    ) -> Result<Vec<kanban_dto::NotificationDeliveryRecord>, ApiError> {
        let conn = self.conn.lock();
        let mut query=conn.prepare(&format!("SELECT {DELIVERY_COLUMNS} FROM notification_deliveries WHERE project_id=?1 ORDER BY id DESC")).map_err(internal)?;
        query
            .query_map([sqlite_id(project_id)?], decode_delivery)
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)
    }
    fn retry(
        &self,
        request: &kanban_dto::NotificationRetryRequest,
    ) -> Result<kanban_dto::NotificationDeliveryRecord, ApiError> {
        let conn = self.conn.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let current = load_delivery(&span, request.delivery_id)?;
        if current.version != request.mutation.optimistic_version {
            return Err(ApiError::stale_version(
                request.mutation.optimistic_version,
                current.version,
            ));
        }
        if current.status != kanban_dto::NotificationDeliveryStatus::Failed {
            return Err(ApiError::invalid_request(
                "only known unsent failures may be retried; submitted or uncertain deliveries must not be repeated",
            ));
        }
        let eligible:bool=span.query_row("SELECT EXISTS(SELECT 1 FROM attention_items i JOIN projects p ON p.id=i.project_id
            WHERE i.id=?1 AND i.version=?2 AND i.active=1 AND i.acknowledged_by IS NULL AND p.archived=0)",params![current.item_id,sqlite_id(current.item_version)?],|r|r.get(0)).map_err(internal)?;
        if !eligible {
            return Err(ApiError::invalid_request(
                "the attention source changed or was acknowledged; refresh before retrying",
            ));
        }
        span.execute("UPDATE notification_deliveries SET status='queued',version=version+1,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",[sqlite_id(request.delivery_id)?]).map_err(internal)?;
        let record = load_delivery(&span, request.delivery_id)?;
        span.commit().map_err(internal)?;
        Ok(record)
    }

    fn prepare(
        &self,
        draft: &kanban_app::notifications::NotificationDeliveryDraft,
        at: &str,
    ) -> Result<Option<kanban_dto::NotificationDeliveryRecord>, ApiError> {
        use kanban_dto::{
            NotificationChannel, NotificationDeliveryRecord, NotificationDeliveryStatus,
            NotificationTarget,
        };
        let conn = self.conn.lock();
        let span =
            rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)
                .map_err(internal)?;
        let eligible:bool=span.query_row("SELECT EXISTS(SELECT 1 FROM attention_items i
            JOIN notification_settings s ON s.project_id=i.project_id JOIN projects p ON p.id=i.project_id
            WHERE i.id=?1 AND i.version=?2 AND i.project_id=?3 AND i.active=1 AND i.acknowledged_by IS NULL
                AND p.archived=0 AND s.version=?4)",params![draft.item_id,draft.item_version as i64,draft.project_id as i64,draft.settings_version as i64],|r|r.get(0)).map_err(internal)?;
        if !eligible {
            return Ok(None);
        }
        let current = settings(&span, draft.project_id)?;
        let enabled = match (&draft.channel, &draft.target) {
            (NotificationChannel::Local, NotificationTarget::Local {}) => current.local_enabled,
            (NotificationChannel::HerdrMirror, NotificationTarget::HerdrMirror { role, .. }) => {
                current.mirror_role.as_ref() == Some(role)
            }
            _ => {
                return Err(ApiError::invalid_request(
                    "notification channel and target disagree",
                ));
            }
        };
        if !enabled {
            return Ok(None);
        }
        let existing=span.query_row("SELECT id,status FROM notification_deliveries WHERE item_id=?1 AND item_version=?2 AND channel=?3",
            params![draft.item_id,draft.item_version as i64,draft.channel.wire_name()],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).optional().map_err(internal)?;
        if let Some((id, status)) = existing {
            if status != "queued" {
                return Ok(None);
            }
            span.execute("UPDATE notification_deliveries SET status='prepared',settings_version=?2,target=?3,receipt=NULL,last_error=NULL,version=version+1,updated_at=?4 WHERE id=?1",
                params![id,draft.settings_version as i64,serde_json::to_string(&draft.target).map_err(internal)?,at]).map_err(internal)?;
            let record = load_delivery(&span, u64::try_from(id).map_err(internal)?)?;
            span.commit().map_err(internal)?;
            return Ok(Some(record));
        }
        span.execute("INSERT INTO notification_deliveries
            (project_id,item_id,item_version,settings_version,channel,target,status,receipt,last_error,created_at,updated_at)
            VALUES (?1,?2,?3,?4,?5,?6,'prepared',NULL,NULL,?7,?7)",
            params![draft.project_id as i64,draft.item_id,draft.item_version as i64,draft.settings_version as i64,
                draft.channel.wire_name(),serde_json::to_string(&draft.target).map_err(internal)?,at]).map_err(internal)?;
        let id = u64::try_from(span.last_insert_rowid()).map_err(internal)?;
        let record = NotificationDeliveryRecord {
            id,
            project_id: draft.project_id,
            item_id: draft.item_id.clone(),
            item_version: draft.item_version,
            channel: draft.channel,
            target: draft.target.clone(),
            status: NotificationDeliveryStatus::Prepared,
            receipt: None,
            last_error: None,
            created_at: at.to_owned(),
            updated_at: at.to_owned(),
            version: 1,
        };
        span.commit().map_err(internal)?;
        Ok(Some(record))
    }
    fn finish(
        &self,
        id: u64,
        outcome: &kanban_app::notifications::NotificationOutcome,
    ) -> Result<(), ApiError> {
        use kanban_app::notifications::NotificationOutcome;
        let (status, receipt, error) = match outcome {
            NotificationOutcome::Submitted { receipt } => {
                ("submitted", Some(receipt.as_str()), None)
            }
            NotificationOutcome::NotSent { reason } => ("failed", None, Some(reason.as_str())),
            NotificationOutcome::Uncertain { reason } => ("uncertain", None, Some(reason.as_str())),
        };
        let changed = self
            .conn
            .lock()
            .execute(
                "UPDATE notification_deliveries SET status=?2,receipt=?3,last_error=?4,version=version+1,
            updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1 AND status='prepared'",
                params![id as i64, status, receipt, error],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(ApiError::invalid_request(
                "notification delivery is no longer prepared",
            ));
        }
        Ok(())
    }
}

const DELIVERY_COLUMNS: &str = "id, project_id, item_id, item_version, channel, target, status, receipt, last_error, created_at, updated_at, version";
fn sqlite_id(value: u64) -> Result<i64, ApiError> {
    i64::try_from(value)
        .map_err(|_| ApiError::invalid_request("identity is outside the supported SQLite range"))
}
fn load_delivery(
    conn: &rusqlite::Connection,
    id: u64,
) -> Result<kanban_dto::NotificationDeliveryRecord, ApiError> {
    conn.query_row(
        &format!("SELECT {DELIVERY_COLUMNS} FROM notification_deliveries WHERE id=?1"),
        [sqlite_id(id)?],
        decode_delivery,
    )
    .optional()
    .map_err(internal)?
    .ok_or_else(|| ApiError::not_found("notification delivery"))
}
fn decode_delivery(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<kanban_dto::NotificationDeliveryRecord> {
    fn bad() -> rusqlite::Error {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid stored notification delivery",
            )),
        )
    }
    Ok(kanban_dto::NotificationDeliveryRecord {
        id: u64::try_from(row.get::<_, i64>(0)?).map_err(|_| bad())?,
        project_id: u64::try_from(row.get::<_, i64>(1)?).map_err(|_| bad())?,
        item_id: row.get(2)?,
        item_version: u64::try_from(row.get::<_, i64>(3)?).map_err(|_| bad())?,
        channel: serde_json::from_value(serde_json::json!(row.get::<_, String>(4)?))
            .map_err(|_| bad())?,
        target: serde_json::from_str(&row.get::<_, String>(5)?).map_err(|_| bad())?,
        status: serde_json::from_value(serde_json::json!(row.get::<_, String>(6)?))
            .map_err(|_| bad())?,
        receipt: row.get(7)?,
        last_error: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        version: u64::try_from(row.get::<_, i64>(11)?).map_err(|_| bad())?,
    })
}
