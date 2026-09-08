//! Attention projection starts from real source rows, not invented inbox fixtures.
use kanban_app::catalog::exposed_operations;
use kanban_app::{Core, NoopEventSink};
use kanban_storage::{AllowAllMigrations, Database, RetentionPolicy, SqliteIdempotencyStore};
use serde_json::json;
use std::num::NonZeroU32;
use std::sync::Arc;

mod common;

#[derive(Default)]
struct PermissionFixture(std::sync::atomic::AtomicUsize);
impl kanban_app::notifications::NotificationPermissionPort for PermissionFixture {
    fn status(&self) -> kanban_dto::NotificationPermissionRecord {
        kanban_dto::NotificationPermissionRecord {
            state: kanban_dto::NotificationPermissionState::NotDetermined,
            reason: None,
            request_pending: false,
        }
    }
    fn request(&self) -> Result<bool, kanban_dto::ApiError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(true)
    }
}

struct Harness {
    core: Core,
    projector: kanban_app::attention::AttentionProjector,
    conn: rusqlite::Connection,
    _database: Database,
    _dir: tempfile::TempDir,
    ticket: u64,
    permissions: Arc<PermissionFixture>,
}
fn harness() -> Harness {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("attention.sqlite");
    let mut database = Database::open(&path).unwrap();
    database.migrate(&AllowAllMigrations).unwrap();
    common::seed_project_profile(&database);
    common::insert_ticket(&path, 1, "normal");
    let ticket = common::insert_ticket(&path, 2, "normal");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("UPDATE projects SET ticket_counter=(SELECT MAX(number) FROM tickets WHERE project_id=1) WHERE id=1",[]).unwrap();
    let mut core = Core::new(
        exposed_operations(),
        Arc::new(SqliteIdempotencyStore::new(
            &database,
            RetentionPolicy::keep_most_recent(NonZeroU32::new(100).unwrap()),
        )),
        Arc::new(NoopEventSink),
    );
    core.register_lifecycle(
        Arc::new(kanban_storage::SqliteTicketStore::new(&database)),
        Arc::new(kanban_storage::SqliteDependencyStore::new(&database)),
        Arc::new(kanban_storage::SqliteProjectStore::new(&database)),
        Arc::new(kanban_storage::SqliteScheduleStore::new(&database)),
        None,
    )
    .unwrap();
    core.register_deferrals(
        Arc::new(kanban_storage::SqliteDeferralStore::new(&database)),
        Arc::new(kanban_storage::SqliteProjectStore::new(&database)),
    )
    .unwrap();
    core.register_notification_settings(
        Arc::new(kanban_storage::notifications::SqliteNotificationSettingsStore::new(&database)),
        Arc::new(kanban_storage::SqliteProjectStore::new(&database)),
    )
    .unwrap();
    let permissions = Arc::new(PermissionFixture::default());
    core.register_notification_permissions(permissions.clone())
        .unwrap();
    core.register_notification_deliveries(
        Arc::new(kanban_storage::notifications::SqliteNotificationDeliveryStore::new(&database)),
        Arc::new(kanban_storage::SqliteProjectStore::new(&database)),
    )
    .unwrap();
    let store = Arc::new(kanban_storage::attention::SqliteAttentionStore::new(
        &database,
    ));
    let projector = kanban_app::attention::AttentionProjector::new(
        Arc::new(kanban_storage::attention::SqliteAttentionSource::new(
            &database,
        )),
        store.clone(),
    );
    core.register_attention(store).unwrap();
    Harness {
        core,
        projector,
        conn,
        _database: database,
        _dir: dir,
        ticket,
        permissions,
    }
}

#[test]
fn attention_consolidation_projects_a_real_blocker() {
    let h = harness();
    h.conn.execute("INSERT INTO ticket_blockers (ticket_id,description) VALUES (2,'Operator approval is required')",[]).unwrap();
    h.projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let inbox = h
        .core
        .query("attention.list", &json!({}))
        .expect("real blockers appear in the Attention Inbox");
    let items = inbox["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "blocker");
    assert_eq!(items[0]["project_id"], 1);
    assert_eq!(items[0]["subject_id"], h.ticket.to_string());
    assert!(
        items[0]["summary"]
            .as_str()
            .unwrap()
            .contains("Operator approval")
    );
    assert_eq!(items[0]["acknowledged_by"], serde_json::Value::Null);
}

#[test]
fn attention_consolidation_projects_a_dependency_and_does_not_ack_its_resolution() {
    let h = harness();
    h.conn
        .execute(
            "INSERT INTO ticket_dependencies (from_ticket,to_ticket) VALUES (1,2)",
            [],
        )
        .unwrap();
    h.projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let inbox = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(
        inbox["items"].as_array().unwrap().len(),
        1,
        "a prerequisite is an attention source too"
    );
    assert_eq!(inbox["items"][0]["kind"], "blocker");
    assert_eq!(inbox["items"][0]["subject_id"], h.ticket.to_string());
    h.conn
        .execute("UPDATE tickets SET state='done' WHERE id=1", [])
        .unwrap();
    h.projector.refresh("2026-09-08T00:01:00Z").unwrap();
    assert!(
        h.core.query("attention.list", &json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let history = h
        .core
        .query("attention.list", &json!({"include_inactive":true}))
        .unwrap();
    assert_eq!(history["items"][0]["active"], false);
    assert_eq!(
        history["items"][0]["acknowledged_by"],
        serde_json::Value::Null
    );
}

#[test]
fn attention_consolidation_acknowledgement_records_operator_time_and_replays() {
    let h = harness();
    h.conn.execute("INSERT INTO ticket_blockers (ticket_id,description) VALUES (2,'An operator must choose')",[]).unwrap();
    h.projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let item = h.core.query("attention.list", &json!({})).unwrap()["items"][0].clone();
    let request = json!({"mutation":common::mutation(item["version"].as_u64().unwrap(),"acknowledge-blocker"),
        "item_id":item["id"],"who":"Operator A"});
    let acknowledged = h
        .core
        .command("attention.acknowledge", &request)
        .expect("only an explicit operator action acknowledges the item");
    assert_eq!(acknowledged["acknowledged_by"], "Operator A");
    let at = acknowledged["acknowledged_at"].as_str().unwrap();
    assert!(
        time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339).is_ok()
    );
    assert!(at.ends_with('Z'));
    assert!(
        h.core.query("attention.list", &json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        h.core.command("attention.acknowledge", &request).unwrap(),
        acknowledged
    );
    let all = h
        .core
        .query("attention.list", &json!({"include_acknowledged":true}))
        .unwrap();
    assert_eq!(all["items"], json!([acknowledged]));
    assert_eq!(
        h.conn
            .query_row("SELECT COUNT(*) FROM attention_acknowledgements", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        h.conn
            .query_row("SELECT state FROM tickets WHERE id=2", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "draft"
    );
}

#[test]
fn attention_consolidation_keeps_acknowledgements_until_a_new_cause_arrives() {
    let h = harness();
    h.conn.execute("INSERT INTO ticket_blockers (ticket_id,description) VALUES (2,'Choose the release window')",[]).unwrap();
    h.projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let item = h.core.query("attention.list", &json!({})).unwrap()["items"][0].clone();
    let acknowledged = h
        .core
        .command(
            "attention.acknowledge",
            &json!({
        "mutation":common::mutation(item["version"].as_u64().unwrap(),"ack-window"),
        "item_id":item["id"],"who":"Operator A",}),
        )
        .unwrap();
    h.projector.refresh("2026-09-08T00:01:00Z").unwrap();
    assert!(
        h.core.query("attention.list", &json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let unchanged = h
        .core
        .query("attention.list", &json!({"include_acknowledged":true}))
        .unwrap();
    assert_eq!(unchanged["items"][0]["version"], acknowledged["version"]);
    h.conn.execute("INSERT INTO ticket_blockers (ticket_id,description) VALUES (2,'A new safety decision is required')",[]).unwrap();
    h.projector.refresh("2026-09-08T00:02:00Z").unwrap();
    let fresh = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(fresh["items"].as_array().unwrap().len(), 1);
    assert_eq!(fresh["items"][0]["id"], item["id"]);
    assert_eq!(
        fresh["items"][0]["acknowledged_by"],
        serde_json::Value::Null
    );
    assert_eq!(
        h.conn
            .query_row("SELECT who FROM attention_acknowledgements", [], |r| r
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "Operator A"
    );
}

#[test]
fn attention_consolidation_includes_real_missed_schedule_windows() {
    let h = harness();
    h.core.command("ticket.schedule",&json!({
        "mutation":common::mutation(1,"schedule-for-attention"),"ticket_id":h.ticket,
        "cron":"*/15 * * * *","timezone":"UTC","after":"2026-09-08T09:00:00Z","profile":"standard",
    })).unwrap();
    let schedules = kanban_app::recurrence::RecurrencePass::new(Arc::new(
        kanban_storage::recurrence::SqliteRecurrenceStore::new(&h._database),
    ));
    assert!(
        schedules
            .tick("2026-09-08T09:46:00Z", &NoopEventSink)
            .unwrap()
            .minted
            .is_empty()
    );
    h.projector.refresh("2026-09-08T09:46:00Z").unwrap();
    let inbox = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(
        inbox["items"].as_array().unwrap().len(),
        1,
        "missed windows flow to the Inbox"
    );
    assert_eq!(inbox["items"][0]["kind"], "failed_schedule");
    assert_eq!(inbox["items"][0]["subject_kind"], "schedule");
    assert!(
        inbox["items"][0]["summary"]
            .as_str()
            .unwrap()
            .contains("Missed")
    );
}

#[test]
fn attention_consolidation_includes_invalid_bindings_without_changing_the_verdict() {
    let h = harness();
    h.conn
        .execute(
            "INSERT INTO criterion_bindings
        (ticket_id,criterion_index,kind,evidence_id,tip,review,satisfied,void)
        VALUES (2,0,'task',0,?1,'validated',0,1)",
            ["a".repeat(40)],
        )
        .unwrap();
    h.projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let inbox = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(
        inbox["items"].as_array().unwrap().len(),
        1,
        "a void approval must be visible"
    );
    assert_eq!(inbox["items"][0]["kind"], "invalid_approval");
    assert_eq!(inbox["items"][0]["subject_id"], h.ticket.to_string());
    assert_eq!(
        h.conn
            .query_row("SELECT state FROM tickets WHERE id=2", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "draft"
    );
    assert_eq!(
        h.conn
            .query_row(
                "SELECT void FROM criterion_bindings WHERE ticket_id=2",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn attention_consolidation_includes_a_pending_deferral_as_a_human_decision() {
    let h = harness();
    let deferral = h
        .core
        .command(
            "deferral.record",
            &json!({
                "mutation":common::mutation(0,"defer-for-human"),"project_id":1,
                "finding_id":"finding-1","reason":"An operator must choose the follow-up scope",
            }),
        )
        .unwrap();
    h.projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let inbox = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(
        inbox["items"].as_array().unwrap().len(),
        1,
        "deferred follow-up needs an explicit decision"
    );
    assert_eq!(inbox["items"][0]["kind"], "human_decision");
    assert_eq!(inbox["items"][0]["subject_kind"], "deferral");
    assert_eq!(
        inbox["items"][0]["subject_id"],
        deferral["id"].as_u64().unwrap().to_string()
    );
    assert_eq!(
        h.conn
            .query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn attention_consolidation_includes_only_a_current_pending_human_review() {
    let (mut h, ticket, submission) = common::review::prepared();
    let review = h
        .core
        .command(
            "review.start",
            &json!({"mutation":common::mutation(0,"review-for-inbox"),
        "ticket_id":ticket,"submission_id":submission["id"]}),
        )
        .unwrap();
    let database = Database::open(&h.database_path).unwrap();
    let store = Arc::new(kanban_storage::attention::SqliteAttentionStore::new(
        &database,
    ));
    let projector = kanban_app::attention::AttentionProjector::new(
        Arc::new(kanban_storage::attention::SqliteAttentionSource::new(
            &database,
        )),
        store.clone(),
    );
    h.core.register_attention(store).unwrap();
    projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let inbox = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(
        inbox["items"].as_array().unwrap().len(),
        1,
        "the waiting human slot must be visible"
    );
    assert_eq!(inbox["items"][0]["kind"], "review_request");
    assert_eq!(inbox["items"][0]["subject_id"], ticket.to_string());
    let current = h
        .core
        .query("review.get", &json!({"review_id":review["id"]}))
        .unwrap();
    let updated = h
        .core
        .command(
            "review.human.submit",
            &json!({
                "mutation":common::mutation(current["version"].as_u64().unwrap(),"human-reviewed"),
                "review_id":review["id"],"slot_id":review["stages"][0]["slots"][1]["id"],
                "tip":"a".repeat(40),"approve":true,"summary":"Human decision recorded",
            }),
        )
        .unwrap();
    assert_eq!(
        updated["status"], "in_progress",
        "the agent review is still required"
    );
    projector.refresh("2026-09-08T00:01:00Z").unwrap();
    assert!(
        h.core.query("attention.list", &json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn attention_consolidation_deadlines_keep_the_observed_run_identity() {
    use kanban_app::deadlines::{DeadlineConfig, DeadlineMonitor};
    use std::time::{Duration, UNIX_EPOCH};
    let mut deadlines = DeadlineMonitor::new(DeadlineConfig::from_secs(10, 10));
    deadlines.observe_event(
        UNIX_EPOCH + Duration::from_secs(100),
        &json!({"kind":"role.settled","role":"implementer","run":7}),
    );
    let signals = deadlines.evaluate(1, UNIX_EPOCH + Duration::from_secs(111));
    assert_eq!(signals.len(), 1);
    assert_eq!(
        signals[0].detail["run_id"], 7,
        "a bound deadline must deduplicate with its Run's missing submission"
    );
}

#[test]
fn attention_consolidation_new_runs_do_not_inherit_old_settlement_deadlines() {
    use kanban_app::deadlines::{DeadlineConfig, DeadlineMonitor, STALL_DEADLINE_REASON};
    use std::time::{Duration, UNIX_EPOCH};
    let mut deadlines = DeadlineMonitor::new(DeadlineConfig::from_secs(10, 10));
    deadlines.observe_event(
        UNIX_EPOCH + Duration::from_secs(100),
        &json!({"kind":"role.settled","role":"implementer","run":7}),
    );
    deadlines.observe_event(
        UNIX_EPOCH + Duration::from_secs(200),
        &json!({"kind":"role.output","role":"implementer","run":8}),
    );
    let signals = deadlines.evaluate(1, UNIX_EPOCH + Duration::from_secs(211));
    assert_eq!(signals.len(), 1);
    assert_eq!(
        signals[0].reason, STALL_DEADLINE_REASON,
        "the old run settled, not this run"
    );
    assert_eq!(signals[0].detail["run_id"], 8);
}

struct RuntimeFixture(std::sync::Mutex<kanban_app::attention::RuntimeAttentionSnapshot>);
impl kanban_app::attention::RuntimeAttentionFeed for RuntimeFixture {
    fn snapshot(
        &self,
        _project: &kanban_domain::Project,
    ) -> Result<kanban_app::attention::RuntimeAttentionSnapshot, kanban_dto::ApiError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

fn runtime_snapshot(database: &Database) -> kanban_app::attention::RuntimeAttentionSnapshot {
    use kanban_app::ProjectStore;
    let project = kanban_storage::SqliteProjectStore::new(database)
        .find(kanban_domain::ProjectId::new(1))
        .unwrap()
        .unwrap();
    kanban_app::attention::RuntimeAttentionSnapshot {
        diagnostics: kanban_dto::HerdrConnectionDiagnostics {
            session_name: project.registration().herdr_session().map(str::to_owned),
            product_workspace: project.registration().seed_workspace().to_owned(),
            herdr_workspace: project.registration().herdr_workspace().to_owned(),
            connected: true,
            last_snapshot_at: Some("2026-09-08T00:00:00Z".to_owned()),
            last_error: None,
        },
        signals: Vec::new(),
    }
}

#[test]
fn attention_consolidation_includes_a_known_disconnected_binding() {
    let h = harness();
    let mut snapshot = runtime_snapshot(&h._database);
    snapshot.diagnostics.connected = false;
    snapshot.diagnostics.last_error = Some("session socket is unavailable".to_owned());
    let runtime = Arc::new(RuntimeFixture(std::sync::Mutex::new(snapshot)));
    let projector = kanban_app::attention::AttentionProjector::new(
        Arc::new(
            kanban_storage::attention::SqliteAttentionSource::new(&h._database)
                .with_runtime(runtime.clone()),
        ),
        Arc::new(kanban_storage::attention::SqliteAttentionStore::new(
            &h._database,
        )),
    );
    projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let inbox = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(
        inbox["items"].as_array().unwrap().len(),
        1,
        "a known disconnect belongs in the global Inbox"
    );
    assert_eq!(inbox["items"][0]["kind"], "disconnected_session");
    runtime.0.lock().unwrap().diagnostics.connected = true;
    projector.refresh("2026-09-08T00:01:00Z").unwrap();
    assert!(
        h.core.query("attention.list", &json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

fn pending_run() -> (
    common::DispatchHarness,
    u64,
    serde_json::Value,
    serde_json::Value,
) {
    let h = common::harness();
    let ticket = common::insert_ticket(&h.database_path, 1, "normal");
    common::assign_lane(&h.database_path, ticket);
    let queue = h
        .core
        .command(
            "dispatch.request",
            &json!({"mutation":common::mutation(0,"queue-runtime"),"ticket_id":ticket}),
        )
        .unwrap();
    let claim=h.core.command("dispatch.claim",&json!({"mutation":common::mutation(1,"claim-runtime"),"dispatch_request_id":queue["id"]})).unwrap();
    let run=h.core.command("run.acknowledge",&json!({"mutation":common::mutation(2,"run-runtime"),"dispatch_request_id":queue["id"]})).unwrap();
    (h, ticket, claim, run)
}

#[test]
fn attention_consolidation_merges_missing_results_across_real_producers() {
    use kanban_app::deadlines::{DeadlineConfig, DeadlineMonitor};
    use std::time::{Duration, UNIX_EPOCH};
    let (mut h, ticket, claim, run) = pending_run();
    let database = Database::open(&h.database_path).unwrap();
    let event = json!({"kind":"role.settled","role":"implementer","run":run["id"]});
    let missing = kanban_app::submission::missing_submission_signal(
        &kanban_storage::SqliteSubmissionStore::new(&database),
        1,
        &event,
    )
    .unwrap()
    .unwrap();
    let mut deadlines = DeadlineMonitor::new(DeadlineConfig::from_secs(10, 10));
    deadlines.observe_event(UNIX_EPOCH + Duration::from_secs(100), &event);
    let mut snapshot = runtime_snapshot(&database);
    snapshot.signals = deadlines.evaluate(1, UNIX_EPOCH + Duration::from_secs(111));
    snapshot.signals.extend([missing.clone(), missing]);
    let runtime = Arc::new(RuntimeFixture(std::sync::Mutex::new(snapshot)));
    let store = Arc::new(kanban_storage::attention::SqliteAttentionStore::new(
        &database,
    ));
    let projector = kanban_app::attention::AttentionProjector::new(
        Arc::new(
            kanban_storage::attention::SqliteAttentionSource::new(&database).with_runtime(runtime),
        ),
        store.clone(),
    );
    h.core.register_attention(store).unwrap();
    projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let inbox = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(
        inbox["items"].as_array().unwrap().len(),
        1,
        "a deadline and a missing submission describe one Run"
    );
    let item = &inbox["items"][0];
    assert_eq!(item["kind"], "missing_result");
    assert_eq!(item["subject_kind"], "run");
    assert_eq!(item["subject_id"], run["id"].as_u64().unwrap().to_string());
    assert_eq!(
        item["detail"]["sources"].as_array().unwrap().len(),
        2,
        "duplicate producer reports must collapse"
    );
    assert!(
        item["detail"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .all(|source| source["ticket_id"] == ticket)
    );
    let restarted = kanban_app::attention::AttentionProjector::new(
        Arc::new(kanban_storage::attention::SqliteAttentionSource::new(
            &database,
        )),
        Arc::new(kanban_storage::attention::SqliteAttentionStore::new(
            &database,
        )),
    );
    restarted.refresh("2026-09-08T00:00:30Z").unwrap();
    assert_eq!(
        h.core.query("attention.list", &json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "a restart must not forget a known missing result while observation reconnects"
    );
    h.core.command("submission.submit",&json!({"mutation":common::mutation(1,"runtime-result"),
        "run_id":run["id"],"capability_id":claim["capability"]["id"],
        "result":{"kind":"implementation","tip":"a".repeat(40),"summary":"Required result arrived"}})).unwrap();
    projector.refresh("2026-09-08T00:01:00Z").unwrap();
    assert!(
        h.core.query("attention.list", &json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn attention_consolidation_stall_re_reports_do_not_create_new_decisions() {
    use kanban_app::deadlines::{DeadlineConfig, DeadlineMonitor};
    use std::time::{Duration, UNIX_EPOCH};
    let (mut h, _, _, run) = pending_run();
    let database = Database::open(&h.database_path).unwrap();
    let mut deadlines = DeadlineMonitor::new(DeadlineConfig::from_secs(10, 10));
    deadlines.observe_event(
        UNIX_EPOCH + Duration::from_secs(100),
        &json!({"kind":"role.output","role":"implementer","run":run["id"]}),
    );
    let mut snapshot = runtime_snapshot(&database);
    snapshot.signals = deadlines.evaluate(1, UNIX_EPOCH + Duration::from_secs(111));
    let runtime = Arc::new(RuntimeFixture(std::sync::Mutex::new(snapshot)));
    let store = Arc::new(kanban_storage::attention::SqliteAttentionStore::new(
        &database,
    ));
    let projector = kanban_app::attention::AttentionProjector::new(
        Arc::new(
            kanban_storage::attention::SqliteAttentionSource::new(&database)
                .with_runtime(runtime.clone()),
        ),
        store.clone(),
    );
    h.core.register_attention(store).unwrap();
    projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let first = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(
        first["items"].as_array().unwrap().len(),
        1,
        "a breached stall deadline must surface"
    );
    assert_eq!(first["items"][0]["kind"], "stale_run");
    assert_eq!(
        first["items"][0]["subject_id"],
        run["id"].as_u64().unwrap().to_string()
    );
    runtime.0.lock().unwrap().signals =
        deadlines.evaluate(1, UNIX_EPOCH + Duration::from_secs(120));
    projector.refresh("2026-09-08T00:01:00Z").unwrap();
    let repeated = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(repeated["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        repeated["items"][0]["version"],
        first["items"][0]["version"]
    );
    assert_eq!(
        h.core.query("run.list", &json!({"project_id":1})).unwrap()["runs"][0]["status"],
        "executing"
    );
}

#[test]
fn attention_consolidation_reports_scheduler_failures_without_starving_other_work() {
    let h = harness();
    for ticket in [1, h.ticket] {
        h.core
            .command(
                "ticket.schedule",
                &json!({"mutation":common::mutation(1,format!("schedule-{ticket}")),
            "ticket_id":ticket,"cron":"*/15 * * * *","timezone":"UTC",
            "after":"2026-09-08T09:00:00Z","profile":"standard"}),
            )
            .unwrap();
    }
    h.conn
        .execute(
            "UPDATE schedules SET timezone='Unknown/Zone' WHERE ticket_id=1",
            [],
        )
        .unwrap();
    let pass = kanban_app::recurrence::RecurrencePass::new(Arc::new(
        kanban_storage::recurrence::SqliteRecurrenceStore::new(&h._database),
    ));
    let report = pass
        .tick("2026-09-08T09:15:20Z", &NoopEventSink)
        .expect("a failed schedule must not starve an independent due schedule");
    assert_eq!(report.minted.len(), 1);
    assert_eq!(report.failed, 1);
    h.projector.refresh("2026-09-08T09:15:20Z").unwrap();
    let inbox = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(inbox["items"].as_array().unwrap().len(), 1);
    assert_eq!(inbox["items"][0]["kind"], "failed_schedule");
    assert!(
        inbox["items"][0]["summary"]
            .as_str()
            .unwrap()
            .contains("could not advance")
    );
    assert_eq!(
        h.conn
            .query_row(
                "SELECT COUNT(*) FROM task_occurrences WHERE template_ticket_id=1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn attention_consolidation_all_eight_classes_share_one_global_projection() {
    use kanban_app::deadlines::{DeadlineConfig, DeadlineMonitor};
    use std::collections::BTreeSet;
    use std::time::{Duration, UNIX_EPOCH};
    let (mut h, review_ticket, submission) = common::review::prepared();
    h.core
        .register_lifecycle(
            Arc::new(kanban_storage::SqliteTicketStore::new(&h.database)),
            Arc::new(kanban_storage::SqliteDependencyStore::new(&h.database)),
            Arc::new(kanban_storage::SqliteProjectStore::new(&h.database)),
            Arc::new(kanban_storage::SqliteScheduleStore::new(&h.database)),
            None,
        )
        .unwrap();
    h.core
        .register_deferrals(
            Arc::new(kanban_storage::SqliteDeferralStore::new(&h.database)),
            Arc::new(kanban_storage::SqliteProjectStore::new(&h.database)),
        )
        .unwrap();
    h.core
        .command(
            "review.start",
            &json!({"mutation":common::mutation(0,"all-review"),
        "ticket_id":review_ticket,"submission_id":submission["id"]}),
        )
        .unwrap();
    h.core
        .command(
            "deferral.record",
            &json!({"mutation":common::mutation(0,"all-deferral"),
        "project_id":1,"finding_id":"finding-all","reason":"Choose a follow-up"}),
        )
        .unwrap();
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    conn.execute("UPDATE capacity_global_defaults SET max_active_per_harness=8,max_active_per_model=8,max_active_per_usage_pool=8,version=version+1 WHERE id=1",[]).unwrap();
    conn.execute(
        "INSERT INTO ticket_blockers(ticket_id,description) VALUES (?1,'Operator input required')",
        [review_ticket as i64],
    )
    .unwrap();
    conn.execute("INSERT INTO criterion_bindings(ticket_id,criterion_index,kind,evidence_id,tip,review,satisfied,void)
        VALUES (?1,0,'task',0,?2,'validated',0,1)",rusqlite::params![review_ticket as i64,"a".repeat(40)]).unwrap();
    let mut deadlines = DeadlineMonitor::new(DeadlineConfig::from_secs(10, 10));
    let mut missing = None;
    for (number, role, kind) in [
        (2, "missing", "role.settled"),
        (3, "stalled", "role.output"),
    ] {
        let ticket = common::insert_ticket(&h.database_path, number, "normal");
        common::assign_lane(&h.database_path, ticket);
        let queue=h.core.command("dispatch.request",&json!({"mutation":common::mutation(0,format!("all-queue-{number}")),"ticket_id":ticket})).unwrap();
        h.core.command("dispatch.claim",&json!({"mutation":common::mutation(1,format!("all-claim-{number}")),"dispatch_request_id":queue["id"]})).unwrap();
        let run=h.core.command("run.acknowledge",&json!({"mutation":common::mutation(2,format!("all-run-{number}")),"dispatch_request_id":queue["id"]})).unwrap();
        let event = json!({"kind":kind,"role":role,"run":run["id"]});
        deadlines.observe_event(UNIX_EPOCH + Duration::from_secs(100), &event);
        if kind == "role.settled" {
            missing = kanban_app::submission::missing_submission_signal(
                &kanban_storage::SqliteSubmissionStore::new(&h.database),
                1,
                &event,
            )
            .unwrap();
        }
    }
    let scheduled = common::insert_ticket(&h.database_path, 4, "normal");
    h.core.command("ticket.schedule",&json!({"mutation":common::mutation(1,"all-schedule"),"ticket_id":scheduled,
        "cron":"*/15 * * * *","timezone":"UTC","after":"2026-09-08T09:00:00Z","profile":"standard"})).unwrap();
    kanban_app::recurrence::RecurrencePass::new(Arc::new(
        kanban_storage::recurrence::SqliteRecurrenceStore::new(&h.database),
    ))
    .tick("2026-09-08T09:46:00Z", &NoopEventSink)
    .unwrap();
    let mut snapshot = runtime_snapshot(&h.database);
    snapshot.diagnostics.connected = false;
    snapshot.diagnostics.last_error = Some("offline".to_owned());
    snapshot.signals = deadlines.evaluate(1, UNIX_EPOCH + Duration::from_secs(111));
    let missing = missing.unwrap();
    snapshot.signals.extend([missing.clone(), missing]);
    let store = Arc::new(kanban_storage::attention::SqliteAttentionStore::new(
        &h.database,
    ));
    let projector = kanban_app::attention::AttentionProjector::new(
        Arc::new(
            kanban_storage::attention::SqliteAttentionSource::new(&h.database)
                .with_runtime(Arc::new(RuntimeFixture(std::sync::Mutex::new(snapshot)))),
        ),
        store.clone(),
    );
    h.core.register_attention(store).unwrap();
    projector.refresh("2026-09-08T09:46:00Z").unwrap();
    let inbox = h.core.query("attention.list", &json!({})).unwrap();
    let items = inbox["items"].as_array().unwrap();
    let kinds = items
        .iter()
        .map(|i| i["kind"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let expected = kanban_dto::AttentionState::ALL
        .iter()
        .map(|kind| kind.wire_name())
        .collect::<BTreeSet<_>>();
    assert_eq!(kinds, expected);
    assert_eq!(
        items.len(),
        expected.len(),
        "duplicate producer reports must not add extra Items"
    );
}

#[test]
fn attention_consolidation_populates_the_global_board_attention_filter() {
    let mut h = harness();
    h.core
        .register_board(
            Arc::new(kanban_storage::SqliteInitiativeStore::new(&h._database)),
            Arc::new(kanban_storage::SqliteProjectStore::new(&h._database)),
            Arc::new(kanban_storage::SqlitePlanStore::new(&h._database)),
            Arc::new(kanban_storage::SqliteSpecStore::new(&h._database)),
            Arc::new(kanban_storage::SqliteTicketStore::new(&h._database)),
            Arc::new(kanban_storage::SqliteLaneStore::new(&h._database)),
            Arc::new(kanban_storage::SqliteProfileStore::new(&h._database)),
            Some(Arc::new(
                kanban_storage::attention::SqliteAttentionStore::new(&h._database),
            )),
        )
        .unwrap();
    h.conn.execute("INSERT INTO ticket_blockers(ticket_id,description) VALUES (2,'Operator review needed')",[]).unwrap();
    h.projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let board = h
        .core
        .query("board.global", &json!({"filter":{"attention":["blocker"]}}))
        .unwrap();
    assert_eq!(
        board["cards"].as_array().unwrap().len(),
        1,
        "the board must consume the same attention projection"
    );
    assert_eq!(board["cards"][0]["ticket"]["id"], h.ticket);
    let item = h.core.query("attention.list", &json!({})).unwrap()["items"][0].clone();
    h.core
        .command(
            "attention.acknowledge",
            &json!({"mutation":common::mutation(item["version"].as_u64().unwrap(),"ack-board"),
        "item_id":item["id"],"who":"Operator A"}),
        )
        .unwrap();
    assert!(
        h.core
            .query("board.global", &json!({"filter":{"attention":["blocker"]}}))
            .unwrap()["cards"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn notification_no_ack_configuration_is_explicit_and_does_not_resolve_items() {
    let h = harness();
    h.conn.execute("INSERT INTO ticket_blockers(ticket_id,description) VALUES (2,'Keep this obligation open')",[]).unwrap();
    h.projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let defaults = h
        .core
        .query("notification.settings.get", &json!({"project_id":1}))
        .expect("notification preferences are available to the operator");
    assert_eq!(defaults["local_enabled"], false);
    assert_eq!(defaults["mirror_role"], serde_json::Value::Null);
    let saved=h.core.command("notification.settings.update",&json!({
        "mutation":common::mutation(defaults["version"].as_u64().unwrap(),"enable-notifications"),
        "project_id":1,"local_enabled":true,"mirror_role":"observer",
    })).unwrap();
    assert_eq!(saved["local_enabled"], true);
    assert_eq!(saved["mirror_role"], "observer");
    assert_eq!(
        h.core
            .query("notification.settings.get", &json!({"project_id":1}))
            .unwrap(),
        saved
    );
    let inbox = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(inbox["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        inbox["items"][0]["acknowledged_by"],
        serde_json::Value::Null
    );
    assert_eq!(
        h.conn
            .query_row("SELECT COUNT(*) FROM attention_acknowledgements", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

struct RecordingNotifications {
    path: std::path::PathBuf,
    messages: std::sync::Mutex<Vec<kanban_app::notifications::NotificationMessage>>,
    outcomes: std::sync::Mutex<
        std::collections::VecDeque<kanban_app::notifications::NotificationOutcome>,
    >,
}
impl kanban_app::notifications::NotificationSink for RecordingNotifications {
    fn deliver(
        &self,
        message: &kanban_app::notifications::NotificationMessage,
    ) -> kanban_app::notifications::NotificationOutcome {
        let conn = rusqlite::Connection::open(&self.path).unwrap();
        let status: String = conn
            .query_row(
                "SELECT status FROM notification_deliveries WHERE id=?1",
                [message.delivery_id as i64],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "prepared",
            "intent must be durable before the external effect"
        );
        self.messages.lock().unwrap().push(message.clone());
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(
                || kanban_app::notifications::NotificationOutcome::Submitted {
                    receipt: format!("fixture-{}", message.delivery_id),
                },
            )
    }
}

#[test]
fn notification_no_ack_delivery_records_only_delivery() {
    let h = harness();
    h.conn.execute("INSERT INTO ticket_blockers(ticket_id,description) VALUES (2,'SECRET-IN-SOURCE-MUST-STAY-IN-INBOX')",[]).unwrap();
    h.projector.refresh("2026-09-08T00:00:00Z").unwrap();
    h.core
        .command(
            "notification.settings.update",
            &json!({"mutation":common::mutation(0,"notify-explicitly"),
        "project_id":1,"local_enabled":true,"mirror_role":"observer"}),
        )
        .unwrap();
    let sink = Arc::new(RecordingNotifications {
        path: h._dir.path().join("attention.sqlite"),
        messages: std::sync::Mutex::new(Vec::new()),
        outcomes: std::sync::Mutex::new(std::collections::VecDeque::new()),
    });
    let dispatcher = kanban_app::notifications::NotificationDispatcher::new(
        Arc::new(kanban_storage::attention::SqliteAttentionStore::new(
            &h._database,
        )),
        Arc::new(kanban_storage::notifications::SqliteNotificationSettingsStore::new(&h._database)),
        Arc::new(kanban_storage::notifications::SqliteNotificationDeliveryStore::new(&h._database)),
        Arc::new(kanban_storage::SqliteProjectStore::new(&h._database)),
        sink.clone(),
    );
    let report = dispatcher.dispatch_once("2026-09-08T00:00:00Z").unwrap();
    assert_eq!(report.submitted, 2);
    let messages = sink.messages.lock().unwrap();
    assert_eq!(messages.len(), 2);
    assert!(
        messages
            .iter()
            .all(|message| !message.title.contains("SECRET-IN-SOURCE")
                && !message.body.contains("SECRET-IN-SOURCE"))
    );
    drop(messages);
    assert_eq!(
        dispatcher
            .dispatch_once("2026-09-08T00:01:00Z")
            .unwrap()
            .submitted,
        0
    );
    assert_eq!(
        h.conn
            .query_row(
                "SELECT COUNT(*) FROM notification_deliveries WHERE status='submitted'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(
        h.conn
            .query_row("SELECT COUNT(*) FROM attention_acknowledgements", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let inbox = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(inbox["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        inbox["items"][0]["acknowledged_by"],
        serde_json::Value::Null
    );
}

#[test]
fn notification_no_ack_permission_requests_are_not_grants_or_acknowledgements() {
    let h = harness();
    h.conn
        .execute(
            "INSERT INTO ticket_blockers(ticket_id,description) VALUES (2,'Still needs a person')",
            [],
        )
        .unwrap();
    h.projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let permission = h
        .core
        .query("notification.permission.get", &json!({}))
        .expect("the operator can read actual platform permission state");
    assert_eq!(permission["state"], "not_determined");
    let request = json!({"mutation":common::mutation(0,"request-native-permission")});
    let accepted = h
        .core
        .command("notification.permission.request", &request)
        .unwrap();
    assert_eq!(accepted["accepted"], true);
    assert_eq!(
        h.core
            .command("notification.permission.request", &request)
            .unwrap(),
        accepted
    );
    assert_eq!(h.permissions.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        h.core
            .query("notification.permission.get", &json!({}))
            .unwrap()["state"],
        "not_determined"
    );
    assert_eq!(
        h.conn
            .query_row("SELECT COUNT(*) FROM attention_acknowledgements", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn notification_no_ack_known_failures_can_be_retried_but_submitted_deliveries_cannot() {
    use kanban_app::notifications::NotificationOutcome;
    let h = harness();
    h.conn
        .execute(
            "INSERT INTO ticket_blockers(ticket_id,description) VALUES (2,'A decision remains')",
            [],
        )
        .unwrap();
    h.projector.refresh("2026-09-08T00:00:00Z").unwrap();
    h.core
        .command(
            "notification.settings.update",
            &json!({"mutation":common::mutation(0,"local-only"),
        "project_id":1,"local_enabled":true,"mirror_role":null}),
        )
        .unwrap();
    let sink = Arc::new(RecordingNotifications {
        path: h._dir.path().join("attention.sqlite"),
        messages: std::sync::Mutex::new(Vec::new()),
        outcomes: std::sync::Mutex::new(std::collections::VecDeque::from([
            NotificationOutcome::NotSent {
                reason: "permission is unavailable".to_owned(),
            },
            NotificationOutcome::Submitted {
                receipt: "retry accepted".to_owned(),
            },
        ])),
    });
    let dispatcher = kanban_app::notifications::NotificationDispatcher::new(
        Arc::new(kanban_storage::attention::SqliteAttentionStore::new(
            &h._database,
        )),
        Arc::new(kanban_storage::notifications::SqliteNotificationSettingsStore::new(&h._database)),
        Arc::new(kanban_storage::notifications::SqliteNotificationDeliveryStore::new(&h._database)),
        Arc::new(kanban_storage::SqliteProjectStore::new(&h._database)),
        sink,
    );
    assert_eq!(
        dispatcher
            .dispatch_once("2026-09-08T00:00:00Z")
            .unwrap()
            .failed,
        1
    );
    let listed = h
        .core
        .query("notification.deliveries", &json!({"project_id":1}))
        .expect("delivery failures are visible without resolving attention");
    let failed = &listed["deliveries"][0];
    assert_eq!(failed["status"], "failed");
    h.core.command("notification.retry",&json!({"mutation":common::mutation(failed["version"].as_u64().unwrap(),"retry-known-failure"),"delivery_id":failed["id"]})).unwrap();
    assert_eq!(
        dispatcher
            .dispatch_once("2026-09-08T00:01:00Z")
            .unwrap()
            .submitted,
        1
    );
    let listed = h
        .core
        .query("notification.deliveries", &json!({"project_id":1}))
        .unwrap();
    let sent = &listed["deliveries"][0];
    assert_eq!(sent["status"], "submitted");
    assert!(h.core.command("notification.retry",&json!({"mutation":common::mutation(sent["version"].as_u64().unwrap(),"no-repeat"),"delivery_id":sent["id"]})).is_err());
    assert_eq!(
        h.conn
            .query_row("SELECT COUNT(*) FROM attention_acknowledgements", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn attention_consolidation_includes_a_gate_requiring_revalidation() {
    let (mut h, ticket, submission) = common::review::prepared();
    let review = h
        .core
        .command(
            "review.start",
            &json!({"mutation":common::mutation(0,"gate-for-inbox"),
        "ticket_id":ticket,"submission_id":submission["id"]}),
        )
        .unwrap();
    h.core.command("review.expire",&json!({"mutation":common::mutation(review["version"].as_u64().unwrap(),"expire-for-inbox"),
        "review_id":review["id"]})).unwrap();
    let store = Arc::new(kanban_storage::attention::SqliteAttentionStore::new(
        &h.database,
    ));
    let projector = kanban_app::attention::AttentionProjector::new(
        Arc::new(kanban_storage::attention::SqliteAttentionSource::new(
            &h.database,
        )),
        store.clone(),
    );
    h.core.register_attention(store).unwrap();
    projector.refresh("2026-09-08T00:00:00Z").unwrap();
    let inbox = h.core.query("attention.list", &json!({})).unwrap();
    assert_eq!(
        inbox["items"].as_array().unwrap().len(),
        1,
        "gate revalidation requires an operator decision"
    );
    assert_eq!(inbox["items"][0]["kind"], "invalid_approval");
    assert_eq!(inbox["items"][0]["subject_id"], ticket.to_string());
}
