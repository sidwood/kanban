//! App gate for execution admission (KAN-T138, KAN-S9-US2, DR-EP-08):
//! every ordinary claim and acknowledgement rechecks the Ticket's
//! current executable eligibility and fails closed, leaving the
//! Ticket, the Dispatch Request, the Run, the Lane, and capacity
//! exactly as they stood. Every scenario drives the production
//! handlers over a disposable SQLite database.

use std::sync::Arc;

use kanban_app::{Core, RunStore};
use kanban_domain::DispatchRequestId;
use kanban_dto::ErrorCode;
use kanban_storage::{
    Database, SqliteDependencyStore, SqliteEvidenceStore, SqliteGraphProposalStore,
    SqliteLaneStore, SqlitePlanStore, SqliteProfileStore, SqliteProjectStore, SqliteRunStore,
    SqliteScheduleStore, SqliteSpecStore, SqliteTicketStore, SqliteWorkspaceStore,
};
use serde_json::{Value, json};

mod common;

use common::{DispatchHarness, core_over, harness, mutation};

/// The dispatch harness with the Ticket authoring, lifecycle, Lane,
/// profile, dependency, Spec, and graph operations registered, so
/// every fixture moves through production commands.
fn admitting() -> DispatchHarness {
    let mut h = harness();
    register_authoring(&mut h.core, &h.database, h._dir.path());
    h
}

fn register_authoring(core: &mut Core, database: &Database, scratch: &std::path::Path) {
    let attachments = scratch.join("attachments");
    std::fs::create_dir_all(&attachments).expect("the attachment directory exists");
    let projects = Arc::new(SqliteProjectStore::new(database));
    let tickets = Arc::new(SqliteTicketStore::new(database));
    let specs = Arc::new(SqliteSpecStore::new(database));
    let plans = Arc::new(SqlitePlanStore::new(database));
    let evidence = Arc::new(SqliteEvidenceStore::new(database, attachments));
    let dependencies = Arc::new(SqliteDependencyStore::new(database));
    let profiles = Arc::new(SqliteProfileStore::new(database));
    let lanes = Arc::new(SqliteLaneStore::new(database));
    let workspaces = Arc::new(SqliteWorkspaceStore::new(database));
    let schedules = Arc::new(SqliteScheduleStore::new(database));
    let proposals = Arc::new(SqliteGraphProposalStore::new(database));
    core.register_plans(plans.clone(), projects.clone(), specs.clone())
        .expect("the plan operations register");
    core.register_specs(specs.clone(), projects.clone(), plans)
        .expect("the spec operations register");
    core.register_tickets(
        tickets.clone(),
        projects.clone(),
        specs.clone(),
        evidence.clone(),
    )
    .expect("the ticket operations register");
    core.register_lifecycle(
        tickets.clone(),
        dependencies.clone(),
        projects.clone(),
        schedules,
        None,
    )
    .expect("the lifecycle operations register");
    core.register_profiles(profiles.clone(), tickets.clone(), projects.clone())
        .expect("the profile operations register");
    core.register_lanes(lanes, projects.clone(), workspaces, tickets.clone())
        .expect("the lane operations register");
    core.register_dependencies(dependencies.clone(), tickets.clone(), projects.clone())
        .expect("the dependency operations register");
    core.register_graph_proposals(proposals, dependencies, tickets, specs, projects, profiles)
        .expect("the graph operations register");
}

/// Reopen the same database through a fresh Core, as a restarted
/// service would.
fn reopen(h: &DispatchHarness) -> (Database, Core) {
    let database = Database::open(&h.database_path).expect("the database reopens");
    let (mut core, _) = core_over(&database);
    register_authoring(&mut core, &database, h._dir.path());
    (database, core)
}

fn version(record: &Value) -> u64 {
    record["version"]
        .as_u64()
        .expect("the record carries a version")
}

fn id(record: &Value) -> u64 {
    record["id"]
        .as_u64()
        .expect("the record carries an identity")
}

/// Quick capture one Bug: title, actual behaviour, reporter evidence.
fn quick_capture(core: &Core, key: &str) -> Value {
    core.command(
        "ticket.create",
        &json!({
            "mutation": mutation(0, format!("{key}-capture")),
            "project_id": 1,
            "kind": "bug",
            "priority": "normal",
            "title": "Claim admits work it should refuse",
            "actual_behaviour": "An unqualified Bug was claimed and acknowledged.",
            "reporter_evidence": "The recovery audit probe transcript.",
        }),
    )
    .expect("quick capture lands")
}

/// Qualify `bug` completely, as the operator does before it may leave draft.
fn qualify(core: &Core, bug: &Value, key: &str) -> Value {
    core.command(
        "ticket.bug.qualify",
        &json!({
            "mutation": mutation(version(bug), format!("{key}-qualify")),
            "ticket_id": id(bug),
            "qualification": {
                "expected_behaviour": "Ordinary admission refuses an ineligible Ticket.",
                "reproduction": "Enqueue an unqualified Bug and claim it.",
                "environment": "macOS 26, disposable SQLite.",
                "severity": "critical",
                "frequency": "Every claim.",
                "affected_scope": "Every dispatch path.",
                "risk": "Unauthorised execution.",
                "criteria": [{
                    "outcome": "The claim is refused and nothing changes.",
                    "stories": ["CORE-S1-US1"]
                }],
                "verification_steps": [{ "command": "cargo test -p kanban-app --test dispatch_admission" }]
            }
        }),
    )
    .expect("the qualification lands")
}

/// Author one Task Ticket in `project`, draft as created.
fn task(core: &Core, project: u64, key: &str) -> Value {
    core.command(
        "ticket.create",
        &json!({
            "mutation": mutation(0, format!("{key}-task")),
            "project_id": project,
            "kind": "task",
            "priority": "normal",
            "title": "Bounded operational work",
            "subtype": "operational",
            "mode": "agent",
            "completion": ["The work is done."],
        }),
    )
    .expect("the Task authors")
}

/// Drag a Task to ready through the operator's own surface.
fn make_ready(core: &Core, ticket: &Value, key: &str) -> Value {
    core.command(
        "ticket.transition",
        &json!({
            "mutation": mutation(version(ticket), format!("{key}-ready")),
            "ticket_id": id(ticket),
            "to": "ready",
        }),
    )
    .expect("the Task moves to ready")
}

/// Park then unpark: the human commands that carry an agent-owned
/// kind from draft to ready without a drag.
fn park_and_unpark(core: &Core, ticket: &Value, key: &str) -> Value {
    let parked = core
        .command(
            "ticket.park",
            &json!({
                "mutation": mutation(version(ticket), format!("{key}-park")),
                "ticket_id": id(ticket),
            }),
        )
        .expect("the Ticket parks");
    core.command(
        "ticket.unpark",
        &json!({
            "mutation": mutation(version(&parked), format!("{key}-unpark")),
            "ticket_id": id(ticket),
        }),
    )
    .expect("the Ticket returns to ready")
}

fn assign_profile(core: &Core, ticket: &Value, key: &str) -> Value {
    core.command(
        "ticket.assign",
        &json!({
            "mutation": mutation(version(ticket), format!("{key}-assign")),
            "ticket_id": id(ticket),
            "profile": "standard",
        }),
    )
    .expect("the assignment lands")
}

/// Seat `ticket` in a fresh Lane through the Lane commands.
fn seat(core: &Core, ticket: u64, key: &str) -> u64 {
    let lane = core
        .command(
            "lane.create",
            &json!({
                "mutation": mutation(0, format!("{key}-lane")),
                "project_id": 1,
            }),
        )
        .expect("the Lane creates");
    core.command(
        "lane.ticket.assign",
        &json!({
            "mutation": mutation(version(&lane), format!("{key}-seat")),
            "lane_id": id(&lane),
            "ticket_id": ticket,
        }),
    )
    .expect("the Ticket seats");
    id(&lane)
}

fn enqueue(core: &Core, ticket: u64, key: &str) -> u64 {
    let created = core
        .command(
            "dispatch.request",
            &json!({
                "mutation": mutation(0, format!("{key}-request")),
                "ticket_id": ticket,
            }),
        )
        .expect("the request is created");
    id(&created)
}

fn claim(
    core: &Core,
    request: u64,
    request_version: u64,
    key: &str,
) -> Result<Value, kanban_dto::ApiError> {
    core.command(
        "dispatch.claim",
        &json!({
            "mutation": mutation(request_version, format!("{key}-claim")),
            "dispatch_request_id": request,
        }),
    )
}

fn acknowledge(
    core: &Core,
    request: u64,
    request_version: u64,
    key: &str,
) -> Result<Value, kanban_dto::ApiError> {
    core.command(
        "run.acknowledge",
        &json!({
            "mutation": mutation(request_version, format!("{key}-ack")),
            "dispatch_request_id": request,
        }),
    )
}

fn cancel(core: &Core, ticket: u64, key: &str) {
    let current = core
        .query("ticket.get", &json!({ "ticket_id": ticket }))
        .expect("the Ticket reads");
    core.command(
        "ticket.cancel",
        &json!({
            "mutation": mutation(version(&current), format!("{key}-cancel")),
            "ticket_id": ticket,
        }),
    )
    .expect("the cancel lands");
}

/// A ready Task with its profile assigned and a Lane holding it: the
/// legal implementer candidate every stale-request scenario starts from.
fn seated_ready_task(core: &Core, key: &str) -> u64 {
    let created = task(core, 1, key);
    let ready = make_ready(core, &created, key);
    assign_profile(core, &ready, key);
    seat(core, id(&created), key);
    id(&created)
}

/// Every authoritative record a refusal must leave untouched.
#[derive(Debug, PartialEq, Eq)]
struct Authoritative {
    tickets: Vec<(i64, String, i64)>,
    requests: Vec<(i64, String, i64, Option<i64>)>,
    runs: i64,
    capabilities: Vec<(i64, String)>,
    lanes: Vec<(i64, Option<i64>, i64)>,
    claimed_capacity: i64,
}

fn authoritative(path: &std::path::Path) -> Authoritative {
    let conn = rusqlite::Connection::open(path).expect("the database reopens");
    let tickets = conn
        .prepare("SELECT id, state, version FROM tickets ORDER BY id")
        .expect("the statement prepares")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("the rows serve")
        .collect::<Result<Vec<_>, _>>()
        .expect("the rows decode");
    let requests = conn
        .prepare("SELECT id, status, version, completed_at FROM dispatch_requests ORDER BY id")
        .expect("the statement prepares")
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .expect("the rows serve")
        .collect::<Result<Vec<_>, _>>()
        .expect("the rows decode");
    let runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))
        .expect("the count serves");
    let capabilities = conn
        .prepare("SELECT id, status FROM capabilities ORDER BY id")
        .expect("the statement prepares")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("the rows serve")
        .collect::<Result<Vec<_>, _>>()
        .expect("the rows decode");
    let lanes = conn
        .prepare("SELECT id, ticket_id, version FROM lanes ORDER BY id")
        .expect("the statement prepares")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("the rows serve")
        .collect::<Result<Vec<_>, _>>()
        .expect("the rows decode");
    let claimed_capacity: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dispatch_requests WHERE status = 'claimed' AND completed_at IS NULL",
            [],
            |row| row.get(0),
        )
        .expect("the count serves");
    Authoritative {
        tickets,
        requests,
        runs,
        capabilities,
        lanes,
        claimed_capacity,
    }
}

fn queued_ids(core: &Core) -> Vec<u64> {
    core.query("dispatch.queue", &json!({ "project_id": 1 }))
        .expect("the queue serves")["requests"]
        .as_array()
        .expect("the requests are a list")
        .iter()
        .map(id)
        .collect()
}

fn assert_refused(outcome: Result<Value, kanban_dto::ApiError>, message: &str) {
    let error = outcome.expect_err("admission fails closed");
    assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    assert_eq!(error.message, message);
}

#[test]
fn dispatch_admission_refuses_an_unqualified_draft_bug_and_changes_nothing() {
    let h = admitting();
    let bug = quick_capture(&h.core, "unqualified");
    assign_profile(&h.core, &bug, "unqualified");
    seat(&h.core, id(&bug), "unqualified");
    let request = enqueue(&h.core, id(&bug), "unqualified");
    let before = authoritative(&h.database_path);

    assert_refused(
        claim(&h.core, request, 1, "unqualified"),
        "the Bug is unqualified; a Bug executes only once its qualification is complete",
    );
    assert_eq!(authoritative(&h.database_path), before);
    assert_eq!(
        queued_ids(&h.core),
        vec![request],
        "the request stays queued"
    );

    let ack = acknowledge(&h.core, request, 1, "unqualified").expect_err("nothing was claimed");
    assert_eq!(ack.code, ErrorCode::InvalidRequest);
    assert_eq!(authoritative(&h.database_path), before);

    let (_database, core) = reopen(&h);
    assert_refused(
        claim(&core, request, 1, "unqualified-reloaded"),
        "the Bug is unqualified; a Bug executes only once its qualification is complete",
    );
    assert_eq!(authoritative(&h.database_path), before);
}

#[test]
fn dispatch_admission_refuses_a_cancelled_queued_bug_and_changes_nothing() {
    let h = admitting();
    let bug = quick_capture(&h.core, "cancelled");
    let qualified = qualify(&h.core, &bug, "cancelled");
    let ready = park_and_unpark(&h.core, &qualified, "cancelled");
    assert_eq!(ready["state"], json!("ready"));
    assign_profile(&h.core, &ready, "cancelled");
    seat(&h.core, id(&bug), "cancelled");
    let request = enqueue(&h.core, id(&bug), "cancelled");
    cancel(&h.core, id(&bug), "cancelled");
    let before = authoritative(&h.database_path);

    assert_refused(
        claim(&h.core, request, 1, "cancelled"),
        "cancelled and superseded are terminal; the Ticket accepts no further changes",
    );
    assert_eq!(authoritative(&h.database_path), before);
    assert_eq!(queued_ids(&h.core), vec![request]);

    let (_database, core) = reopen(&h);
    assert_refused(
        claim(&core, request, 1, "cancelled-reloaded"),
        "cancelled and superseded are terminal; the Ticket accepts no further changes",
    );
    assert_eq!(authoritative(&h.database_path), before);
}

#[test]
fn dispatch_admission_refuses_acknowledgement_after_a_claimed_ticket_is_cancelled() {
    let h = admitting();
    let ticket = seated_ready_task(&h.core, "stale-cancel");
    let request = enqueue(&h.core, ticket, "stale-cancel");
    let won = claim(&h.core, request, 1, "stale-cancel").expect("the claim is attempted");
    assert_eq!(won["claimed"], json!(true), "a ready Task claims: {won}");
    cancel(&h.core, ticket, "stale-cancel");
    let before = authoritative(&h.database_path);

    assert_refused(
        acknowledge(&h.core, request, version(&won["request"]), "stale-cancel"),
        "cancelled and superseded are terminal; the Ticket accepts no further changes",
    );
    assert_eq!(authoritative(&h.database_path), before);
    assert_eq!(before.runs, 0, "no run is minted for a cancelled Ticket");

    let (database, core) = reopen(&h);
    assert_refused(
        acknowledge(
            &core,
            request,
            version(&won["request"]),
            "stale-cancel-reloaded",
        ),
        "cancelled and superseded are terminal; the Ticket accepts no further changes",
    );
    assert!(
        SqliteRunStore::new(&database)
            .executing_for_request(DispatchRequestId::new(request))
            .expect("the lookup serves")
            .is_none()
    );
}

#[test]
fn dispatch_admission_rechecks_blockers_added_after_enqueue() {
    let h = admitting();
    let ticket = seated_ready_task(&h.core, "blocker");
    let request = enqueue(&h.core, ticket, "blocker");
    let current = h
        .core
        .query("ticket.get", &json!({ "ticket_id": ticket }))
        .expect("the Ticket reads");
    let blocked = h
        .core
        .command(
            "ticket.blocker.add",
            &json!({
                "mutation": mutation(version(&current), "blocker-add"),
                "ticket_id": ticket,
                "description": "waiting on an unregistered vendor",
            }),
        )
        .expect("the blocker lands");
    let before = authoritative(&h.database_path);

    assert_refused(
        claim(&h.core, request, 1, "blocker"),
        "the Ticket is held back by 1 unresolved dependencies or external blockers",
    );
    assert_eq!(authoritative(&h.database_path), before);

    let blocker_id = blocked["blockers"]
        .as_array()
        .and_then(|blockers| blockers.first())
        .map(id)
        .expect("the record lists the blocker");
    let after_blocker = h
        .core
        .query("ticket.get", &json!({ "ticket_id": ticket }))
        .expect("the Ticket reads");
    h.core
        .command(
            "ticket.blocker.remove",
            &json!({
                "mutation": mutation(version(&after_blocker), "blocker-remove"),
                "ticket_id": ticket,
                "blocker_id": blocker_id,
            }),
        )
        .expect("the blocker clears");
    let won = claim(&h.core, request, 1, "blocker-cleared").expect("the claim is attempted");
    assert_eq!(
        won["claimed"],
        json!(true),
        "current readiness admits the same queued request once the blocker clears: {won}"
    );
}

#[test]
fn dispatch_admission_refuses_acknowledgement_when_a_blocker_arrives_after_the_claim() {
    let h = admitting();
    let ticket = seated_ready_task(&h.core, "late-blocker");
    let request = enqueue(&h.core, ticket, "late-blocker");
    let won = claim(&h.core, request, 1, "late-blocker").expect("the claim is attempted");
    assert_eq!(won["claimed"], json!(true));
    let current = h
        .core
        .query("ticket.get", &json!({ "ticket_id": ticket }))
        .expect("the Ticket reads");
    h.core
        .command(
            "ticket.blocker.add",
            &json!({
                "mutation": mutation(version(&current), "late-blocker-add"),
                "ticket_id": ticket,
                "description": "waiting on an unregistered vendor",
            }),
        )
        .expect("the blocker lands");
    let before = authoritative(&h.database_path);

    assert_refused(
        acknowledge(&h.core, request, version(&won["request"]), "late-blocker"),
        "the Ticket is held back by 1 unresolved dependencies or external blockers",
    );
    assert_eq!(authoritative(&h.database_path), before);
    assert_eq!(before.runs, 0);
}

#[test]
fn dispatch_admission_refuses_an_archived_project() {
    let h = admitting();
    let ticket = seated_ready_task(&h.core, "archived");
    let request = enqueue(&h.core, ticket, "archived");
    rusqlite::Connection::open(&h.database_path)
        .expect("the database reopens")
        .execute("UPDATE projects SET archived = 1 WHERE id = 1", [])
        .expect("the fixture Project archives");
    let before = authoritative(&h.database_path);

    assert_refused(
        claim(&h.core, request, 1, "archived"),
        "archived is terminal; the Project accepts no further changes",
    );
    assert_eq!(authoritative(&h.database_path), before);
}

#[test]
fn dispatch_admission_refuses_a_scheduled_ticket_until_activation() {
    let h = admitting();
    let created = task(&h.core, 1, "scheduled");
    let scheduled = h
        .core
        .command(
            "ticket.schedule",
            &json!({
                "mutation": mutation(version(&created), "scheduled-schedule"),
                "ticket_id": id(&created),
                "activation": "2099-01-01T09:00:00Z",
                "timezone": "UTC",
                "profile": "standard",
            }),
        )
        .expect("the one-time Schedule lands");
    assert_eq!(scheduled["state"], json!("scheduled"));
    assign_profile(&h.core, &scheduled, "scheduled");
    seat(&h.core, id(&created), "scheduled");
    let request = enqueue(&h.core, id(&created), "scheduled");
    let before = authoritative(&h.database_path);

    assert_refused(
        claim(&h.core, request, 1, "scheduled"),
        "the Ticket is scheduled; it is unavailable until its activation",
    );
    assert_eq!(authoritative(&h.database_path), before);
}

#[test]
fn dispatch_admission_refuses_a_draft_task_as_not_executable() {
    let h = admitting();
    let created = task(&h.core, 1, "draft");
    assign_profile(&h.core, &created, "draft");
    seat(&h.core, id(&created), "draft");
    let request = enqueue(&h.core, id(&created), "draft");
    let before = authoritative(&h.database_path);

    assert_refused(
        claim(&h.core, request, 1, "draft"),
        "a draft Ticket is not executable; an implementer run admits a ready or active Ticket",
    );
    assert_eq!(authoritative(&h.database_path), before);
}

/// Author one Spec claiming one story and approve its first version.
fn approved_spec(core: &Core) -> u64 {
    let created = core
        .command(
            "spec.create",
            &json!({
                "mutation": mutation(0, "graph-spec"),
                "project_id": 1,
                "content": {
                    "name": "Admission",
                    "short_description": "Executable eligibility at every admission path",
                    "problem_statement": "Claims admitted ineligible work.",
                    "solution": "One shared invariant.",
                    "user_stories": "- CORE-S1-US1: As an operator, I want claims to fail closed.\n",
                    "implementation_decisions": "Domain rule, application loader.",
                    "testing_decisions": "Production handlers over disposable SQLite.",
                    "out_of_scope": "Lifecycle completion.",
                    "further_notes": "None",
                },
            }),
        )
        .expect("the Spec authors");
    core.command(
        "spec.version.approve",
        &json!({
            "mutation": mutation(version(&created), "graph-approve-spec"),
            "spec_id": id(&created),
        }),
    )
    .expect("the draft approves");
    id(&created)
}

#[test]
fn dispatch_admission_refuses_an_implementation_ticket_until_its_graph_is_approved() {
    let h = admitting();
    let spec = approved_spec(&h.core);
    let created = h
        .core
        .command(
            "ticket.create",
            &json!({
                "mutation": mutation(0, "graph-ticket"),
                "project_id": 1,
                "kind": "implementation",
                "priority": "normal",
                "spec_id": spec,
                "slice": "Refuse unsafe admission",
                "criteria": [{
                    "outcome": "Claims fail closed.",
                    "stories": ["CORE-S1-US1"]
                }],
            }),
        )
        .expect("the Implementation authors");
    let ready = park_and_unpark(&h.core, &created, "graph");
    assert_eq!(ready["state"], json!("ready"));
    assign_profile(&h.core, &ready, "graph");
    seat(&h.core, id(&created), "graph");
    let request = enqueue(&h.core, id(&created), "graph");
    let before = authoritative(&h.database_path);

    assert_refused(
        claim(&h.core, request, 1, "graph-unapproved"),
        "the Implementation Ticket belongs to no approved Ticket graph; a human approves the graph before execution",
    );
    assert_eq!(authoritative(&h.database_path), before);

    let proposal = h
        .core
        .command(
            "ticket.graph.propose",
            &json!({
                "mutation": mutation(0, "graph-propose"),
                "spec_id": spec,
                "spec_version": 1,
                "tickets": [id(&created)],
                "edges": [],
            }),
        )
        .expect("the graph proposes");
    h.core
        .command(
            "ticket.graph.approve",
            &json!({
                "mutation": mutation(version(&proposal), "graph-approve"),
                "proposal_id": id(&proposal),
            }),
        )
        .expect("the human gate approves");
    let won = claim(&h.core, request, 1, "graph-approved").expect("the claim is attempted");
    assert_eq!(
        won["claimed"],
        json!(true),
        "the pinned Implementation admits through the same queued request: {won}"
    );
}

#[test]
fn dispatch_admission_admits_a_qualified_ready_bug_and_survives_reload() {
    let h = admitting();
    let bug = quick_capture(&h.core, "legal");
    let qualified = qualify(&h.core, &bug, "legal");
    let ready = park_and_unpark(&h.core, &qualified, "legal");
    assign_profile(&h.core, &ready, "legal");
    let lane = seat(&h.core, id(&bug), "legal");
    let request = enqueue(&h.core, id(&bug), "legal");

    let won = claim(&h.core, request, 1, "legal").expect("the claim is attempted");
    assert_eq!(won["claimed"], json!(true), "{won}");
    assert_eq!(won["capability"]["lane_id"], json!(lane));
    let run = acknowledge(&h.core, request, version(&won["request"]), "legal")
        .expect("the run acknowledges");
    assert_eq!(run["status"], json!("executing"));
    assert_eq!(run["ticket_id"], json!(id(&bug)));

    let (database, core) = reopen(&h);
    let restored = SqliteRunStore::new(&database)
        .executing_for_request(DispatchRequestId::new(request))
        .expect("the lookup serves")
        .expect("the run is durable");
    assert_eq!(restored.id().value(), id(&run));
    assert!(
        queued_ids(&core).is_empty(),
        "a claimed request leaves the queue"
    );
    let again = acknowledge(&core, request, version(&won["request"]), "legal-reloaded")
        .expect_err("one executing run per request");
    assert!(again.message.contains("already"), "{again:?}");
}

#[test]
fn dispatch_admission_refuses_a_reviewer_claim_after_cancellation() {
    let (mut h, ticket, submission) = common::review::prepared();
    register_authoring(&mut h.core, &h.database, h._dir.path());
    let review = h
        .core
        .command(
            "review.start",
            &json!({
                "mutation": mutation(0, "reviewer-review"),
                "ticket_id": ticket,
                "submission_id": submission["id"],
            }),
        )
        .expect("the review starts");
    let request = review["stages"][0]["slots"][0]["dispatch_request_id"]
        .as_u64()
        .expect("the reviewer slot holds a request");
    cancel(&h.core, ticket, "reviewer");
    let before = authoritative(&h.database_path);

    assert_refused(
        claim(&h.core, request, 1, "reviewer"),
        "cancelled and superseded are terminal; the Ticket accepts no further changes",
    );
    assert_eq!(authoritative(&h.database_path), before);
    assert!(
        queued_ids(&h.core).contains(&request),
        "the reviewer request stays queued"
    );
}

#[test]
fn dispatch_admission_infers_no_override_and_yields_only_to_an_explicit_one() {
    let h = admitting();
    let ticket = seated_ready_task(&h.core, "override");
    let request = enqueue(&h.core, ticket, "override");
    cancel(&h.core, ticket, "override");
    let before = authoritative(&h.database_path);

    assert_refused(
        claim(&h.core, request, 1, "override"),
        "cancelled and superseded are terminal; the Ticket accepts no further changes",
    );
    assert_eq!(authoritative(&h.database_path), before);
    assert_refused(
        claim(&h.core, request, 1, "override-again"),
        "cancelled and superseded are terminal; the Ticket accepts no further changes",
    );
    assert_eq!(
        authoritative(&h.database_path),
        before,
        "a repeated claim infers no override from its own refusal"
    );

    // Recovery's one audited way past the rule (DR-LC-10) is a named
    // operator's own command, never something admission reads into a
    // refused claim.
    let current = h
        .core
        .query("ticket.get", &json!({ "ticket_id": ticket }))
        .expect("the Ticket reads");
    h.core
        .command(
            "ticket.emergency.override",
            &json!({
                "mutation": mutation(version(&current), "override-recover"),
                "ticket_id": ticket,
                "to": "ready",
                "who": "Sid",
                "why": "The recovery audit cancelled the wrong Ticket.",
            }),
        )
        .expect("the override lands");

    let won = claim(&h.core, request, 1, "override-after").expect("the claim is attempted");
    assert_eq!(won["claimed"], json!(true), "{won}");
}

/// Author one Task in human mode: Sid executes the work himself, so
/// no agent run is dispatched for it.
fn human_task(core: &Core, key: &str) -> Value {
    core.command(
        "ticket.create",
        &json!({
            "mutation": mutation(0, format!("{key}-task")),
            "project_id": 1,
            "kind": "task",
            "priority": "normal",
            "title": "Sid performs this manual action",
            "subtype": "operational",
            "mode": "human",
            "completion": ["Sid completes the manual action."],
        }),
    )
    .expect("the human-mode Task authors")
}

/// The mode a stored Task carries, as admission reads it.
fn set_mode(database_path: &std::path::Path, ticket: u64, mode: &str) {
    rusqlite::Connection::open(database_path)
        .expect("the database reopens")
        .execute(
            "UPDATE tickets SET mode = ?2 WHERE id = ?1",
            rusqlite::params![ticket as i64, mode],
        )
        .expect("the stored mode changes");
}

const HUMAN_MODE_REFUSAL: &str =
    "the Task is human-mode; Sid executes it, so no implementer run is dispatched";

#[test]
fn dispatch_admission_refuses_an_implementer_claim_for_a_human_mode_task() {
    let h = admitting();
    let created = human_task(&h.core, "human");
    assert_eq!(created["mode"], json!("human"));
    let ready = make_ready(&h.core, &created, "human");
    assign_profile(&h.core, &ready, "human");
    seat(&h.core, id(&created), "human");
    let request = enqueue(&h.core, id(&created), "human");
    let before = authoritative(&h.database_path);

    assert_refused(claim(&h.core, request, 1, "human"), HUMAN_MODE_REFUSAL);
    assert_eq!(authoritative(&h.database_path), before);
    assert_eq!(queued_ids(&h.core), vec![request]);

    let ack = acknowledge(&h.core, request, 1, "human").expect_err("nothing was claimed");
    assert_eq!(ack.code, ErrorCode::InvalidRequest);
    assert_eq!(authoritative(&h.database_path), before);

    let (_database, core) = reopen(&h);
    assert_refused(
        claim(&core, request, 1, "human-reloaded"),
        HUMAN_MODE_REFUSAL,
    );
    assert_eq!(authoritative(&h.database_path), before);
}

/// The mode admission answers is the one the Ticket carries now, not
/// the one it carried when the request was enqueued or claimed. No
/// command edits a Task's mode, so the change arrives as a stored
/// fact — exactly the shape a stale queue record or a restored
/// database presents.
#[test]
fn dispatch_admission_rechecks_the_task_mode_after_enqueue_and_after_the_claim() {
    let h = admitting();
    let ticket = seated_ready_task(&h.core, "mode-change");
    let request = enqueue(&h.core, ticket, "mode-change");
    set_mode(&h.database_path, ticket, "human");
    let queued = authoritative(&h.database_path);

    assert_refused(
        claim(&h.core, request, 1, "mode-change"),
        HUMAN_MODE_REFUSAL,
    );
    assert_eq!(authoritative(&h.database_path), queued);

    set_mode(&h.database_path, ticket, "agent");
    let won = claim(&h.core, request, 1, "mode-change-agent").expect("the claim is attempted");
    assert_eq!(won["claimed"], json!(true), "{won}");
    set_mode(&h.database_path, ticket, "human");
    let claimed = authoritative(&h.database_path);

    assert_refused(
        acknowledge(
            &h.core,
            request,
            version(&won["request"]),
            "mode-change-ack",
        ),
        HUMAN_MODE_REFUSAL,
    );
    assert_eq!(authoritative(&h.database_path), claimed);
    assert_eq!(claimed.runs, 0, "no run is minted for human-mode work");
}

/// Attach `kind` to `spec` and carry it to ready, assigned, and
/// seated: the shape a graph member holds when its Dispatch Request
/// is enqueued.
fn spec_attached(h: &DispatchHarness, spec: u64, kind: &str, key: &str) -> Value {
    let mut payload = json!({
        "mutation": mutation(0, format!("{key}-create")),
        "project_id": 1,
        "kind": kind,
        "priority": "normal",
        "spec_id": spec,
        "title": "Execute only after graph approval",
    });
    if kind == "bug" {
        payload["actual_behaviour"] = json!("The member executed before graph approval.");
        payload["reporter_evidence"] = json!("The recovery audit probe transcript.");
    } else {
        payload["subtype"] = json!("operational");
        payload["mode"] = json!("agent");
        payload["completion"] = json!(["The approved graph is respected."]);
    }
    let created = h
        .core
        .command("ticket.create", &payload)
        .expect("the member authors");
    let prepared = if kind == "bug" {
        qualify(&h.core, &created, key)
    } else {
        created.clone()
    };
    let ready = park_and_unpark(&h.core, &prepared, key);
    assert_eq!(ready["state"], json!("ready"));
    assign_profile(&h.core, &ready, key);
    seat(&h.core, id(&created), key);
    created
}

/// One Implementation prerequisite of `spec`, claiming the Spec's
/// only story so the graph it joins is granular.
fn prerequisite(h: &DispatchHarness, spec: u64, key: &str) -> Value {
    h.core
        .command(
            "ticket.create",
            &json!({
                "mutation": mutation(0, format!("{key}-prerequisite")),
                "project_id": 1,
                "kind": "implementation",
                "priority": "normal",
                "spec_id": spec,
                "slice": "Produce the prerequisite",
                "criteria": [{
                    "outcome": "Claims fail closed.",
                    "stories": ["CORE-S1-US1"]
                }],
            }),
        )
        .expect("the prerequisite authors")
}

fn propose_graph(h: &DispatchHarness, spec: u64, from: u64, to: u64, key: &str) -> Value {
    let proposal = h
        .core
        .command(
            "ticket.graph.propose",
            &json!({
                "mutation": mutation(0, format!("{key}-propose")),
                "spec_id": spec,
                "spec_version": 1,
                "tickets": [from, to],
                "edges": [{ "from_ticket": from, "to_ticket": to }],
            }),
        )
        .expect("the graph proposes");
    assert_eq!(proposal["state"], json!("proposed"));
    proposal
}

const GRAPH_AWAITING_REFUSAL: &str = "the Ticket is named by a Ticket graph still awaiting approval; \
     a human approves the graph before execution";

/// KAN-T138-AC1, KAN-T138-AC3: graph participation, not Ticket kind,
/// decides whether the human approval gate still stands in the way. A
/// Bug or a Task named by a proposed graph executes no sooner than the
/// Implementation beside it.
#[test]
fn dispatch_admission_refuses_a_bug_or_task_named_by_an_unapproved_graph() {
    for kind in ["bug", "task"] {
        let h = admitting();
        let spec = approved_spec(&h.core);
        let ordered = prerequisite(&h, spec, kind);
        let candidate = spec_attached(&h, spec, kind, kind);
        propose_graph(&h, spec, id(&ordered), id(&candidate), kind);
        let request = enqueue(&h.core, id(&candidate), kind);
        let before = authoritative(&h.database_path);

        assert_refused(
            claim(&h.core, request, 1, &format!("{kind}-claim")),
            GRAPH_AWAITING_REFUSAL,
        );
        assert_eq!(authoritative(&h.database_path), before, "{kind}");
        assert_eq!(queued_ids(&h.core), vec![request], "{kind}");

        let ack = acknowledge(&h.core, request, 1, &format!("{kind}-ack"))
            .expect_err("nothing was claimed");
        assert_eq!(ack.code, ErrorCode::InvalidRequest, "{kind}");
        assert_eq!(authoritative(&h.database_path), before, "{kind}");
    }
}

/// KAN-T138-AC1: approval is not the whole gate. Once the human
/// approves the graph, the edge it installs holds the member back
/// until its prerequisite is done.
#[test]
fn dispatch_admission_holds_an_approved_graph_member_until_its_prerequisite_lands() {
    let h = admitting();
    let spec = approved_spec(&h.core);
    let ordered = prerequisite(&h, spec, "approved");
    let candidate = spec_attached(&h, spec, "task", "approved");
    let proposal = propose_graph(&h, spec, id(&ordered), id(&candidate), "approved");
    let request = enqueue(&h.core, id(&candidate), "approved");

    assert_refused(
        claim(&h.core, request, 1, "approved-before"),
        GRAPH_AWAITING_REFUSAL,
    );

    h.core
        .command(
            "ticket.graph.approve",
            &json!({
                "mutation": mutation(version(&proposal), "approved-approve"),
                "proposal_id": id(&proposal),
            }),
        )
        .expect("the human gate approves");
    let before = authoritative(&h.database_path);

    assert_refused(
        claim(&h.core, request, 1, "approved-blocked"),
        "the Ticket is held back by 1 unresolved dependencies or external blockers",
    );
    assert_eq!(
        authoritative(&h.database_path),
        before,
        "the approved but blocked member reserves nothing"
    );

    let landed = h
        .core
        .query("ticket.get", &json!({ "ticket_id": id(&ordered) }))
        .expect("the prerequisite reads");
    h.core
        .command(
            "ticket.emergency.override",
            &json!({
                "mutation": mutation(version(&landed), "approved-land"),
                "ticket_id": id(&ordered),
                "to": "done",
                "who": "Sid",
                "why": "The prerequisite landed outside this fixture's lifecycle path.",
            }),
        )
        .expect("the prerequisite lands");

    let won = claim(&h.core, request, 1, "approved-after").expect("the claim is attempted");
    assert_eq!(
        won["claimed"],
        json!(true),
        "the satisfied dependency admits the same queued request: {won}"
    );
}

/// KAN-T138-AC1: a Bug or Task that no graph names executes on its
/// own, Spec attachment and all. The approval gate answers
/// participation, never mere attachment.
#[test]
fn dispatch_admission_admits_a_spec_attached_bug_or_task_no_graph_names() {
    for kind in ["bug", "task"] {
        let h = admitting();
        let spec = approved_spec(&h.core);
        let candidate = spec_attached(&h, spec, kind, kind);
        let request = enqueue(&h.core, id(&candidate), kind);

        let won =
            claim(&h.core, request, 1, &format!("{kind}-claim")).expect("the claim is attempted");
        assert_eq!(won["claimed"], json!(true), "{kind}: {won}");
        let run = acknowledge(
            &h.core,
            request,
            version(&won["request"]),
            &format!("{kind}-ack"),
        )
        .expect("the run acknowledges");
        assert_eq!(run["status"], json!("executing"), "{kind}");
    }
}
