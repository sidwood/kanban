//! Recovery resume eligibility (KAN-T142, KAN-S13-US1): a terminal
//! Ticket or an archived Project refuses a resume intent and is never
//! advertised as an available choice, an invalidation landing between
//! acceptance and delivery stops the delivery without erasing the
//! operator's own change, and eligible recovery stays idempotent and
//! auditable while granting no lifecycle override of its own. Every
//! scenario drives the production commands, the production recovery
//! store, and the production resume-delivery consumer over a
//! disposable SQLite database and a disposable Herdr socket root.

use std::sync::Arc;
use std::time::Duration;

use kanban_app::herdr::NoopHerdrProjectObserver;
use kanban_app::run_recovery::{PendingRunResume, ResumeDelivery, ResumeDispatch};
use kanban_app::{Core, ProjectStore, RunRecoveryStore};
use kanban_domain::{HerdrSession, ProjectId, TicketId};
use kanban_dto::{ApiError, ErrorCode};
use kanban_herdr::fixture::{ScriptedSession, SessionScript};
use kanban_herdr::{COORDINATOR_ROLE, HerdrRequest, PromptRequest, SessionClient, SessionMapping};
use kanban_service::LocalRepositories;
use kanban_service::herdr::{BackoffPolicy, HerdrObserver, ObservationTuning};
use kanban_storage::{
    Database, SqliteDependencyStore, SqliteHerdrSettingsStore, SqliteInitiativeStore,
    SqliteProjectStore, SqliteRunRecoveryStore, SqliteScheduleStore, SqliteTicketStore,
};
use serde_json::{Value, json};

mod common;

use common::{DispatchHarness, harness, mutation};

/// An accepted implementer attempt whose Run exists and whose
/// authority is still active: recovery's eligible starting point.
/// The lifecycle and Project commands are registered beside recovery
/// so every invalidation in these tests is the operator's own
/// production command, not a hand-written row.
fn attempting() -> (DispatchHarness, u64, u64) {
    let mut h = harness();
    register_recovery(&mut h.core, &h.database, h.wake.clone());
    register_operator_commands(&mut h.core, &h.database);
    let ticket = common::insert_ready_ticket(&h.database_path, 1, "normal");
    common::assign_lane(&h.database_path, ticket);
    let request = h
        .core
        .command(
            "dispatch.request",
            &json!({
                "mutation": mutation(0, "eligibility-request"),
                "ticket_id": ticket,
            }),
        )
        .expect("the request is created")["id"]
        .as_u64()
        .expect("the request carries an identity");
    h.core
        .command(
            "dispatch.claim",
            &json!({
                "mutation": mutation(1, "eligibility-claim"),
                "dispatch_request_id": request,
            }),
        )
        .expect("the claim lands");
    let run = h
        .core
        .command(
            "run.acknowledge",
            &json!({
                "mutation": mutation(2, "eligibility-acknowledge"),
                "dispatch_request_id": request,
            }),
        )
        .expect("the run starts")["id"]
        .as_u64()
        .expect("the run carries an identity");
    (h, run, ticket)
}

fn register_recovery(
    core: &mut Core,
    database: &Database,
    wake: Arc<dyn kanban_app::CoordinatorWake>,
) {
    core.register_run_recovery(
        Arc::new(SqliteRunRecoveryStore::new(database)),
        Arc::new(SqliteProjectStore::new(database)),
        wake,
    )
    .expect("the recovery operations register");
}

/// The lifecycle and Project commands the operator invalidates work
/// through. Observation is the noop port here: these tests start the
/// production observer themselves, so archiving must be refused by
/// recovery rather than by a stopped observation thread.
fn register_operator_commands(core: &mut Core, database: &Database) {
    let projects = Arc::new(SqliteProjectStore::new(database));
    let tickets = Arc::new(SqliteTicketStore::new(database));
    core.register_lifecycle(
        tickets,
        Arc::new(SqliteDependencyStore::new(database)),
        projects.clone(),
        Arc::new(SqliteScheduleStore::new(database)),
        None,
    )
    .expect("the lifecycle operations register");
    core.register_projects(
        projects,
        Arc::new(LocalRepositories),
        Arc::new(SqliteInitiativeStore::new(database)),
        Arc::new(SqliteHerdrSettingsStore::new(database)),
        Arc::new(NoopHerdrProjectObserver),
    )
    .expect("the Project operations register");
}

fn ticket_state(h: &DispatchHarness, ticket: u64) -> String {
    use kanban_app::TicketStore;
    SqliteTicketStore::new(&h.database)
        .find(TicketId::new(ticket))
        .expect("the Ticket reads")
        .expect("the fixture Ticket exists")
        .state()
        .wire_name()
        .to_owned()
}

fn ticket_version(h: &DispatchHarness, ticket: u64) -> u64 {
    use kanban_app::TicketStore;
    SqliteTicketStore::new(&h.database)
        .find(TicketId::new(ticket))
        .expect("the Ticket reads")
        .expect("the fixture Ticket exists")
        .version()
}

fn project_version(h: &DispatchHarness) -> u64 {
    SqliteProjectStore::new(&h.database)
        .find(ProjectId::new(1))
        .expect("the Project reads")
        .expect("the fixture Project exists")
        .version()
}

/// Invalidate the attempt the way the operator does: `cancelled`
/// cancels the Ticket, `archived` archives the owning Project. Both
/// are terminal states the rest of the product already refuses every
/// further change from.
fn invalidate(h: &DispatchHarness, ticket: u64, invalidation: &str, key: &str) {
    match invalidation {
        "cancelled" => {
            h.core
                .command(
                    "ticket.cancel",
                    &json!({
                        "mutation": mutation(ticket_version(h, ticket), format!("{key}-cancel")),
                        "ticket_id": ticket,
                    }),
                )
                .expect("the operator cancels the Ticket");
        }
        "archived" => {
            h.core
                .command(
                    "project.archive",
                    &json!({
                        "mutation": mutation(project_version(h), format!("{key}-archive")),
                        "project_id": 1,
                    }),
                )
                .expect("the operator archives the Project");
        }
        other => panic!("unknown invalidation {other}"),
    }
}

fn choices(h: &DispatchHarness, run: u64) -> Value {
    h.core
        .query("run.recovery.list", &json!({ "run_id": run }))
        .expect("the recovery choices read")
}

fn resume(h: &DispatchHarness, run: u64, key: &str, summary: &str) -> Result<Value, ApiError> {
    h.core.command(
        "run.recovery.resume",
        &json!({
            "mutation": mutation(0, key),
            "run_id": run,
            "summary": summary,
        }),
    )
}

fn count(h: &DispatchHarness, table: &str) -> i64 {
    rusqlite::Connection::open(&h.database_path)
        .expect("the database reopens")
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("the count reads")
}

/// The rows an accepted recovery writes, so a refusal can be proven
/// to have written none of them and an invalidation to have erased
/// none of them.
fn recovery_rows(h: &DispatchHarness) -> (i64, i64, i64) {
    (
        count(h, "run_recoveries"),
        count(h, "rulings"),
        count(h, "run_resume_deliveries"),
    )
}

fn delivery_status(h: &DispatchHarness, recovery_id: u64) -> String {
    rusqlite::Connection::open(&h.database_path)
        .expect("the database reopens")
        .query_row(
            "SELECT status FROM run_resume_deliveries WHERE recovery_id = ?1",
            [recovery_id as i64],
            |row| row.get(0),
        )
        .expect("the delivery row reads")
}

fn override_rows(h: &DispatchHarness) -> i64 {
    rusqlite::Connection::open(&h.database_path)
        .expect("the database reopens")
        .query_row(
            "SELECT count(*) FROM timeline_events
             WHERE json_extract(detail, '$.action') = 'emergency_override'",
            [],
            |row| row.get(0),
        )
        .expect("the override audit reads")
}

fn eventually(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(
            std::time::Instant::now() < deadline,
            "the recovery observer did not reach the expected state"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The production resume-delivery consumer: the Herdr observation
/// worker that reads pending resume intents and prompts the
/// Coordinator. Nothing native is launched — the session is the
/// scripted fixture bound into the scratch socket root.
fn recovery_observer(h: &DispatchHarness) -> Arc<HerdrObserver> {
    let observer = HerdrObserver::with_observation(
        Arc::new(Database::open(&h.database_path).expect("the database reopens")),
        h._dir.path().join("sessions"),
        observation_within(Duration::from_millis(100)),
    );
    observer.observe_projects(&[SqliteProjectStore::new(&h.database)
        .find(ProjectId::new(1))
        .expect("the Project reads")
        .expect("the fixture Project exists")]);
    observer
}

/// The tuning these tests drive the production consumer with:
/// backoff and settle short enough to keep a test brisk, and
/// `io_timeout` as the deadline one whole Herdr round trip has to
/// finish inside.
fn observation_within(io_timeout: Duration) -> ObservationTuning {
    ObservationTuning {
        backoff: BackoffPolicy::new(Duration::from_millis(20), Duration::from_millis(40)),
        settle: Duration::from_millis(20),
        io_timeout,
    }
}

fn accepting_session(h: &DispatchHarness) -> ScriptedSession {
    ScriptedSession::bind(
        &h._dir.path().join("sessions"),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default().with_prompt_accepted(true),
    )
}

fn prompts(fixture: &ScriptedSession) -> usize {
    fixture
        .recorded_requests()
        .iter()
        .filter(|request| matches!(request, HerdrRequest::Prompt { .. }))
        .count()
}

/// KAN-T142-AC1.
#[test]
fn a_terminal_ticket_or_archived_project_refuses_a_new_resume_intent() {
    for invalidation in ["cancelled", "archived"] {
        let (h, run, ticket) = attempting();
        assert_eq!(
            choices(&h, run)["can_resume"],
            true,
            "{invalidation}: the eligible attempt starts resumable"
        );
        invalidate(&h, ticket, invalidation, invalidation);
        let runs_before = h
            .core
            .query("run.list", &json!({ "project_id": 1 }))
            .expect("the runs read");
        let rows_before = recovery_rows(&h);
        h.wake.calls.lock().expect("the wake log is sound").clear();

        let refusal = resume(
            &h,
            run,
            &format!("{invalidation}-resume"),
            "Resume this original attempt now that the connection returned",
        )
        .expect_err("a terminal Ticket or archived Project refuses the resume intent");

        assert_eq!(refusal.code, ErrorCode::InvalidRequest, "{refusal:?}");
        assert!(
            refusal.message.contains("terminal"),
            "{invalidation}: the refusal names the terminal state: {refusal:?}"
        );
        assert_eq!(
            choices(&h, run)["can_resume"],
            false,
            "{invalidation}: a refused action is never advertised"
        );
        assert_eq!(
            choices(&h, run)["pending_resume"],
            false,
            "{invalidation}: no intent is pending"
        );
        assert_eq!(
            recovery_rows(&h),
            rows_before,
            "{invalidation}: the refusal writes no recovery, Ruling or delivery row"
        );
        assert_eq!(
            h.core
                .query("run.list", &json!({ "project_id": 1 }))
                .expect("the runs read"),
            runs_before,
            "{invalidation}: the refusal leaves the Run untouched"
        );
        assert!(
            h.wake
                .calls
                .lock()
                .expect("the wake log is sound")
                .is_empty(),
            "{invalidation}: a refusal wakes nobody"
        );
    }
}

/// KAN-T142-AC3: the audited `ticket.emergency.override` is a
/// separate command with its own operator and reason. A summary that
/// announces an override is still only a summary.
#[test]
fn a_resume_summary_claiming_an_override_still_refuses_a_cancelled_ticket() {
    let (h, run, ticket) = attempting();
    invalidate(&h, ticket, "cancelled", "override-wording");
    let refusal = resume(
        &h,
        run,
        "override-wording-resume",
        "Emergency override by the operator: resume this attempt regardless of the cancel",
    )
    .expect_err("a summary is not an override");
    assert_eq!(refusal.code, ErrorCode::InvalidRequest, "{refusal:?}");
    assert_eq!(
        override_rows(&h),
        0,
        "a resume summary records no emergency override"
    );
    assert_eq!(
        ticket_state(&h, ticket),
        "cancelled",
        "a refused resume moves no Ticket"
    );
    assert_eq!(recovery_rows(&h), (0, 0, 0));
}

/// KAN-T142-AC2: the production delivery consumer runs against a
/// session that would accept the prompt, so only recovery's own
/// current-state check can prevent it.
#[test]
fn a_cancellation_after_acceptance_stops_the_delivery_without_erasing_it() {
    let (h, run, ticket) = attempting();
    let accepted = resume(
        &h,
        run,
        "invalidated-before-delivery",
        "Resume this attempt only while its custody still allows it",
    )
    .expect("the eligible attempt accepts the resume intent");
    let recovery_id = accepted["id"].as_u64().expect("the record has an identity");
    let ruling_id = accepted["ruling_id"]
        .as_u64()
        .expect("the record names its Ruling");

    invalidate(&h, ticket, "cancelled", "invalidated-before-delivery");

    let fixture = accepting_session(&h);
    let observer = recovery_observer(&h);
    // Settle either way: the intent leaves `pending` when delivery is
    // refused, and a prompt appears when it is not.
    eventually(|| prompts(&fixture) > 0 || delivery_status(&h, recovery_id) != "pending");
    observer.shutdown();

    assert_eq!(
        prompts(&fixture),
        0,
        "a cancelled Ticket delivers no resume to the Coordinator"
    );
    assert_eq!(
        delivery_status(&h, recovery_id),
        "obsolete",
        "the refused intent is closed, not left pending forever"
    );
    assert_eq!(
        ticket_state(&h, ticket),
        "cancelled",
        "the delivery refusal preserves the operator's own change"
    );
    assert_eq!(
        recovery_rows(&h),
        (1, 1, 1),
        "the accepted intent stays auditable after its delivery is refused"
    );
    assert_eq!(
        count(&h, "runs"),
        1,
        "a refused delivery mints no replacement Run"
    );
    assert_eq!(
        choices(&h, run)["records"][0]["ruling_id"],
        json!(ruling_id),
        "the recorded Ruling survives the refusal"
    );
    assert_eq!(choices(&h, run)["can_resume"], false);
    assert_eq!(choices(&h, run)["pending_resume"], false);
}

/// KAN-T142-AC3.
#[test]
fn eligible_recovery_resumes_once_and_grants_no_lifecycle_override() {
    let (h, run, ticket) = attempting();
    let before = ticket_state(&h, ticket);
    let fixture = accepting_session(&h);
    let accepted = resume(
        &h,
        run,
        "eligible-positive",
        "The connection returned; resume this original attempt",
    )
    .expect("eligible recovery resumes");
    let recovery_id = accepted["id"].as_u64().expect("the record has an identity");

    let observer = recovery_observer(&h);
    eventually(|| delivery_status(&h, recovery_id) == "delivered");
    observer.shutdown();
    assert_eq!(prompts(&fixture), 1, "delivery happens exactly once");

    let replayed = resume(
        &h,
        run,
        "eligible-positive",
        "The connection returned; resume this original attempt",
    )
    .expect("the replay returns the recorded intent");
    assert_eq!(replayed, accepted, "a replay records nothing new");

    let restarted = recovery_observer(&h);
    eventually(|| {
        fixture
            .recorded_requests()
            .iter()
            .filter(|request| matches!(request, HerdrRequest::Subscribe))
            .count()
            >= 2
    });
    restarted.shutdown();

    assert_eq!(
        prompts(&fixture),
        1,
        "a replay introduces no second delivery"
    );
    assert_eq!(recovery_rows(&h), (1, 1, 1));
    assert_eq!(count(&h, "runs"), 1, "resume reuses the original attempt");
    assert_eq!(
        ticket_state(&h, ticket),
        before,
        "resume is no lifecycle movement"
    );
    assert_eq!(
        override_rows(&h),
        0,
        "resume grants no emergency override of its own"
    );
    assert_eq!(
        count(&h, "capabilities"),
        1,
        "resume reuses, rather than renews, the existing authority"
    );
}

/// KAN-T142-AC3, KAN-T142-AC4: `ticket.emergency.override` names who
/// ran it and why, and is the one audited way back.
#[test]
fn only_the_explicit_lifecycle_override_restores_a_cancelled_attempt() {
    let (h, run, ticket) = attempting();
    invalidate(&h, ticket, "cancelled", "override-restores");

    for attempt in 0..3 {
        let refusal = resume(
            &h,
            run,
            &format!("override-restores-retry-{attempt}"),
            "Resume the original attempt",
        )
        .expect_err("every retry of the refused intent is refused again");
        assert_eq!(refusal.code, ErrorCode::InvalidRequest, "{refusal:?}");
        assert_eq!(
            recovery_rows(&h),
            (0, 0, 0),
            "retry {attempt} writes nothing"
        );
        assert_eq!(choices(&h, run)["can_resume"], false, "retry {attempt}");
    }

    h.core
        .command(
            "ticket.emergency.override",
            &json!({
                "mutation": mutation(ticket_version(&h, ticket), "override-restores-override"),
                "ticket_id": ticket,
                "to": "active",
                "who": "Sid",
                "why": "The cancel was a mistake; the attempt is still live",
            }),
        )
        .expect("the audited emergency override moves the terminal Ticket");

    assert_eq!(ticket_state(&h, ticket), "active");
    assert_eq!(override_rows(&h), 1, "the override is audited exactly once");
    assert_eq!(
        choices(&h, run)["can_resume"],
        true,
        "the restored Ticket advertises resume again"
    );
    let accepted = resume(
        &h,
        run,
        "override-restores-resume",
        "Resume the original attempt after the audited override",
    )
    .expect("recovery resumes once the override restored an executable state");
    assert_eq!(accepted["action"], "resume");
    assert_eq!(recovery_rows(&h), (1, 1, 1));
}

/// KAN-T142-AC4: a retry stays available for a cancelled Ticket
/// because it queues a replacement request rather than reusing the
/// old authority, so ordinary admission — not recovery — is what
/// refuses that replacement's claim.
#[test]
fn a_cancelled_ticket_keeps_retry_and_leaves_ordinary_admission_to_refuse_it() {
    let (h, run, ticket) = attempting();
    invalidate(&h, ticket, "cancelled", "retry-untouched");
    assert_eq!(
        choices(&h, run)["can_retry"],
        true,
        "retry policy is unchanged by this Ticket"
    );
    let retried = h
        .core
        .command(
            "run.recovery.retry",
            &json!({
                "mutation": mutation(0, "retry-untouched-retry"),
                "run_id": run,
                "summary": "Queue a replacement request for the cancelled attempt",
            }),
        )
        .expect("retry stays available");
    let replacement = retried["replacement_dispatch_request_id"]
        .as_u64()
        .expect("a retry names its replacement request");
    let refusal = h
        .core
        .command(
            "dispatch.claim",
            &json!({
                "mutation": mutation(1, "retry-untouched-claim"),
                "dispatch_request_id": replacement,
            }),
        )
        .expect_err("ordinary admission refuses the terminal Ticket");
    assert_eq!(refusal.code, ErrorCode::InvalidRequest, "{refusal:?}");
    assert_eq!(ticket_state(&h, ticket), "cancelled");
}

/// The Coordinator session one delivery would prompt, connected the
/// way the production consumer connects.
fn coordinator_client(h: &DispatchHarness) -> SessionClient {
    SessionClient::connect(
        SessionMapping::new(
            HerdrSession::named("kanban-main").expect("the session name validates"),
            "/workspaces/kanban.seed",
            "kanban.seed",
        ),
        &h._dir.path().join("sessions"),
    )
    .expect("the scripted session accepts the connection")
}

/// How long an invalidation raised inside a delivery window is given
/// to prove it cannot commit. Exclusion holds it for the whole
/// window, so this only bounds how long the test waits to say so;
/// without exclusion the command lands in microseconds.
const BARRIER: Duration = Duration::from_millis(250);

fn resume_prompt(delivery: &PendingRunResume) -> PromptRequest {
    PromptRequest {
        role: COORDINATOR_ROLE.to_owned(),
        message: format!(
            "Reconcile Recovery {} once: Resume existing Run {} for Dispatch Request {}.",
            delivery.recovery_id, delivery.run_id, delivery.dispatch_request_id
        ),
    }
}

/// KAN-T142-AC2: an invalidation raised while a delivery holds its
/// authorisation cannot commit inside that window, so the prompt that
/// window sends was authorised by state that still held when it went
/// out — and the operator's own change still lands the moment the
/// window closes.
#[test]
fn a_lifecycle_invalidation_cannot_commit_inside_an_authorised_delivery() {
    for invalidation in ["cancelled", "archived"] {
        let (h, run, ticket) = attempting();
        let accepted = resume(
            &h,
            run,
            &format!("{invalidation}-authorised-window"),
            "Resume this attempt while its custody still allows it",
        )
        .expect("the eligible attempt accepts the resume intent");
        let recovery_id = accepted["id"].as_u64().expect("the record has an identity");
        let fixture = accepting_session(&h);
        let store = SqliteRunRecoveryStore::new(&h.database);
        let mut client = coordinator_client(&h);

        std::thread::scope(|scope| {
            let (open, opened) = std::sync::mpsc::channel();
            let (close, closed) = std::sync::mpsc::channel();
            let operator = &h;
            scope.spawn(move || {
                opened.recv().expect("the delivery window opens");
                invalidate(operator, ticket, invalidation, invalidation);
                close.send(()).expect("the invalidation reports back");
            });

            let delivered = store
                .deliver_resume(recovery_id, &mut |authorised| {
                    open.send(()).expect("the invalidating thread is released");
                    assert!(
                        closed.recv_timeout(BARRIER).is_err(),
                        "{invalidation}: an invalidation cannot commit inside an authorised delivery"
                    );
                    match client.prompt(resume_prompt(authorised)) {
                        Ok(true) => ResumeDispatch::Accepted,
                        Ok(_) => ResumeDispatch::Declined,
                        Err(_) => ResumeDispatch::Failed,
                    }
                })
                .expect("the delivery reads");

            assert_eq!(
                delivered,
                ResumeDelivery::Delivered,
                "{invalidation}: the authorised delivery is the one that wins"
            );
            closed
                .recv_timeout(Duration::from_secs(5))
                .expect("the invalidation lands once the delivery window closes");
        });

        assert_eq!(
            prompts(&fixture),
            1,
            "{invalidation}: the authorised delivery prompts exactly once"
        );
        assert_eq!(
            delivery_status(&h, recovery_id),
            "delivered",
            "{invalidation}: the authorised delivery is acknowledged"
        );
        assert_eq!(
            recovery_rows(&h),
            (1, 1, 1),
            "{invalidation}: the delivery erases no recovery, Ruling or delivery row"
        );
        assert_eq!(
            count(&h, "runs"),
            1,
            "{invalidation}: the delivery mints no replacement Run"
        );
        match invalidation {
            "cancelled" => assert_eq!(
                ticket_state(&h, ticket),
                "cancelled",
                "the operator's cancellation survives the delivery"
            ),
            _ => assert!(
                SqliteProjectStore::new(&h.database)
                    .find(ProjectId::new(1))
                    .expect("the Project reads")
                    .expect("the fixture Project exists")
                    .is_archived(),
                "the operator's archival survives the delivery"
            ),
        }
    }
}

/// KAN-T142-AC4: the query reports a pending intent for as long as
/// the delivery row is pending — including the interval after an
/// invalidation, when no new resume may be accepted and the queued
/// one has not yet reached its disposition.
#[test]
fn a_pending_intent_stays_visible_until_it_is_authoritatively_closed() {
    let (h, run, ticket) = attempting();
    let accepted = resume(
        &h,
        run,
        "pending-visible",
        "Resume this attempt while its custody still allows it",
    )
    .expect("the eligible attempt accepts the resume intent");
    let recovery_id = accepted["id"].as_u64().expect("the record has an identity");
    assert_eq!(
        choices(&h, run)["pending_resume"],
        true,
        "an accepted intent awaiting delivery is pending"
    );
    assert_eq!(
        choices(&h, run)["can_resume"],
        false,
        "a pending intent admits no second one"
    );

    invalidate(&h, ticket, "cancelled", "pending-visible");

    assert_eq!(
        delivery_status(&h, recovery_id),
        "pending",
        "the durable delivery row is still queued"
    );
    assert_eq!(
        choices(&h, run)["can_resume"],
        false,
        "the cancelled Ticket advertises no new resume"
    );
    assert_eq!(
        choices(&h, run)["pending_resume"],
        true,
        "the queued intent stays visible while it awaits disposition"
    );

    let fixture = accepting_session(&h);
    let observer = recovery_observer(&h);
    eventually(|| delivery_status(&h, recovery_id) != "pending");
    observer.shutdown();

    assert_eq!(
        prompts(&fixture),
        0,
        "the invalidated intent prompts nobody"
    );
    assert_eq!(delivery_status(&h, recovery_id), "obsolete");
    assert_eq!(
        choices(&h, run)["pending_resume"],
        false,
        "a closed intent is no longer pending"
    );
    assert_eq!(choices(&h, run)["can_resume"], false);
}

/// The deadline the bounded-delivery scenarios give one Herdr round
/// trip. Several times a scripted pacing gap, so no individual read
/// is what ends a request.
const DELIVERY_DEADLINE: Duration = Duration::from_millis(500);

/// How far apart a stalling Coordinator places the pieces of its
/// answer. A `thread::sleep` on this platform can overshoot its
/// request by well over a tenth of a second, so the gap is small and
/// the deadline it must fit inside generous: what these scenarios
/// need is that every individual read is answered in time while the
/// request as a whole is not.
const STALL_GAP: Duration = Duration::from_millis(50);

/// The exact non-empty write count for a delivery-deadline scenario.
/// At the pacing's lower bound, those writes take 2 seconds: four
/// request deadlines and longer than the unrelated-command bound.
const STALL_PIECES: usize = 40;

/// How long an unrelated Core command may wait behind a stalled
/// delivery: one second beyond the delivery deadline for gate hand-off
/// and scheduling, but 500 ms below the defective transport floor.
const UNRELATED_BOUND: Duration = Duration::from_millis(1_500);

/// The deadline the shutdown scenario gives one round trip. Long, so
/// that a shutdown which merely waited the request out would blow the
/// bound below and only interrupting the socket can meet it.
const SHUTDOWN_DEADLINE: Duration = Duration::from_millis(3_000);

/// The accepted prompt-result line has 41 bytes, so this requests one
/// non-empty write per byte. Its 2.05-second floor exceeds the shutdown
/// bound even though the finite answer can precede the request deadline.
const SHUTDOWN_PIECES: usize = 41;

/// How long an owned shutdown may take while a worker is blocked on
/// a transient resume prompt.
const SHUTDOWN_BOUND: Duration = Duration::from_millis(1_000);

/// A Coordinator session that answers a prompt at a crawl: `events`
/// complete push event frames, then its answer in `fragments`
/// pieces, each `STALL_GAP` after the last. Snapshots and
/// subscriptions are answered at once, so only the prompt stalls.
fn stalling_session(h: &DispatchHarness, events: usize, fragments: usize) -> ScriptedSession {
    ScriptedSession::bind(
        &h._dir.path().join("sessions"),
        "kanban-main",
        "/workspaces/kanban.seed",
        SessionScript::default()
            .with_prompt_accepted(true)
            .with_paced_prompt(events, fragments, STALL_GAP),
    )
}

fn subscribes(fixture: &ScriptedSession) -> usize {
    fixture
        .recorded_requests()
        .iter()
        .filter(|request| matches!(request, HerdrRequest::Subscribe))
        .count()
}

/// What the session was asked for immediately before its first
/// prompt. This is what tells the two delivery paths apart: the live
/// subscription prompts over the connection it just subscribed on,
/// while a transient delivery opens its own and verifies the mapping
/// through a snapshot first.
fn before_first_prompt(fixture: &ScriptedSession) -> HerdrRequest {
    let recorded = fixture.recorded_requests();
    let first = recorded
        .iter()
        .position(|request| matches!(request, HerdrRequest::Prompt { .. }))
        .expect("the session was prompted");
    recorded[first - 1].clone()
}

/// A second Ticket no delivery touches. A command against it is an
/// ordinary unrelated mutation: it waits for the same delivery gate
/// every Core write waits for, so how long it waits is how long the
/// product is held.
fn unrelated_ticket(h: &DispatchHarness) -> u64 {
    common::insert_ready_ticket(&h.database_path, 2, "normal")
}

/// The command Core the delivery-bound scenarios drive, over a
/// database handle the observation worker will share: a second
/// handle on the same file would put the worker's telemetry appends
/// on their own SQLite connection, contending for the write lock
/// instead of queueing behind the same one, which the single-process
/// product never does.
fn commanding_core(h: &DispatchHarness) -> (Arc<Database>, Core) {
    let database = Arc::new(Database::open(&h.database_path).expect("the database reopens"));
    let (mut core, wake) = common::core_over(&database);
    register_recovery(&mut core, &database, wake);
    register_operator_commands(&mut core, &database);
    (database, core)
}

/// Start the production observation worker over that same handle.
/// Whether an intent is already pending when this is called decides
/// which connection carries its prompt: the worker delivers queued
/// intents before it redials, so an intent accepted first goes out
/// over a transient connection, and one accepted after the
/// subscription settles goes out over the live one.
fn observing(
    h: &DispatchHarness,
    database: Arc<Database>,
    io_timeout: Duration,
) -> Arc<HerdrObserver> {
    let observer = HerdrObserver::with_observation(
        database.clone(),
        h._dir.path().join("sessions"),
        observation_within(io_timeout),
    );
    observer.observe_projects(&[SqliteProjectStore::new(&database)
        .find(ProjectId::new(1))
        .expect("the Project reads")
        .expect("the fixture Project exists")]);
    observer
}

/// Accept a resume intent through an explicitly chosen Core.
fn resume_through(core: &Core, run: u64, key: &str, summary: &str) -> Result<Value, ApiError> {
    core.command(
        "run.recovery.resume",
        &json!({
            "mutation": mutation(0, key),
            "run_id": run,
            "summary": summary,
        }),
    )
}

/// Cancel the unrelated Ticket through the production command and
/// report how long the whole command took, gate wait included.
fn time_unrelated_cancel(h: &DispatchHarness, core: &Core, ticket: u64, key: &str) -> Duration {
    let version = ticket_version(h, ticket);
    let started = std::time::Instant::now();
    core.command(
        "ticket.cancel",
        &json!({
            "mutation": mutation(version, key),
            "ticket_id": ticket,
        }),
    )
    .expect("the unrelated Ticket cancels");
    started.elapsed()
}

fn time_shutdown(observer: &Arc<HerdrObserver>) -> Duration {
    let started = std::time::Instant::now();
    observer.shutdown();
    started.elapsed()
}

/// KAN-T142-AC2: the live subscription's own connection carries the
/// resume prompt, and a Coordinator that keeps pushing complete
/// event frames answers every read on time while never answering the
/// request. The delivery holds the mutation gate across that prompt,
/// so its deadline is what bounds every unrelated command behind it.
#[test]
fn a_stalled_live_prompt_cannot_hold_an_unrelated_command_past_the_delivery_deadline() {
    let (h, run, ticket) = attempting();
    let before = ticket_state(&h, ticket);
    let other = unrelated_ticket(&h);
    let fixture = stalling_session(&h, STALL_PIECES, 1);
    let (database, core) = commanding_core(&h);
    // The subscription settles before the intent exists, so the
    // worker has a live connection to carry the prompt.
    let observer = observing(&h, database, DELIVERY_DEADLINE);
    eventually(|| subscribes(&fixture) >= 1);

    let accepted = resume_through(
        &core,
        run,
        "stalled-live",
        "Resume this attempt while its custody still allows it",
    )
    .expect("the eligible attempt accepts the resume intent");
    let recovery_id = accepted["id"].as_u64().expect("the record has an identity");
    eventually(|| prompts(&fixture) >= 1);

    let waited = time_unrelated_cancel(&h, &core, other, "stalled-live-unrelated");
    let stopped = time_shutdown(&observer);

    assert!(
        waited < UNRELATED_BOUND,
        "an unrelated command waited {waited:?} behind a stalled delivery"
    );
    assert!(
        stopped < UNRELATED_BOUND,
        "the owned shutdown took {stopped:?}"
    );
    assert_eq!(
        ticket_state(&h, other),
        "cancelled",
        "the unrelated command landed rather than being abandoned"
    );
    assert_eq!(
        delivery_status(&h, recovery_id),
        "pending",
        "an answer that never arrived claims no delivery"
    );
    assert!(
        matches!(before_first_prompt(&fixture), HerdrRequest::Subscribe),
        "the prompt went out over the live subscription's own connection"
    );
    assert_eq!(
        ticket_state(&h, ticket),
        before,
        "the stalled delivery moves no lifecycle state"
    );
    assert_eq!(
        recovery_rows(&h),
        (1, 1, 1),
        "the stalled delivery erases no recovery, Ruling or delivery row"
    );
}

/// KAN-T142-AC2: the same bound holds when the answer itself dribbles
/// in. A response line assembled one fragment at a time keeps every
/// read timely, so only a deadline over the whole request releases
/// the gate — and the intent it failed to deliver stays pending for
/// its ordinary retry rather than claiming a delivery.
#[test]
fn a_stalled_transient_prompt_releases_the_delivery_gate_when_its_deadline_expires() {
    assert!(UNRELATED_BOUND < STALL_GAP * STALL_PIECES as u32);
    let (h, run, _ticket) = attempting();
    let other = unrelated_ticket(&h);
    let fixture = stalling_session(&h, 0, STALL_PIECES);
    let (database, core) = commanding_core(&h);
    let accepted = resume_through(
        &core,
        run,
        "stalled-transient",
        "Resume this attempt while its custody still allows it",
    )
    .expect("the eligible attempt accepts the resume intent");
    let recovery_id = accepted["id"].as_u64().expect("the record has an identity");
    // The intent is durable before observation starts, so the
    // worker's first act is to deliver it over its own connection.
    let observer = observing(&h, database, DELIVERY_DEADLINE);
    eventually(|| prompts(&fixture) >= 1);

    let waited = time_unrelated_cancel(&h, &core, other, "stalled-transient-unrelated");
    let stopped = time_shutdown(&observer);

    assert!(
        waited < UNRELATED_BOUND,
        "an unrelated command waited {waited:?} behind a stalled delivery"
    );
    assert!(
        stopped < UNRELATED_BOUND,
        "the owned shutdown took {stopped:?}"
    );
    assert_eq!(
        ticket_state(&h, other),
        "cancelled",
        "the unrelated command landed rather than being abandoned"
    );
    assert_eq!(
        delivery_status(&h, recovery_id),
        "pending",
        "an ambiguous transport failure stays pending for its retry"
    );
    assert!(
        matches!(before_first_prompt(&fixture), HerdrRequest::Snapshot),
        "the prompt went out over a transient connection of its own"
    );
    assert_eq!(
        choices(&h, run)["pending_resume"],
        true,
        "the undelivered intent is still advertised as pending"
    );
    assert_eq!(recovery_rows(&h), (1, 1, 1));
}

/// KAN-T142-AC2, KAN-T78-AC1: the connection a transient delivery
/// opens is owned like the live one. A shutdown raised while that
/// connection is blocked on a prompt interrupts it, rather than
/// waiting out the request deadline with the gate still held.
#[test]
fn a_shutdown_interrupts_a_transient_resume_prompt_instead_of_waiting_out_its_deadline() {
    let (h, run, _ticket) = attempting();
    let other = unrelated_ticket(&h);
    let fixture = stalling_session(&h, 0, SHUTDOWN_PIECES);
    let (database, core) = commanding_core(&h);
    let accepted = resume_through(
        &core,
        run,
        "shutdown-transient",
        "Resume this attempt while its custody still allows it",
    )
    .expect("the eligible attempt accepts the resume intent");
    let recovery_id = accepted["id"].as_u64().expect("the record has an identity");
    // The intent is durable before observation starts, so the
    // worker's first act is to deliver it over its own connection.
    let observer = observing(&h, database, SHUTDOWN_DEADLINE);
    eventually(|| prompts(&fixture) >= 1);

    let stopped = time_shutdown(&observer);
    let waited = time_unrelated_cancel(&h, &core, other, "shutdown-transient-unrelated");

    assert!(
        stopped < SHUTDOWN_BOUND,
        "the owned shutdown waited {stopped:?} on a blocked transient prompt"
    );
    assert!(
        waited < UNRELATED_BOUND,
        "a command after the shutdown waited {waited:?}, so the gate outlived the worker"
    );
    assert_eq!(
        ticket_state(&h, other),
        "cancelled",
        "the command after the shutdown landed"
    );
    assert!(
        matches!(before_first_prompt(&fixture), HerdrRequest::Snapshot),
        "the interrupted prompt was on a transient connection of its own"
    );
    assert_eq!(
        delivery_status(&h, recovery_id),
        "pending",
        "an interrupted prompt claims no delivery"
    );
    assert_eq!(recovery_rows(&h), (1, 1, 1));
}
