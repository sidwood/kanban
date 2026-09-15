//! Every committed criterion-binding change announces itself, so a
//! mounted surface showing completion progress can read it again
//! (KAN-T137-AC7, KAN-S2-US1, KAN-S5-US7). A refused change announces
//! nothing: the mutation is discarded and there is nothing to say.

mod common;

use std::sync::{Arc, Mutex};

use common::{mutation, seed_project_profile};
use kanban_app::dispatch::Core;
use kanban_app::events::EventSink;
use kanban_dto::{LiveEventName, event_descriptor};
use kanban_storage::{
    AllowAllMigrations, Database, SqliteCriterionBindingStore, SqliteEvidenceStore,
    SqlitePlanStore, SqliteProjectStore, SqliteSpecStore, SqliteTicketStore,
};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The wire name a criterion-binding change is announced under.
const CHANGED: &str = "criterion.binding.changed";

#[derive(Debug, Default)]
struct RecordingSink {
    events: Mutex<Vec<(String, Value)>>,
}

impl RecordingSink {
    fn announced(&self) -> Vec<(String, Value)> {
        self.events
            .lock()
            .expect("the recorder lock is sound")
            .clone()
    }

    fn changes(&self) -> Vec<Value> {
        self.announced()
            .into_iter()
            .filter(|(name, _)| name == CHANGED)
            .map(|(_, payload)| payload)
            .collect()
    }

    fn forget(&self) {
        self.events
            .lock()
            .expect("the recorder lock is sound")
            .clear();
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, event_type: &str, payload: Value) {
        self.events
            .lock()
            .expect("the recorder lock is sound")
            .push((event_type.to_owned(), payload));
    }
}

struct Harness {
    _dir: TempDir,
    core: Core,
    sink: Arc<RecordingSink>,
    database_path: std::path::PathBuf,
}

/// A Core over a scratch SQLite database, publishing to a sink this
/// test can read: the same registration path the service wires, so
/// what the handlers announce is what a subscriber would receive.
fn wired() -> Harness {
    let dir = TempDir::new().expect("a scratch directory is available");
    let path = dir.path().join("kanban.sqlite");
    let mut database = Database::open(&path).expect("the database opens");
    database
        .migrate(&AllowAllMigrations)
        .expect("the migrations apply");
    seed_project_profile(&database);
    let attachments = dir.path().join("attachments");
    std::fs::create_dir_all(&attachments).expect("the attachment root is created");

    let sink = Arc::new(RecordingSink::default());
    let (mut core, _) = common::core_over_with_events(&database, sink.clone());
    core.register_plans(
        Arc::new(SqlitePlanStore::new(&database)),
        Arc::new(SqliteProjectStore::new(&database)),
        Arc::new(SqliteSpecStore::new(&database)),
    )
    .expect("the plan operations register");
    core.register_specs(
        Arc::new(SqliteSpecStore::new(&database)),
        Arc::new(SqliteProjectStore::new(&database)),
        Arc::new(SqlitePlanStore::new(&database)),
    )
    .expect("the spec operations register");
    let evidence = Arc::new(SqliteEvidenceStore::new(&database, attachments.clone()));
    core.register_tickets(
        Arc::new(SqliteTicketStore::new(&database)),
        Arc::new(SqliteProjectStore::new(&database)),
        Arc::new(SqliteSpecStore::new(&database)),
        Arc::new(SqliteEvidenceStore::new(&database, attachments)),
    )
    .expect("the ticket operations register");
    core.register_evidence(
        evidence.clone(),
        Arc::new(SqliteProjectStore::new(&database)),
    )
    .expect("the evidence operations register");
    core.register_criterion_bindings(
        Arc::new(SqliteCriterionBindingStore::new(&database)),
        Arc::new(SqliteTicketStore::new(&database)),
        evidence,
        Arc::new(kanban_storage::SqliteReviewExecutionStore::new(&database)),
    )
    .expect("the criterion operations register");
    Harness {
        _dir: dir,
        core,
        sink,
        database_path: path,
    }
}

fn implementation_ticket(core: &Core) -> u64 {
    let spec = core
        .command(
            "spec.create",
            &json!({
                "mutation": mutation(0, "spec"),
                "project_id": 1,
                "content": {
                    "name": "Registration",
                    "short_description": "Versioned Plan graphs of Specs",
                    "problem_statement": "Planning must survive change without losing truth.",
                    "solution": "Immutable approved versions.",
                    "user_stories": "KAN-S3-US4",
                    "implementation_decisions": "Supersession is explicit.",
                    "testing_decisions": "Domain tests prove immutability.",
                    "out_of_scope": "The Ticket graph proposal.",
                    "further_notes": "None",
                },
            }),
        )
        .expect("the Spec authors");
    let created = core
        .command(
            "ticket.create",
            &json!({
                "mutation": mutation(0, "ticket"),
                "project_id": 1,
                "kind": "implementation",
                "priority": "normal",
                "spec_id": spec["id"],
                "slice": "Bind evidence to criteria",
                "criteria": [{
                    "outcome": "Reviewers validate evidence at one tip.",
                    "stories": ["CORE-S1-US1"]
                }],
            }),
        )
        .expect("the Ticket authors");
    created["id"].as_u64().expect("the identity is a number")
}

fn task_ticket(core: &Core) -> u64 {
    let created = core
        .command(
            "ticket.create",
            &json!({
                "mutation": mutation(0, "task"),
                "project_id": 1,
                "kind": "task",
                "priority": "normal",
                "title": "Archive the register",
                "subtype": "administrative",
                "mode": "human",
                "completion": ["The old register is archived and restorable."],
            }),
        )
        .expect("the Task authors");
    created["id"].as_u64().expect("the identity is a number")
}

fn attach_repository(core: &Core, ticket: u64, key: &str) -> u64 {
    let item = core
        .command(
            "evidence.attach",
            &json!({
                "mutation": mutation(0, key),
                "project_id": 1,
                "entity_kind": "ticket",
                "entity_id": ticket.to_string(),
                "evidence_kind": "repository",
                "relative_path": "crates/kanban-app/src/evidence.rs",
                "commit_identity": "a".repeat(40),
            }),
        )
        .expect("evidence attaches");
    item["id"].as_u64().expect("the identity is a number")
}

#[test]
fn criterion_binding_changes_are_a_catalogued_event() {
    let name = LiveEventName::parse(CHANGED).expect("the catalogue carries the change event");
    let descriptor = event_descriptor(name);

    assert_eq!(descriptor.name.as_str(), CHANGED);
    assert_eq!(
        descriptor.payload_schema, "CriterionBindingRecord",
        "the payload is the record the progress is counted from"
    );
}

#[test]
fn attaching_evidence_announces_the_binding_it_committed() {
    let h = wired();
    let ticket = implementation_ticket(&h.core);
    let evidence = attach_repository(&h.core, ticket, "proof");
    h.sink.forget();

    h.core
        .command(
            "criterion.evidence.attach",
            &json!({
                "mutation": mutation(0, "bind"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "evidence_id": evidence,
                "tip": "a".repeat(40),
            }),
        )
        .expect("the evidence binds");

    let changes = h.sink.changes();
    assert_eq!(changes.len(), 1, "one binding was written: {changes:?}");
    assert_eq!(changes[0]["ticket_id"], ticket);
    assert_eq!(changes[0]["criterion_index"], 0);
    assert_eq!(changes[0]["kind"], "acceptance");
    assert_eq!(changes[0]["review"], "pending");
    assert_eq!(changes[0]["satisfied"], false);
}

#[test]
fn reviewing_satisfying_and_invalidating_each_announce_their_change() {
    let h = wired();
    let (ticket, submission) = common::review::prepare_on(&h.core, &h.database_path);
    let evidence = attach_repository(&h.core, ticket, "proof");
    h.core
        .command(
            "criterion.evidence.attach",
            &json!({
                "mutation": mutation(0, "bind"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "evidence_id": evidence,
                "tip": "a".repeat(40),
            }),
        )
        .expect("the evidence binds");

    h.sink.forget();
    h.core
        .command(
            "criterion.evidence.review",
            &json!({
                "mutation": mutation(0, "validate"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "review": "validated",
            }),
        )
        .expect("the reviewer validates the evidence");
    let reviewed = h.sink.changes();
    assert_eq!(reviewed.len(), 1, "the review announced itself");
    assert_eq!(reviewed[0]["review"], "validated");
    assert_eq!(reviewed[0]["ticket_id"], ticket);

    let review = common::review::start(&h.core, ticket, &submission, "review-start");
    common::review::approve_required_stage(&h.core, &review, "review-approve");

    h.sink.forget();
    h.core
        .command(
            "criterion.satisfy",
            &json!({
                "mutation": mutation(0, "approve"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "tip": "a".repeat(40),
            }),
        )
        .expect("the approved tip satisfies the criterion");
    let satisfied = h.sink.changes();
    assert_eq!(satisfied.len(), 1, "the satisfaction announced itself");
    assert_eq!(satisfied[0]["satisfied"], true);

    h.sink.forget();
    h.core
        .command(
            "criterion.invalidate",
            &json!({
                "mutation": mutation(0, "changed"),
                "ticket_id": ticket,
                "observed_tip": "b".repeat(40),
            }),
        )
        .expect("a content change invalidates what it reached");
    let invalidated = h.sink.changes();
    assert_eq!(
        invalidated.len(),
        1,
        "every binding the invalidation wrote announced itself"
    );
    assert_eq!(invalidated[0]["void"], true);
    assert_eq!(invalidated[0]["satisfied"], false);
}

#[test]
fn completing_a_task_criterion_announces_its_binding() {
    let h = wired();
    let ticket = task_ticket(&h.core);
    h.sink.forget();

    h.core
        .command(
            "criterion.complete",
            &json!({
                "mutation": mutation(0, "done"),
                "ticket_id": ticket,
                "criterion_index": 0,
            }),
        )
        .expect("a human completes a Task criterion directly");

    let changes = h.sink.changes();
    assert_eq!(changes.len(), 1, "the completion announced itself");
    assert_eq!(changes[0]["ticket_id"], ticket);
    assert_eq!(changes[0]["kind"], "task");
    assert_eq!(changes[0]["satisfied"], true);
}

#[test]
fn a_refused_criterion_change_announces_nothing() {
    let h = wired();
    let ticket = implementation_ticket(&h.core);
    let evidence = attach_repository(&h.core, ticket, "proof");
    h.core
        .command(
            "criterion.evidence.attach",
            &json!({
                "mutation": mutation(0, "bind"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "evidence_id": evidence,
                "tip": "a".repeat(40),
            }),
        )
        .expect("the evidence binds");
    h.sink.forget();

    // Satisfaction before the reviewer has validated the evidence.
    h.core
        .command(
            "criterion.satisfy",
            &json!({
                "mutation": mutation(0, "too-soon"),
                "ticket_id": ticket,
                "criterion_index": 0,
                "tip": "a".repeat(40),
            }),
        )
        .expect_err("unvalidated evidence cannot satisfy a criterion");
    // A criterion the Ticket does not carry.
    h.core
        .command(
            "criterion.evidence.attach",
            &json!({
                "mutation": mutation(0, "unknown-index"),
                "ticket_id": ticket,
                "criterion_index": 9,
                "evidence_id": evidence,
                "tip": "a".repeat(40),
            }),
        )
        .expect_err("an unknown criterion index is refused");
    // Direct completion of an Acceptance Criterion.
    h.core
        .command(
            "criterion.complete",
            &json!({
                "mutation": mutation(0, "not-a-task"),
                "ticket_id": ticket,
                "criterion_index": 0,
            }),
        )
        .expect_err("only humans complete Task criteria directly");

    assert_eq!(
        h.sink.announced(),
        Vec::new(),
        "a mutation that was discarded has nothing to announce"
    );
}
