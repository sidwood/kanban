//! Saved Views stored in SQLite (KAN-T28): the named operator
//! perspectives, per-operator presentation data in the authoritative
//! store (DR-BP-05, DR-BP-06). One row holds one view's whole owned
//! set — the filter axes and the group sets as JSON arrays of the
//! vocabularies' wire names — and the schema itself carries the
//! collection rules: one name per scope, one default per scope. No
//! row is seeded here: the application layer generates the missing
//! defaults before any read answers, so the store records what it was
//! given and nothing it was not.

use kanban_app::SavedViewStore;
use kanban_domain::{
    AttentionState as DomainAttention, BoardFilter as DomainFilter, BoardGroup as DomainGroup,
    DonePlacement as DomainDone, InitiativeId, LaneId, PlanId, Priority as DomainPriority,
    ProfileName, ProjectId, SavedView, SavedViewId, SpecId, TicketKind as DomainKind,
    TicketState as DomainState, ViewMode as DomainMode, ViewName, ViewScope as DomainScope,
    ViewSorting as DomainSorting,
};
use kanban_dto::ApiError;
use rusqlite::params;
use serde_json::{Value, json};

use crate::db::{ConnectionHandle, Database, WriteSpan};

/// Every stored column of one view row, in select order after the
/// identity.
const VIEW_COLUMNS: &str = "scope_kind, project_id, name, is_default, filter, \
                            expanded_groups, hidden_columns, mode, done, sorting, version";

/// The saved view port over the authoritative database.
pub struct SqliteSavedViewStore {
    conn: ConnectionHandle,
}

impl SqliteSavedViewStore {
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

impl SavedViewStore for SqliteSavedViewStore {
    fn list(&self) -> Result<Vec<SavedView>, ApiError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(&format!(
                "SELECT id, {VIEW_COLUMNS} FROM saved_views ORDER BY id"
            ))
            .map_err(internal)?;
        let rows = statement
            .query_map([], decode_row)
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        Ok(rows)
    }

    fn find(&self, id: SavedViewId) -> Result<Option<SavedView>, ApiError> {
        let conn = self.lock();
        let row = conn.query_row(
            &format!("SELECT id, {VIEW_COLUMNS} FROM saved_views WHERE id = ?1"),
            params![id.value() as i64],
            decode_row,
        );
        match row {
            Ok(view) => Ok(Some(view)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }

    fn insert(&self, draft: &SavedView) -> Result<SavedView, ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let outcome = span.execute(
            "INSERT INTO saved_views
                 (scope_kind, project_id, name, is_default, filter,
                  expanded_groups, hidden_columns, mode, done, sorting, version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                scope_kind_of(draft.scope()),
                project_column_of(draft.scope()),
                draft.name().as_str(),
                draft.is_default(),
                filter_json(draft.filter()),
                groups_json(draft.expanded()),
                groups_json(draft.hidden()),
                draft.mode().wire_name(),
                draft.done().wire_name(),
                draft.sorting().wire_name(),
                draft.version() as i64,
            ],
        );
        match outcome {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                return Err(kanban_app::already_taken_view_name_error(
                    draft.name().as_str(),
                ));
            }
            Err(error) => return Err(internal(error)),
        }
        let id = SavedViewId::new(span.last_insert_rowid().unsigned_abs());
        span.commit().map_err(internal)?;
        Ok(SavedView::restore(
            id,
            draft.name().clone(),
            draft.scope(),
            draft.filter().clone(),
            draft.expanded().to_vec(),
            draft.hidden().to_vec(),
            draft.mode(),
            draft.done(),
            draft.sorting(),
            draft.is_default(),
            draft.version(),
        ))
    }

    fn save(&self, view: &SavedView) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let preceding_version = view.version() - 1;
        let changed = span
            .execute(
                "UPDATE saved_views
                 SET name = ?2,
                     filter = ?3,
                     expanded_groups = ?4,
                     hidden_columns = ?5,
                     mode = ?6,
                     done = ?7,
                     sorting = ?8,
                     version = ?9
                 WHERE id = ?1 AND version = ?10",
                params![
                    view.id().value() as i64,
                    view.name().as_str(),
                    filter_json(view.filter()),
                    groups_json(view.expanded()),
                    groups_json(view.hidden()),
                    view.mode().wire_name(),
                    view.done().wire_name(),
                    view.sorting().wire_name(),
                    view.version() as i64,
                    preceding_version as i64,
                ],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(save_refused(&span, view.id(), preceding_version));
        }
        span.commit().map_err(internal)?;
        Ok(())
    }

    fn remove(&self, id: SavedViewId) -> Result<(), ApiError> {
        let conn = self.lock();
        let span = WriteSpan::begin(&conn).map_err(internal)?;
        let changed = span
            .execute(
                "DELETE FROM saved_views WHERE id = ?1",
                params![id.value() as i64],
            )
            .map_err(internal)?;
        if changed != 1 {
            return Err(ApiError::not_found(&format!("view {}", id.value())));
        }
        span.commit().map_err(internal)?;
        Ok(())
    }
}

/// One stored row as its entity, revalidated against the domain
/// vocabularies it names.
fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SavedView> {
    let id = row.get::<_, i64>(0)?.unsigned_abs();
    let scope_kind: String = row.get(1)?;
    let project_id: i64 = row.get(2)?;
    let name: String = row.get(3)?;
    let is_default: i64 = row.get(4)?;
    let filter: String = row.get(5)?;
    let expanded: String = row.get(6)?;
    let hidden: String = row.get(7)?;
    let mode: String = row.get(8)?;
    let done: String = row.get(9)?;
    let sorting: String = row.get(10)?;
    let version = row.get::<_, i64>(11)?.unsigned_abs();
    let corrupt = |field: &'static str| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(CorruptRow(field)),
        )
    };
    let scope = match (scope_kind.as_str(), project_id) {
        ("global", 0) => DomainScope::Global,
        ("project", project) => DomainScope::Project(ProjectId::new(project.unsigned_abs())),
        _ => return Err(corrupt("scope")),
    };
    let name = ViewName::new(&name).map_err(|_| corrupt("name"))?;
    let filter = filter_of(&filter).ok_or_else(|| corrupt("filter"))?;
    let expanded = groups_of(&expanded).ok_or_else(|| corrupt("expanded_groups"))?;
    let hidden = groups_of(&hidden).ok_or_else(|| corrupt("hidden_columns"))?;
    let mode = DomainMode::parse(&mode).ok_or_else(|| corrupt("mode"))?;
    let done = DomainDone::parse(&done).ok_or_else(|| corrupt("done"))?;
    let sorting = DomainSorting::parse(&sorting).ok_or_else(|| corrupt("sorting"))?;
    Ok(SavedView::restore(
        SavedViewId::new(id),
        name,
        scope,
        filter,
        expanded,
        hidden,
        mode,
        done,
        sorting,
        is_default == 1,
        version,
    ))
}

/// Why a guarded write was refused, read from the row's current
/// state.
fn save_refused(span: &WriteSpan<'_>, id: SavedViewId, attempted_from: u64) -> ApiError {
    match span.query_row(
        "SELECT version FROM saved_views WHERE id = ?1",
        params![id.value() as i64],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(current) => ApiError::stale_version(attempted_from, current.unsigned_abs()),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            ApiError::not_found(&format!("view {}", id.value()))
        }
        Err(error) => internal(error),
    }
}

/// The scope kind column of one scope.
fn scope_kind_of(scope: DomainScope) -> &'static str {
    match scope {
        DomainScope::Global => "global",
        DomainScope::Project(_) => "project",
    }
}

/// The Project column of one scope: 0 for the global scope, so the
/// unique keys hold NULLs nowhere.
fn project_column_of(scope: DomainScope) -> i64 {
    match scope {
        DomainScope::Global => 0,
        DomainScope::Project(project) => project.value() as i64,
    }
}

/// The filter as one JSON object of wire-named axes.
fn filter_json(filter: &DomainFilter) -> String {
    json!({
        "initiatives": filter.initiatives.iter().map(|id| id.value()).collect::<Vec<_>>(),
        "projects": filter.projects.iter().map(|id| id.value()).collect::<Vec<_>>(),
        "plans": filter.plans.iter().map(|id| id.value()).collect::<Vec<_>>(),
        "specs": filter.specs.iter().map(|id| id.value()).collect::<Vec<_>>(),
        "kinds": filter.kinds.iter().map(|kind| kind.wire_name()).collect::<Vec<_>>(),
        "states": filter.states.iter().map(|state| state.wire_name()).collect::<Vec<_>>(),
        "priorities": filter.priorities.iter().map(|p| p.wire_name()).collect::<Vec<_>>(),
        "lanes": filter.lanes.iter().map(|id| id.value()).collect::<Vec<_>>(),
        "profiles": filter.profiles.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
        "attention": filter.attention.iter().map(|c| c.wire_name()).collect::<Vec<_>>(),
    })
    .to_string()
}

/// The filter one stored JSON object names, or `None` outside the
/// vocabularies.
fn filter_of(text: &str) -> Option<DomainFilter> {
    let parsed: Value = serde_json::from_str(text).ok()?;
    let ids = |axis: &str| -> Option<Vec<u64>> {
        parsed[axis]
            .as_array()?
            .iter()
            .map(|value| value.as_u64())
            .collect()
    };
    let words = |axis: &str| -> Option<Vec<String>> {
        parsed[axis]
            .as_array()?
            .iter()
            .map(|value| value.as_str().map(str::to_owned))
            .collect()
    };
    Some(DomainFilter {
        initiatives: ids("initiatives")?
            .into_iter()
            .map(InitiativeId::new)
            .collect(),
        projects: ids("projects")?.into_iter().map(ProjectId::new).collect(),
        plans: ids("plans")?.into_iter().map(PlanId::new).collect(),
        specs: ids("specs")?.into_iter().map(SpecId::new).collect(),
        kinds: words("kinds")?
            .iter()
            .map(|word| DomainKind::parse(word))
            .collect::<Option<Vec<_>>>()?,
        states: words("states")?
            .iter()
            .map(|word| DomainState::parse(word))
            .collect::<Option<Vec<_>>>()?,
        priorities: words("priorities")?
            .iter()
            .map(|word| DomainPriority::parse(word))
            .collect::<Option<Vec<_>>>()?,
        lanes: ids("lanes")?.into_iter().map(LaneId::new).collect(),
        profiles: words("profiles")?
            .iter()
            .map(|word| ProfileName::new(word))
            .collect::<Result<Vec<_>, _>>()
            .ok()?,
        attention: words("attention")?
            .iter()
            .map(|word| DomainAttention::parse(word))
            .collect::<Option<Vec<_>>>()?,
    })
}

/// The group sets as JSON arrays of wire names.
fn groups_json(groups: &[DomainGroup]) -> String {
    json!(
        groups
            .iter()
            .map(|group| group.wire_name())
            .collect::<Vec<_>>()
    )
    .to_string()
}

/// The groups one stored JSON array names, or `None` outside the
/// vocabulary.
fn groups_of(text: &str) -> Option<Vec<DomainGroup>> {
    let parsed: Value = serde_json::from_str(text).ok()?;
    parsed
        .as_array()?
        .iter()
        .map(|word| word.as_str().and_then(DomainGroup::parse))
        .collect()
}

/// Report a SQLite failure the caller cannot act on.
fn internal(error: rusqlite::Error) -> ApiError {
    ApiError::internal(&error.to_string())
}

/// A stored view row failed domain validation.
#[derive(Debug)]
struct CorruptRow(&'static str);

impl std::fmt::Display for CorruptRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a stored saved view row failed validation: {}", self.0)
    }
}

impl std::error::Error for CorruptRow {}

#[cfg(test)]
mod saved_view_rows {
    use kanban_domain::{BoardGroup, Priority, ProfileName, TicketKind, TicketState};

    use super::SavedViewStore as _;
    use super::SqliteSavedViewStore;
    use crate::migrations::AllowAllMigrations;
    use crate::test_support::scratch_database;

    /// One view varied away from every default, so a round trip
    /// proves each owned property.
    fn owned() -> kanban_domain::SavedView {
        kanban_domain::SavedView::create(
            kanban_domain::ViewName::new("Review queue").expect("the name validates"),
            kanban_domain::ViewScope::Project(kanban_domain::ProjectId::new(2)),
            kanban_domain::BoardFilter {
                initiatives: vec![kanban_domain::InitiativeId::new(4)],
                projects: vec![kanban_domain::ProjectId::new(2)],
                plans: vec![kanban_domain::PlanId::new(7)],
                specs: vec![kanban_domain::SpecId::new(9)],
                kinds: vec![TicketKind::Implementation, TicketKind::Bug],
                states: vec![TicketState::InReview, TicketState::Landing],
                priorities: vec![Priority::Urgent, Priority::Low],
                lanes: vec![kanban_domain::LaneId::new(5)],
                profiles: vec![
                    ProfileName::new("standard").expect("the name validates"),
                    ProfileName::new("nightly").expect("the name validates"),
                ],
                attention: vec![kanban_domain::AttentionState::StaleRun],
            },
            &[BoardGroup::Backlog, BoardGroup::Staged],
            &[BoardGroup::Draft, BoardGroup::Done],
            kanban_domain::ViewMode::Register,
            kanban_domain::DonePlacement::Table,
            kanban_domain::ViewSorting::Readiness,
        )
        .expect("the view validates")
    }

    #[test]
    fn a_row_round_trips_every_property_it_owns() {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("migrations apply");
        let store = SqliteSavedViewStore::new(&database);

        let stored = store.insert(&owned()).expect("the view lands");

        assert_eq!(stored.id().value(), 1, "the identity is minted");
        assert_eq!(stored.name().as_str(), "Review queue");
        assert_eq!(
            stored.scope(),
            kanban_domain::ViewScope::Project(kanban_domain::ProjectId::new(2))
        );
        assert_eq!(
            store.find(stored.id()).expect("the find serves"),
            Some(stored.clone()),
            "every axis round trips"
        );
        assert_eq!(
            store.list().expect("the list serves"),
            vec![stored.clone()],
            "the list answers with the same row"
        );
        // The filter survives axis by axis, profiles included.
        let round = store
            .find(stored.id())
            .expect("the find serves")
            .expect("the view stands");
        assert_eq!(round.filter().attention, stored.filter().attention);
        assert_eq!(round.filter().profiles, stored.filter().profiles);
        assert_eq!(round.filter().priorities, stored.filter().priorities);
    }

    #[test]
    fn a_global_default_round_trips_its_empty_filter() {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("migrations apply");
        let store = SqliteSavedViewStore::new(&database);

        let stored = store
            .insert(&kanban_domain::SavedView::generate(
                kanban_domain::SavedViewId::new(0),
                kanban_domain::ViewScope::Global,
            ))
            .expect("the default lands");

        assert!(stored.is_default());
        assert_eq!(
            store.find(stored.id()).expect("the find serves"),
            Some(stored),
            "an empty filter and the Draft-hidden column set round trip"
        );
    }

    #[test]
    fn save_guards_the_version_and_persists_the_whole_owned_set() {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("migrations apply");
        let store = SqliteSavedViewStore::new(&database);
        let stored = store.insert(&owned()).expect("the view lands");

        let mut revised = stored.clone();
        revised
            .adopt(
                kanban_domain::BoardFilter::default(),
                &[BoardGroup::Staged],
                &[],
                kanban_domain::ViewMode::Board,
                kanban_domain::DonePlacement::Column,
                kanban_domain::ViewSorting::Priority,
            )
            .expect("the revision validates");
        store.save(&revised).expect("the revision lands");
        assert_eq!(
            store.find(revised.id()).expect("the find serves"),
            Some(revised.clone()),
            "the whole owned set is replaced, version included"
        );

        // The replaced version no longer matches: the guard refuses.
        let stale = store.save(&revised).expect_err("the same version is stale");
        assert_eq!(stale.code, kanban_dto::ErrorCode::StaleVersion);

        // A view that never stood is not found, not stale.
        let mut ghost = revised.clone();
        ghost.rename(kanban_domain::ViewName::new("Ghost").expect("the name validates"));
        let missing = store
            .save(&kanban_domain::SavedView::restore(
                kanban_domain::SavedViewId::new(99),
                ghost.name().clone(),
                ghost.scope(),
                ghost.filter().clone(),
                ghost.expanded().to_vec(),
                ghost.hidden().to_vec(),
                ghost.mode(),
                ghost.done(),
                ghost.sorting(),
                false,
                1,
            ))
            .expect_err("an unknown view is not found");
        assert_eq!(missing.code, kanban_dto::ErrorCode::NotFound);
    }

    #[test]
    fn save_persists_a_renamed_name() {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("migrations apply");
        let store = SqliteSavedViewStore::new(&database);
        let stored = store.insert(&owned()).expect("the view lands");

        let mut renamed = stored.clone();
        renamed.rename(kanban_domain::ViewName::new("Deep work").expect("the name validates"));
        store.save(&renamed).expect("the rename lands");

        assert_eq!(
            store.find(renamed.id()).expect("the find serves"),
            Some(renamed.clone()),
            "the new name round trips under its new version"
        );
        assert_eq!(
            store.list().expect("the list serves")[0].name().as_str(),
            "Deep work",
            "the stored row answers under the new name alone"
        );
    }

    #[test]
    fn removal_removes_and_removal_of_the_absent_is_not_found() {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("migrations apply");
        let store = SqliteSavedViewStore::new(&database);
        let stored = store.insert(&owned()).expect("the view lands");

        store.remove(stored.id()).expect("the removal lands");
        assert_eq!(store.find(stored.id()).expect("the find serves"), None);

        let again = store
            .remove(stored.id())
            .expect_err("the view is already gone");
        assert_eq!(again.code, kanban_dto::ErrorCode::NotFound);
    }

    #[test]
    fn one_name_per_scope_and_one_default_per_scope_are_constraints() {
        let (_dir, mut database) = scratch_database();
        database
            .migrate(&AllowAllMigrations)
            .expect("migrations apply");
        let store = SqliteSavedViewStore::new(&database);
        store.insert(&owned()).expect("the view lands");

        // The same name in the same scope is refused; in another
        // scope it is free.
        let duplicate = store.insert(&owned()).expect_err("the name is taken");
        assert_eq!(duplicate.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            duplicate.message,
            "the view name `Review queue` is already taken in its scope"
        );
        let elsewhere = kanban_domain::SavedView::create(
            kanban_domain::ViewName::new("Review queue").expect("the name validates"),
            kanban_domain::ViewScope::Global,
            kanban_domain::BoardFilter::default(),
            &[],
            &[],
            kanban_domain::ViewMode::Board,
            kanban_domain::DonePlacement::Column,
            kanban_domain::ViewSorting::Priority,
        )
        .expect("the view validates");
        store
            .insert(&elsewhere)
            .expect("the same name is free in another scope");

        // Two defaults in one scope never land, whatever the names.
        let default_one = kanban_domain::SavedView::generate(
            kanban_domain::SavedViewId::new(0),
            kanban_domain::ViewScope::Project(kanban_domain::ProjectId::new(2)),
        );
        store.insert(&default_one).expect("the first default lands");
        let mut default_two = kanban_domain::SavedView::generate(
            kanban_domain::SavedViewId::new(0),
            kanban_domain::ViewScope::Project(kanban_domain::ProjectId::new(2)),
        );
        default_two.rename(kanban_domain::ViewName::new("Second").expect("the name validates"));
        let refused = store
            .insert(&default_two)
            .expect_err("one default per scope");
        assert_eq!(refused.code, kanban_dto::ErrorCode::InvalidRequest);
    }
}
