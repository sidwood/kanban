//! SQLite-backed implementation of the timeline query port.
//!
//! Rows are written by the entity stores inside their own
//! transactions; this adapter reads them back and refuses any row
//! that no longer decodes into the payload vocabulary.

use std::sync::Arc;

use kanban_app::{TimelineError, TimelineStore};
use kanban_dto::{
    TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineEventRecord, TimelineQuery,
    TimelineScope,
};
use kanban_storage::{Database, TimelineFilter, TimelineRow};

/// The storage adapter the core uses for timeline queries.
///
/// It shares the database handle rather than wrapping it: the
/// database already serialises its own connection, and a second lock
/// here would duplicate ownership of the authoritative connection.
pub struct StorageTimelineStore {
    database: Arc<Database>,
}

impl StorageTimelineStore {
    /// Wraps the authoritative database handle.
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }
}

impl TimelineStore for StorageTimelineStore {
    fn query(&self, query: &TimelineQuery) -> Result<Vec<TimelineEventRecord>, TimelineError> {
        let filter = TimelineFilter {
            scope: query.scope.clone(),
            entity_kind: query
                .entity
                .as_ref()
                .map(|entity| entity.kind.as_str().to_owned()),
            entity_id: query.entity.as_ref().map(|entity| entity.id.clone()),
            kinds: query
                .kinds
                .as_ref()
                .map(|kinds| kinds.iter().map(|kind| kind.as_str().to_owned()).collect())
                .unwrap_or_default(),
            since: query.since.clone(),
            until: query.until.clone(),
        };
        let rows = self
            .database
            .query_timeline(&filter)
            .map_err(|error| TimelineError::Storage(error.to_string()))?;
        rows.into_iter().map(row_to_record).collect()
    }
}

fn row_to_record(row: TimelineRow) -> Result<TimelineEventRecord, TimelineError> {
    let scope = row_scope(&row.scope, row.project_id)?;
    let kind = dto_event_kind(&row.kind)?;
    let entity = match (row.entity_kind, row.entity_id) {
        (Some(kind), Some(id)) => Some(TimelineEntityRef {
            kind: dto_entity_kind(&kind)?,
            id,
        }),
        (None, None) => None,
        _ => {
            return Err(TimelineError::Storage(
                "timeline row carried a partial entity reference".to_owned(),
            ));
        }
    };
    Ok(TimelineEventRecord {
        id: row.id,
        scope,
        kind,
        entity,
        recorded_at: row.recorded_at,
        detail: row.detail,
    })
}

fn row_scope(scope: &str, project_id: String) -> Result<TimelineScope, TimelineError> {
    match scope {
        "global" if project_id.is_empty() => Ok(TimelineScope::Global),
        "global" => Err(TimelineError::Storage(format!(
            "a global timeline row named the Project `{project_id}`"
        ))),
        "project" if project_id.is_empty() => Err(TimelineError::Storage(
            "a Project timeline row named no Project".to_owned(),
        )),
        "project" => Ok(TimelineScope::Project(project_id)),
        other => Err(TimelineError::Storage(format!(
            "unknown stored timeline scope `{other}`"
        ))),
    }
}

fn dto_event_kind(kind: &str) -> Result<TimelineEventKind, TimelineError> {
    TimelineEventKind::parse(kind).ok_or_else(|| {
        TimelineError::Storage(format!("unknown stored timeline event kind `{kind}`"))
    })
}

fn dto_entity_kind(kind: &str) -> Result<TimelineEntityKind, TimelineError> {
    TimelineEntityKind::parse(kind).ok_or_else(|| {
        TimelineError::Storage(format!("unknown stored timeline entity kind `{kind}`"))
    })
}

#[cfg(test)]
mod timeline_socket_sqlite {
    //! Initiative commands and timeline queries over the real socket
    //! against the real database, with no substitutes anywhere in the
    //! path (KAN-S2-US1).

    use serde_json::{Value, json};
    use tempfile::TempDir;

    use crate::CoreProcess;
    use crate::test_client::{Client, boot};

    /// A serving core with one Initiative created, renamed, and
    /// archived, and a second left active.
    fn core_with_initiative_history(dir: &TempDir) -> (CoreProcess, Client) {
        let core = boot(dir);
        let mut client = Client::connect(core.socket_path());
        client.command(
            "initiative.create",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "create-1" },
                "name": "Reliability",
            }),
        );
        client.command(
            "initiative.rename",
            json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "rename-1" },
                "initiative_id": 1,
                "name": "Reliability and Recovery",
            }),
        );
        client.command(
            "initiative.archive",
            json!({
                "mutation": { "optimistic_version": 2, "idempotency_key": "archive-1" },
                "initiative_id": 1,
            }),
        );
        client.command(
            "initiative.create",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "create-2" },
                "name": "Packaging",
            }),
        );
        (core, client)
    }

    fn events(payload: &Value) -> &Vec<Value> {
        payload["events"]
            .as_array()
            .expect("the query answers with events")
    }

    #[test]
    fn initiative_history_is_queryable_in_the_global_scope() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let (core, mut client) = core_with_initiative_history(&dir);

        let answer = client.query_with("timeline.query", json!({ "scope": "global" }));

        let recorded: Vec<(&str, &str, &str)> = events(&answer)
            .iter()
            .map(|event| {
                (
                    event["kind"].as_str().expect("a kind"),
                    event["entity"]["id"].as_str().expect("an entity id"),
                    event["detail"]["action"].as_str().expect("an action"),
                )
            })
            .collect();
        assert_eq!(
            recorded,
            vec![
                ("transition", "1", "created"),
                ("transition", "1", "renamed"),
                ("transition", "1", "archived"),
                ("transition", "2", "created"),
            ],
            "every Initiative mutation is written and readable in its own scope"
        );
        assert_eq!(
            events(&answer)[0]["scope"],
            json!("global"),
            "Initiatives sit above every Project"
        );
        assert_eq!(
            events(&answer)[0]["detail"]["name"],
            json!("Reliability"),
            "the facts of the change survive the round trip"
        );

        core.shutdown();
    }

    #[test]
    fn archived_initiative_history_survives_a_restart() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let (core, _) = core_with_initiative_history(&dir);
        core.shutdown();

        let rebooted = boot(&dir);
        let mut client = Client::connect(rebooted.socket_path());
        let answer = client.query_with(
            "timeline.query",
            json!({
                "scope": "global",
                "entity": { "kind": "initiative", "id": "1" },
            }),
        );

        assert_eq!(
            events(&answer).len(),
            3,
            "archiving never touches the timeline it left behind"
        );

        rebooted.shutdown();
    }

    #[test]
    fn the_project_scope_holds_no_initiative_history() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let (core, mut client) = core_with_initiative_history(&dir);

        let answer = client.query_with("timeline.query", json!({ "scope": { "project": "kan" } }));

        assert!(
            events(&answer).is_empty(),
            "a Project timeline holds only its own rows"
        );

        core.shutdown();
    }

    #[test]
    fn the_entity_filter_narrows_to_one_initiative() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let (core, mut client) = core_with_initiative_history(&dir);

        let answer = client.query_with(
            "timeline.query",
            json!({
                "scope": "global",
                "entity": { "kind": "initiative", "id": "2" },
            }),
        );

        assert_eq!(events(&answer).len(), 1);
        assert_eq!(events(&answer)[0]["detail"]["name"], json!("Packaging"));

        core.shutdown();
    }

    #[test]
    fn the_kind_filter_answers_from_the_closed_vocabulary() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let (core, mut client) = core_with_initiative_history(&dir);

        let transitions = client.query_with(
            "timeline.query",
            json!({ "scope": "global", "kinds": ["transition"] }),
        );
        let comments = client.query_with(
            "timeline.query",
            json!({ "scope": "global", "kinds": ["comment"] }),
        );

        assert_eq!(events(&transitions).len(), 4);
        assert!(
            events(&comments).is_empty(),
            "Initiative changes are transitions, not comments"
        );

        core.shutdown();
    }

    #[test]
    fn a_project_scope_without_an_identity_is_refused() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let core = boot(&dir);
        let mut client = Client::connect(core.socket_path());

        let frame = client.attempt(
            "query",
            "timeline.query",
            json!({ "scope": { "project": "" } }),
        );

        assert_eq!(frame["kind"], json!("error"), "got: {frame}");
        assert_eq!(frame["error"]["code"], json!("invalid_request"));

        core.shutdown();
    }

    #[test]
    fn a_row_outside_the_vocabulary_is_refused_rather_than_hidden() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let (core, mut client) = core_with_initiative_history(&dir);
        // The shape this Ticket removed, written straight into the
        // file: an audit surface must say it cannot read a row, never
        // quietly drop it.
        let conn = rusqlite::Connection::open(dir.path().join("kanban.sqlite"))
            .expect("a second connection opens");
        conn.execute(
            "INSERT INTO timeline_events
                 (scope, project_id, kind, entity_kind, entity_id, detail)
             VALUES ('global', '', 'initiative.created', 'initiative', '3', '{}')",
            [],
        )
        .expect("the corrupt row lands");

        let frame = client.attempt("query", "timeline.query", json!({ "scope": "global" }));

        assert_eq!(frame["kind"], json!("error"), "got: {frame}");
        assert!(
            frame["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("initiative.created")),
            "the refusal names the row it could not read: {frame}"
        );

        core.shutdown();
    }
}
