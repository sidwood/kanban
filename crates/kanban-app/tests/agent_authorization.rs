//! Transport-independent authorization against real dispatch grants.
use kanban_domain::CapabilityId;
use kanban_storage::{SqliteEvidenceStore, SqliteProjectStore, SqliteSpecStore, SqliteTicketStore};
use serde_json::{Value, json};
use std::sync::Arc;
#[path = "common/mod.rs"]
mod common;

fn fixture() -> (common::DispatchHarness, u64, u64, CapabilityId) {
    fixture_with_acknowledgement(true)
}
fn fixture_with_acknowledgement(
    acknowledge: bool,
) -> (common::DispatchHarness, u64, u64, CapabilityId) {
    let mut h = common::harness();
    h.core.register_agent_authority(Arc::new(
        kanban_app::agent_authorization::RunAuthority::new(
            Arc::new(kanban_storage::SqliteCapabilityStore::new(&h.database)),
            Arc::new(SqliteTicketStore::new(&h.database)),
            Arc::new(kanban_storage::SqliteRunStore::new(&h.database)),
        ),
    ));
    h.core
        .register_tickets(
            Arc::new(SqliteTicketStore::new(&h.database)),
            Arc::new(SqliteProjectStore::new(&h.database)),
            Arc::new(SqliteSpecStore::new(&h.database)),
            Arc::new(SqliteEvidenceStore::new(
                &h.database,
                h._dir.path().join("attachments"),
            )),
        )
        .unwrap();
    let first = common::insert_ticket(&h.database_path, 1, "normal");
    let other = common::insert_ticket(&h.database_path, 2, "normal");
    common::assign_lane(&h.database_path, first);
    let request = h
        .core
        .command(
            "dispatch.request",
            &json!({"mutation":common::mutation(0,"dispatch"),"ticket_id":first}),
        )
        .unwrap();
    let claim = h
        .core
        .command(
            "dispatch.claim",
            &json!({"mutation":common::mutation(1,"claim"),"dispatch_request_id":request["id"]}),
        )
        .unwrap();
    if acknowledge {
        h.core
            .command(
                "run.acknowledge",
                &json!({"mutation":common::mutation(2,"run"),"dispatch_request_id":request["id"]}),
            )
            .unwrap();
    }
    let capability = CapabilityId::new(claim["capability"]["id"].as_u64().unwrap());
    (h, first, other, capability)
}

#[test]
fn capability_enforcement_reads_only_the_granted_ticket() {
    let (h, first, other, cap) = fixture();
    let own = h
        .core
        .agent_query(cap, "ticket.get", &json!({"ticket_id":first}))
        .unwrap();
    assert_eq!(own["id"], first);
    let failure = h
        .core
        .agent_query(cap, "ticket.get", &json!({"ticket_id":other}));
    assert!(
        failure.is_err(),
        "a permitted tool is not permission to read another Ticket"
    );
    let still_readable: Value = h
        .core
        .query("ticket.get", &json!({"ticket_id":other}))
        .unwrap();
    assert_eq!(
        still_readable["id"], other,
        "operator authority remains separate"
    );
}

#[test]
fn capability_enforcement_scopes_comments_before_replay() {
    let (mut h, first, other, cap) = fixture();
    h.core
        .register_comments(
            Arc::new(kanban_storage::SqliteCommentStore::new(&h.database)),
            Arc::new(SqliteProjectStore::new(&h.database)),
        )
        .unwrap();
    let request = json!({"mutation":common::mutation(0,"agent-comment"),"project_id":1,
        "target":{"kind":"ticket","id":first.to_string()},"text":"Implementation evidence is ready."});
    let written = h
        .core
        .agent_command(cap, "comment.create", &request)
        .unwrap();
    assert_eq!(
        written,
        h.core
            .agent_command(cap, "comment.create", &request)
            .unwrap()
    );
    for (project, kind, id) in [
        (1, "ticket", other.to_string()),
        (2, "ticket", first.to_string()),
        (1, "project", "1".to_owned()),
    ] {
        let mut outside = request.clone();
        outside["mutation"] = common::mutation(0, format!("outside-{project}-{kind}-{id}"));
        outside["project_id"] = json!(project);
        outside["target"] = json!({"kind":kind,"id":id});
        assert!(
            h.core
                .agent_command(cap, "comment.create", &outside)
                .is_err()
        );
    }
    use kanban_app::CapabilityStore;
    kanban_storage::SqliteCapabilityStore::new(&h.database)
        .settle(
            cap,
            99,
            kanban_app::TimelineEnvelope::global(
                kanban_dto::TimelineEventKind::Transition,
                None,
                json!({"action":"settled"}),
            ),
        )
        .unwrap();
    assert!(
        h.core
            .agent_command(cap, "comment.create", &request)
            .is_err(),
        "expired authority cannot replay a prior success"
    );
    assert!(
        h.core
            .agent_query(cap, "ticket.get", &json!({"ticket_id":first}))
            .is_err()
    );
    assert_eq!(
        written,
        h.core.command("comment.create", &request).unwrap(),
        "the stored operator outcome is preserved"
    );
}

#[test]
fn capability_enforcement_requires_an_executing_run() {
    let (h, ticket, _, cap) = fixture_with_acknowledgement(false);
    assert!(
        h.core
            .agent_query(cap, "ticket.get", &json!({"ticket_id":ticket}))
            .is_err(),
        "a minted grant is not an acknowledged run"
    );
}

#[test]
fn capability_enforcement_attaches_and_lists_only_ticket_evidence() {
    let (mut h, ticket, other, cap) = fixture();
    h.core
        .register_evidence(
            Arc::new(SqliteEvidenceStore::new(
                &h.database,
                h._dir.path().join("attachments"),
            )),
            Arc::new(SqliteProjectStore::new(&h.database)),
        )
        .unwrap();
    let input = json!({"mutation":common::mutation(0,"agent-evidence"),"project_id":1,
        "entity_kind":"ticket","entity_id":ticket.to_string(),"evidence_kind":"repository",
        "relative_path":"src/lib.rs","commit_identity":"a".repeat(40)});
    let added = h
        .core
        .agent_command(cap, "evidence.attach", &input)
        .unwrap();
    let query = json!({"project_id":1,"entity_kind":"ticket","entity_id":ticket.to_string()});
    let own = h.core.agent_query(cap, "evidence.list", &query).unwrap();
    assert_eq!(own["evidence"], json!([added]));
    for q in [
        json!({"project_id":1}),
        json!({"project_id":2,"entity_kind":"ticket","entity_id":ticket.to_string()}),
        json!({"project_id":1,"entity_kind":"ticket","entity_id":other.to_string()}),
    ] {
        assert!(h.core.agent_query(cap, "evidence.list", &q).is_err());
    }
    let mut outside = input;
    outside["entity_id"] = json!(other.to_string());
    outside["mutation"] = common::mutation(0, "outside-evidence");
    assert!(
        h.core
            .agent_command(cap, "evidence.attach", &outside)
            .is_err()
    );
}

#[test]
fn capability_enforcement_binds_submission_to_authenticated_identity() {
    let (h, _, _, cap) = fixture();
    let run = h.core.query("run.list", &json!({"project_id":1})).unwrap()["runs"][0]["id"].clone();
    let request = json!({"mutation":common::mutation(1,"agent-result"),"run_id":run,"capability_id":cap.value(),
        "result":{"kind":"implementation","tip":"b".repeat(40),"summary":"Verified result"}});
    for key in ["capability_id", "run_id"] {
        let mut forged = request.clone();
        forged[key] = json!(999);
        forged["mutation"] = common::mutation(1, format!("forged-{key}"));
        assert!(
            h.core
                .agent_command(cap, "submission.submit", &forged)
                .is_err()
        );
    }
    let mut reviewer = request.clone();
    reviewer["result"] =
        json!({"kind":"review","tip":"b".repeat(40),"summary":"Self approval","approve":true});
    assert!(
        h.core
            .agent_command(cap, "submission.submit", &reviewer)
            .is_err()
    );
    let submitted = h
        .core
        .agent_command(cap, "submission.submit", &request)
        .unwrap();
    assert_eq!(submitted["capability_id"], cap.value());
    assert_eq!(submitted["run_id"], run);
}

fn authority(h: &common::DispatchHarness) -> kanban_app::agent_authorization::RunAuthority {
    kanban_app::agent_authorization::RunAuthority::new(
        Arc::new(kanban_storage::SqliteCapabilityStore::new(&h.database)),
        Arc::new(SqliteTicketStore::new(&h.database)),
        Arc::new(kanban_storage::SqliteRunStore::new(&h.database)),
    )
}
#[test]
fn capability_enforcement_requires_an_exact_timeline_subject() {
    let (h, ticket, other, cap) = fixture();
    let authority = authority(&h);
    let own = json!({"scope":{"project":1},"entity":{"kind":"ticket","id":ticket.to_string()}});
    authority.authorize(cap, "timeline.query", &own).unwrap();
    for query in [
        json!({"scope":{"project":1}}),
        json!({"scope":"global"}),
        json!({"scope":{"project":2},"entity":{"kind":"ticket","id":ticket.to_string()}}),
        json!({"scope":{"project":1},"entity":{"kind":"ticket","id":other.to_string()}}),
    ] {
        assert!(authority.authorize(cap, "timeline.query", &query).is_err());
    }
}

#[test]
fn capability_enforcement_separates_criterion_implementation_and_review() {
    let (h, ticket, other, cap) = fixture();
    let implementer = authority(&h);
    let attach = json!({"mutation":common::mutation(0,"binding"),"ticket_id":ticket,"criterion_index":0,"evidence_id":1,"tip":"a".repeat(40)});
    let review = json!({"mutation":common::mutation(0,"review-binding"),"ticket_id":ticket,"criterion_index":0,"review":"validated"});
    let satisfy = json!({"mutation":common::mutation(0,"satisfy"),"ticket_id":ticket,"criterion_index":0,"tip":"a".repeat(40)});
    implementer
        .authorize(cap, "criterion.evidence.attach", &attach)
        .unwrap();
    assert!(
        implementer
            .authorize(cap, "criterion.evidence.review", &review)
            .is_err()
    );
    assert!(
        implementer
            .authorize(cap, "criterion.satisfy", &satisfy)
            .is_err()
    );
    let (mut h, ticket, submission) = common::review::prepared();
    let review_run=h.core.command("review.start",&json!({"mutation":common::mutation(0,"start"),"ticket_id":ticket,"submission_id":submission["id"]})).unwrap();
    let slot = &review_run["stages"][0]["slots"][0];
    let claim=h.core.command("dispatch.claim",&json!({"mutation":common::mutation(1,"review-claim"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
    h.core.command("run.acknowledge",&json!({"mutation":common::mutation(2,"review-ack"),"dispatch_request_id":slot["dispatch_request_id"]})).unwrap();
    let cap = CapabilityId::new(claim["capability"]["id"].as_u64().unwrap());
    let reviewer = authority(&h);
    h.core.register_agent_authority(Arc::new(authority(&h)));
    reviewer
        .authorize(cap, "criterion.evidence.review", &review)
        .unwrap();
    reviewer
        .authorize(cap, "criterion.satisfy", &satisfy)
        .unwrap();
    assert!(
        reviewer
            .authorize(cap, "criterion.evidence.attach", &attach)
            .is_err()
    );
    let mut outside = review;
    outside["ticket_id"] = json!(other);
    assert!(
        reviewer
            .authorize(cap, "criterion.evidence.review", &outside)
            .is_err()
    );
    assert!(
        !h.core
            .agent_operations(cap)
            .unwrap()
            .iter()
            .any(|name| name == "criterion.evidence.attach")
    );
}

#[test]
fn capability_enforcement_resolves_only_the_pinned_spec_content() {
    let (mut h, ticket, _, cap) = fixture();
    h.core
        .register_specs(
            Arc::new(SqliteSpecStore::new(&h.database)),
            Arc::new(SqliteProjectStore::new(&h.database)),
            Arc::new(kanban_storage::SqlitePlanStore::new(&h.database)),
        )
        .unwrap();
    let db = rusqlite::Connection::open(&h.database_path).unwrap();
    db.execute(
        "INSERT INTO specs(project_id,number,execution,version) VALUES (1,1,'active',1)",
        [],
    )
    .unwrap();
    let spec = db.last_insert_rowid();
    for number in 1..=2 {
        db.execute("INSERT INTO spec_versions(spec_id,number,state,name,short_description,problem_statement,solution,user_stories,implementation_decisions,testing_decisions,out_of_scope,further_notes) VALUES (?1,?2,'draft',?3,'Scope','Problem','Solution','Story','Decision','Test','None','None')",rusqlite::params![spec,number,format!("Content {number}")]).unwrap();
    }
    db.execute(
        "UPDATE tickets SET spec_id=?1,pinned_version=1 WHERE id=?2",
        rusqlite::params![spec, ticket as i64],
    )
    .unwrap();
    let pinned = h
        .core
        .agent_query(cap, "spec.version.get", &json!({"spec_id":spec,"number":1}))
        .unwrap();
    assert_eq!(pinned["number"], 1);
    assert!(
        h.core
            .agent_query(cap, "spec.version.get", &json!({"spec_id":spec,"number":2}))
            .is_err()
    );
    assert!(
        h.core
            .agent_query(cap, "spec.get", &json!({"spec_id":spec+1}))
            .is_err()
    );
    let projected = h
        .core
        .agent_query(cap, "spec.get", &json!({"spec_id":spec}))
        .unwrap();
    assert_eq!(projected["versions"], json!([pinned]));
    assert_eq!(projected["spec"]["name"], "Content 1");
    assert_eq!(
        h.core.query("spec.get", &json!({"spec_id":spec})).unwrap()["versions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn capability_enforcement_allows_only_the_declared_health_query_shape() {
    let (h, _, _, cap) = fixture();
    let access = authority(&h);
    access.authorize(cap, "health.get", &json!({})).unwrap();
    assert!(
        access
            .authorize(cap, "health.get", &json!({"capability_id":999}))
            .is_err()
    );
}

#[test]
fn capability_enforcement_rechecks_expiry_inside_the_write_span() {
    use kanban_app::CapabilityStore;
    struct ExpireAfterRead(
        kanban_storage::SqliteCapabilityStore,
        std::sync::atomic::AtomicBool,
    );
    impl CapabilityStore for ExpireAfterRead {
        fn find(
            &self,
            id: CapabilityId,
        ) -> Result<Option<kanban_domain::Capability>, kanban_dto::ApiError> {
            let observed = self.0.find(id)?;
            if !self.1.swap(true, std::sync::atomic::Ordering::SeqCst) {
                self.0.settle(
                    id,
                    99,
                    kanban_app::TimelineEnvelope::global(
                        kanban_dto::TimelineEventKind::Transition,
                        None,
                        json!({"action":"settled"}),
                    ),
                )?;
            }
            Ok(observed)
        }
        fn settle(
            &self,
            id: CapabilityId,
            at: u64,
            envelope: kanban_app::TimelineEnvelope,
        ) -> Result<kanban_domain::Capability, kanban_dto::ApiError> {
            self.0.settle(id, at, envelope)
        }
    }
    let (mut h, ticket, _, cap) = fixture();
    h.core
        .register_comments(
            Arc::new(kanban_storage::SqliteCommentStore::new(&h.database)),
            Arc::new(SqliteProjectStore::new(&h.database)),
        )
        .unwrap();
    h.core.register_agent_authority(Arc::new(
        kanban_app::agent_authorization::RunAuthority::new(
            Arc::new(ExpireAfterRead(
                kanban_storage::SqliteCapabilityStore::new(&h.database),
                std::sync::atomic::AtomicBool::new(false),
            )),
            Arc::new(SqliteTicketStore::new(&h.database)),
            Arc::new(kanban_storage::SqliteRunStore::new(&h.database)),
        ),
    ));
    let input = json!({"mutation":common::mutation(0,"expiry-race"),"project_id":1,"target":{"kind":"ticket","id":ticket.to_string()},"text":"No late write"});
    assert!(
        h.core.agent_command(cap, "comment.create", &input).is_err(),
        "expiry between initial check and mutation must prevent the write"
    );
    let count: i64 = rusqlite::Connection::open(&h.database_path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM comments", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
