//! App gate for Dispatch Request refusals (KAN-T42 review): terminal
//! Tickets and archived Projects dispatch nothing — each refusal
//! creates no Dispatch Request and raises no Coordinator wake.

use kanban_dto::ErrorCode;
use serde_json::json;

mod common;

use common::{harness, insert_ticket, mutation};

#[test]
fn a_terminal_ticket_dispatches_no_request_and_wakes_nobody() {
    let harness = harness();
    let ticket = insert_ticket(&harness.database_path, 1, "normal");
    let conn = rusqlite::Connection::open(&harness.database_path).expect("the database reopens");
    conn.execute(
        "UPDATE tickets SET state = 'cancelled' WHERE id = ?1",
        rusqlite::params![ticket as i64],
    )
    .expect("the fixture Ticket is cancelled");

    let error = harness
        .core
        .command(
            "dispatch.request",
            &json!({
                "mutation": mutation(0, "key-cancelled"),
                "ticket_id": ticket,
            }),
        )
        .expect_err("a terminal Ticket is refused");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(
        error.message,
        "cancelled and superseded are terminal; the Ticket accepts no further changes"
    );
    let listed = harness
        .core
        .query("dispatch.queue", &json!({ "project_id": 1 }))
        .expect("the queue serves");
    assert_eq!(
        listed["requests"]
            .as_array()
            .expect("the requests are a list")
            .len(),
        0,
        "the refusal queues no Dispatch Request"
    );
    assert!(
        harness
            .wake
            .calls
            .lock()
            .expect("the wake log is sound")
            .is_empty(),
        "the refusal wakes no Coordinator"
    );
}

#[test]
fn an_archived_project_dispatches_no_request_and_wakes_nobody() {
    let harness = harness();
    let ticket = insert_ticket(&harness.database_path, 1, "normal");
    let conn = rusqlite::Connection::open(&harness.database_path).expect("the database reopens");
    conn.execute(
        "UPDATE projects SET archived = 1 WHERE id = 1",
        rusqlite::params![],
    )
    .expect("the fixture Project archives");

    let error = harness
        .core
        .command(
            "dispatch.request",
            &json!({
                "mutation": mutation(0, "key-archived"),
                "ticket_id": ticket,
            }),
        )
        .expect_err("an archived Project is refused");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(
        error.message,
        "archived is terminal; the Project accepts no further changes"
    );
    let listed = harness
        .core
        .query("dispatch.queue", &json!({ "project_id": 1 }))
        .expect("the queue serves");
    assert_eq!(
        listed["requests"]
            .as_array()
            .expect("the requests are a list")
            .len(),
        0,
        "the refusal queues no Dispatch Request"
    );
    assert!(
        harness
            .wake
            .calls
            .lock()
            .expect("the wake log is sound")
            .is_empty(),
        "the refusal wakes no Coordinator"
    );
}
