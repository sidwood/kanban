//! The SQLite implementation of the Plan storage port: rows in
//! `plans`, the working graph in `plan_specs` (display order) and
//! `plan_edges` (a separate relation), the immutable frozen versions
//! in `plan_versions` with their own spec and edge rows, and the
//! application's timeline envelope landing unchanged in the same
//! transaction as every change. Creating a Plan persists the Project
//! counter its number minted in the same write.

use kanban_app::{PlanStore, TimelineEnvelope};
use kanban_domain::{
    DependencyEdge, Plan, PlanId, PlanShape, PlanState, PlanVersion, Project, ProjectId, SpecNumber,
};
use kanban_dto::ApiError;
use rusqlite::params;

use crate::db::{ConnectionHandle, Database, WriteSpan};
use crate::timeline::insert_event;

/// Every stored column of one Plan row, in select order.
const PLAN_COLUMNS: &str = "id, project_id, number, state, version";

/// The Plan port over the authoritative database.
pub struct SqlitePlanStore {
    conn: ConnectionHandle,
}

impl SqlitePlanStore {
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

impl PlanStore for SqlitePlanStore {
    fn create(
        &self,
        project: &Project,
        number: u64,
        envelope: &dyn Fn(PlanId) -> TimelineEnvelope,
    ) -> Result<Plan, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        // The minted number and the Project row move together: a
        // stale writer can never rewind a minted counter.
        let preceding_version = project.version() - 1;
        let changed = span
            .execute(
                "UPDATE projects
                 SET plan_counter = ?2,
                     version = ?3
                 WHERE id = ?1 AND version = ?4",
                params![
                    project.id().value() as i64,
                    number as i64,
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
            "INSERT INTO plans (project_id, number, state, version)
             VALUES (?1, ?2, 'draft', 1)",
            params![project.id().value() as i64, number as i64],
        )
        .map_err(internal)?;
        let id = PlanId::new(
            span.last_insert_rowid()
                .try_into()
                .map_err(|_| ApiError::internal("the Plan identity overflowed"))?,
        );
        append_timeline(&span, &envelope(id))?;
        span.commit().map_err(internal)?;
        Ok(Plan::new(id, project.id(), number))
    }

    fn find(&self, id: PlanId) -> Result<Option<Plan>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            &format!("SELECT {PLAN_COLUMNS} FROM plans WHERE id = ?1"),
            params![id.value() as i64],
            |row| {
                Ok(LoadedPlan {
                    id: row.get::<_, i64>(0)?.unsigned_abs(),
                    project: row.get::<_, i64>(1)?.unsigned_abs(),
                    number: row.get::<_, i64>(2)?.unsigned_abs(),
                    state: row.get::<_, String>(3)?,
                    version: row.get::<_, i64>(4)?.unsigned_abs(),
                })
            },
        );
        match row {
            Ok(loaded) => loaded.rehydrate(&conn).map(Some).map_err(internal),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn save(
        &self,
        plan: &Plan,
        freeze: Option<&PlanVersion>,
        envelope: TimelineEnvelope,
    ) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let preceding_version = plan.version() - 1;
        let changed = span
            .execute(
                "UPDATE plans SET state = ?2, version = ?3 WHERE id = ?1 AND version = ?4",
                params![
                    plan.id().value() as i64,
                    state_column(plan.state()),
                    plan.version() as i64,
                    preceding_version as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(plan_write_refused(&span, plan.id(), preceding_version));
        }
        write_working_graph(&span, plan)?;
        if let Some(frozen) = freeze {
            write_frozen_version(&span, plan.id(), frozen)?;
        }
        append_timeline(&span, &envelope)?;
        span.commit().map_err(internal)?;
        Ok(())
    }

    fn list(&self, project: ProjectId) -> Result<Vec<Plan>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {PLAN_COLUMNS} FROM plans WHERE project_id = ?1 ORDER BY id"
            ))
            .map_err(internal)?;
        let rows = statement
            .query_map(params![project.value() as i64], |row| {
                Ok(LoadedPlan {
                    id: row.get::<_, i64>(0)?.unsigned_abs(),
                    project: row.get::<_, i64>(1)?.unsigned_abs(),
                    number: row.get::<_, i64>(2)?.unsigned_abs(),
                    state: row.get::<_, String>(3)?,
                    version: row.get::<_, i64>(4)?.unsigned_abs(),
                })
            })
            .map_err(internal)?;
        let mut plans = Vec::new();
        for row in rows {
            let loaded = row.map_err(internal)?;
            plans.push(loaded.rehydrate(&conn).map_err(internal)?);
        }
        Ok(plans)
    }
}

/// One decoded `plans` row before its graph is loaded.
struct LoadedPlan {
    id: u64,
    project: u64,
    number: u64,
    state: String,
    version: u64,
}

impl LoadedPlan {
    /// Load the working graph and every frozen version, and assemble
    /// the aggregate. Every stored value passed validation on the way
    /// in, so a failure here is corruption the caller must hear
    /// about, not silently accept.
    fn rehydrate(&self, conn: &rusqlite::Connection) -> rusqlite::Result<Plan> {
        let shape = PlanShape::new(
            load_specs(
                conn,
                "SELECT spec_number FROM plan_specs WHERE plan_id = ?1 ORDER BY position",
                self.id,
            )?,
            load_edges(
                conn,
                "SELECT from_spec, to_spec FROM plan_edges WHERE plan_id = ?1 ORDER BY from_spec, to_spec",
                self.id,
            )?,
        );
        let mut versions = Vec::new();
        let mut statement = conn
            .prepare("SELECT id, number FROM plan_versions WHERE plan_id = ?1 ORDER BY number")?;
        let rows: Vec<(i64, i64)> = statement
            .query_map(params![self.id as i64], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (row_id, number) in rows {
            let version_id = row_id.unsigned_abs();
            versions.push(PlanVersion::new(
                number.unsigned_abs(),
                load_specs(
                    conn,
                    "SELECT spec_number FROM plan_version_specs
                     WHERE version_id = ?1 ORDER BY position",
                    version_id,
                )?,
                load_edges(
                    conn,
                    "SELECT from_spec, to_spec FROM plan_version_edges
                     WHERE version_id = ?1 ORDER BY from_spec, to_spec",
                    version_id,
                )?,
            ));
        }
        Ok(Plan::restore(
            PlanId::new(self.id),
            ProjectId::new(self.project),
            self.number,
            parse_state(&self.state)?,
            shape,
            versions,
            self.version,
        ))
    }
}

/// The display order of one plan or version, in stored position
/// order.
fn load_specs(
    conn: &rusqlite::Connection,
    sql: &str,
    owner: u64,
) -> rusqlite::Result<Vec<SpecNumber>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params![owner as i64], |row| {
        row.get::<_, i64>(0).map(|number| number.unsigned_abs())
    })?;
    let mut specs = Vec::new();
    for row in rows {
        specs.push(SpecNumber::new(row?).map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Integer,
                Box::new(CorruptRow),
            )
        })?);
    }
    Ok(specs)
}

/// The dependency edges of one plan or version.
fn load_edges(
    conn: &rusqlite::Connection,
    sql: &str,
    owner: u64,
) -> rusqlite::Result<Vec<DependencyEdge>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params![owner as i64], |row| {
        Ok((
            row.get::<_, i64>(0)?.unsigned_abs(),
            row.get::<_, i64>(1)?.unsigned_abs(),
        ))
    })?;
    let mut edges = Vec::new();
    for row in rows {
        let (from, to) = row?;
        edges.push(DependencyEdge::new(
            SpecNumber::new(from).map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Integer,
                    Box::new(CorruptRow),
                )
            })?,
            SpecNumber::new(to).map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Integer,
                    Box::new(CorruptRow),
                )
            })?,
        ));
    }
    Ok(edges)
}

/// Replace the working graph: the mutable shape a draft edits. The
/// frozen version rows are the audit and are never touched here.
fn write_working_graph(span: &WriteSpan<'_>, plan: &Plan) -> Result<(), ApiError> {
    span.execute(
        "DELETE FROM plan_specs WHERE plan_id = ?1",
        params![plan.id().value() as i64],
    )
    .map_err(internal)?;
    for (position, spec) in plan.order().iter().enumerate() {
        span.execute(
            "INSERT INTO plan_specs (plan_id, position, spec_number) VALUES (?1, ?2, ?3)",
            params![
                plan.id().value() as i64,
                position as i64,
                spec.value() as i64
            ],
        )
        .map_err(internal)?;
    }
    span.execute(
        "DELETE FROM plan_edges WHERE plan_id = ?1",
        params![plan.id().value() as i64],
    )
    .map_err(internal)?;
    for edge in plan.edges() {
        span.execute(
            "INSERT INTO plan_edges (plan_id, from_spec, to_spec) VALUES (?1, ?2, ?3)",
            params![
                plan.id().value() as i64,
                edge.from().value() as i64,
                edge.to().value() as i64,
            ],
        )
        .map_err(internal)?;
    }
    Ok(())
}

/// Append one immutable frozen version. Append-only: a number minted
/// once is never rewritten.
fn write_frozen_version(
    span: &WriteSpan<'_>,
    plan: PlanId,
    frozen: &PlanVersion,
) -> Result<(), ApiError> {
    span.execute(
        "INSERT INTO plan_versions (plan_id, number) VALUES (?1, ?2)",
        params![plan.value() as i64, frozen.number() as i64],
    )
    .map_err(internal)?;
    let version_id = span.last_insert_rowid();
    for (position, spec) in frozen.order().iter().enumerate() {
        span.execute(
            "INSERT INTO plan_version_specs (version_id, position, spec_number)
             VALUES (?1, ?2, ?3)",
            params![version_id, position as i64, spec.value() as i64],
        )
        .map_err(internal)?;
    }
    for edge in frozen.edges() {
        span.execute(
            "INSERT INTO plan_version_edges (version_id, from_spec, to_spec)
             VALUES (?1, ?2, ?3)",
            params![
                version_id,
                edge.from().value() as i64,
                edge.to().value() as i64,
            ],
        )
        .map_err(internal)?;
    }
    Ok(())
}

/// The stored name of one lifecycle state.
fn state_column(state: PlanState) -> &'static str {
    match state {
        PlanState::Draft => "draft",
        PlanState::Active => "active",
        PlanState::Complete => "complete",
        PlanState::Cancelled => "cancelled",
        PlanState::Archived => "archived",
    }
}

/// The lifecycle state a stored row names.
fn parse_state(column: &str) -> rusqlite::Result<PlanState> {
    match column {
        "draft" => Ok(PlanState::Draft),
        "active" => Ok(PlanState::Active),
        "complete" => Ok(PlanState::Complete),
        "cancelled" => Ok(PlanState::Cancelled),
        "archived" => Ok(PlanState::Archived),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(CorruptRow),
        )),
    }
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

/// Why a guarded Plan write was refused, read from the row's current
/// state.
fn plan_write_refused(conn: &rusqlite::Connection, id: PlanId, attempted_from: u64) -> ApiError {
    match conn.query_row(
        "SELECT version FROM plans WHERE id = ?1",
        params![id.value() as i64],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(current) => ApiError::stale_version(attempted_from, current.unsigned_abs()),
        Err(rusqlite::Error::QueryReturnedNoRows) => ApiError::not_found(&format!("plan {id}")),
        Err(error) => internal(error),
    }
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

/// A stored Plan row failed domain validation.
#[derive(Debug)]
struct CorruptRow;

impl std::fmt::Display for CorruptRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stored Plan row failed validation")
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
    use kanban_app::{PlanStore, ProjectStore, TimelineEnvelope};
    use kanban_domain::{
        DependencyEdge, NumberKind, Plan, PlanId, PlanShape, PlanState, PlanVersion, Project,
        ProjectCounters, ProjectId, ProjectRegistration, ProjectState, SpecNumber,
    };
    use kanban_dto::{
        ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineScope,
    };
    use serde_json::json;

    use super::SqlitePlanStore;
    use crate::db::Database;
    use crate::migrations::AllowAllMigrations;
    use crate::projects::SqliteProjectStore;
    use crate::test_support::scratch_database;
    use crate::timeline::TimelineFilter;

    fn spec(number: u64) -> SpecNumber {
        SpecNumber::new(number).expect("the fixture number is positive")
    }

    fn edge(from: u64, to: u64) -> DependencyEdge {
        DependencyEdge::new(spec(from), spec(to))
    }

    fn store() -> (tempfile::TempDir, Database, SqlitePlanStore) {
        let (dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let store = SqlitePlanStore::new(&database);
        (dir, database, store)
    }

    fn registration() -> ProjectRegistration {
        ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban-main",
            None,
        )
        .expect("the fixture registration validates")
    }

    /// Seed the Project row the plans write against, with three
    /// minted Spec numbers, and return the aggregate as stored.
    fn seeded_project(database: &Database) -> Project {
        let projects = SqliteProjectStore::new(database);
        let created = projects
            .create(&registration(), &|id| {
                TimelineEnvelope::project(
                    &id.value().to_string(),
                    TimelineEventKind::Transition,
                    Some(TimelineEntityRef {
                        kind: TimelineEntityKind::Project,
                        id: id.value().to_string(),
                    }),
                    json!({ "action": "registered", "id": id.value() }),
                )
                .expect("a minted Project identity names a Project")
            })
            .expect("the fixture Project lands");
        database
            .connection()
            .execute("UPDATE projects SET spec_counter = 3 WHERE id = 1", [])
            .expect("the fixture Spec numbers are minted");
        Project::restore(
            created.id(),
            created.registration().clone(),
            ProjectState::Active,
            ProjectCounters::restore(0, 3, 0),
            1,
        )
    }

    /// The envelope the application layer builds for one Plan
    /// transition, on the seeded Project's timeline.
    fn transition(plan: PlanId, action: &str, facts: serde_json::Value) -> TimelineEnvelope {
        let mut detail = facts;
        let object = detail.as_object_mut().expect("the facts are an object");
        object.insert("action".to_owned(), serde_json::Value::from(action));
        object.insert("id".to_owned(), serde_json::Value::from(plan.value()));
        TimelineEnvelope::project(
            "1",
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Plan,
                id: plan.value().to_string(),
            }),
            detail,
        )
        .expect("a minted Plan identity names a Plan")
    }

    /// Create a Plan and drive it through the domain into the shaped
    /// draft — membership 1, 3, 2 with edges 1 → 2 and 3 → 2 —
    /// persisting every step.
    fn shaped_draft(store: &SqlitePlanStore, project: &Project) -> Plan {
        let mut project = project.clone();
        let number = project.mint(NumberKind::Plan);
        let mut plan = store
            .create(&project, number, &|id| {
                transition(id, "created", json!({ "number": number }))
            })
            .expect("the Plan lands");
        for number in [1, 3, 2] {
            plan.add_spec(spec(number))
                .expect("the fixture membership lands");
            store
                .save(
                    &plan,
                    None,
                    transition(plan.id(), "spec_added", json!({ "spec_number": number })),
                )
                .expect("the edit lands");
        }
        for (from, to) in [(1, 2), (3, 2)] {
            plan.add_edge(spec(from), spec(to))
                .expect("the fixture edge lands");
            store
                .save(
                    &plan,
                    None,
                    transition(
                        plan.id(),
                        "edge_added",
                        json!({ "from_spec": from, "to_spec": to }),
                    ),
                )
                .expect("the edit lands");
        }
        plan
    }

    /// Every plan-scoped timeline row's detail, in landing order.
    fn plan_timeline(database: &Database) -> Vec<serde_json::Value> {
        let conn = database.connection();
        let mut statement = conn
            .prepare(
                "SELECT detail FROM timeline_events
                 WHERE scope = 'project' AND entity_kind = 'plan' ORDER BY id",
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
        let mut project = seeded_project(&database);

        let number = project.mint(NumberKind::Plan);
        let plan = store
            .create(&project, number, &|id| {
                transition(id, "created", json!({ "number": number }))
            })
            .expect("the Plan lands");

        assert_eq!(plan.id().value(), 1);
        assert_eq!(plan.state(), PlanState::Draft);
        let stored: (i64, i64, i64) = database
            .connection()
            .query_row(
                "SELECT (SELECT plan_counter FROM projects WHERE id = 1),
                        (SELECT version FROM projects WHERE id = 1),
                        (SELECT COUNT(*) FROM plans WHERE project_id = 1)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("the rows are readable");
        assert_eq!(
            stored,
            (1, 2, 1),
            "the minted number, the Project's version move, and the Plan row land together"
        );
        assert_eq!(
            plan_timeline(&database),
            vec![json!({ "action": "created", "id": 1, "number": 1 })],
            "the envelope reaches the Project's own timeline unchanged"
        );
    }

    #[test]
    fn creating_two_plans_mints_unique_numbers() {
        let (_dir, database, store) = store();
        let mut project = seeded_project(&database);

        let first_number = project.mint(NumberKind::Plan);
        let first = store
            .create(&project, first_number, &|id| {
                transition(id, "created", json!({ "number": first_number }))
            })
            .expect("the first Plan lands");
        let second_number = project.mint(NumberKind::Plan);
        let second = store
            .create(&project, second_number, &|id| {
                transition(id, "created", json!({ "number": second_number }))
            })
            .expect("the second Plan lands");

        assert_ne!(first.id(), second.id());
        assert_eq!((first.number(), second.number()), (1, 2));
        let listed = store.list(ProjectId::new(1)).expect("the list serves");
        assert_eq!(listed.len(), 2);
    }

    #[test]
    fn the_shaped_draft_round_trips_with_both_relations_separate() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let plan = shaped_draft(&store, &project);

        let found = store
            .find(plan.id())
            .expect("the find serves")
            .expect("the Plan exists");

        assert_eq!(found.state(), PlanState::Draft);
        assert_eq!(found.order(), [spec(1), spec(3), spec(2)].as_slice());
        assert_eq!(
            found.edges(),
            [edge(1, 2), edge(3, 2)].as_slice(),
            "the display order and the edges rehydrate as the two relations they are"
        );
        assert_eq!(
            found,
            Plan::restore(
                plan.id(),
                ProjectId::new(1),
                1,
                PlanState::Draft,
                PlanShape::new(
                    vec![spec(1), spec(3), spec(2)],
                    vec![edge(1, 2), edge(3, 2)],
                ),
                Vec::new(),
                plan.version(),
            ),
            "the rehydrated aggregate equals the stored one"
        );
    }

    #[test]
    fn saving_replaces_the_working_graph() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let mut plan = shaped_draft(&store, &project);

        plan.remove_edge(spec(1), spec(2)).expect("the edge leaves");
        store
            .save(
                &plan,
                None,
                transition(
                    plan.id(),
                    "edge_removed",
                    json!({ "from_spec": 1, "to_spec": 2 }),
                ),
            )
            .expect("the edge removal lands");
        plan.remove_spec(spec(1)).expect("the Spec leaves");
        store
            .save(
                &plan,
                None,
                transition(plan.id(), "spec_removed", json!({ "spec_number": 1 })),
            )
            .expect("the spec removal lands");

        let found = store
            .find(plan.id())
            .expect("the find serves")
            .expect("the Plan exists");
        assert_eq!(found.order(), [spec(3), spec(2)].as_slice());
        assert_eq!(found.edges(), [edge(3, 2)].as_slice());
    }

    #[test]
    fn activating_lands_the_frozen_version_rows_once() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let mut plan = shaped_draft(&store, &project);

        let frozen = plan.activate().expect("the shape freezes");
        store
            .save(
                &plan,
                Some(&frozen),
                transition(
                    plan.id(),
                    "activated",
                    json!({ "frozen_version": frozen.number() }),
                ),
            )
            .expect("the activation lands");

        let stored: (i64, i64, i64, String) = database
            .connection()
            .query_row(
                "SELECT (SELECT COUNT(*) FROM plan_versions WHERE plan_id = 1),
                        (SELECT COUNT(*) FROM plan_version_specs),
                        (SELECT COUNT(*) FROM plan_version_edges),
                        (SELECT state FROM plans WHERE id = 1)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("the frozen rows are readable");
        assert_eq!(
            stored,
            (1, 3, 2, "active".to_owned()),
            "one frozen version, its shape, and the active state"
        );

        let found = store
            .find(plan.id())
            .expect("the find serves")
            .expect("the Plan exists");
        assert_eq!(
            found.versions(),
            [PlanVersion::new(
                1,
                vec![spec(1), spec(3), spec(2)],
                vec![edge(1, 2), edge(3, 2)],
            )]
            .as_slice()
        );
    }

    #[test]
    fn replanning_and_reactivating_keeps_both_versions_queryable() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let mut plan = shaped_draft(&store, &project);

        let first = plan.activate().expect("the first freeze lands");
        store
            .save(
                &plan,
                Some(&first),
                transition(plan.id(), "activated", json!({ "frozen_version": 1 })),
            )
            .expect("the activation lands");
        plan.replan().expect("the draft reopens");
        store
            .save(
                &plan,
                None,
                transition(
                    plan.id(),
                    "replanned",
                    json!({ "reserved_version": 2, "superseded_version": 1 }),
                ),
            )
            .expect("the replan lands");
        plan.move_spec(spec(2), 0).expect("the shape changes");
        store
            .save(
                &plan,
                None,
                transition(
                    plan.id(),
                    "spec_moved",
                    json!({ "spec_number": 2, "position": 0 }),
                ),
            )
            .expect("the move lands");
        let second = plan.activate().expect("the replacement freezes");
        store
            .save(
                &plan,
                Some(&second),
                transition(plan.id(), "activated", json!({ "frozen_version": 2 })),
            )
            .expect("the reactivation lands");

        let found = store
            .find(plan.id())
            .expect("the find serves")
            .expect("the Plan exists");
        assert_eq!(second.number(), 2);
        assert_eq!(
            found.versions(),
            [
                PlanVersion::new(
                    1,
                    vec![spec(1), spec(3), spec(2)],
                    vec![edge(1, 2), edge(3, 2)],
                ),
                PlanVersion::new(
                    2,
                    vec![spec(2), spec(1), spec(3)],
                    vec![edge(1, 2), edge(3, 2)],
                ),
            ]
            .as_slice(),
            "the prior version stays queryable beside its replacement"
        );
    }

    #[test]
    fn a_stale_save_is_refused_without_a_timeline_row() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let plan = shaped_draft(&store, &project);
        let mut stale = plan.clone();
        let mut current = plan;
        current.cancel().expect("the Plan cancels");
        store
            .save(
                &current,
                None,
                transition(current.id(), "cancelled", json!({})),
            )
            .expect("the first save lands");
        let timeline_before = plan_timeline(&database).len();
        stale.cancel().expect("the stale Plan cancels");

        let error = store
            .save(&stale, None, transition(stale.id(), "cancelled", json!({})))
            .expect_err("the stale save is refused");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(
            plan_timeline(&database).len(),
            timeline_before,
            "a stale save must not append a timeline row"
        );
    }

    #[test]
    fn saving_an_unknown_plan_is_not_found() {
        let (_dir, _database, store) = store();
        let ghost = Plan::new(PlanId::new(9), ProjectId::new(1), 1);

        let error = store
            .save(&ghost, None, transition(ghost.id(), "cancelled", json!({})))
            .expect_err("the unknown Plan is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn deleting_a_plan_is_refused_by_the_schema() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let plan = shaped_draft(&store, &project);

        let outcome = store.lock().execute(
            "DELETE FROM plans WHERE id = ?1",
            rusqlite::params![plan.id().value() as i64],
        );

        let error = outcome.expect_err("the schema must refuse deletes");
        assert!(
            error.to_string().contains("never deleted"),
            "the refusal should say never deleted, got: {error}"
        );
    }

    #[test]
    fn frozen_version_rows_refuse_updates_and_deletes() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let mut plan = shaped_draft(&store, &project);
        let frozen = plan.activate().expect("the shape freezes");
        store
            .save(
                &plan,
                Some(&frozen),
                transition(
                    plan.id(),
                    "activated",
                    json!({ "frozen_version": frozen.number() }),
                ),
            )
            .expect("the activation lands");

        let conn = database.connection();
        for sql in [
            "UPDATE plan_versions SET number = 9",
            "DELETE FROM plan_versions",
            "UPDATE plan_version_specs SET position = 9",
            "DELETE FROM plan_version_specs",
            "UPDATE plan_version_edges SET from_spec = 9",
            "DELETE FROM plan_version_edges",
        ] {
            let outcome = conn.execute(sql, []);

            let error = outcome.expect_err("the frozen rows are append-only");
            assert!(
                error.to_string().contains("append-only"),
                "`{sql}` should refuse with append-only, got: {error}"
            );
        }

        let stored: (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM plan_versions),
                        (SELECT COUNT(*) FROM plan_version_specs),
                        (SELECT COUNT(*) FROM plan_version_edges)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("the frozen rows are readable");
        assert_eq!(
            stored,
            (1, 3, 2),
            "every refused statement left the frozen shape intact"
        );
    }

    #[test]
    fn listing_covers_every_plan_of_one_project_in_id_order() {
        let (_dir, database, store) = store();
        let mut project = seeded_project(&database);
        for _ in 0..2 {
            let number = project.mint(NumberKind::Plan);
            store
                .create(&project, number, &|id| {
                    transition(id, "created", json!({ "number": number }))
                })
                .expect("the Plan lands");
        }

        let listed = store.list(ProjectId::new(1)).expect("the list serves");

        let numbers: Vec<_> = listed.iter().map(|plan| plan.number()).collect();
        assert_eq!(numbers, vec![1, 2]);
        assert!(
            store
                .list(ProjectId::new(9))
                .expect("the list serves")
                .is_empty(),
            "another Project's Plans stay out"
        );
    }

    #[test]
    fn plan_history_decodes_from_the_projects_own_timeline() {
        let (_dir, database, store) = store();
        let project = seeded_project(&database);
        let mut plan = shaped_draft(&store, &project);
        let frozen = plan.activate().expect("the shape freezes");
        store
            .save(
                &plan,
                Some(&frozen),
                transition(
                    plan.id(),
                    "activated",
                    json!({ "frozen_version": frozen.number() }),
                ),
            )
            .expect("the activation lands");

        let rows = database
            .query_timeline(&TimelineFilter::of(TimelineScope::Project("1".to_owned())))
            .expect("the Project timeline is readable");

        let plan_rows: Vec<_> = rows
            .iter()
            .filter(|row| row.entity_kind.as_deref() == Some("plan"))
            .collect();
        assert_eq!(
            plan_rows.len(),
            7,
            "creation, three adds, two edges, and the activation"
        );
        for row in &plan_rows {
            assert_eq!(
                TimelineEventKind::parse(&row.kind),
                Some(TimelineEventKind::Transition),
                "`{}` must decode without migration repair",
                row.kind
            );
        }
        assert_eq!(
            plan_rows.last().expect("the activation row").detail["action"],
            json!("activated")
        );
    }

    #[test]
    fn the_store_serves_through_a_shared_connection() {
        let (_dir, database, store) = store();
        let mut project = seeded_project(&database);
        let number = project.mint(NumberKind::Plan);
        let boxed: Box<dyn PlanStore> = Box::new(store);

        let served = std::thread::spawn(move || {
            boxed
                .create(&project, number, &|id| {
                    transition(id, "created", json!({ "number": number }))
                })
                .map(|plan| plan.number())
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
