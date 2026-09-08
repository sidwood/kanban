//! Recurring schedules through the real SQLite command boundary.
use std::num::NonZeroU32;
use std::sync::Arc;

use kanban_app::catalog::exposed_operations;
use kanban_app::{Core, NoopEventSink};
use kanban_storage::{
    AllowAllMigrations, Database, RetentionPolicy, SqliteDependencyStore, SqliteIdempotencyStore,
    SqliteProjectStore, SqliteScheduleStore, SqliteTicketStore,
};
use serde_json::json;
use tempfile::TempDir;

mod common;
use common::{insert_ticket, mutation, seed_project_profile};

struct Harness {
    core: Core,
    _database: Database,
    _dir: TempDir,
}

fn harness() -> Harness {
    let dir = TempDir::new().unwrap();
    let mut database = Database::open(&dir.path().join("recurrence.sqlite")).unwrap();
    database.migrate(&AllowAllMigrations).unwrap();
    seed_project_profile(&database);
    let mut core = Core::new(
        exposed_operations(),
        Arc::new(SqliteIdempotencyStore::new(
            &database,
            RetentionPolicy::keep_most_recent(NonZeroU32::new(100).unwrap()),
        )),
        Arc::new(NoopEventSink),
    );
    core.register_lifecycle(
        Arc::new(SqliteTicketStore::new(&database)),
        Arc::new(SqliteDependencyStore::new(&database)),
        Arc::new(SqliteProjectStore::new(&database)),
        Arc::new(SqliteScheduleStore::new(&database)),
        None,
    )
    .unwrap();
    core.register_scheduling_policy(
        Arc::new(kanban_storage::recurrence::SqliteRecurrenceStore::new(
            &database,
        )),
        Arc::new(SqliteProjectStore::new(&database)),
    )
    .unwrap();
    core.register_dispatch(
        Arc::new(kanban_storage::SqliteDispatchStore::new(&database)),
        Arc::new(SqliteTicketStore::new(&database)),
        Arc::new(kanban_storage::SqliteProfileStore::new(&database)),
        Arc::new(SqliteProjectStore::new(&database)),
        Arc::new(kanban_storage::SqliteCapacityStore::new(&database)),
        Arc::new(kanban_storage::SqliteLaneStore::new(&database)),
        Arc::new(SqliteDependencyStore::new(&database)),
        Arc::new(kanban_app::NoopCoordinatorWake),
    )
    .unwrap();
    core.register_runs(
        Arc::new(kanban_storage::SqliteRunStore::new(&database)),
        Arc::new(kanban_storage::SqliteDispatchStore::new(&database)),
        Arc::new(SqliteTicketStore::new(&database)),
        Arc::new(kanban_storage::SqliteProfileStore::new(&database)),
        Arc::new(SqliteProjectStore::new(&database)),
    )
    .unwrap();
    Harness {
        core,
        _database: database,
        _dir: dir,
    }
}

#[test]
fn recurrence_schedule_computes_its_first_activation_and_replays_once() {
    let h = harness();
    let ticket = insert_ticket(&h._dir.path().join("recurrence.sqlite"), 1, "normal");
    let payload = json!({
        "mutation": mutation(1, "recurring-template"), "ticket_id": ticket,
        "cron": "*/15 * * * *", "after": "2026-09-08T09:00:00Z",
        "timezone": "UTC", "profile": "standard",
    });
    let saved = h
        .core
        .command("ticket.schedule", &payload)
        .expect("a Task can carry a recurring Schedule");
    assert_eq!(saved["state"], "scheduled");
    assert_eq!(saved, h.core.command("ticket.schedule", &payload).unwrap());
    let conn = rusqlite::Connection::open(h._dir.path().join("recurrence.sqlite")).unwrap();
    let rows: (i64, String) = conn.query_row(
        "SELECT COUNT(*), next_activation FROM schedules WHERE ticket_id=?1 AND trigger_kind='cron'",
        [ticket as i64], |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(rows, (1, "2026-09-08T09:15:00.000Z".to_owned()));
}

#[test]
fn recurrence_mints_a_fresh_task_not_the_template_identity() {
    use kanban_app::recurrence::RecurrencePass;
    use kanban_storage::recurrence::SqliteRecurrenceStore;
    let h = scheduled_pair();
    let conn = rusqlite::Connection::open(h._dir.path().join("recurrence.sqlite")).unwrap();
    let pass = RecurrencePass::new(Arc::new(SqliteRecurrenceStore::new(&h._database)));
    let report = pass.tick("2026-09-08T09:15:20Z", &NoopEventSink).unwrap();
    assert_eq!(
        report.minted.len(),
        1,
        "one due window creates one fresh Task"
    );
    assert_eq!(report.minted[0].value(), 3);
    assert_eq!(
        conn.query_row("SELECT state FROM tickets WHERE id = 2", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "scheduled"
    );
    let occurrence = conn
        .query_row(
            "SELECT number, state, mode, subtype, profile FROM tickets WHERE id = 3",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        occurrence,
        (
            3,
            "ready".to_owned(),
            "human".to_owned(),
            "operational".to_owned(),
            "standard".to_owned()
        )
    );
    let lineage = conn
        .query_row(
            "SELECT template_ticket_id, occurrence_ticket_id, window_at
        FROM task_occurrences",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(lineage, (2, 3, "2026-09-08T09:15:00.000Z".to_owned()));
    assert!(
        pass.tick("2026-09-08T09:15:20Z", &NoopEventSink)
            .unwrap()
            .minted
            .is_empty()
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM task_occurrences", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

fn scheduled_pair() -> Harness {
    let h = harness();
    let path = h._dir.path().join("recurrence.sqlite");
    let conn = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(insert_ticket(&path, 1, "normal"), 1);
    assert_eq!(insert_ticket(&path, 2, "normal"), 2);
    conn.execute("UPDATE projects SET ticket_counter = 2 WHERE id = 1", [])
        .unwrap();
    conn.execute("UPDATE tickets SET profile = 'standard' WHERE id = 2", [])
        .unwrap();
    let mut request = json!({"mutation": mutation(1, "recurring"), "ticket_id": 2});
    request["cron"] = json!("*/15 * * * *");
    request["after"] = json!("2026-09-08T09:00:00Z");
    request["timezone"] = json!("UTC");
    request["profile"] = json!("standard");
    h.core.command("ticket.schedule", &request).unwrap();
    h
}

#[test]
fn recurrence_project_opt_in_catches_up_once_not_a_backlog() {
    use kanban_app::recurrence::RecurrencePass;
    use kanban_storage::recurrence::SqliteRecurrenceStore;
    let h = scheduled_pair();
    let payload = json!({"mutation":mutation(0,"opt-in"), "project_id":1, "catch_up_one":true});
    let policy = h
        .core
        .command("project.schedule_policy.set", &payload)
        .expect("a Project may opt into one catch-up");
    assert_eq!(policy["catch_up_one"], true);
    assert_eq!(policy["version"], 1);
    assert_eq!(
        h.core
            .command("project.schedule_policy.set", &payload)
            .unwrap(),
        policy
    );
    let pass = RecurrencePass::new(Arc::new(SqliteRecurrenceStore::new(&h._database)));
    assert_eq!(
        pass.tick("2026-09-08T12:07:00Z", &NoopEventSink)
            .unwrap()
            .minted
            .len(),
        1
    );
    assert!(
        pass.tick("2026-09-08T12:07:00Z", &NoopEventSink)
            .unwrap()
            .minted
            .is_empty()
    );
    let conn = rusqlite::Connection::open(h._dir.path().join("recurrence.sqlite")).unwrap();
    assert_eq!(
        conn.query_row("SELECT window_at FROM task_occurrences", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "2026-09-08T12:00:00.000Z"
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM task_occurrences", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn recurrence_skips_history_and_records_attention_even_with_an_open_task() {
    use kanban_app::recurrence::RecurrencePass;
    use kanban_storage::recurrence::SqliteRecurrenceStore;
    for open in [false, true] {
        let h = scheduled_pair();
        let conn = rusqlite::Connection::open(h._dir.path().join("recurrence.sqlite")).unwrap();
        let pass = RecurrencePass::new(Arc::new(SqliteRecurrenceStore::new(&h._database)));
        if open {
            assert_eq!(
                pass.tick("2026-09-08T09:15:20Z", &NoopEventSink)
                    .unwrap()
                    .minted
                    .len(),
                1
            );
        }
        let from: String = conn
            .query_row("SELECT next_activation FROM schedules", [], |r| r.get(0))
            .unwrap();
        assert!(
            pass.tick("2026-09-08T12:07:00Z", &NoopEventSink)
                .unwrap()
                .minted
                .is_empty()
        );
        assert_eq!(
            conn.query_row("SELECT next_activation FROM schedules", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "2026-09-08T12:15:00.000Z"
        );
        let attention = h
            .core
            .query("schedule.attention.list", &json!({"project_id":1}))
            .expect("missed windows raise visible attention");
        assert_eq!(attention["signals"].as_array().unwrap().len(), 1);
        assert_eq!(attention["signals"][0]["template_ticket_id"], 2);
        assert_eq!(attention["signals"][0]["reason"], "missed_window");
        assert_eq!(attention["signals"][0]["first_window"], from);
        assert_eq!(
            attention["signals"][0]["last_window"],
            "2026-09-08T12:00:00.000Z"
        );
    }
}

#[test]
fn recurrence_never_widens_the_templates_dependency_scope() {
    use kanban_app::recurrence::RecurrencePass;
    use kanban_storage::recurrence::SqliteRecurrenceStore;
    for blocker in ["dependency", "external"] {
        let h = scheduled_pair();
        let conn = rusqlite::Connection::open(h._dir.path().join("recurrence.sqlite")).unwrap();
        if blocker == "dependency" {
            conn.execute(
                "INSERT INTO ticket_dependencies (from_ticket,to_ticket) VALUES (1,2)",
                [],
            )
            .unwrap();
        } else {
            conn.execute("INSERT INTO ticket_blockers (ticket_id,description) VALUES (2,'Waiting for access')",[]).unwrap();
        }
        let pass = RecurrencePass::new(Arc::new(SqliteRecurrenceStore::new(&h._database)));
        assert!(
            pass.tick("2026-09-08T09:15:20Z", &NoopEventSink)
                .unwrap()
                .minted
                .is_empty(),
            "a template blocker must not disappear at occurrence creation"
        );
        let attention = h
            .core
            .query("schedule.attention.list", &json!({"project_id":1}))
            .unwrap();
        assert_eq!(attention["signals"][0]["reason"], "blocked");
        if blocker == "dependency" {
            conn.execute("UPDATE tickets SET state='done' WHERE id=1", [])
                .unwrap();
        } else {
            conn.execute("DELETE FROM ticket_blockers WHERE ticket_id=2", [])
                .unwrap();
        }
        assert_eq!(
            pass.tick("2026-09-08T09:30:20Z", &NoopEventSink)
                .unwrap()
                .minted
                .len(),
            1
        );
        if blocker == "dependency" {
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM ticket_dependencies WHERE from_ticket=1 AND to_ticket=3",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
        }
    }
}

#[test]
fn recurrence_lineage_cannot_be_deleted_or_replaced() {
    use kanban_app::recurrence::RecurrencePass;
    use kanban_storage::recurrence::SqliteRecurrenceStore;
    for action in ["delete", "replace"] {
        let h = scheduled_pair();
        let pass = RecurrencePass::new(Arc::new(SqliteRecurrenceStore::new(&h._database)));
        pass.tick("2026-09-08T09:15:20Z", &NoopEventSink).unwrap();
        let conn = rusqlite::Connection::open(h._dir.path().join("recurrence.sqlite")).unwrap();
        let id: i64 = conn
            .query_row("SELECT id FROM task_occurrences", [], |r| r.get(0))
            .unwrap();
        let result = if action == "delete" {
            conn.execute("DELETE FROM task_occurrences WHERE id=?1", [id])
        } else {
            conn.execute(
                "INSERT OR REPLACE INTO task_occurrences
            (id,schedule_id,template_ticket_id,occurrence_ticket_id,window_at)
            VALUES (?1,1,2,3,'2026-09-08T09:30:00.000Z')",
                [id],
            )
        };
        assert!(
            result.is_err(),
            "{action} must not erase occurrence custody"
        );
        assert_eq!(
            conn.query_row("SELECT window_at FROM task_occurrences", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "2026-09-08T09:15:00.000Z"
        );
    }
}

fn two_occurrences() -> Harness {
    use kanban_app::recurrence::RecurrencePass;
    use kanban_storage::recurrence::SqliteRecurrenceStore;
    let h = scheduled_pair();
    let pass = RecurrencePass::new(Arc::new(SqliteRecurrenceStore::new(&h._database)));
    pass.tick("2026-09-08T09:15:20Z", &NoopEventSink).unwrap();
    let conn = rusqlite::Connection::open(h._dir.path().join("recurrence.sqlite")).unwrap();
    conn.execute("UPDATE tickets SET state='done' WHERE id=3", [])
        .unwrap();
    pass.tick("2026-09-08T09:30:20Z", &NoopEventSink).unwrap();
    conn.execute("UPDATE tickets SET mode='agent' WHERE id IN (3,4)", [])
        .unwrap();
    h
}

#[test]
fn recurrence_dispatch_refuses_an_occurrence_when_its_peer_is_reopened() {
    let h = two_occurrences();
    let conn = rusqlite::Connection::open(h._dir.path().join("recurrence.sqlite")).unwrap();
    conn.execute("UPDATE tickets SET state='ready' WHERE id=3", [])
        .unwrap();
    let result = h.core.command(
        "dispatch.request",
        &json!({
        "mutation":common::mutation(0,"overlapping-dispatch"),"ticket_id":4}),
    );
    let error = result.expect_err("dispatch must enforce occurrence overlap too");
    assert!(error.message.contains("occurrence"), "{error:?}");
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM dispatch_requests", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn recurrence_claim_rechecks_peers_after_a_request_was_queued() {
    let h = two_occurrences();
    let path = h._dir.path().join("recurrence.sqlite");
    common::assign_lane(&path, 4);
    let queued = h
        .core
        .command(
            "dispatch.request",
            &json!({
        "mutation":common::mutation(0,"before-peer-reopen"),"ticket_id":4}),
        )
        .unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("UPDATE tickets SET state='ready' WHERE id=3", [])
        .unwrap();
    let result=h.core.command("dispatch.claim",&json!({
        "mutation":common::mutation(1,"claim-after-peer-reopen"),"dispatch_request_id":queued["id"]}));
    let error = result.expect_err("the claim must recheck recurring overlap atomically");
    assert!(error.message.contains("occurrence"), "{error:?}");
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM capabilities", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT status FROM dispatch_requests", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "queued"
    );
}

#[test]
fn recurrence_run_rechecks_peers_after_the_claim() {
    let h = two_occurrences();
    let path = h._dir.path().join("recurrence.sqlite");
    common::assign_lane(&path, 4);
    let queued = h
        .core
        .command(
            "dispatch.request",
            &json!({
        "mutation":common::mutation(0,"before-late-reopen"),"ticket_id":4}),
        )
        .unwrap();
    let claim=h.core.command("dispatch.claim",&json!({
        "mutation":common::mutation(1,"claim-before-late-reopen"),"dispatch_request_id":queued["id"]})).unwrap();
    assert_eq!(claim["claimed"], true);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("UPDATE tickets SET state='ready' WHERE id=3", [])
        .unwrap();
    let result=h.core.command("run.acknowledge",&json!({
        "mutation":common::mutation(claim["request"]["version"].as_u64().unwrap(),"run-after-peer-reopen"),
        "dispatch_request_id":queued["id"]}));
    let error = result.expect_err("the run must recheck recurring overlap before execution");
    assert!(error.message.contains("occurrence"), "{error:?}");
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM runs", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn recurrence_template_is_not_an_executable_occurrence() {
    let h = scheduled_pair();
    let conn = rusqlite::Connection::open(h._dir.path().join("recurrence.sqlite")).unwrap();
    conn.execute(
        "UPDATE tickets SET profile='standard',mode='agent' WHERE id=2",
        [],
    )
    .unwrap();
    let error = h
        .core
        .command(
            "dispatch.request",
            &json!({
        "mutation":common::mutation(0,"template-dispatch"),"ticket_id":2}),
        )
        .expect_err("a template cannot execute instead of a fresh occurrence");
    assert!(error.message.contains("template"), "{error:?}");
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM dispatch_requests", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn recurrence_outcome_failure_rolls_back_the_whole_window() {
    use kanban_app::recurrence::RecurrencePass;
    use kanban_storage::recurrence::SqliteRecurrenceStore;
    for now in ["2026-09-08T09:15:20Z", "2026-09-08T09:46:00Z"] {
        let h = scheduled_pair();
        let pass = RecurrencePass::new(Arc::new(SqliteRecurrenceStore::new(&h._database)));
        let conn = rusqlite::Connection::open(h._dir.path().join("recurrence.sqlite")).unwrap();
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM timeline_events", [], |r| r.get(0))
            .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_recurrence BEFORE INSERT ON timeline_events
            WHEN json_extract(NEW.detail,'$.action')='recurrence_advanced'
            BEGIN SELECT RAISE(ABORT,'fixture rejects recurrence receipt'); END;",
        )
        .unwrap();
        assert!(pass.tick(now, &NoopEventSink).is_err());
        for table in ["task_occurrences", "schedule_attention"] {
            assert_eq!(
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row("SELECT ticket_counter FROM projects WHERE id=1", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row(
                "SELECT next_activation FROM schedules WHERE id=1",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "2026-09-08T09:15:00.000Z"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM timeline_events", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            before
        );
        conn.execute_batch("DROP TRIGGER fail_recurrence").unwrap();
        pass.tick(now, &NoopEventSink).unwrap();
        assert_ne!(
            conn.query_row(
                "SELECT next_activation FROM schedules WHERE id=1",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "2026-09-08T09:15:00.000Z"
        );
    }
}

#[test]
fn recurrence_two_writers_cannot_duplicate_a_window() {
    let h = scheduled_pair();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let barrier = barrier.clone();
        let path = h._dir.path().join("recurrence.sqlite");
        workers.push(std::thread::spawn(move || {
            let database = Database::open(&path).unwrap();
            let pass = kanban_app::recurrence::RecurrencePass::new(Arc::new(
                kanban_storage::recurrence::SqliteRecurrenceStore::new(&database),
            ));
            barrier.wait();
            pass.tick("2026-09-08T09:15:20Z", &NoopEventSink)
                .map(|r| r.minted.len())
        }));
    }
    let minted: usize = workers
        .into_iter()
        .map(|w| w.join().unwrap().unwrap())
        .sum();
    assert_eq!(minted, 1);
    let conn = rusqlite::Connection::open(h._dir.path().join("recurrence.sqlite")).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM task_occurrences", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT ticket_counter FROM projects WHERE id=1", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        3
    );
}
