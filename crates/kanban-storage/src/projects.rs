//! The SQLite implementation of the Project storage port: rows in
//! `projects`, the uniqueness rules on codes and Herdr session
//! names, and the application's timeline envelope landing unchanged
//! in the same transaction as every change.

use kanban_app::{ProjectStore, TimelineEnvelope, duplicate_code_error, duplicate_session_error};
use kanban_domain::{
    InitiativeId, NumberKind, Project, ProjectCounters, ProjectId, ProjectRegistration,
    ProjectState,
};
use kanban_dto::ApiError;
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

/// Every stored column of one Project row, in select order.
const PROJECT_COLUMNS: &str = "id, code, name, repository, seed_workspace, default_branch, \
                               herdr_session, initiative_id, archived, plan_counter, \
                               spec_counter, ticket_counter, version";

/// The Project port over the authoritative database.
pub struct SqliteProjectStore {
    conn: ConnectionHandle,
}

impl SqliteProjectStore {
    /// Share the connection the `database` owns.
    pub fn new(database: &Database) -> Self {
        Self {
            conn: database.connection_handle(),
        }
    }

    /// Lock the shared connection.
    fn lock(&self) -> parking_lot::ReentrantMutexGuard<'_, rusqlite::Connection> {
        self.conn.lock()
    }
}

impl ProjectStore for SqliteProjectStore {
    fn create(
        &self,
        registration: &ProjectRegistration,
        envelope: &dyn Fn(ProjectId) -> TimelineEnvelope,
    ) -> Result<Project, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        // The friendly refusals; the UNIQUE constraints below remain
        // the schema-level guarantee.
        let code = registration.code().as_str();
        if holder_of(&span, "SELECT id FROM projects WHERE code = ?1", code)?.is_some() {
            return Err(duplicate_code_error(code));
        }
        let session = registration.herdr_session();
        if holder_of(
            &span,
            "SELECT id FROM projects WHERE herdr_session = ?1",
            session,
        )?
        .is_some()
        {
            return Err(duplicate_session_error(session));
        }
        span.execute(
            "INSERT INTO projects
                 (code, name, repository, seed_workspace, default_branch,
                  herdr_session, initiative_id, archived, version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 1)",
            params![
                code,
                registration.name(),
                registration.repository(),
                registration.seed_workspace(),
                registration.default_branch(),
                session,
                registration.initiative().map(|id| id.value() as i64),
            ],
        )
        .map_err(internal)?;
        let id = ProjectId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the Project identity overflowed"))?,
        );
        append_timeline(&span, &envelope(id))?;
        span.commit().map_err(internal)?;
        Ok(Project::new(id, registration.clone()))
    }

    fn find(&self, id: ProjectId) -> Result<Option<Project>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE id = ?1"),
            params![id.value() as i64],
            decode_row,
        );
        match row {
            Ok(project) => Ok(Some(project)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn save(&self, project: &Project, envelope: TimelineEnvelope) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let archived = project.is_archived();
        let counters = project.counters();
        let preceding_version = project.version() - 1;
        let changed = span
            .execute(
                "UPDATE projects
                 SET archived = ?2,
                     version = ?3,
                     plan_counter = ?4,
                     spec_counter = ?5,
                     ticket_counter = ?6,
                     archived_at = CASE
                         WHEN ?2 = 1 THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                         ELSE archived_at
                     END
                 WHERE id = ?1 AND version = ?7",
                params![
                    project.id().value() as i64,
                    archived,
                    project.version() as i64,
                    counters.last(NumberKind::Plan) as i64,
                    counters.last(NumberKind::Spec) as i64,
                    counters.last(NumberKind::Ticket) as i64,
                    preceding_version as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            let current = span.query_row(
                "SELECT version FROM projects WHERE id = ?1",
                params![project.id().value() as i64],
                |row| row.get::<_, i64>(0),
            );
            return match current {
                Ok(current) => Err(ApiError::stale_version(
                    preceding_version,
                    current.unsigned_abs(),
                )),
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    Err(ApiError::not_found(&format!("project {}", project.id())))
                }
                Err(error) => Err(internal(error)),
            };
        }
        append_timeline(&span, &envelope)?;
        span.commit().map_err(internal)?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<Project>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {PROJECT_COLUMNS} FROM projects ORDER BY id"
            ))
            .map_err(internal)?;
        let rows = statement.query_map([], decode_row).map_err(internal)?;
        let mut projects = Vec::new();
        for row in rows {
            projects.push(row.map_err(internal)?);
        }
        Ok(projects)
    }
}

/// The stored identity whose column holds `value`, if any. The SQL
/// text is fixed per call site; only the bound value varies.
fn holder_of(conn: &rusqlite::Connection, sql: &str, value: &str) -> Result<Option<i64>, ApiError> {
    match conn.query_row(sql, params![value], |row| row.get(0)) {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(internal(error)),
    }
}

/// Decode one stored row into the domain aggregate. Every stored
/// value passed validation on the way in, so a failure here is
/// corruption the caller must hear about, not silently accept.
fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    let id = row.get::<_, i64>(0)?.unsigned_abs();
    let code: String = row.get(1)?;
    let name: String = row.get(2)?;
    let repository: String = row.get(3)?;
    let seed_workspace: String = row.get(4)?;
    let default_branch: String = row.get(5)?;
    let herdr_session: String = row.get(6)?;
    let initiative_id = row
        .get::<_, Option<i64>>(7)?
        .map(|value| value.unsigned_abs());
    let archived: i64 = row.get(8)?;
    let counters = ProjectCounters::restore(
        row.get::<_, i64>(9)?.unsigned_abs(),
        row.get::<_, i64>(10)?.unsigned_abs(),
        row.get::<_, i64>(11)?.unsigned_abs(),
    );
    let version = row.get::<_, i64>(12)?.unsigned_abs();
    let registration = ProjectRegistration::new(
        &code,
        &name,
        &repository,
        &seed_workspace,
        &default_branch,
        &herdr_session,
        initiative_id.map(InitiativeId::new),
    )
    .map_err(|_| rusqlite::Error::ToSqlConversionFailure(Box::new(CorruptRow)))?;
    let state = if archived == 1 {
        ProjectState::Archived
    } else {
        ProjectState::Active
    };
    Ok(Project::restore(
        ProjectId::new(id),
        registration,
        state,
        counters,
        version,
    ))
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

/// A stored Project row failed domain validation.
#[derive(Debug)]
struct CorruptRow;

impl std::fmt::Display for CorruptRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stored Project row failed validation")
    }
}

impl std::error::Error for CorruptRow {}

/// Insert the application's envelope, unchanged, on the same
/// transaction as the row it records.
fn append_timeline(
    conn: &rusqlite::Connection,
    envelope: &TimelineEnvelope,
) -> Result<(), ApiError> {
    insert_event(conn, envelope).map_err(|error| ApiError::internal(&error.to_string()))
}

#[cfg(test)]
mod tests {
    use kanban_app::{InitiativeStore, ProjectStore, TimelineEnvelope};
    use kanban_domain::{
        InitiativeId, InitiativeName, NumberKind, Project, ProjectCounters, ProjectId,
        ProjectRegistration, ProjectState,
    };
    use kanban_dto::{
        ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineScope,
    };
    use serde_json::json;

    use super::SqliteProjectStore;
    use crate::db::Database;
    use crate::initiatives::SqliteInitiativeStore;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;
    use crate::timeline::TimelineFilter;

    fn store() -> (tempfile::TempDir, Database, SqliteProjectStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let store = SqliteProjectStore::new(&database);
        (dir, database, store)
    }

    fn registration(code: &str, session: &str) -> ProjectRegistration {
        ProjectRegistration::new(
            code,
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            session,
            None,
        )
        .expect("the fixture registration validates")
    }

    /// The envelope the application layer builds for one Project
    /// transition, as the store receives it: on the Project's own
    /// timeline, inside the closed `transition` kind.
    fn transition(id: ProjectId, action: &str, facts: serde_json::Value) -> TimelineEnvelope {
        let mut detail = facts;
        let object = detail.as_object_mut().expect("the facts are an object");
        object.insert("action".to_owned(), serde_json::Value::from(action));
        object.insert("id".to_owned(), serde_json::Value::from(id.value()));
        TimelineEnvelope::project(
            &id.value().to_string(),
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Project,
                id: id.value().to_string(),
            }),
            detail,
        )
        .expect("a minted Project identity names a Project")
    }

    /// The envelope builder a registration hands the store.
    fn registered(code: &'static str) -> impl Fn(ProjectId) -> TimelineEnvelope {
        move |id| transition(id, "registered", json!({ "code": code }))
    }

    /// The archive envelope for one Project.
    fn archived(id: ProjectId) -> TimelineEnvelope {
        transition(id, "archived", json!({}))
    }

    fn stored_rows(database: &Database) -> Vec<(i64, String, String, i64, i64, i64, i64)> {
        let conn = database.connection();
        let mut statement = conn
            .prepare(
                "SELECT id, code, herdr_session, archived, plan_counter, spec_counter, \
                 ticket_counter FROM projects ORDER BY id",
            )
            .expect("the projects table is readable");
        statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .expect("the row query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the rows decode")
    }

    /// Every timeline row as stored: scope, Project identity, kind,
    /// entity reference, and detail.
    type StoredTimelineRow = (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        serde_json::Value,
    );

    fn timeline_rows(database: &Database) -> Vec<StoredTimelineRow> {
        let conn = database.connection();
        let mut statement = conn
            .prepare(
                "SELECT scope, project_id, kind, entity_kind, entity_id, detail
                 FROM timeline_events ORDER BY id",
            )
            .expect("the timeline is readable");
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    serde_json::from_str(&row.get::<_, String>(5)?)
                        .expect("the stored detail is JSON"),
                ))
            })
            .expect("the timeline query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the timeline rows decode")
    }

    #[test]
    fn creating_lands_the_row_and_its_timeline_append() {
        let (_dir, database, store) = store();

        let project = store
            .create(&registration("CORE", "kanban-main"), &registered("CORE"))
            .expect("the registration lands");

        assert_eq!(project.id().value(), 1);
        assert_eq!(
            stored_rows(&database),
            vec![(1, "CORE".to_owned(), "kanban-main".to_owned(), 0, 0, 0, 0)]
        );
        assert_eq!(
            timeline_rows(&database),
            vec![(
                "project".to_owned(),
                "1".to_owned(),
                "transition".to_owned(),
                Some("project".to_owned()),
                Some("1".to_owned()),
                json!({ "code": "CORE", "action": "registered", "id": 1 }),
            )],
            "the envelope reaches the Project's own timeline unchanged"
        );
    }

    #[test]
    fn creating_refuses_a_duplicate_code() {
        let (_dir, database, store) = store();
        store
            .create(&registration("CORE", "kanban-main"), &registered("CORE"))
            .expect("the first registration lands");

        let error = store
            .create(&registration("CORE", "wave-main"), &registered("CORE"))
            .expect_err("the duplicate code is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the project code `CORE` is already registered"
        );
        assert_eq!(stored_rows(&database).len(), 1);
        assert_eq!(
            timeline_rows(&database).len(),
            1,
            "the refusal appended nothing"
        );
    }

    #[test]
    fn creating_refuses_a_duplicate_herdr_session_name() {
        let (_dir, database, store) = store();
        store
            .create(&registration("CORE", "kanban-main"), &registered("CORE"))
            .expect("the first registration lands");

        let error = store
            .create(&registration("WAVE", "kanban-main"), &registered("WAVE"))
            .expect_err("the duplicate session name is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the Herdr session name `kanban-main` is already exclusive to another Project"
        );
        assert_eq!(stored_rows(&database).len(), 1);
    }

    #[test]
    fn an_archived_project_keeps_its_session_name_exclusive() {
        let (_dir, database, store) = store();
        let mut first = store
            .create(&registration("CORE", "kanban-main"), &registered("CORE"))
            .expect("the first registration lands");
        first.archive().expect("active archives");
        store
            .save(&first, archived(first.id()))
            .expect("the archive lands");

        let error = store
            .create(&registration("WAVE", "kanban-main"), &registered("WAVE"))
            .expect_err("the session name stays exclusive");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(stored_rows(&database).len(), 1);
    }

    #[test]
    fn creating_refuses_an_unknown_initiative() {
        let (_dir, database, store) = store();
        let registration = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban-main",
            Some(InitiativeId::new(9)),
        )
        .expect("the fixture registration validates");

        let error = store
            .create(&registration, &registered("CORE"))
            .expect_err("the foreign key refuses the unknown Initiative");

        assert_eq!(error.code, ErrorCode::Internal, "the app checks first");
        assert!(stored_rows(&database).is_empty());
        assert!(timeline_rows(&database).is_empty());
    }

    #[test]
    fn creating_links_a_stored_initiative() {
        let (_dir, database, _store) = store();
        let initiatives = SqliteInitiativeStore::new(&database);
        let initiative = initiatives
            .create(
                &InitiativeName::new("Reliability").expect("the name validates"),
                &|id| {
                    TimelineEnvelope::global(
                        TimelineEventKind::Transition,
                        Some(TimelineEntityRef {
                            kind: TimelineEntityKind::Initiative,
                            id: id.value().to_string(),
                        }),
                        json!({ "name": "Reliability", "action": "created", "id": id.value() }),
                    )
                },
            )
            .expect("the Initiative lands");
        let store = SqliteProjectStore::new(&database);
        let registration = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban-main",
            Some(initiative.id()),
        )
        .expect("the fixture registration validates");
        store
            .create(&registration, &registered("CORE"))
            .expect("the registration lands");

        let found = store
            .find(ProjectId::new(1))
            .expect("the find serves")
            .expect("the Project exists");
        assert_eq!(
            found.registration().initiative(),
            Some(initiative.id()),
            "the Initiative link round-trips"
        );
    }

    #[test]
    fn finding_returns_the_stored_project_or_none() {
        let (_dir, _database, store) = store();
        store
            .create(&registration("CORE", "kanban-main"), &registered("CORE"))
            .expect("the registration lands");

        let found = store
            .find(ProjectId::new(1))
            .expect("the find serves")
            .expect("the Project exists");
        assert_eq!(found.code().as_str(), "CORE");
        assert_eq!(found.version(), 1);
        assert!(
            store
                .find(ProjectId::new(9))
                .expect("the find serves")
                .is_none()
        );
    }

    #[test]
    fn saving_persists_the_archive_and_its_append() {
        let (_dir, database, store) = store();
        let mut project = store
            .create(&registration("CORE", "kanban-main"), &registered("CORE"))
            .expect("the registration lands");
        project.archive().expect("active archives");

        store
            .save(&project, archived(project.id()))
            .expect("the save lands");

        assert_eq!(
            stored_rows(&database),
            vec![(1, "CORE".to_owned(), "kanban-main".to_owned(), 1, 0, 0, 0)]
        );
        let archived_at: Option<String> = {
            let conn = database.connection();
            conn.query_row("SELECT archived_at FROM projects WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("the archived timestamp is readable")
        };
        assert!(
            archived_at.is_some_and(|stamp| !stamp.is_empty()),
            "archiving records when it happened"
        );
        assert_eq!(
            timeline_rows(&database)
                .last()
                .cloned()
                .expect("the archive appended"),
            (
                "project".to_owned(),
                "1".to_owned(),
                "transition".to_owned(),
                Some("project".to_owned()),
                Some("1".to_owned()),
                json!({ "action": "archived", "id": 1 }),
            )
        );
    }

    #[test]
    fn archiving_preserves_the_minted_counters() {
        let (_dir, database, store) = store();
        store
            .create(&registration("CORE", "kanban-main"), &registered("CORE"))
            .expect("the registration lands");
        // Stand in for the later slices that mint numbers: the stored
        // counters move, the aggregate rehydrates them, then the
        // Project archives.
        database
            .connection()
            .execute(
                "UPDATE projects SET plan_counter = 3, spec_counter = 1, ticket_counter = 7 \
                 WHERE id = 1",
                [],
            )
            .expect("the counters advance");
        let mut project = store
            .find(ProjectId::new(1))
            .expect("the find serves")
            .expect("the Project exists");
        project.archive().expect("active archives");

        store
            .save(&project, archived(project.id()))
            .expect("the save lands");

        let found = store
            .find(ProjectId::new(1))
            .expect("the find serves")
            .expect("the Project exists");
        assert!(found.is_archived());
        assert_eq!(found.counters().last(NumberKind::Plan), 3);
        assert_eq!(found.counters().last(NumberKind::Spec), 1);
        assert_eq!(found.counters().last(NumberKind::Ticket), 7);
        assert_eq!(
            stored_rows(&database),
            vec![(1, "CORE".to_owned(), "kanban-main".to_owned(), 1, 3, 1, 7)]
        );
    }

    #[test]
    fn saving_persists_the_counters_the_aggregate_holds() {
        let (_dir, database, store) = store();
        let created = store
            .create(&registration("CORE", "kanban-main"), &registered("CORE"))
            .expect("the registration lands");
        // Stand in for the later slices that mint numbers: the
        // aggregate rehydrates, mints in memory, and saves, so the row
        // must take the aggregate's counters rather than keep stale
        // ones a reload would mint again.
        let mut minted = Project::restore(
            created.id(),
            created.registration().clone(),
            ProjectState::Active,
            ProjectCounters::restore(4, 2, 6),
            created.version(),
        );
        minted.archive().expect("active archives");

        store
            .save(&minted, archived(minted.id()))
            .expect("the save lands");

        assert_eq!(
            stored_rows(&database),
            vec![(1, "CORE".to_owned(), "kanban-main".to_owned(), 1, 4, 2, 6)]
        );
        let found = store
            .find(ProjectId::new(1))
            .expect("the find serves")
            .expect("the Project exists");
        assert_eq!(found.counters().last(NumberKind::Plan), 4);
        assert_eq!(found.counters().last(NumberKind::Spec), 2);
        assert_eq!(found.counters().last(NumberKind::Ticket), 6);
    }

    #[test]
    fn saving_an_unknown_project_is_not_found() {
        let (_dir, _database, store) = store();
        let ghost = Project::new(ProjectId::new(9), registration("CORE", "kanban-main"));

        let error = store
            .save(&ghost, archived(ghost.id()))
            .expect_err("the unknown Project is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn a_stale_storage_write_returns_stale_version_without_a_timeline_row() {
        let (_dir, database, store) = store();
        let project = store
            .create(&registration("CORE", "kanban-main"), &registered("CORE"))
            .expect("the registration lands");
        let mut stale = project.clone();
        stale.archive().expect("active archives");
        let mut current = project;
        current.archive().expect("active archives");
        store
            .save(&current, archived(current.id()))
            .expect("the first save lands");
        let timeline_before = timeline_rows(&database).len();

        let error = store
            .save(&stale, archived(stale.id()))
            .expect_err("the stale save is refused");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(2));
        assert_eq!(
            timeline_rows(&database).len(),
            timeline_before,
            "a stale save must not append a timeline row"
        );
        assert_eq!(
            stored_rows(&database),
            vec![(1, "CORE".to_owned(), "kanban-main".to_owned(), 1, 0, 0, 0)],
            "the winning write must remain authoritative"
        );
    }

    /// A Project's own history is read back through the timeline query
    /// surface, so every row it lands must sit inside the closed
    /// vocabulary the query decodes. Rows outside it would make the
    /// whole Project query fail and need a migration to repair.
    #[test]
    fn project_history_decodes_from_the_projects_own_timeline() {
        let (_dir, database, store) = store();
        let mut project = store
            .create(&registration("CORE", "kanban-main"), &registered("CORE"))
            .expect("the registration lands");
        project.archive().expect("active archives");
        store
            .save(&project, archived(project.id()))
            .expect("the archive lands");

        let rows = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project("1".to_owned())))
            .expect("the Project timeline is readable");

        assert_eq!(rows.len(), 2, "registration and archive both land");
        for row in &rows {
            assert_eq!(row.scope, "project");
            assert_eq!(row.project_id, "1");
            assert_eq!(
                TimelineEventKind::parse(&row.kind),
                Some(TimelineEventKind::Transition),
                "`{}` must decode without migration repair",
                row.kind
            );
            assert_eq!(
                row.entity_kind
                    .as_deref()
                    .and_then(TimelineEntityKind::parse),
                Some(TimelineEntityKind::Project)
            );
            assert_eq!(row.entity_id.as_deref(), Some("1"));
        }
        assert_eq!(
            rows.iter()
                .map(|row| row.detail["action"].clone())
                .collect::<Vec<_>>(),
            vec![json!("registered"), json!("archived")],
            "the action names each transition"
        );
    }

    #[test]
    fn listing_covers_every_project_in_id_order() {
        let (_dir, _database, store) = store();
        for (code, session) in [("CORE", "kanban-main"), ("WAVE", "wave-main")] {
            store
                .create(&registration(code, session), &move |id| {
                    transition(id, "registered", json!({ "code": code }))
                })
                .expect("the registration lands");
        }

        let listed = store.list().expect("the list serves");

        let codes: Vec<_> = listed
            .iter()
            .map(|project| project.code().as_str())
            .collect();
        assert_eq!(codes, vec!["CORE", "WAVE"]);
    }

    #[test]
    fn deleting_a_project_is_refused_by_the_schema() {
        let (_dir, _database, store) = store();
        store
            .create(&registration("CORE", "kanban-main"), &registered("CORE"))
            .expect("the registration lands");

        let outcome = store
            .lock()
            .execute("DELETE FROM projects WHERE id = 1", []);

        let error = outcome.expect_err("the schema must refuse deletes");
        assert!(
            error.to_string().contains("never deleted"),
            "the refusal should say never deleted, got: {error}"
        );
        let survivor = store
            .find(ProjectId::new(1))
            .expect("the find serves")
            .expect("the Project survives");
        assert_eq!(survivor.code().as_str(), "CORE");
    }

    #[test]
    fn the_store_serves_through_a_shared_connection() {
        let (_dir, _database, store) = store();
        let boxed: Box<dyn ProjectStore> = Box::new(store);

        // The boxed trait object must be usable from another thread:
        // the core serves commands across transport threads.
        let served = std::thread::spawn(move || {
            boxed
                .create(&registration("CORE", "kanban-main"), &registered("CORE"))
                .map(|project| project.code().as_str().to_owned())
        })
        .join()
        .expect("the serving thread finishes");

        assert_eq!(
            served.expect("the threaded registration lands"),
            "CORE",
            "the port is Send + Sync over one connection"
        );
    }
}
