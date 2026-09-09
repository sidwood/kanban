//! Secrets are rejected before parsing, persistence, or error reflection.
use kanban_app::secrets::InstallationSecret;
use serde_json::json;
use std::sync::Arc;
#[path = "common/mod.rs"]
mod common;

#[test]
fn secret_exclusion_prevents_persisting_installation_credentials() {
    let mut h = common::harness();
    let secret = Arc::new(InstallationSecret::from_key(&[7; 32]));
    h.core.protect_installation_secret(secret.clone());
    let ticket = common::insert_ticket(&h.database_path, 1, "normal");
    common::assign_lane(&h.database_path, ticket);
    let outcome = h.core.command(
        "dispatch.request",
        &json!({
            "mutation": common::mutation(0, secret.expose()), "ticket_id": ticket
        }),
    );
    assert!(
        outcome.is_err(),
        "credential-bearing mutations must be refused before they reach SQLite"
    );
    assert!(!format!("{outcome:?}").contains(secret.expose()));
    let database = rusqlite::Connection::open(&h.database_path).unwrap();
    let count: i64 = database
        .query_row("SELECT count(*) FROM dispatch_requests", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn secret_exclusion_prevents_reflecting_credentials_in_errors() {
    let mut h = common::harness();
    let secret = Arc::new(InstallationSecret::from_key(&[8; 32]));
    h.core.protect_installation_secret(secret.clone());
    for response in [
        h.core.query(secret.expose(), &json!({})),
        h.core.command(secret.expose(), &json!({})),
        h.core
            .query("health.get", &json!({"unexpected": secret.expose()})),
    ] {
        assert!(response.is_err());
        assert!(!format!("{response:?}").contains(secret.expose()));
    }
    assert!(!format!("{secret:?}").contains(secret.expose()));
}

#[test]
fn secret_exclusion_refuses_credentials_inside_encoded_evidence() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let mut h = common::harness();
    let secret = Arc::new(InstallationSecret::from_key(&[13; 32]));
    h.core.protect_installation_secret(secret.clone());
    let attachments = h._dir.path().join("attachments");
    let ticket = common::insert_ticket(&h.database_path, 1, "normal");
    h.core
        .register_evidence(
            Arc::new(kanban_storage::SqliteEvidenceStore::new(
                &h.database,
                attachments.clone(),
            )),
            Arc::new(kanban_storage::SqliteProjectStore::new(&h.database)),
        )
        .unwrap();
    let mut bytes = vec![0xff, 0x00];
    bytes.extend_from_slice(secret.expose().as_bytes());
    bytes.push(0xfe);
    let response = h.core.command(
        "evidence.attach",
        &json!({
            "project_id": 1, "entity_kind": "ticket", "entity_id": ticket.to_string(),
            "evidence_kind": "managed_file", "content_base64": STANDARD.encode(bytes),
            "mutation": common::mutation(0, "credential-upload")
        }),
    );
    let error = response.expect_err("encoded uploads must not bypass credential exclusion");
    assert!(error.message.contains("credentials"));
    assert!(
        !attachments.exists(),
        "refusal precedes attachment file writes"
    );
    let count: i64 = rusqlite::Connection::open(&h.database_path)
        .unwrap()
        .query_row("SELECT count(*) FROM evidence_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
