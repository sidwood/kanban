//! The SQLite implementation of the Spec storage port: rows in
//! `specs` carrying the execution state and Plan binding, the content
//! versions in `spec_versions` with all nine PRD sections and their
//! state, and the application's timeline envelope landing unchanged
//! in the same transaction as every change. Creating a Spec persists
//! the Project counter its number minted in the same write. The
//! schema-level triggers keep approved and superseded content
//! immutable and version states moving only forward, so a Ticket
//! pinned to any version keeps resolving.

use kanban_app::{SpecStore, TimelineEnvelope};
use kanban_domain::{
    Project, ProjectId, Spec, SpecContent, SpecContentState, SpecExecutionState, SpecId,
    SpecNumber, SpecVersion,
};
use kanban_dto::ApiError;
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

/// Every stored column of one Spec row, in select order.
const SPEC_COLUMNS: &str = "id, project_id, number, execution, plan_id, version";

/// The Spec port over the authoritative database.
pub struct SqliteSpecStore {
    conn: ConnectionHandle,
}

impl SqliteSpecStore {
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

impl SpecStore for SqliteSpecStore {
    fn create(
        &self,
        project: &Project,
        number: SpecNumber,
        content: &SpecContent,
        envelope: &dyn Fn(SpecId) -> TimelineEnvelope,
    ) -> Result<Spec, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        // The minted number and the Project row move together: a
        // stale writer can never rewind a minted counter.
        let preceding_version = project.version() - 1;
        let changed = span
            .execute(
                "UPDATE projects
                 SET spec_counter = ?2,
                     version = ?3
                 WHERE id = ?1 AND version = ?4",
                params![
                    project.id().value() as i64,
                    number.value() as i64,
                    project.version() as i64,
                    preceding_version as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(project_write_refused(
                &span,
                project.id(),
                preceding_version,
            ));
        }
        span.execute(
            "INSERT INTO specs (project_id, number, execution, plan_id, version)
             VALUES (?1, ?2, 'unplanned', NULL, 1)",
            params![project.id().value() as i64, number.value() as i64],
        )
        .map_err(internal)?;
        let id = SpecId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the Spec identity overflowed"))?,
        );
        write_version_row(
            &span,
            id,
            &SpecVersion::new(1, SpecContentState::Draft, content.clone()),
        )
        .map_err(internal)?;
        append_timeline(&span, &envelope(id))?;
        span.commit().map_err(internal)?;
        Spec::new(id, project.id(), number, content.clone())
            .map_err(|error| ApiError::internal(&error.to_string()))
    }

    fn find(&self, id: SpecId) -> Result<Option<Spec>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            &format!("SELECT {SPEC_COLUMNS} FROM specs WHERE id = ?1"),
            params![id.value() as i64],
            load_spec_row,
        );
        match row {
            Ok(loaded) => loaded.rehydrate(&conn).map(Some).map_err(internal),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn find_by_number(
        &self,
        project: ProjectId,
        number: SpecNumber,
    ) -> Result<Option<Spec>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            &format!("SELECT {SPEC_COLUMNS} FROM specs WHERE project_id = ?1 AND number = ?2"),
            params![project.value() as i64, number.value() as i64],
            load_spec_row,
        );
        match row {
            Ok(loaded) => loaded.rehydrate(&conn).map(Some).map_err(internal),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn save(&self, spec: &Spec, envelope: TimelineEnvelope) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let preceding_version = spec.version() - 1;
        let changed = span
            .execute(
                "UPDATE specs SET execution = ?2, plan_id = ?3, version = ?4
                 WHERE id = ?1 AND version = ?5",
                params![
                    spec.id().value() as i64,
                    execution_column(spec.execution()),
                    spec.plan().map(|plan| plan.value() as i64),
                    spec.version() as i64,
                    preceding_version as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(spec_write_refused(&span, spec.id(), preceding_version));
        }
        for version in spec.versions() {
            upsert_version_row(&span, spec.id(), version).map_err(internal)?;
        }
        append_timeline(&span, &envelope)?;
        span.commit().map_err(internal)?;
        Ok(())
    }

    fn list(&self, project: ProjectId) -> Result<Vec<Spec>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {SPEC_COLUMNS} FROM specs WHERE project_id = ?1 ORDER BY id"
            ))
            .map_err(internal)?;
        let rows = statement
            .query_map(params![project.value() as i64], load_spec_row)
            .map_err(internal)?;
        let mut specs = Vec::new();
        for row in rows {
            let loaded = row.map_err(internal)?;
            specs.push(loaded.rehydrate(&conn).map_err(internal)?);
        }
        Ok(specs)
    }
}

/// One decoded `specs` row before its versions are loaded.
struct LoadedSpec {
    id: u64,
    project: u64,
    number: u64,
    execution: String,
    plan: Option<i64>,
    version: u64,
}

/// Decode one `specs` row.
fn load_spec_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LoadedSpec> {
    Ok(LoadedSpec {
        id: row.get::<_, i64>(0)?.unsigned_abs(),
        project: row.get::<_, i64>(1)?.unsigned_abs(),
        number: row.get::<_, i64>(2)?.unsigned_abs(),
        execution: row.get::<_, String>(3)?,
        plan: row.get::<_, Option<i64>>(4)?,
        version: row.get::<_, i64>(5)?.unsigned_abs(),
    })
}

impl LoadedSpec {
    /// Load every content version and assemble the aggregate. Every
    /// stored value passed validation on the way in, so a failure
    /// here is corruption the caller must hear about, not silently
    /// accept.
    fn rehydrate(&self, conn: &rusqlite::Connection) -> rusqlite::Result<Spec> {
        let mut statement = conn.prepare(
            "SELECT number, state, name, short_description, problem_statement,
                    solution, user_stories, implementation_decisions,
                    testing_decisions, out_of_scope, further_notes
             FROM spec_versions WHERE spec_id = ?1 ORDER BY number",
        )?;
        let rows = statement.query_map(params![self.id as i64], |row| {
            Ok(SpecVersion::new(
                row.get::<_, i64>(0)?.unsigned_abs(),
                parse_content_state(&row.get::<_, String>(1)?).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(CorruptRow),
                    )
                })?,
                SpecContent::restore(
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ),
            ))
        })?;
        let mut versions = Vec::new();
        for row in rows {
            versions.push(row?);
        }
        let plan = self
            .plan
            .map(|plan| kanban_domain::PlanId::new(plan.unsigned_abs()));
        Ok(Spec::restore(
            SpecId::new(self.id),
            ProjectId::new(self.project),
            SpecNumber::new(self.number).map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Integer,
                    Box::new(CorruptRow),
                )
            })?,
            versions,
            parse_execution(&self.execution).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(CorruptRow),
                )
            })?,
            plan,
            self.version,
        ))
    }
}

/// Append one content version row. Minted once, never rewritten
/// outside the schema's frozen-content rules.
fn write_version_row(
    span: &WriteSpan<'_>,
    spec: SpecId,
    version: &SpecVersion,
) -> rusqlite::Result<()> {
    span.execute(
        "INSERT INTO spec_versions
             (spec_id, number, state, name, short_description, problem_statement,
              solution, user_stories, implementation_decisions, testing_decisions,
              out_of_scope, further_notes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            spec.value() as i64,
            version.number() as i64,
            content_state_column(version.state()),
            version.content().name(),
            version.content().short_description(),
            version.content().problem_statement(),
            version.content().solution(),
            version.content().user_stories(),
            version.content().implementation_decisions(),
            version.content().testing_decisions(),
            version.content().out_of_scope(),
            version.content().further_notes(),
        ],
    )?;
    Ok(())
}

/// Land one version row whether it was just minted or already
/// recorded. Rewriting a row that has left draft is refused by the
/// schema's immutability trigger, so a rewrite that would silently
/// lose approved content cannot land here.
fn upsert_version_row(
    span: &WriteSpan<'_>,
    spec: SpecId,
    version: &SpecVersion,
) -> rusqlite::Result<()> {
    span.execute(
        "INSERT INTO spec_versions
             (spec_id, number, state, name, short_description, problem_statement,
              solution, user_stories, implementation_decisions, testing_decisions,
              out_of_scope, further_notes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (spec_id, number) DO UPDATE SET
             state = excluded.state,
             name = excluded.name,
             short_description = excluded.short_description,
             problem_statement = excluded.problem_statement,
             solution = excluded.solution,
             user_stories = excluded.user_stories,
             implementation_decisions = excluded.implementation_decisions,
             testing_decisions = excluded.testing_decisions,
             out_of_scope = excluded.out_of_scope,
             further_notes = excluded.further_notes",
        params![
            spec.value() as i64,
            version.number() as i64,
            content_state_column(version.state()),
            version.content().name(),
            version.content().short_description(),
            version.content().problem_statement(),
            version.content().solution(),
            version.content().user_stories(),
            version.content().implementation_decisions(),
            version.content().testing_decisions(),
            version.content().out_of_scope(),
            version.content().further_notes(),
        ],
    )?;
    Ok(())
}

/// The stored name of one execution state.
fn execution_column(state: SpecExecutionState) -> &'static str {
    state.wire_name()
}

/// The execution state a stored row names.
fn parse_execution(column: &str) -> Option<SpecExecutionState> {
    SpecExecutionState::parse(column)
}

/// The stored name of one content state.
fn content_state_column(state: SpecContentState) -> &'static str {
    state.wire_name()
}

/// The content state a stored row names.
fn parse_content_state(column: &str) -> Option<SpecContentState> {
    SpecContentState::parse(column)
}

/// Why a guarded Project write was refused, read from the row's
/// current state.
fn project_write_refused(
    conn: &rusqlite::Connection,
    id: ProjectId,
    attempted_from: u64,
) -> ApiError {
    match conn.query_row(
        "SELECT version FROM projects WHERE id = ?1",
        params![id.value() as i64],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(current) => ApiError::stale_version(attempted_from, current.unsigned_abs()),
        Err(rusqlite::Error::QueryReturnedNoRows) => ApiError::not_found(&format!("project {id}")),
        Err(error) => internal(error),
    }
}

/// Why a guarded Spec write was refused, read from the row's current
/// state.
fn spec_write_refused(conn: &rusqlite::Connection, id: SpecId, attempted_from: u64) -> ApiError {
    match conn.query_row(
        "SELECT version FROM specs WHERE id = ?1",
        params![id.value() as i64],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(current) => ApiError::stale_version(attempted_from, current.unsigned_abs()),
        Err(rusqlite::Error::QueryReturnedNoRows) => ApiError::not_found(&format!("spec {id}")),
        Err(error) => internal(error),
    }
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

/// A stored Spec row failed domain validation.
#[derive(Debug)]
struct CorruptRow;

impl std::fmt::Display for CorruptRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stored Spec row failed validation")
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
    use kanban_app::{PlanStore, ProjectStore, SpecStore, TimelineEnvelope};
    use kanban_domain::{
        NumberKind, Project, ProjectCounters, ProjectId, ProjectRegistration, ProjectState,
        SpecContent, SpecContentState, SpecExecutionState, SpecId, SpecNumber,
    };
    use kanban_dto::{
        ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineScope,
    };
    use serde_json::json;

    use super::SqliteSpecStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::projects::SqliteProjectStore;
    use crate::test_support::scratch_database;
    use crate::timeline::TimelineFilter;

    fn content(name: &str) -> SpecContent {
        SpecContent::new(
            name,
            "Versioned Plan graphs of Specs",
            "Planning must survive change.",
            "Immutable approved versions.",
            "KAN-S3-US4",
            "Supersession is explicit.",
            "Domain tests prove immutability.",
            "The Ticket graph proposal.",
            "None",
        )
        .expect("the fixture content validates")
    }

    /// Content with a changed short description, for material
    /// changes.
    fn revised(name: &str) -> SpecContent {
        SpecContent::new(
            name,
            "A changed short description",
            "Planning must survive change.",
            "Immutable approved versions.",
            "KAN-S3-US4",
            "Supersession is explicit.",
            "Domain tests prove immutability.",
            "The Ticket graph proposal.",
            "None",
        )
        .expect("the fixture content validates")
    }

    fn store() -> (tempfile::TempDir, Database, SqliteSpecStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let store = SqliteSpecStore::new(&database);
        (dir, database, store)
    }

    fn registration() -> ProjectRegistration {
        ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            None,
        )
        .expect("the fixture registration validates")
    }

    /// Seed the Project row the specs write against, returning the
    /// aggregate as stored.
    fn seeded_project(database: &Database) -> Project {
        let projects = SqliteProjectStore::new(database);
        let created = projects
            .create(&registration(), &|id| {
                TimelineEnvelope::project(
                    id.value(),
                    TimelineEventKind::Transition,
                    Some(TimelineEntityRef {
                        kind: TimelineEntityKind::Project,
                        id: id.value().to_string(),
                    }),
                    json!({ "action": "registered", "id": id.value() }),
                )
            })
            .expect("the fixture Project lands");
        Project::restore(
            created.id(),
            created.registration().clone(),
            ProjectState::Active,
            ProjectCounters::restore(0, 0, 0),
            1,
        )
    }

    /// The envelope the application layer builds for one Spec
    /// transition, on the seeded Project's timeline.
    fn transition(spec: SpecId, action: &str, facts: serde_json::Value) -> TimelineEnvelope {
        let mut detail = facts;
        let object = detail.as_object_mut().expect("the facts are an object");
        object.insert("action".to_owned(), serde_json::Value::from(action));
        object.insert("id".to_owned(), serde_json::Value::from(spec.value()));
        TimelineEnvelope::project(
            1,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Spec,
                id: spec.value().to_string(),
            }),
            detail,
        )
    }

    /// Author one Spec on the seeded Project, persisting it, and
    /// return the aggregate as stored.
    fn authored(store: &SqliteSpecStore, project: &Project, name: &str) -> kanban_domain::Spec {
        let mut project = project.clone();
        let number = SpecNumber::new(project.mint(NumberKind::Spec).expect("active mints"))
            .expect("a minted number is positive");
        store
            .create(&project, number, &content(name), &|id| {
                transition(id, "created", json!({ "number": number.value() }))
            })
            .expect("the Spec lands")
    }

    /// Every spec-scoped timeline row's detail, in landing order.
    fn spec_timeline(database: &Database) -> Vec<serde_json::Value> {
        let conn = database.connection();
        let mut statement = conn
            .prepare(
                "SELECT detail FROM timeline_events
                 WHERE scope = 'project' AND entity_kind = 'spec' ORDER BY id",
            )
            .expect("the timeline is readable");
        statement
            .query_map([], |row| {
                let detail: String = row.get(0)?;
                Ok(serde_json::from_str(&detail).expect("stored detail is JSON"))
            })
            .expect("the query runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("the rows decode")
    }

    #[test]
    fn creating_lands_the_row_the_counter_and_the_timeline_append() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);

        let spec = authored(&store, &project, "Registration");

        assert_eq!(spec.id().value(), 1);
        assert_eq!(spec.execution(), SpecExecutionState::Unplanned);
        let stored: (i64, i64, i64, i64) = database
            .connection()
            .query_row(
                "SELECT (SELECT spec_counter FROM projects WHERE id = 1),
                        (SELECT version FROM projects WHERE id = 1),
                        (SELECT COUNT(*) FROM specs WHERE project_id = 1),
                        (SELECT COUNT(*) FROM spec_versions)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("the rows are readable");
        assert_eq!(
            stored,
            (1, 2, 1, 1),
            "the minted number, the Project's version move, the Spec row, and version one land together"
        );
        assert_eq!(
            spec_timeline(&database),
            vec![json!({ "action": "created", "id": 1, "number": 1 })],
            "the envelope reaches the Project's own timeline unchanged"
        );
    }

    #[test]
    fn creating_two_specs_mints_unique_numbers() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let first = authored(&store, &project, "Registration");

        // The first create moved the Project row: reload the
        // aggregate the second create guards on.
        let reloaded = SqliteProjectStore::new(&database)
            .find(project.id())
            .expect("the reload serves")
            .expect("the Project exists");
        let mut moved = reloaded.clone();
        let second_number = SpecNumber::new(moved.mint(NumberKind::Spec).expect("active mints"))
            .expect("a minted number is positive");
        let second = store
            .create(&moved, second_number, &content("Timeline"), &|id| {
                transition(id, "created", json!({ "number": second_number.value() }))
            })
            .expect("the second Spec lands");

        assert_ne!(first.id(), second.id());
        assert_eq!((first.number().value(), second.number().value()), (1, 2));
    }

    #[test]
    fn the_authored_spec_round_trips_with_its_content() {
        let (_dir, _database, store) = store();
        let project = seeded_project(&_database);
        let spec = authored(&store, &project, "Registration");

        let found = store
            .find(spec.id())
            .expect("the find serves")
            .expect("the Spec exists");

        assert_eq!(found, spec);
        assert_eq!(found.versions().len(), 1);
        assert_eq!(found.versions()[0].state(), SpecContentState::Draft);
        assert_eq!(
            found.versions()[0].content().short_description(),
            "Versioned Plan graphs of Specs"
        );
        assert_eq!(found.name(), "Registration");
        assert_eq!(found.next_version_number(), 2);
    }

    #[test]
    fn find_by_number_resolves_within_one_project() {
        let (_dir, _database, store) = store();
        let project = seeded_project(&_database);
        let spec = authored(&store, &project, "Registration");

        let found = store
            .find_by_number(ProjectId::new(1), spec.number())
            .expect("the find serves")
            .expect("the Spec exists");
        assert_eq!(found.id(), spec.id());

        let other = SpecNumber::new(9).expect("the fixture number is positive");
        assert!(
            store
                .find_by_number(ProjectId::new(2), spec.number())
                .expect("the find serves")
                .is_none(),
            "another Project's numbers stay out"
        );
        assert!(
            store
                .find_by_number(ProjectId::new(1), other)
                .expect("the find serves")
                .is_none(),
            "an unminted number resolves to nothing"
        );
    }

    #[test]
    fn editing_the_draft_persists_the_revised_content() {
        let (_dir, _database, store) = store();
        let project = seeded_project(&_database);
        let mut spec = authored(&store, &project, "Registration");

        spec.update_content(revised("Registration"))
            .expect("the draft edits");
        store
            .save(
                &spec,
                transition(spec.id(), "content_edited", json!({ "version": 1 })),
            )
            .expect("the edit lands");

        let found = store
            .find(spec.id())
            .expect("the find serves")
            .expect("the Spec exists");
        assert_eq!(found.versions().len(), 1, "no second version minted");
        assert_eq!(
            found.versions()[0].content().short_description(),
            "A changed short description"
        );
    }

    #[test]
    fn a_material_change_after_approval_mints_and_round_trips() {
        let (_dir, _database, store) = store();
        let project = seeded_project(&_database);
        let mut spec = authored(&store, &project, "Registration");
        spec.approve_version().expect("the draft approves");
        store
            .save(
                &spec,
                transition(spec.id(), "version_approved", json!({ "version": 1 })),
            )
            .expect("the approval lands");

        spec.update_content(revised("Registration, revised"))
            .expect("the material change mints");
        store
            .save(
                &spec,
                transition(spec.id(), "version_minted", json!({ "version": 2 })),
            )
            .expect("the mint lands");

        let found = store
            .find(spec.id())
            .expect("the find serves")
            .expect("the Spec exists");
        assert_eq!(found.versions().len(), 2);
        assert_eq!(found.versions()[0].state(), SpecContentState::Approved);
        assert_eq!(
            found.versions()[0].content().short_description(),
            "Versioned Plan graphs of Specs",
            "the approved content is untouched beside its replacement"
        );
        assert_eq!(found.versions()[1].state(), SpecContentState::Draft);
        assert_eq!(
            found.versions()[1].content().name(),
            "Registration, revised"
        );
    }

    #[test]
    fn superseded_versions_round_trip_with_their_content_intact() {
        let (_dir, _database, store) = store();
        let project = seeded_project(&_database);
        let mut spec = authored(&store, &project, "Registration");
        spec.approve_version().expect("the draft approves");
        store
            .save(
                &spec,
                transition(spec.id(), "version_approved", json!({ "version": 1 })),
            )
            .expect("the approval lands");
        spec.supersede_version(1).expect("the version supersedes");
        store
            .save(
                &spec,
                transition(spec.id(), "version_superseded", json!({ "version": 1 })),
            )
            .expect("the supersession lands");

        let found = store
            .find(spec.id())
            .expect("the find serves")
            .expect("the Spec exists");
        let pinned = found
            .pinned_version(1)
            .expect("the superseded version stays queryable");
        assert_eq!(pinned.state(), SpecContentState::Superseded);
        assert_eq!(pinned.content().name(), "Registration");
    }

    #[test]
    fn planning_persists_the_binding_and_execution() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let mut spec = authored(&store, &project, "Registration");
        // A Plan row to join, created through the plan store's own
        // tables. The Spec's mint already moved the Project row, so
        // reload the aggregate the plan create guards on.
        let plan_store = crate::plan::SqlitePlanStore::new(&database);
        let mut project = SqliteProjectStore::new(&database)
            .find(project.id())
            .expect("the reload serves")
            .expect("the Project exists");
        let plan_number = project.mint(NumberKind::Plan).expect("active mints");
        let plan = plan_store
            .create(&project, plan_number, &|id| {
                TimelineEnvelope::project(
                    1,
                    TimelineEventKind::Transition,
                    Some(TimelineEntityRef {
                        kind: TimelineEntityKind::Plan,
                        id: id.value().to_string(),
                    }),
                    json!({ "action": "created", "id": id.value() }),
                )
            })
            .expect("the fixture Plan lands");

        spec.assign_to_plan(plan.id())
            .expect("the Spec joins its Plan");
        store
            .save(
                &spec,
                transition(
                    spec.id(),
                    "planned",
                    json!({ "plan_id": plan.id().value() }),
                ),
            )
            .expect("the join lands");

        let found = store
            .find(spec.id())
            .expect("the find serves")
            .expect("the Spec exists");
        assert_eq!(found.execution(), SpecExecutionState::Planned);
        assert_eq!(found.plan(), Some(plan.id()));
    }

    #[test]
    fn leaving_a_plan_persists_the_cleared_binding() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let mut spec = authored(&store, &project, "Registration");
        // A Plan row to join, created through the plan store's own
        // tables, because the binding's foreign key must resolve.
        let plan_store = crate::plan::SqlitePlanStore::new(&database);
        let mut project = SqliteProjectStore::new(&database)
            .find(project.id())
            .expect("the reload serves")
            .expect("the Project exists");
        let plan_number = project.mint(NumberKind::Plan).expect("active mints");
        let plan = plan_store
            .create(&project, plan_number, &|id| {
                TimelineEnvelope::project(
                    1,
                    TimelineEventKind::Transition,
                    Some(TimelineEntityRef {
                        kind: TimelineEntityKind::Plan,
                        id: id.value().to_string(),
                    }),
                    json!({ "action": "created", "id": id.value() }),
                )
            })
            .expect("the fixture Plan lands");

        spec.assign_to_plan(plan.id())
            .expect("the Spec joins its Plan");
        store
            .save(
                &spec,
                transition(
                    spec.id(),
                    "planned",
                    json!({ "plan_id": plan.id().value() }),
                ),
            )
            .expect("the join lands");
        spec.leave_plan(plan.id())
            .expect("the Spec leaves its Plan");
        store
            .save(
                &spec,
                transition(
                    spec.id(),
                    "unplanned",
                    json!({ "plan_id": plan.id().value() }),
                ),
            )
            .expect("the leave lands");

        let found = store
            .find(spec.id())
            .expect("the find serves")
            .expect("the Spec exists");
        assert_eq!(found.execution(), SpecExecutionState::Unplanned);
        assert_eq!(found.plan(), None);
    }

    #[test]
    fn a_stale_save_is_refused_without_a_timeline_row() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let spec = authored(&store, &project, "Registration");
        let mut stale = spec.clone();
        let mut current = spec;
        current
            .transition_execution(SpecExecutionState::Cancelled)
            .expect("the Spec cancels");
        store
            .save(
                &current,
                transition(current.id(), "execution_moved", json!({})),
            )
            .expect("the first save lands");
        let timeline_before = spec_timeline(&database).len();
        stale
            .transition_execution(SpecExecutionState::Cancelled)
            .expect("the stale Spec cancels");

        let error = store
            .save(&stale, transition(stale.id(), "execution_moved", json!({})))
            .expect_err("the stale save is refused");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(
            spec_timeline(&database).len(),
            timeline_before,
            "a stale save must not append a timeline row"
        );
    }

    #[test]
    fn saving_an_unknown_spec_is_not_found() {
        let (_dir, _database, store) = store();
        let ghost = kanban_domain::Spec::new(
            SpecId::new(9),
            ProjectId::new(1),
            SpecNumber::new(1).expect("the fixture number is positive"),
            content("Registration"),
        )
        .expect("the fixture content validates");

        let error = store
            .save(&ghost, transition(ghost.id(), "execution_moved", json!({})))
            .expect_err("the unknown Spec is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn deleting_a_spec_is_refused_by_the_schema() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let spec = authored(&store, &project, "Registration");

        let outcome = store.lock().execute(
            "DELETE FROM specs WHERE id = ?1",
            rusqlite::params![spec.id().value() as i64],
        );

        let error = outcome.expect_err("the schema must refuse deletes");
        assert!(
            error.to_string().contains("never deleted"),
            "the refusal should say never deleted, got: {error}"
        );
    }

    #[test]
    fn version_rows_refuse_deletes_and_frozen_rewrites() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let mut spec = authored(&store, &project, "Registration");
        spec.approve_version().expect("the draft approves");
        store
            .save(
                &spec,
                transition(spec.id(), "version_approved", json!({ "version": 1 })),
            )
            .expect("the approval lands");

        let conn = database.connection();
        for sql in [
            "DELETE FROM spec_versions",
            "UPDATE spec_versions SET name = 'Rewritten'",
            "UPDATE spec_versions SET short_description = 'Rewritten'",
            "UPDATE spec_versions SET user_stories = 'Rewritten'",
        ] {
            let outcome = conn.execute(sql, []);

            let error = outcome.expect_err("the frozen rows refuse rewrites");
            assert!(
                error.to_string().contains("immutable")
                    || error.to_string().contains("never deleted"),
                "`{sql}` should refuse, got: {error}"
            );
        }

        let stored: (i64, String) = conn
            .query_row("SELECT COUNT(*), MAX(name) FROM spec_versions", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("the frozen rows are readable");
        assert_eq!(
            stored,
            (1, "Registration".to_owned()),
            "every refused statement left the content intact"
        );
    }

    #[test]
    fn version_state_moves_are_forward_only() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let spec = authored(&store, &project, "Registration");

        let conn = database.connection();
        // Draft → approved is the legal approval.
        conn.execute(
            "UPDATE spec_versions SET state = 'approved' WHERE spec_id = ?1",
            rusqlite::params![spec.id().value() as i64],
        )
        .expect("the approval-shaped move lands");
        // Approved → superseded is the explicit supersession.
        conn.execute(
            "UPDATE spec_versions SET state = 'superseded' WHERE spec_id = ?1",
            rusqlite::params![spec.id().value() as i64],
        )
        .expect("the supersession-shaped move lands");

        for refused in [
            "UPDATE spec_versions SET state = 'draft'",
            "UPDATE spec_versions SET state = 'approved'",
        ] {
            let error = conn
                .execute(refused, [])
                .expect_err("superseded is terminal");
            assert!(
                error.to_string().contains("draft, approved, superseded"),
                "`{refused}` should name the legal moves, got: {error}"
            );
        }
    }

    #[test]
    fn listing_covers_every_spec_of_one_project_in_id_order() {
        let (_dir, _database, store) = store();
        let mut project = seeded_project(&_database);
        for name in ["Registration", "Timeline"] {
            let number = SpecNumber::new(project.mint(NumberKind::Spec).expect("active mints"))
                .expect("a minted number is positive");
            store
                .create(&project, number, &content(name), &|id| {
                    transition(id, "created", json!({ "number": number.value() }))
                })
                .expect("the Spec lands");
        }

        let listed = store.list(ProjectId::new(1)).expect("the list serves");

        let numbers: Vec<_> = listed.iter().map(|spec| spec.number().value()).collect();
        assert_eq!(numbers, vec![1, 2]);
        assert!(
            store
                .list(ProjectId::new(9))
                .expect("the list serves")
                .is_empty(),
            "another Project's Specs stay out"
        );
    }

    #[test]
    fn spec_history_decodes_from_the_projects_own_timeline() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let mut spec = authored(&store, &project, "Registration");
        spec.approve_version().expect("the draft approves");
        store
            .save(
                &spec,
                transition(spec.id(), "version_approved", json!({ "version": 1 })),
            )
            .expect("the approval lands");

        let rows = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project(1)))
            .expect("the Project timeline is readable");

        let spec_rows: Vec<_> = rows
            .iter()
            .filter(|row| row.entity_kind.as_deref() == Some("spec"))
            .collect();
        assert_eq!(spec_rows.len(), 2, "creation and the approval");
        for row in &spec_rows {
            assert_eq!(
                TimelineEventKind::parse(&row.kind),
                Some(TimelineEventKind::Transition),
                "`{}` must decode without migration repair",
                row.kind
            );
        }
        assert_eq!(
            spec_rows.last().expect("the approval row").detail["action"],
            json!("version_approved")
        );
    }

    #[test]
    fn the_store_serves_through_a_shared_connection() {
        let (_dir, _database, store) = store();
        let mut project = seeded_project(&_database);
        let number = SpecNumber::new(project.mint(NumberKind::Spec).expect("active mints"))
            .expect("a minted number is positive");
        let boxed: Box<dyn SpecStore> = Box::new(store);

        let served = std::thread::spawn(move || {
            boxed
                .create(&project, number, &content("Registration"), &|id| {
                    transition(id, "created", json!({ "number": number.value() }))
                })
                .map(|spec| spec.number().value())
        })
        .join()
        .expect("the serving thread finishes");

        assert_eq!(
            served.expect("the threaded creation lands"),
            1,
            "the port is Send + Sync over one connection"
        );
    }
}
