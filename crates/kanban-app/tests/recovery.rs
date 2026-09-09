//! Recovery is explicit and append-only; telemetry is never a verdict.
use kanban_app::TicketStore;
use serde_json::{Value, json};

#[path = "common/mod.rs"]
mod common;

fn running() -> (common::DispatchHarness, u64, u64) {
    let mut h = common::harness();
    h.core
        .register_run_recovery(
            std::sync::Arc::new(kanban_storage::SqliteRunRecoveryStore::new(&h.database)),
            std::sync::Arc::new(kanban_storage::SqliteProjectStore::new(&h.database)),
            h.wake.clone(),
        )
        .unwrap();
    let ticket = common::insert_ticket(&h.database_path, 1, "normal");
    common::assign_lane(&h.database_path, ticket);
    let requested = h
        .core
        .command(
            "dispatch.request",
            &json!({
                "mutation": common::mutation(0, "recovery-request"),
                "ticket_id": ticket,
            }),
        )
        .unwrap();
    let id = requested["id"].as_u64().unwrap();
    h.core
        .command(
            "dispatch.claim",
            &json!({
                "mutation": common::mutation(1, "recovery-claim"),
                "dispatch_request_id": id,
            }),
        )
        .unwrap();
    let run = h
        .core
        .command(
            "run.acknowledge",
            &json!({
                "mutation": common::mutation(2, "recovery-acknowledge"),
                "dispatch_request_id": id,
            }),
        )
        .unwrap();
    (h, run["id"].as_u64().unwrap(), ticket)
}

#[test]
fn recovery_idempotency_records_a_ruling_without_a_ticket_verdict() {
    let (h, run, ticket) = running();
    let tickets = kanban_storage::SqliteTicketStore::new(&h.database);
    let before = tickets
        .find(kanban_domain::TicketId::new(ticket))
        .unwrap()
        .unwrap()
        .state();
    let request = json!({
        "mutation": common::mutation(0, "recovery-ruling"),
        "run_id": run,
        "summary": "No result arrived. Hold this attempt for an operator decision.",
    });
    let record = h
        .core
        .command("run.recovery.rule", &request)
        .expect("an explicit operator ruling is recorded");
    assert_eq!(record["run_id"], run);
    assert_eq!(record["action"], "operator_ruling");
    assert!(record["ruling_id"].as_u64().is_some());
    assert_eq!(
        h.core.command("run.recovery.rule", &request).unwrap(),
        record
    );
    assert_eq!(
        tickets
            .find(kanban_domain::TicketId::new(ticket))
            .unwrap()
            .unwrap()
            .state(),
        before
    );
    let history = h
        .core
        .query("run.recovery.list", &json!({"run_id": run}))
        .unwrap();
    assert_eq!(history["version"], 1);
    assert_eq!(history["records"], Value::Array(vec![record.clone()]));
    let db = rusqlite::Connection::open(&h.database_path).unwrap();
    let ruling: String = db
        .query_row(
            "SELECT summary FROM rulings WHERE id = ?1",
            [record["ruling_id"].as_u64().unwrap() as i64],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        ruling,
        "No result arrived. Hold this attempt for an operator decision."
    );
    let attempts: i64 = db
        .query_row("SELECT count(*) FROM runs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(attempts, 1, "recovery preserves the original attempt");
}

#[test]
fn recovery_idempotency_retries_in_a_new_run_with_auditable_supersession() {
    let (h, old_run_id, ticket_id) = running();
    let before = h.core.query("run.list", &json!({"project_id":1})).unwrap()["runs"][0].clone();
    let payload = json!({"mutation":common::mutation(0,"retry-after-lost-agent"),
        "run_id":old_run_id,"summary":"Retry after the original agent disappeared without a result"});
    let retry = h
        .core
        .command("run.recovery.retry", &payload)
        .expect("the operator can explicitly retry without a verdict");
    let request_id = retry["replacement_dispatch_request_id"].as_u64().unwrap();
    assert_ne!(request_id, before["dispatch_request_id"].as_u64().unwrap());
    assert_eq!(
        h.core.command("run.recovery.retry", &payload).unwrap(),
        retry
    );
    let claim = h
        .core
        .command(
            "dispatch.claim",
            &json!({"mutation":common::mutation(1,"retry-claim"),
        "dispatch_request_id":request_id}),
        )
        .unwrap();
    assert_eq!(claim["claimed"], true);
    let new_run = h
        .core
        .command(
            "run.acknowledge",
            &json!({"mutation":common::mutation(2,"retry-start"),
        "dispatch_request_id":request_id}),
        )
        .unwrap();
    assert_ne!(new_run["id"].as_u64().unwrap(), old_run_id);
    let runs = h.core.query("run.list", &json!({"project_id":1})).unwrap();
    assert_eq!(runs["runs"].as_array().unwrap().len(), 2);
    assert_eq!(runs["runs"][0]["status"], "superseded");
    assert_eq!(runs["runs"][0]["requested"], before["requested"]);
    assert_eq!(runs["runs"][0]["effective"], before["effective"]);
    assert_eq!(new_run["ticket_id"], ticket_id);
    let db = rusqlite::Connection::open(&h.database_path).unwrap();
    let settled: Option<i64> = db
        .query_row(
            "SELECT settled_at FROM capabilities WHERE dispatch_request_id=?1",
            [before["dispatch_request_id"].as_u64().unwrap() as i64],
            |row| row.get(0),
        )
        .unwrap();
    assert!(settled.is_some(), "supersession revokes the old authority");
    let history = h
        .core
        .query("run.recovery.list", &json!({"run_id":old_run_id}))
        .unwrap();
    assert_eq!(
        history["records"][0]["replacement_dispatch_request_id"],
        request_id
    );
}

#[test]
fn recovery_retry_releases_the_original_capacity_slot() {
    let (h, run, _) = running();
    common::constrain_global(&h.database_path, "max_active_per_harness", 1);
    let recovered = h.core.command("run.recovery.retry", &json!({
        "mutation":common::mutation(0,"retry-at-capacity"),"run_id":run,"summary":"Replace the stalled attempt"
    })).unwrap();
    let claim = h
        .core
        .command(
            "dispatch.claim",
            &json!({
                "mutation":common::mutation(1,"claim-replacement-at-capacity"),
                "dispatch_request_id":recovered["replacement_dispatch_request_id"]
            }),
        )
        .unwrap();
    assert_eq!(
        claim["claimed"], true,
        "the old attempt no longer occupies the only available slot"
    );
}

#[test]
fn recovery_idempotency_requests_resume_without_minting_a_run() {
    let (h, run, _) = running();
    h.wake.calls.lock().unwrap().clear();
    let before = h.core.query("run.list", &json!({"project_id":1})).unwrap();
    let payload = json!({"mutation":common::mutation(0,"resume-original"),"run_id":run,
        "summary":"The connection returned; resume this original attempt"});
    let record = h
        .core
        .command("run.recovery.resume", &payload)
        .expect("resume is an explicit operator action");
    assert_eq!(record["action"], "resume");
    assert_eq!(
        h.core.command("run.recovery.resume", &payload).unwrap(),
        record
    );
    assert_eq!(
        h.core.query("run.list", &json!({"project_id":1})).unwrap(),
        before
    );
    let calls = h.wake.calls.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        1,
        "only the committed intent wakes the Coordinator"
    );
    assert_eq!(
        json!(calls[0].dispatch_request_id),
        before["runs"][0]["dispatch_request_id"]
    );
    let database = rusqlite::Connection::open(&h.database_path).unwrap();
    let active: i64 = database
        .query_row(
            "SELECT count(*) FROM capabilities WHERE status='active' AND settled_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        active, 1,
        "resuming reuses, rather than renews, the existing authority"
    );
}

fn eventually(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !ready() {
        assert!(
            std::time::Instant::now() < deadline,
            "recovery delivery timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn recovery_observer(
    h: &common::DispatchHarness,
) -> std::sync::Arc<kanban_service::herdr::HerdrObserver> {
    use kanban_app::ProjectStore;
    use kanban_service::herdr::{BackoffPolicy, HerdrObserver, ObservationTuning};
    use std::time::Duration;
    let observer = HerdrObserver::with_observation(
        std::sync::Arc::new(kanban_storage::Database::open(&h.database_path).unwrap()),
        h._dir.path().join("sessions"),
        ObservationTuning {
            backoff: BackoffPolicy::new(Duration::from_millis(20), Duration::from_millis(40)),
            settle: Duration::from_millis(20),
            io_timeout: Duration::from_millis(100),
        },
    );
    observer.observe_projects(&[kanban_storage::SqliteProjectStore::new(&h.database)
        .find(kanban_domain::ProjectId::new(1))
        .unwrap()
        .unwrap()]);
    observer
}

#[test]
fn recovery_resume_survives_disconnect_and_service_restart() {
    use kanban_herdr::{
        HerdrRequest,
        fixture::{ScriptedSession, SessionScript},
    };
    let (mut h, run, _) = running();
    let observer = recovery_observer(&h);
    h.core
        .register_run_recovery(
            std::sync::Arc::new(kanban_storage::SqliteRunRecoveryStore::new(&h.database)),
            std::sync::Arc::new(kanban_storage::SqliteProjectStore::new(&h.database)),
            observer.clone(),
        )
        .unwrap();
    let request = json!({"mutation":common::mutation(0,"durable-resume"),"run_id":run,"summary":"Resume after reconnect without replacing the attempt"});
    let recorded = h.core.command("run.recovery.resume", &request).unwrap();
    eventually(|| observer.consecutive_failures(1) >= 2);
    observer.shutdown();
    let fixture = ScriptedSession::bind(
        &h._dir.path().join("sessions"),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_prompt_accepted(true),
    );
    let restarted = recovery_observer(&h);
    eventually(|| {
        fixture
            .recorded_requests()
            .iter()
            .any(|r| matches!(r, HerdrRequest::Prompt { .. }))
    });
    assert_eq!(
        h.core.command("run.recovery.resume", &request).unwrap(),
        recorded
    );
    restarted.shutdown();
    let prompts: Vec<_> = fixture
        .recorded_requests()
        .into_iter()
        .filter(|r| matches!(r, HerdrRequest::Prompt { .. }))
        .collect();
    assert_eq!(prompts.len(), 1, "replay must not enqueue another resume");
    assert_eq!(
        h.core.query("run.list", &json!({"project_id":1})).unwrap()["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn recovery_idempotency_wakes_the_replacement_request_once() {
    // Queue wake hints are distinct from durable resume deliveries.
    let (h, run, _) = running();
    h.wake.calls.lock().unwrap().clear();
    let request = json!({"mutation":common::mutation(0,"retry-wake"),"run_id":run,"summary":"Queue a fresh attempt"});
    let result = h.core.command("run.recovery.retry", &request).unwrap();
    assert_eq!(
        h.core.command("run.recovery.retry", &request).unwrap(),
        result
    );
    let calls = h.wake.calls.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        1,
        "retry wakes the replacement only after its intent commits"
    );
    assert_eq!(
        json!(calls[0].dispatch_request_id),
        result["replacement_dispatch_request_id"]
    );
    assert_eq!(calls[0].resume_run_id, None);
}

#[test]
fn recovery_resume_rejection_stays_pending_until_accepted() {
    use kanban_herdr::{
        HerdrRequest,
        fixture::{ScriptedSession, SessionScript},
    };
    let (h, run, _) = running();
    h.core.command("run.recovery.resume", &json!({"mutation":common::mutation(0,"rejected-resume"),"run_id":run,"summary":"Keep the same intent until delivery is accepted"})).unwrap();
    let fixture = ScriptedSession::bind(
        &h._dir.path().join("sessions"),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_prompt_accepted(false),
    );
    let observer = recovery_observer(&h);
    eventually(|| {
        fixture
            .recorded_requests()
            .iter()
            .any(|r| matches!(r, HerdrRequest::Prompt { .. }))
    });
    observer.shutdown();
    assert_eq!(
        h.core
            .query("run.recovery.list", &json!({"run_id":run}))
            .unwrap()["pending_resume"],
        true
    );
    drop(fixture);
    let fixture = ScriptedSession::bind(
        &h._dir.path().join("sessions"),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_prompt_accepted(true),
    );
    let observer = recovery_observer(&h);
    eventually(|| {
        h.core
            .query("run.recovery.list", &json!({"run_id":run}))
            .unwrap()["pending_resume"]
            == false
    });
    observer.shutdown();
    let accepted = fixture
        .recorded_requests()
        .iter()
        .filter(|r| matches!(r, HerdrRequest::Prompt { .. }))
        .count();
    assert_eq!(accepted, 1);
    let observer = recovery_observer(&h);
    eventually(|| {
        fixture
            .recorded_requests()
            .iter()
            .filter(|r| matches!(r, HerdrRequest::Subscribe))
            .count()
            >= 2
    });
    observer.shutdown();
    assert_eq!(
        fixture
            .recorded_requests()
            .iter()
            .filter(|r| matches!(r, HerdrRequest::Prompt { .. }))
            .count(),
        accepted,
        "accepted delivery survives restart without another prompt"
    );
}

#[test]
fn recovery_resume_cancels_obsolete_intents_before_delivery() {
    use kanban_herdr::{
        HerdrRequest,
        fixture::{ScriptedSession, SessionScript},
    };
    for action in ["retry", "submission", "review_expiry"] {
        let (h, run, review) = if action == "review_expiry" {
            let (h, review, run) = reviewing();
            (h, run["id"].as_u64().unwrap(), Some(review))
        } else {
            let (h, run, _) = running();
            (h, run, None)
        };
        let receipt = h.core.command("run.recovery.resume", &json!({"mutation":common::mutation(0,"obsolete-resume"),"run_id":run,"summary":"Resume only if custody is still valid"})).unwrap();
        match action {
            "retry" => {
                h.core.command("run.recovery.retry",&json!({"mutation":common::mutation(1,"supersede-pending-resume"),"run_id":run,"summary":"Use a new attempt instead"})).unwrap();
            }
            "submission" => {
                let db = rusqlite::Connection::open(&h.database_path).unwrap();
                let cap:i64=db.query_row("SELECT c.id FROM capabilities c JOIN runs r ON r.dispatch_request_id=c.dispatch_request_id WHERE r.id=?1",[run as i64],|r|r.get(0)).unwrap();
                h.core.command("submission.submit",&json!({"mutation":common::mutation(1,"result-before-resume-delivery"),"run_id":run,"capability_id":cap,"result":{"kind":"implementation","tip":"a".repeat(40),"summary":"The delayed result arrived"}})).unwrap();
            }
            _ => expire_review(&h, review.as_ref().unwrap()),
        }
        let fixture = ScriptedSession::bind(
            &h._dir.path().join("sessions"),
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_prompt_accepted(true),
        );
        let observer = recovery_observer(&h);
        let db = rusqlite::Connection::open(&h.database_path).unwrap();
        eventually(|| {
            db.query_row(
                "SELECT status FROM run_resume_deliveries WHERE recovery_id=?1",
                [receipt["id"].as_u64().unwrap() as i64],
                |r| r.get::<_, String>(0),
            )
            .unwrap()
                == "obsolete"
        });
        observer.shutdown();
        assert!(
            !fixture
                .recorded_requests()
                .iter()
                .any(|r| matches!(r, HerdrRequest::Prompt { .. })),
            "{action} must prevent stale execution"
        );
    }
}

#[test]
fn recovery_resume_delivery_failure_rolls_back_the_intent_and_ruling() {
    let (h, run, _) = running();
    let db = rusqlite::Connection::open(&h.database_path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_resume BEFORE INSERT ON run_resume_deliveries BEGIN SELECT RAISE(ABORT,'delivery write failed'); END;").unwrap();
    let request = json!({"mutation":common::mutation(0,"atomic-resume"),"run_id":run,"summary":"Delivery and audit commit together"});
    assert!(h.core.command("run.recovery.resume", &request).is_err());
    for table in ["run_recoveries", "rulings", "run_resume_deliveries"] {
        assert_eq!(
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    db.execute_batch("DROP TRIGGER fail_resume").unwrap();
    let recorded = h.core.command("run.recovery.resume", &request).unwrap();
    assert_eq!(
        h.core.command("run.recovery.resume", &request).unwrap(),
        recorded
    );
}

#[test]
fn recovery_no_verdict_rule_preserves_workflow_for_real_observation_failures() {
    use kanban_app::deadlines::{MISSING_RESULT_DEADLINE_REASON, STALL_DEADLINE_REASON};
    use kanban_herdr::fixture::{ScriptedSession, SessionScript};
    for event in [
        "exit_zero",
        "exit_failure",
        "disconnect",
        "stall",
        "missing_result",
    ] {
        let (h, run, ticket) = running();
        let tickets = kanban_storage::SqliteTicketStore::new(&h.database);
        let before = tickets
            .find(kanban_domain::TicketId::new(ticket))
            .unwrap()
            .unwrap()
            .state();
        let runs = h.core.query("run.list", &json!({"project_id":1})).unwrap();
        let db = rusqlite::Connection::open(&h.database_path).unwrap();
        db.execute("INSERT INTO herdr_project_settings(project_id,reconciliation_interval_secs,polling_fallback_enabled,polling_fallback_interval_secs,stall_deadline_secs,missing_result_deadline_secs,version) VALUES(1,300,0,10,1,1,1) ON CONFLICT(project_id) DO UPDATE SET stall_deadline_secs=1,missing_result_deadline_secs=1",[]).unwrap();
        let kind = match event {
            "stall" => "role.output",
            "missing_result" => "role.settled",
            _ => "role.exited",
        };
        let fixture = (event!="disconnect").then(|| ScriptedSession::bind(&h._dir.path().join("sessions"),"kanban-main","/workspaces/kanban.seed",SessionScript::default().with_events(vec![json!({"kind":kind,"role":"implementer","run":run,"exit_code":if event=="exit_zero" {0} else {1}})])));
        let observer = recovery_observer(&h);
        match event {
            "disconnect" => eventually(|| observer.consecutive_failures(1) >= 2),
            "stall" | "missing_result" => {
                let reason = if event == "stall" {
                    STALL_DEADLINE_REASON
                } else {
                    MISSING_RESULT_DEADLINE_REASON
                };
                eventually(|| {
                    observer
                        .attention_signals(1)
                        .iter()
                        .any(|s| s.reason == reason)
                });
            }
            _ => eventually(|| {
                db.query_row("SELECT count(*) FROM timeline_events WHERE json_extract(detail,'$.event')='role.exited'",[],|r|r.get::<_,i64>(0)).unwrap()>0
            }),
        }
        observer.shutdown();
        drop(fixture);
        assert_eq!(
            tickets
                .find(kanban_domain::TicketId::new(ticket))
                .unwrap()
                .unwrap()
                .state(),
            before,
            "{event} is not a workflow verdict"
        );
        assert_eq!(
            h.core.query("run.list", &json!({"project_id":1})).unwrap(),
            runs,
            "{event} preserves the attempt until explicit recovery or submission"
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM submissions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn recovery_choices_follow_authority_and_supersession_not_a_verdict() {
    let (h, run, _) = running();
    let state = h
        .core
        .query("run.recovery.list", &json!({"run_id":run}))
        .unwrap();
    assert_eq!(state["can_resume"], true);
    assert_eq!(state["can_retry"], true);
    let database = rusqlite::Connection::open(&h.database_path).unwrap();
    database
        .execute(
            "UPDATE capabilities SET status='settled',settled_at=unixepoch()",
            [],
        )
        .unwrap();
    let state = h
        .core
        .query("run.recovery.list", &json!({"run_id":run}))
        .unwrap();
    assert_eq!(state["can_resume"], false);
    assert_eq!(state["can_retry"], true);
    assert!(h.core.command("run.recovery.resume",&json!({"mutation":common::mutation(0,"cannot-renew"),"run_id":run,"summary":"Must not renew expired authority"})).is_err());
    h.core.command("run.recovery.retry",&json!({"mutation":common::mutation(0,"replace-expired"),"run_id":run,"summary":"Explicitly request new authority"})).unwrap();
    let state = h
        .core
        .query("run.recovery.list", &json!({"run_id":run}))
        .unwrap();
    assert_eq!(state["can_resume"], false);
    assert_eq!(state["can_retry"], false);
}

#[test]
fn recovery_idempotency_collapses_concurrent_retry_storms_for_every_action() {
    for action in ["rule", "resume", "retry"] {
        let (h, run, _) = running();
        h.wake.calls.lock().unwrap().clear();
        let core = std::sync::Arc::new(h.core);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(12));
        let workers:Vec<_>=(0..12).map(|_|{
            let core=core.clone();let barrier=barrier.clone();
            std::thread::spawn(move ||{
                barrier.wait();
                core.command(&format!("run.recovery.{action}"),&json!({"mutation":common::mutation(0,"same-operator-intent"),"run_id":run,"summary":"One deliberate recovery action"})).unwrap()
            })
        }).collect();
        let records: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert!(records.iter().all(|record| record == &records[0]));
        let state = core
            .query("run.recovery.list", &json!({"run_id":run}))
            .unwrap();
        assert_eq!(state["records"].as_array().unwrap().len(), 1);
        assert_eq!(
            h.wake.calls.lock().unwrap().len(),
            usize::from(action != "rule")
        );
        let database = rusqlite::Connection::open(&h.database_path).unwrap();
        let count: i64 = database
            .query_row("SELECT count(*) FROM rulings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}

#[test]
fn recovery_retry_rollback_keeps_authority_capacity_and_audit_together() {
    let (h, run, _) = running();
    h.wake.calls.lock().unwrap().clear();
    let database = rusqlite::Connection::open(&h.database_path).unwrap();
    database.execute_batch("CREATE TRIGGER fail_recovery BEFORE INSERT ON run_recoveries BEGIN SELECT RAISE(ABORT,'injected recovery failure'); END;").unwrap();
    let request = json!({"mutation":common::mutation(0,"failed-then-retried"),"run_id":run,"summary":"Retry must commit all custody or nothing"});
    assert!(h.core.command("run.recovery.retry", &request).is_err());
    assert!(h.wake.calls.lock().unwrap().is_empty());
    for (query, expected) in [
        ("SELECT count(*) FROM dispatch_requests", 1),
        (
            "SELECT count(*) FROM dispatch_requests WHERE completed_at IS NOT NULL",
            0,
        ),
        (
            "SELECT count(*) FROM capabilities WHERE status='active' AND settled_at IS NULL",
            1,
        ),
        ("SELECT count(*) FROM run_recoveries", 0),
        ("SELECT count(*) FROM rulings", 0),
    ] {
        let count: i64 = database.query_row(query, [], |row| row.get(0)).unwrap();
        assert_eq!(count, expected, "{query}");
    }
    database
        .execute_batch("DROP TRIGGER fail_recovery")
        .unwrap();
    assert!(h.core.command("run.recovery.retry", &request).is_ok());
    assert_eq!(h.wake.calls.lock().unwrap().len(), 1);
}

#[test]
fn recovery_does_not_replace_an_authoritative_submission() {
    let (mut h, _, _) = common::review::prepared();
    h.core
        .register_run_recovery(
            std::sync::Arc::new(kanban_storage::SqliteRunRecoveryStore::new(&h.database)),
            std::sync::Arc::new(kanban_storage::SqliteProjectStore::new(&h.database)),
            h.wake.clone(),
        )
        .unwrap();
    let runs = h.core.query("run.list", &json!({"project_id":1})).unwrap();
    let run = runs["runs"][0]["id"].as_u64().unwrap();
    let results = h
        .core
        .query("submission.list", &json!({"project_id":1}))
        .unwrap();
    for action in ["resume", "retry"] {
        assert!(h.core.command(&format!("run.recovery.{action}"),&json!({"mutation":common::mutation(0,format!("cannot-{action}")),"run_id":run,"summary":"A missing process cannot erase its recorded result"})).is_err());
    }
    assert_eq!(
        h.core
            .query("submission.list", &json!({"project_id":1}))
            .unwrap(),
        results
    );
    assert_eq!(
        h.core.query("run.list", &json!({"project_id":1})).unwrap(),
        runs
    );
}

#[test]
fn recovery_retry_preserves_the_frozen_reviewer_slot_and_refuses_the_old_result() {
    let (mut h, ticket, submission) = common::review::prepared();
    h.core
        .register_run_recovery(
            std::sync::Arc::new(kanban_storage::SqliteRunRecoveryStore::new(&h.database)),
            std::sync::Arc::new(kanban_storage::SqliteProjectStore::new(&h.database)),
            h.wake.clone(),
        )
        .unwrap();
    let review=h.core.command("review.start",&json!({"mutation":common::mutation(0,"review-for-recovery"),"ticket_id":ticket,"submission_id":submission["id"]})).unwrap();
    let slot = review["stages"][0]["slots"][0].clone();
    let claim=h.core.command("dispatch.claim",&json!({"mutation":common::mutation(1,"old-review-claim"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
    let original=h.core.command("run.acknowledge",&json!({"mutation":common::mutation(2,"old-review-run"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
    let recovery=h.core.command("run.recovery.retry",&json!({"mutation":common::mutation(0,"retry-reviewer"),"run_id":original["id"],"summary":"The reviewer disappeared without a result"})).unwrap();
    let next = recovery["replacement_dispatch_request_id"].clone();
    let new_claim = h
        .core
        .command(
            "dispatch.claim",
            &json!({"mutation":common::mutation(1,"new-review-claim"),"dispatch_request_id":next}),
        )
        .unwrap();
    assert_eq!(new_claim["capability"]["reviewer_slot_id"], slot["id"]);
    let replacement = h
        .core
        .command(
            "run.acknowledge",
            &json!({"mutation":common::mutation(2,"new-review-run"),"dispatch_request_id":next}),
        )
        .unwrap();
    assert_eq!(replacement["requested"], original["requested"]);
    assert_eq!(replacement["effective"], original["effective"]);
    let late = json!({"mutation":common::mutation(1,"late-old-reviewer"),"run_id":original["id"],"capability_id":claim["capability"]["id"],"result":{"kind":"review","tip":"a".repeat(40),"summary":"Too late","approve":true}});
    assert!(h.core.command("submission.submit", &late).is_err());
    let before = h
        .core
        .query("review.get", &json!({"review_id":review["id"]}))
        .unwrap();
    assert!(before["stages"][0]["slots"][0]["verdict"].is_null());
    assert_eq!(before["stages"][0]["slots"][0]["dispatch_request_id"], next);
    h.core.command("submission.submit",&json!({"mutation":common::mutation(1,"replacement-review-result"),"run_id":replacement["id"],"capability_id":new_claim["capability"]["id"],"result":{"kind":"review","tip":"a".repeat(40),"summary":"The replacement reviewed the frozen tip","approve":true}})).unwrap();
    let after = h
        .core
        .query("review.get", &json!({"review_id":review["id"]}))
        .unwrap();
    assert_eq!(
        after["status"], "in_progress",
        "the required human slot still controls completion"
    );
    assert!(!after["stages"][0]["slots"][0]["verdict"].is_null());
}

fn reviewing() -> (common::DispatchHarness, Value, Value) {
    let (mut h, ticket, submission) = common::review::prepared();
    h.core
        .register_run_recovery(
            std::sync::Arc::new(kanban_storage::SqliteRunRecoveryStore::new(&h.database)),
            std::sync::Arc::new(kanban_storage::SqliteProjectStore::new(&h.database)),
            h.wake.clone(),
        )
        .unwrap();
    let review = h.core.command("review.start", &json!({
        "mutation":common::mutation(0,"expiry-review"),"ticket_id":ticket,"submission_id":submission["id"]
    })).unwrap();
    let request = &review["stages"][0]["slots"][0]["dispatch_request_id"];
    h.core
        .command(
            "dispatch.claim",
            &json!({"mutation":common::mutation(1,"expiry-claim"),"dispatch_request_id":request}),
        )
        .unwrap();
    let run = h
        .core
        .command(
            "run.acknowledge",
            &json!({"mutation":common::mutation(2,"expiry-run"),"dispatch_request_id":request}),
        )
        .unwrap();
    (h, review, run)
}

fn expire_review(h: &common::DispatchHarness, review: &Value) {
    h.core.command("review.expire", &json!({"mutation":common::mutation(review["version"].as_u64().unwrap(),"expire-recovery-review"),"review_id":review["id"]})).unwrap();
}

#[test]
fn recovery_refuses_expired_required_review_slots() {
    let (h, review, run) = reviewing();
    expire_review(&h, &review);
    let before = h.core.query("run.list", &json!({"project_id":1})).unwrap();
    let choices = h
        .core
        .query("run.recovery.list", &json!({"run_id":run["id"]}))
        .unwrap();
    assert_eq!(choices["can_resume"], false);
    assert_eq!(choices["can_retry"], false);
    for action in ["resume", "retry"] {
        assert!(h.core.command(&format!("run.recovery.{action}"), &json!({"mutation":common::mutation(0,format!("expired-{action}")),"run_id":run["id"],"summary":"Do not reopen an expired review"})).is_err());
    }
    assert_eq!(
        h.core.query("run.list", &json!({"project_id":1})).unwrap(),
        before
    );
    assert_eq!(
        h.core
            .query("run.recovery.list", &json!({"run_id":run["id"]}))
            .unwrap()["version"],
        0
    );
}

#[test]
fn recovery_rechecks_review_expiry_before_replacement_claim_and_mint() {
    for expire_after_claim in [false, true] {
        let (h, review, run) = reviewing();
        let retry = h.core.command("run.recovery.retry", &json!({"mutation":common::mutation(0,"before-review-expires"),"run_id":run["id"],"summary":"Replace a stalled reviewer"})).unwrap();
        let request = &retry["replacement_dispatch_request_id"];
        if expire_after_claim {
            h.core.command("dispatch.claim", &json!({"mutation":common::mutation(1,"replacement-claim"),"dispatch_request_id":request})).unwrap();
        }
        expire_review(&h, &review);
        let (operation, version) = if expire_after_claim {
            ("run.acknowledge", 2)
        } else {
            ("dispatch.claim", 1)
        };
        assert!(h.core.command(operation, &json!({"mutation":common::mutation(version,"no-expired-replacement"),"dispatch_request_id":request})).is_err(), "{operation} must recheck review custody");
    }
}

#[test]
fn recovery_is_not_needed_to_release_a_run_with_an_authoritative_result() {
    let (h, _, _) = common::review::prepared();
    let database = rusqlite::Connection::open(&h.database_path).unwrap();
    let active: i64 = database
        .query_row(
            "SELECT count(*) FROM capabilities WHERE status='active' AND settled_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        active, 0,
        "an authoritative result ends authority; an observed exit alone does not"
    );
    let completed = h.core.query("run.list", &json!({"project_id":1})).unwrap();
    assert_eq!(
        completed["runs"][0]["status"], "submitted",
        "a result is not displayed as a still-executing attempt"
    );
    common::constrain_global(&h.database_path, "max_active_per_harness", 1);
    let ticket = common::insert_ticket(&h.database_path, 2, "normal");
    common::assign_lane(&h.database_path, ticket);
    let queued = h
        .core
        .command(
            "dispatch.request",
            &json!({"mutation":common::mutation(0,"next-after-result"),"ticket_id":ticket}),
        )
        .unwrap();
    let claimed=h.core.command("dispatch.claim",&json!({"mutation":common::mutation(1,"claim-after-result"),"dispatch_request_id":queued["id"]})).unwrap();
    assert_eq!(
        claimed["claimed"], true,
        "a completed result cannot permanently consume capacity"
    );
}

#[test]
fn recovery_upgrade_retires_legacy_submitted_authority_without_rewriting_results() {
    let (h, _, _) = common::review::prepared();
    let before = h
        .core
        .query("submission.list", &json!({"project_id":1}))
        .unwrap();
    let connection = rusqlite::Connection::open(&h.database_path).unwrap();
    connection.execute_batch("UPDATE capabilities SET status='active',settled_at=NULL; UPDATE dispatch_requests SET completed_at=NULL; DROP TABLE run_resume_deliveries; DELETE FROM schema_migrations WHERE version>=47;").unwrap();
    let mut reopened = kanban_storage::Database::open(&h.database_path).unwrap();
    reopened
        .migrate(&kanban_storage::AllowAllMigrations)
        .unwrap();
    let active: i64 = connection
        .query_row(
            "SELECT count(*) FROM capabilities WHERE status='active'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        active, 0,
        "a legacy immutable submission retires its stale grant on upgrade"
    );
    let open: i64 = connection
        .query_row(
            "SELECT count(*) FROM dispatch_requests WHERE completed_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(open, 0);
    assert_eq!(
        h.core
            .query("submission.list", &json!({"project_id":1}))
            .unwrap(),
        before
    );
    let faults: i64 = connection
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(faults, 0);
}

#[test]
fn recovery_idempotency_publishes_its_ruling_to_live_clients_once() {
    #[derive(Default)]
    struct Events(std::sync::Mutex<Vec<(String, Value)>>);
    impl kanban_app::events::EventSink for Events {
        fn emit(&self, name: &str, payload: Value) {
            self.0.lock().unwrap().push((name.to_owned(), payload));
        }
    }
    for action in ["rule", "retry", "resume"] {
        let (h, run, _) = running();
        let events = std::sync::Arc::new(Events::default());
        let mut core = kanban_app::Core::new(
            kanban_app::catalog::exposed_operations(),
            std::sync::Arc::new(kanban_storage::SqliteIdempotencyStore::new(
                &h.database,
                kanban_storage::RetentionPolicy::keep_most_recent(
                    std::num::NonZeroU32::new(100).unwrap(),
                ),
            )),
            events.clone(),
        );
        core.register_run_recovery(
            std::sync::Arc::new(kanban_storage::SqliteRunRecoveryStore::new(&h.database)),
            std::sync::Arc::new(kanban_storage::SqliteProjectStore::new(&h.database)),
            h.wake.clone(),
        )
        .unwrap();
        let request = json!({"mutation":common::mutation(0,"notify-recovery"),"run_id":run,"summary":"One visible decision"});
        let result = core
            .command(&format!("run.recovery.{action}"), &request)
            .unwrap();
        assert_eq!(
            core.command(&format!("run.recovery.{action}"), &request)
                .unwrap(),
            result
        );
        let records = events.0.lock().unwrap();
        let rulings: Vec<_> = records
            .iter()
            .filter(|(name, _)| name == kanban_dto::LiveEventName::RulingRecorded.as_str())
            .collect();
        assert_eq!(rulings.len(), 1);
        assert_eq!(rulings[0].1["id"], result["ruling_id"]);
    }
}

#[test]
fn recovery_ruling_reaches_the_running_service() {
    use std::io::{BufRead, BufReader, Write};
    let (h, run, _) = running();
    let service = kanban_service::serve_with_runtime(
        h._dir.path(),
        kanban_service::ServiceRuntime {
            // This operator-socket test never launches the MCP adapter.
            mcp_executable: std::env::current_exe().unwrap(),
            herdr_socket_root: h._dir.path().join("isolated-herdr"),
            installation_secret: None,
        },
    )
    .unwrap();
    let mut channel = std::os::unix::net::UnixStream::connect(service.socket_path()).unwrap();
    channel
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    writeln!(
        channel,
        "{}",
        json!({
            "kind":"command", "operation":"run.recovery.rule", "payload":{
                "mutation":common::mutation(0,"service-ruling"),"run_id":run,
                "summary":"Hold the attempt; do not infer a result.",
            }
        })
    )
    .unwrap();
    let mut line = String::new();
    BufReader::new(channel).read_line(&mut line).unwrap();
    service.shutdown();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["kind"], "response", "{response:?}");
    assert_eq!(response["payload"]["run_id"], run);
}
