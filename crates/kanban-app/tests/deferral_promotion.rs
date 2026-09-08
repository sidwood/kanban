mod common;
use common::mutation;
use common::review::prepared;
use serde_json::{Value, json};

fn recorded_finding() -> (common::DispatchHarness, Value) {
    let (h, ticket, submission) = prepared();
    let review=h.core.command("review.start",&json!({"mutation":mutation(0,"deferred-review"),"ticket_id":ticket,"submission_id":submission["id"]})).unwrap();
    h.core.command("review.human.submit",&json!({"mutation":mutation(1,"advisory-finding"),"review_id":review["id"],"slot_id":review["stages"][0]["slots"][1]["id"],
        "tip":"a".repeat(40),"approve":true,"summary":"Advisory only","findings":[{
            "severity":"p3","in_scope":false,"summary":"Add operator guidance","evidence":"The walkthrough has no help text",
            "location":"review panel","proposed_resolution":"Document the review workflow"}]})).unwrap();
    let listed = h
        .core
        .query(
            "finding.list",
            &json!({"project_id":1,"review_id":review["id"]}),
        )
        .expect("recorded findings can be discovered without reading the database");
    assert_eq!(listed["findings"].as_array().unwrap().len(), 1);
    let finding = listed["findings"][0].clone();
    assert_eq!(finding["review_id"], review["id"]);
    assert_eq!(finding["ticket_id"], ticket);
    assert_eq!(finding["tip"], submission["result"]["tip"]);
    (h, finding)
}

#[test]
fn finding_records_have_stable_project_scoped_custody() {
    let (h, finding) = recorded_finding();
    assert_eq!(finding["blocking"], false);
    assert_eq!(finding["finding"]["severity"], "p3");
    let queried = h
        .core
        .query(
            "finding.get",
            &json!({"project_id":1,"finding_id":finding["id"]}),
        )
        .unwrap();
    assert_eq!(queried, finding);
    let invalid = format!(" {}", finding["id"].as_str().unwrap());
    assert!(
        h.core
            .query("finding.get", &json!({"project_id":1,"finding_id":invalid}))
            .is_err(),
        "malformed identifiers are never normalised into valid lookups"
    );
}

#[test]
fn deferral_promotion_creates_a_draft_bug_without_rewriting_the_deferral() {
    let (h, finding) = recorded_finding();
    let deferred=h.core.command("deferral.record",&json!({"mutation":mutation(0,"defer"),"project_id":1,"finding_id":finding["id"],"reason":"Outside the reviewed slice"})).unwrap();
    let request = json!({"mutation":mutation(0,"promote"),"project_id":1,"deferral_id":deferred["id"],"priority":"normal","target":{"kind":"bug"}});
    let result = h
        .core
        .command("deferral.promote", &request)
        .expect("promotion creates one linked draft Bug");
    assert_eq!(result["ticket"]["kind"], "bug");
    assert_eq!(result["ticket"]["state"], "draft");
    assert_eq!(result["ticket"]["title"], finding["finding"]["summary"]);
    assert_eq!(result["promotion"]["finding_id"], finding["id"]);
    assert_eq!(result["promotion"]["deferral_id"], deferred["id"]);
    assert_eq!(result["promotion"]["ticket_id"], result["ticket"]["id"]);
    assert!(
        result["ticket"]["bug"]["qualification"].is_null(),
        "promotion cannot invent qualification evidence"
    );
    let history = h
        .core
        .query(
            "deferral.list",
            &json!({"project_id":1,"finding_id":finding["id"]}),
        )
        .unwrap();
    assert_eq!(history["deferrals"], json!([deferred]));
    let source = h
        .core
        .query(
            "finding.get",
            &json!({"project_id":1,"finding_id":finding["id"]}),
        )
        .unwrap();
    assert_eq!(source["finding"], finding["finding"]);
    assert_eq!(source["promotion"], result["promotion"]);
    assert_eq!(
        h.core.command("deferral.promote", &request).unwrap(),
        result
    );
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn deferral_promotion_creates_an_explicitly_bounded_task() {
    let (h, finding) = recorded_finding();
    let deferred=h.core.command("deferral.record",&json!({"mutation":mutation(0,"defer-task"),"project_id":1,"finding_id":finding["id"],"reason":"Separate documentation work"})).unwrap();
    let result=h.core.command("deferral.promote",&json!({"mutation":mutation(0,"promote-task"),"project_id":1,"deferral_id":deferred["id"],"priority":"low",
        "target":{"kind":"task","subtype":"administrative","mode":"human","completion":["Publish review workflow help"]}})).expect("Task promotion retains the explicit subtype, mode and bounds");
    assert_eq!(result["ticket"]["kind"], "task");
    assert_eq!(result["ticket"]["state"], "draft");
    assert_eq!(result["ticket"]["priority"], "low");
    assert_eq!(result["ticket"]["subtype"], "administrative");
    assert_eq!(result["ticket"]["mode"], "human");
    assert_eq!(
        result["ticket"]["completion"],
        json!(["Publish review workflow help"])
    );
    assert!(result["ticket"]["profile"].is_null());
    assert_eq!(result["promotion"]["finding_id"], finding["id"]);
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dispatch_requests WHERE ticket_id=?1",
            [result["ticket"]["id"].as_i64().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "promotion alone never dispatches the new work");
}

#[test]
fn finding_records_cannot_be_replaced_under_their_stable_identity() {
    let (h, finding) = recorded_finding();
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    let id = finding["slot_id"].as_i64().unwrap();
    let stored: String = conn
        .query_row(
            "SELECT record FROM review_slot_verdicts WHERE slot_id=?1",
            [id],
            |row| row.get(0),
        )
        .unwrap();
    let mut changed: Value = serde_json::from_str(&stored).unwrap();
    changed["findings"][0]["summary"] = json!("A different finding");
    assert!(
        conn.execute(
            "UPDATE review_slot_verdicts SET record=?2 WHERE slot_id=?1",
            rusqlite::params![id, changed.to_string()]
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "INSERT OR REPLACE INTO review_slot_verdicts(slot_id,record) VALUES (?1,?2)",
            rusqlite::params![id, changed.to_string()]
        )
        .is_err(),
        "replacement must not bypass source immutability"
    );
    assert_eq!(
        h.core
            .query(
                "finding.get",
                &json!({"project_id":1,"finding_id":finding["id"]})
            )
            .unwrap(),
        finding
    );
}

#[test]
fn immutable_deferrals_and_rulings_refuse_replacement_of_ids_and_successors() {
    for table in ["deferrals", "rulings"] {
        for successor in [false, true] {
            let (h, finding) = recorded_finding();
            let conn = rusqlite::Connection::open(&h.database_path).unwrap();
            let (columns, values, changed) = if table == "deferrals" {
                (
                    "project_id,finding_id,reason",
                    "'1','fixture-finding','Original reason'",
                    "reason",
                )
            } else {
                ("project_id,summary", "'1','Original decision'", "summary")
            };
            conn.execute(
                &format!("INSERT INTO {table} ({columns}) VALUES ({values})"),
                [],
            )
            .unwrap();
            let original = conn.last_insert_rowid();
            let id = if successor {
                conn.execute(
                    &format!("INSERT INTO {table} ({columns},supersedes_id) VALUES ({values},?1)"),
                    [original],
                )
                .unwrap();
                conn.last_insert_rowid()
            } else {
                original
            };
            let sql = if successor {
                format!(
                    "INSERT OR REPLACE INTO {table} ({columns},supersedes_id) VALUES ({values},?1)"
                )
            } else {
                format!("INSERT OR REPLACE INTO {table} (id,{columns}) VALUES (?1,{values})")
            };
            let identity = if successor { original } else { id };
            assert!(
                conn.execute(&sql, [identity]).is_err(),
                "{table} replacement must not bypass immutable history (successor={successor})"
            );
            let text: String = conn
                .query_row(
                    &format!("SELECT {changed} FROM {table} WHERE id=?1"),
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(text.starts_with("Original"));
            assert_eq!(
                h.core
                    .query(
                        "finding.get",
                        &json!({"project_id":1,"finding_id":finding["id"]})
                    )
                    .unwrap(),
                finding
            );
        }
    }
}

#[test]
fn deferral_promotion_failure_rolls_back_ticket_counter_events_and_link() {
    let (h, finding) = recorded_finding();
    let deferred=h.core.command("deferral.record",&json!({"mutation":mutation(0,"rollback-defer"),"project_id":1,"finding_id":finding["id"],"reason":"Separate work"})).unwrap();
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    let counters = || {
        conn.query_row(
            "SELECT ticket_counter,version FROM projects WHERE id=1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap()
    };
    let before = counters();
    let events: i64 = conn
        .query_row("SELECT COUNT(*) FROM timeline_events", [], |row| row.get(0))
        .unwrap();
    conn.execute_batch("CREATE TRIGGER refuse_promotion BEFORE INSERT ON finding_promotions BEGIN SELECT RAISE(ABORT,'fixture promotion failure'); END;").unwrap();
    let request = json!({"mutation":mutation(0,"rollback-promote"),"project_id":1,"deferral_id":deferred["id"],"priority":"normal","target":{"kind":"bug"}});
    assert!(h.core.command("deferral.promote", &request).is_err());
    assert_eq!(counters(), before);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM tickets", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM timeline_events", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        events
    );
    assert!(
        h.core
            .query(
                "finding.get",
                &json!({"project_id":1,"finding_id":finding["id"]})
            )
            .unwrap()["promotion"]
            .is_null()
    );
    conn.execute_batch("DROP TRIGGER refuse_promotion").unwrap();
    assert!(
        h.core.command("deferral.promote", &request).is_ok(),
        "a failed promotion does not consume the replay key"
    );
}

#[test]
fn deferral_promotion_refuses_duplicate_and_superseded_promotions() {
    let (h, finding) = recorded_finding();
    let original=h.core.command("deferral.record",&json!({"mutation":mutation(0,"original-defer"),"project_id":1,"finding_id":finding["id"],"reason":"Original reason"})).unwrap();
    let current=h.core.command("deferral.supersede",&json!({"mutation":mutation(0,"new-reason"),"project_id":1,"deferral_id":original["id"],"reason":"Current reason"})).unwrap();
    let payload = |id: Value, key: &str| json!({"mutation":mutation(0,key),"project_id":1,"deferral_id":id,"priority":"normal","target":{"kind":"bug"}});
    assert!(
        h.core
            .command(
                "deferral.promote",
                &payload(original["id"].clone(), "old-promote")
            )
            .is_err()
    );
    let promoted = h
        .core
        .command(
            "deferral.promote",
            &payload(current["id"].clone(), "current-promote"),
        )
        .unwrap();
    assert!(
        h.core
            .command(
                "deferral.promote",
                &payload(current["id"].clone(), "duplicate-promote")
            )
            .is_err()
    );
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(
        conn.execute("UPDATE finding_promotions SET ticket_id=1", [])
            .is_err()
    );
    assert!(conn.execute("DELETE FROM finding_promotions", []).is_err());
    assert!(
        conn.execute(
            "INSERT OR REPLACE INTO finding_promotions SELECT * FROM finding_promotions",
            []
        )
        .is_err()
    );
    assert_eq!(
        h.core
            .query(
                "finding.get",
                &json!({"project_id":1,"finding_id":finding["id"]})
            )
            .unwrap()["promotion"],
        promoted["promotion"]
    );
}

#[test]
fn deferral_promotion_refuses_foreign_findings_even_when_the_deferral_is_local() {
    use kanban_app::ProjectStore;
    let (h, finding) = recorded_finding();
    let db = kanban_storage::Database::open(&h.database_path).unwrap();
    let registration = kanban_domain::ProjectRegistration::new(
        "OTHER",
        "Another Project",
        "/other/repo",
        "/other/seed",
        "main",
        "other.seed",
        None,
        None,
    )
    .unwrap();
    let other = kanban_storage::SqliteProjectStore::new(&db)
        .create(&registration, &|id| {
            kanban_app::TimelineEnvelope::project(
                id.value(),
                kanban_dto::TimelineEventKind::Transition,
                None,
                json!({"action":"fixture"}),
            )
        })
        .unwrap();
    let project_id = other.id().value();
    let listed = h
        .core
        .query("finding.list", &json!({"project_id":project_id}))
        .unwrap();
    assert!(listed["findings"].as_array().unwrap().is_empty());
    assert!(
        h.core
            .query(
                "finding.get",
                &json!({"project_id":project_id,"finding_id":finding["id"]})
            )
            .is_err()
    );
    let deferred=h.core.command("deferral.record",&json!({"mutation":mutation(0,"foreign-reference"),"project_id":project_id,"finding_id":finding["id"],"reason":"Untrusted external reference"})).unwrap();
    assert!(h.core.command("deferral.promote",&json!({"mutation":mutation(0,"foreign-promote"),"project_id":project_id,"deferral_id":deferred["id"],"priority":"normal","target":{"kind":"bug"}})).is_err());
    assert!(
        h.core
            .query(
                "finding.get",
                &json!({"project_id":1,"finding_id":finding["id"]})
            )
            .unwrap()["promotion"]
            .is_null()
    );
}

#[test]
fn deferral_promotion_requires_valid_task_bounds() {
    let (h, finding) = recorded_finding();
    let deferred=h.core.command("deferral.record",&json!({"mutation":mutation(0,"bounds-defer"),"project_id":1,"finding_id":finding["id"],"reason":"Bounded follow-up"})).unwrap();
    assert!(h.core.command("deferral.promote",&json!({"mutation":mutation(0,"empty-bounds"),"project_id":1,"deferral_id":deferred["id"],"priority":"normal",
        "target":{"kind":"task","subtype":"administrative","mode":"human","completion":[]}})).is_err());
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM tickets", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
