//! The invalid-profile loop of the planning diagnostics (KAN-T38,
//! KAN-S7-US4, DR-PS-18): the stored execution profile catalogue
//! feeds the diagnostics seam KAN-T16 left open, and the graph
//! approval gate KAN-T23 owns refuses a graph whose Tickets carry
//! assignments nothing can execute. These tests drive the application
//! core the way the service wires it — Plans, Specs, Tickets,
//! proposals, and catalogue rows over in-memory stores — with
//! `plan.diagnostics` reading the stored catalogue.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::catalog::exposed_operations;
use crate::diagnostics::StoredProfileCatalogue;
use crate::dispatch::Core;
use crate::events::NoopEventSink;
use crate::graph_proposal::testing::{MemoryGraphDependencies, MemoryGraphProposals};
use crate::mutation::MemoryIdempotencyStore;
use crate::plan::testing::{MemoryPlans, MemoryProjects};
use crate::profile::testing::MemoryProfiles;
use crate::spec::testing::MemorySpecs;
use crate::ticket::TicketStore;
use crate::ticket::testing::{MemoryTicketEvidence, MemoryTickets};

/// A core with the Plan, Spec, Ticket, and catalogue operations wired
/// to in-memory stores over one active Project, its planning
/// diagnostics reading the stored profile catalogue.
struct Harness {
    core: Core,
    tickets: Arc<MemoryTickets>,
}

/// The harness the validation tests drive.
fn harness() -> Harness {
    let projects = Arc::new(MemoryProjects::default());
    projects.seed(crate::plan::testing::active_project(
        1,
        "CORE",
        kanban_domain::ProjectCounters::restore(0, 0, 0),
    ));
    let plans = Arc::new(MemoryPlans::sharing(projects.clone()));
    let specs = Arc::new(MemorySpecs::sharing(projects.clone()));
    let tickets = Arc::new(MemoryTickets::sharing(projects.clone()));
    let profiles = Arc::new(MemoryProfiles::default());
    let mut core = Core::new(
        exposed_operations(),
        Arc::new(MemoryIdempotencyStore::new()),
        Arc::new(NoopEventSink),
    );
    core.register_plans(plans.clone(), projects.clone(), specs.clone())
        .expect("the plan operations register");
    core.register_plan_diagnostics(
        plans.clone(),
        projects.clone(),
        specs.clone(),
        Arc::new(StoredProfileCatalogue::new(
            profiles.clone(),
            tickets.clone(),
            specs.clone(),
        )),
    )
    .expect("the diagnostics read the stored catalogue");
    core.register_specs(specs.clone(), projects.clone(), plans.clone())
        .expect("the spec operations register");
    core.register_tickets(
        tickets.clone(),
        projects.clone(),
        specs.clone(),
        Arc::new(MemoryTicketEvidence::default()),
    )
    .expect("the ticket operations register");
    let dependencies = Arc::new(MemoryGraphDependencies::sharing(tickets.clone()));
    core.register_dependencies(dependencies.clone(), tickets.clone(), projects.clone())
        .expect("the dependency operations register");
    core.register_graph_proposals(
        Arc::new(MemoryGraphProposals::sharing(
            tickets.clone(),
            dependencies.clone(),
        )),
        dependencies,
        tickets.clone(),
        specs.clone(),
        projects.clone(),
        profiles.clone(),
    )
    .expect("the graph proposal operations register");
    core.register_profiles(profiles, tickets.clone(), projects)
        .expect("the profile operations register");
    Harness { core, tickets }
}

/// The PRD wire content with a story section naming `user_stories`.
fn content(name: &str, user_stories: &str) -> Value {
    json!({
        "name": name,
        "short_description": "Versioned Plan graphs of Specs",
        "problem_statement": "Planning must survive change without losing truth.",
        "solution": "Enforced story coverage.",
        "user_stories": user_stories,
        "implementation_decisions": "The gate is consumed by graph approval.",
        "testing_decisions": "Application tests prove the gate refuses gaps.",
        "out_of_scope": "The Ticket graph proposal.",
        "further_notes": "None",
    })
}

/// The story section every fixture Spec claims.
const STORIES: &str = "\
- CORE-S1-US1: As an operator, I want linked criteria.
";

/// Author one Spec on the seeded CORE Project with the story section
/// given, returning its identity and minted number.
fn spec_with_stories(core: &Core, user_stories: &str, key: &str) -> (u64, u64) {
    let created = core
        .command(
            "spec.create",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": key },
                "project_id": 1,
                "content": content("Registration", user_stories),
            }),
        )
        .expect("the Spec authors");
    (
        created["id"].as_u64().expect("the identity is a number"),
        created["number"]
            .as_u64()
            .expect("the minted number is a number"),
    )
}

/// Author one Spec claiming the fixture story and approve its version
/// one, returning the identity a Ticket graph proposes against.
fn approved_spec(core: &Core, key: &str) -> u64 {
    let (id, _) = spec_with_stories(core, STORIES, key);
    core.command(
        "spec.version.approve",
        &json!({
            "mutation": { "optimistic_version": 1, "idempotency_key": format!("{key}-approve") },
            "spec_id": id,
        }),
    )
    .expect("the draft approves");
    id
}

/// Create one Implementation Ticket attached to the Spec `spec` holds,
/// claiming its `story`, returning its identity.
fn implementation(core: &Core, spec: u64, story: &str, key: &str) -> u64 {
    let created = core
        .command(
            "ticket.create",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": key },
                "project_id": 1,
                "kind": "implementation",
                "priority": "normal",
                "spec_id": spec,
                "slice": "Registration creates Projects end to end",
                "criteria": [
                    { "outcome": "Graphs record completely.", "stories": [story] }
                ],
            }),
        )
        .expect("the Ticket creates");
    created["id"].as_u64().expect("the identity is a number")
}

/// Define one active catalogue entry named `name`.
fn define(core: &Core, name: &str, key: &str) {
    core.command(
        "profile.define",
        &json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "name": name,
            "harness": "claude-code",
            "model": "opus",
            "effort": "high",
            "usage_pool": "operator",
        }),
    )
    .expect("the entry defines");
}

/// Assign `ticket` to the profile `name` references, at `version`.
fn assign(core: &Core, ticket: u64, name: &str, version: u64, key: &str) {
    core.command(
        "ticket.assign",
        &json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "ticket_id": ticket,
            "profile": name,
        }),
    )
    .expect("the assignment applies");
}

/// Retire the entry `name`, at `version`.
fn retire(core: &Core, name: &str, version: u64, key: &str) {
    core.command(
        "profile.retire",
        &json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "name": name,
        }),
    )
    .expect("the entry retires");
}

/// A draft Plan holding the Specs given, returning its identity and
/// aggregate version.
fn plan_over(core: &Core, specs: &[u64], key: &str) -> (u64, u64) {
    let created = core
        .command(
            "plan.create",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": format!("{key}-create") },
                "project_id": 1,
            }),
        )
        .expect("the Plan creates");
    let id = created["id"].as_u64().expect("the identity is a number");
    let mut version = created["version"]
        .as_u64()
        .expect("the version is a number");
    for spec in specs {
        let response = core
            .command(
                "plan.spec.add",
                &json!({
                    "mutation": {
                        "optimistic_version": version,
                        "idempotency_key": format!("{key}-add-{spec}"),
                    },
                    "plan_id": id,
                    "spec_number": spec,
                }),
            )
            .expect("the Spec joins");
        version = response["version"]
            .as_u64()
            .expect("the version is a number");
    }
    (id, version)
}

/// The diagnostics of one Plan's working shape.
fn diagnose(core: &Core, plan: u64) -> Value {
    core.query(
        "plan.diagnostics",
        &json!({ "plan_id": plan, "version": null }),
    )
    .expect("the diagnostics serve")
}

/// The diagnostics of one Plan's frozen version `number`.
fn diagnose_version(core: &Core, plan: u64, number: u64) -> Value {
    core.query(
        "plan.diagnostics",
        &json!({ "plan_id": plan, "version": number }),
    )
    .expect("the frozen diagnostics serve")
}

/// Move one stored Ticket's assignment to the profile `name`
/// references, planting the row directly: the write path's own gate
/// refuses a name the catalogue does not carry, and the diagnostics
/// must still report one a restored or foreign row holds.
fn plant_reference(store: &MemoryTickets, ticket: u64, name: &str) {
    let standing = store
        .find(kanban_domain::TicketId::new(ticket))
        .expect("the find serves")
        .expect("the Ticket stands");
    let planted = kanban_domain::Ticket::restore(
        standing.id(),
        standing.project(),
        standing.number(),
        standing.priority(),
        standing.state(),
        standing.body().clone(),
        standing.predecessor(),
        Some(kanban_domain::ProfileName::new(name).expect("the fixture name validates")),
        standing.pinned_version(),
        standing.version() + 1,
    );
    store
        .replace_pinned(planted)
        .expect("the row carries the planted reference");
}

#[cfg(test)]
mod invalid_profile_diagnostics {
    use super::*;

    #[test]
    fn a_reference_an_active_entry_answers_stays_clear() {
        let harness = harness();
        let (spec, number) = spec_with_stories(&harness.core, STORIES, "key-spec");
        let ticket = implementation(&harness.core, spec, "CORE-S1-US1", "key-ticket");
        define(&harness.core, "standard", "key-profile");
        assign(&harness.core, ticket, "standard", 1, "key-assign");
        let (plan, _) = plan_over(&harness.core, &[number], "key-plan");

        assert_eq!(
            diagnose(&harness.core, plan),
            json!({
                "cycles": [],
                "coverage_gaps": [
                    {
                        "spec_number": 1,
                        "uncovered": ["CORE-S1-US1"],
                        "claims_no_stories": false,
                    }
                ],
                "invalid_profiles": [],
                "blocking": true,
            }),
            "an assignable reference blocks nothing; the story no approved \
             graph covers yet still does"
        );
    }

    #[test]
    fn a_retired_entry_leaves_the_reference_blocking() {
        let harness = harness();
        let (spec, number) = spec_with_stories(&harness.core, STORIES, "key-spec");
        let ticket = implementation(&harness.core, spec, "CORE-S1-US1", "key-ticket");
        define(&harness.core, "standard", "key-profile");
        assign(&harness.core, ticket, "standard", 1, "key-assign");
        // Retirement never rewrites the assignment (DR-EP-05); the
        // reference now names an entry out of the assignable catalogue.
        retire(&harness.core, "standard", 1, "key-retire");
        let (plan, _) = plan_over(&harness.core, &[number], "key-plan");

        assert_eq!(
            diagnose(&harness.core, plan),
            json!({
                "cycles": [],
                "coverage_gaps": [
                    {
                        "spec_number": 1,
                        "uncovered": ["CORE-S1-US1"],
                        "claims_no_stories": false,
                    }
                ],
                "invalid_profiles": [{ "reference": "standard" }],
                "blocking": true,
            }),
            "the catalogue changed under the assignment, and the Plan's \
             diagnostics say so (DR-EP-03, DR-PS-18)"
        );
    }

    #[test]
    fn a_reference_no_entry_carries_blocks_on_its_own() {
        let harness = harness();
        let (spec, number) = spec_with_stories(&harness.core, STORIES, "key-spec");
        let ticket = implementation(&harness.core, spec, "CORE-S1-US1", "key-ticket");
        plant_reference(&harness.tickets, ticket, "ghost");
        let (plan, _) = plan_over(&harness.core, &[number], "key-plan");

        assert_eq!(
            diagnose(&harness.core, plan)["invalid_profiles"],
            json!([{ "reference": "ghost" }]),
            "a reference nothing carries is invalid exactly as a retired \
             one is"
        );
    }

    #[test]
    fn a_reference_outside_the_member_specs_stays_unreported() {
        let harness = harness();
        let (_, inside) = spec_with_stories(&harness.core, STORIES, "key-spec-one");
        let (outside, _) = spec_with_stories(
            &harness.core,
            "- CORE-S2-US1: As an operator, I want linked criteria.\n",
            "key-spec-two",
        );
        let ticket = implementation(&harness.core, outside, "CORE-S2-US1", "key-ticket");
        define(&harness.core, "standard", "key-profile");
        assign(&harness.core, ticket, "standard", 1, "key-assign");
        retire(&harness.core, "standard", 1, "key-retire");
        let (plan, _) = plan_over(&harness.core, &[inside], "key-plan");

        assert_eq!(
            diagnose(&harness.core, plan)["invalid_profiles"],
            json!([]),
            "the Plan's graph carries only its member Specs' Tickets"
        );
    }

    #[test]
    fn divergent_membership_reports_its_own_references() {
        let harness = harness();
        let (_, kept) = spec_with_stories(&harness.core, STORIES, "key-spec-kept");
        let (frozen_only, frozen_member) = spec_with_stories(
            &harness.core,
            "- CORE-S2-US1: As an operator, I want linked criteria.\n",
            "key-spec-frozen",
        );
        let (working_only, working_member) = spec_with_stories(
            &harness.core,
            "- CORE-S3-US1: As an operator, I want linked criteria.\n",
            "key-spec-working",
        );
        let frozen_ticket = implementation(
            &harness.core,
            frozen_only,
            "CORE-S2-US1",
            "key-ticket-frozen",
        );
        let working_ticket = implementation(
            &harness.core,
            working_only,
            "CORE-S3-US1",
            "key-ticket-working",
        );
        let (plan, mut version) = plan_over(&harness.core, &[kept, frozen_member], "key-plan");
        let activated = harness
            .core
            .command(
                "plan.activate",
                &json!({
                    "mutation": {
                        "optimistic_version": version,
                        "idempotency_key": "key-activate",
                    },
                    "plan_id": plan,
                }),
            )
            .expect("the membership freezes");
        version = activated["version"]
            .as_u64()
            .expect("the version is a number");
        let replanned = harness
            .core
            .command(
                "plan.replan",
                &json!({
                    "mutation": {
                        "optimistic_version": version,
                        "idempotency_key": "key-replan",
                    },
                    "plan_id": plan,
                }),
            )
            .expect("the draft reopens");
        version = replanned["version"]
            .as_u64()
            .expect("the version is a number");
        let removed = harness
            .core
            .command(
                "plan.spec.remove",
                &json!({
                    "mutation": {
                        "optimistic_version": version,
                        "idempotency_key": "key-remove",
                    },
                    "plan_id": plan,
                    "spec_number": frozen_member,
                }),
            )
            .expect("the frozen-only member leaves the working shape");
        version = removed["version"]
            .as_u64()
            .expect("the version is a number");
        harness
            .core
            .command(
                "plan.spec.add",
                &json!({
                    "mutation": {
                        "optimistic_version": version,
                        "idempotency_key": "key-add",
                    },
                    "plan_id": plan,
                    "spec_number": working_member,
                }),
            )
            .expect("the working-only member joins");
        // References land after the freeze, as restored or foreign
        // rows would: each shape must judge the members it holds.
        plant_reference(&harness.tickets, frozen_ticket, "frozen-only");
        plant_reference(&harness.tickets, working_ticket, "working-only");

        let working = diagnose(&harness.core, plan);
        assert_eq!(
            working["invalid_profiles"],
            json!([{ "reference": "working-only" }]),
            "the working shape carries the replanned members alone, so \
             the frozen-only reference stays out"
        );
        assert_eq!(
            working["blocking"],
            json!(true),
            "the working reference blocks"
        );

        let frozen = diagnose_version(&harness.core, plan, 1);
        assert_eq!(
            frozen["invalid_profiles"],
            json!([{ "reference": "frozen-only" }]),
            "the frozen version carries the members it froze, so the \
             working-only reference stays out"
        );
        assert_eq!(
            frozen["blocking"],
            json!(true),
            "the frozen-only reference blocks"
        );
    }
}

/// One proposal request over `tickets` against the Spec's approved
/// version one, returning the recorded proposal's identity.
fn propose(core: &Core, spec: u64, tickets: Value, key: &str) -> u64 {
    let recorded = core
        .command(
            "ticket.graph.propose",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": key },
                "spec_id": spec,
                "spec_version": 1,
                "tickets": tickets,
                "edges": [],
            }),
        )
        .expect("the graph records");
    recorded["id"].as_u64().expect("the identity is a number")
}

/// One approval request for `proposal` at `version`.
fn approve(
    core: &Core,
    proposal: u64,
    version: u64,
    key: &str,
) -> Result<Value, kanban_dto::ApiError> {
    core.command(
        "ticket.graph.approve",
        &json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "proposal_id": proposal,
        }),
    )
}

#[cfg(test)]
mod graph_approval_refusal {
    use super::*;

    #[test]
    fn approval_refuses_a_retired_profile_reference() {
        let harness = harness();
        let spec = approved_spec(&harness.core, "key-spec");
        let ticket = implementation(&harness.core, spec, "CORE-S1-US1", "key-ticket");
        define(&harness.core, "standard", "key-profile");
        assign(&harness.core, ticket, "standard", 1, "key-assign");
        retire(&harness.core, "standard", 1, "key-retire");
        let proposal = propose(&harness.core, spec, json!([ticket]), "key-propose");

        let error = approve(&harness.core, proposal, 1, "key-gate")
            .expect_err("a graph over a retired reference is refused");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the Ticket graph is not assignable; Ticket 1 references the profile `standard`, \
             which is not in the catalogue"
        );
        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": ticket }))
            .expect("the get serves");
        assert_eq!(
            read["pinned_spec_version"],
            json!(null),
            "the refusal pinned nothing"
        );
        let listed = harness
            .core
            .query("ticket.graph.list", &json!({ "spec_id": spec }))
            .expect("the list serves");
        assert_eq!(
            listed["proposals"][0]["state"],
            json!("proposed"),
            "the proposal stands while the assignment is repaired"
        );
    }

    #[test]
    fn approval_refuses_a_reference_no_entry_carries() {
        let harness = harness();
        let spec = approved_spec(&harness.core, "key-spec");
        let ticket = implementation(&harness.core, spec, "CORE-S1-US1", "key-ticket");
        plant_reference(&harness.tickets, ticket, "ghost");
        let proposal = propose(&harness.core, spec, json!([ticket]), "key-propose");

        let error = approve(&harness.core, proposal, 1, "key-gate")
            .expect_err("a graph over an unknown reference is refused");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the Ticket graph is not assignable; Ticket 1 references the profile `ghost`, \
             which is not in the catalogue"
        );
    }

    #[test]
    fn approval_passes_over_assignable_and_absent_references() {
        let harness = harness();
        let spec = approved_spec(&harness.core, "key-spec");
        let assigned = implementation(&harness.core, spec, "CORE-S1-US1", "key-ticket-1");
        let bare = implementation(&harness.core, spec, "CORE-S1-US1", "key-ticket-2");
        define(&harness.core, "standard", "key-profile");
        assign(&harness.core, assigned, "standard", 1, "key-assign");
        let proposal = propose(&harness.core, spec, json!([assigned, bare]), "key-propose");

        let response = approve(&harness.core, proposal, 1, "key-gate")
            .expect("an assignable reference, or none at all, holds");

        assert_eq!(response["state"], json!("approved"));
        for ticket in [assigned, bare] {
            let read = harness
                .core
                .query("ticket.get", &json!({ "ticket_id": ticket }))
                .expect("the get serves");
            assert_eq!(read["pinned_spec_version"], json!(1));
        }
    }
}
