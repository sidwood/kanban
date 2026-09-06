//! The reassignment command (DR-DE-07, KAN-S4-US7): `ticket.reassign`
//! replaces a Ticket by creating a replacement Ticket under its kind's
//! schema — validated exactly as a creation is — and superseding the
//! original. The replacement states its changed plan whole and
//! references its predecessor; the superseded original leaves the
//! active board keeping every recorded fact, its timeline history
//! stands untouched, and its minted number is never reused, because
//! the replacement consumes the Project's next number (KAN-T8's
//! counters, read the way KAN-T17 reads them). The supersession, the
//! replacement's creation, the counter move, and both timeline rows
//! land in one storage write, so a reassignment never splits across a
//! crash boundary.

use std::sync::Arc;

use kanban_domain::{NumberKind, Project, Ticket, TicketId, TicketNumber, apply_reassignment};
use kanban_dto::{ApiError, LiveEventName, TicketCreateRequest, TicketReassignRequest};
use serde_json::Value;

use crate::dispatch::{Core, RegistrationError};
use crate::events::emit_catalogued;
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;
use crate::spec::SpecStore;
use crate::ticket::{TicketStore, body_of, priority_of, record_of, transition};

/// Report a refused domain rule as the stable invalid-request code.
fn refuse(error: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

/// The stores the reassignment command reads and writes through.
#[derive(Clone)]
struct ReassignmentContext {
    tickets: Arc<dyn TicketStore>,
    projects: Arc<dyn ProjectStore>,
    specs: Arc<dyn SpecStore>,
}

impl ReassignmentContext {
    /// The Ticket a reassignment addresses and its Project, refusing
    /// an unknown Ticket and the terminal archived-Project state.
    fn open(&self, id: u64) -> Result<(Project, Ticket), ApiError> {
        let ticket = self
            .tickets
            .find(TicketId::new(id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {id}")))?;
        let project = self.projects.find(ticket.project())?.ok_or_else(|| {
            ApiError::internal(&format!("ticket {id} belongs to no stored Project"))
        })?;
        if project.is_archived() {
            return Err(ApiError::invalid_request(
                "archived is terminal; the Project accepts no further changes",
            ));
        }
        Ok((project, ticket))
    }
}

impl Core {
    /// Register the reassignment operation against `tickets`,
    /// resolving Projects through `projects` and the replacement's
    /// Spec attachment through `specs`.
    pub fn register_reassignment(
        &mut self,
        tickets: Arc<dyn TicketStore>,
        projects: Arc<dyn ProjectStore>,
        specs: Arc<dyn SpecStore>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "ticket.reassign",
            Arc::new(ReassignTicket(ReassignmentContext {
                tickets,
                projects,
                specs,
            })),
        )?;
        Ok(())
    }
}

/// Serves `ticket.reassign`: the replacement is stated whole under its
/// kind's schema, the original is superseded, and both land together.
struct ReassignTicket(ReassignmentContext);

impl CommandHandler for ReassignTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketReassignRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketReassignRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketReassignRequest = parse_payload(&command.payload)?;
        let (mut project, mut original) = self.0.open(request.ticket_id)?;
        // The replacement answers the same per-kind validation a
        // creation answers, so the changed plan is stated whole under
        // its kind's own schema (DR-DE-07).
        let shape = TicketCreateRequest {
            mutation: request.mutation.clone(),
            project_id: original.project().value(),
            kind: request.kind,
            priority: request.priority,
            spec_id: request.spec_id,
            title: request.title.clone(),
            actual_behaviour: request.actual_behaviour.clone(),
            reporter_evidence: request.reporter_evidence.clone(),
            slice: request.slice.clone(),
            criteria: request.criteria.clone(),
            subtype: request.subtype,
            mode: request.mode,
            completion: request.completion.clone(),
            scheduled_for: request.scheduled_for.clone(),
            due: request.due.clone(),
        };
        let body = body_of(&shape, &project, self.0.specs.as_ref())?;
        let from = original.state().wire_name().to_owned();
        let predecessor = original.id();
        // The domain rule supersedes the original before any number is
        // minted, so a refused reassignment consumes nothing.
        apply_reassignment(&mut original, predecessor).map_err(refuse)?;
        let priority = priority_of(request.priority);
        let number = TicketNumber::new(project.mint(NumberKind::Ticket).map_err(refuse)?)
            .expect("a minted number is positive");
        let identity = project.id();
        let kind = body.kind().wire_name().to_owned();
        let minted = number.value();
        let replacement =
            self.0
                .tickets
                .reassign(&project, &original, number, priority, &body, &|id| {
                    (
                        transition(
                            identity,
                            id,
                            "created",
                            serde_json::json!({
                                "project_id": identity.value(),
                                "number": minted,
                                "kind": kind,
                                "predecessor": predecessor.value(),
                            }),
                        ),
                        transition(
                            identity,
                            predecessor,
                            "superseded",
                            serde_json::json!({
                                "from": from,
                                "to": "superseded",
                                "replacement": id.value(),
                            }),
                        ),
                    )
                })?;
        let code = project.code();
        emit_catalogued(
            effects,
            LiveEventName::TicketStateChanged,
            &record_of(&original, code),
        );
        emit_catalogued(
            effects,
            LiveEventName::TicketCreated,
            &record_of(&replacement, code),
        );
        serde_json::to_value(record_of(&replacement, code))
            .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Arc;

    use crate::dispatch::Core;
    use crate::events::EventSink;
    use crate::lifecycle::testing::{LifecycleHarness, lifecycle_harness_with_sink};

    /// A core with the Plan, Spec, Ticket, lifecycle, and reassignment
    /// operations wired to in-memory rows over one active Project, so
    /// a test can drive an original to any state before reassigning
    /// it.
    pub(crate) struct ReassignmentHarness {
        pub(crate) rows: Arc<crate::lifecycle::testing::LifecycleRows>,
        pub(crate) projects: Arc<crate::plan::testing::MemoryProjects>,
        pub(crate) core: Core,
    }

    /// The harness the reassignment tests run against.
    pub(crate) fn reassignment_harness() -> ReassignmentHarness {
        reassignment_harness_with_sink(Arc::new(crate::events::NoopEventSink))
    }

    /// A harness whose event sink the test chooses.
    pub(crate) fn reassignment_harness_with_sink(
        events: Arc<dyn EventSink>,
    ) -> ReassignmentHarness {
        let LifecycleHarness {
            rows,
            projects,
            specs,
            mut core,
        } = lifecycle_harness_with_sink(events);
        core.register_reassignment(rows.clone(), projects.clone(), specs)
            .expect("the reassignment operation registers");
        ReassignmentHarness {
            rows,
            projects,
            core,
        }
    }
}

#[cfg(test)]
mod reassignment_commands {
    use kanban_dto::ErrorCode;
    use serde_json::{Value, json};

    use super::testing::reassignment_harness;
    use super::testing::reassignment_harness_with_sink;

    /// One mutation context addressed to `version`.
    fn mutation(version: u64, key: &str) -> Value {
        json!({
            "optimistic_version": version,
            "idempotency_key": key,
        })
    }

    /// One reassignment request naming `ticket` with the body a test
    /// chooses.
    fn reassigning(ticket: u64, body: Value, version: u64, key: &str) -> Value {
        let mut request = json!({
            "mutation": mutation(version, key),
            "ticket_id": ticket,
            "kind": "task",
            "priority": "high",
        });
        let object = request
            .as_object_mut()
            .expect("the request is a JSON object");
        for (field, value) in body.as_object().expect("the body is a JSON object") {
            object.insert(field.clone(), value.clone());
        }
        request
    }

    /// One rule-valid replacement body, with the fields a test varies.
    fn replaced(title: Option<&str>) -> Value {
        let mut body = json!({
            "title": "Replan the register archive",
            "subtype": "migration",
            "mode": "agent",
            "completion": ["The register moves and restores."],
        });
        if let Some(title) = title {
            body["title"] = json!(title);
        }
        body
    }

    /// Create one Task Ticket through the command surface, returning
    /// its identity.
    fn task(harness: &super::testing::ReassignmentHarness, key: &str) -> u64 {
        let created = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": mutation(0, key),
                    "project_id": 1,
                    "kind": "task",
                    "priority": "normal",
                    "title": "Archive the old register",
                    "subtype": "operational",
                    "mode": "human",
                    "completion": ["The register is archived."],
                }),
            )
            .expect("the Task creates");
        created["id"].as_u64().expect("the identity is a number")
    }

    #[test]
    fn reassignment_returns_the_replacement_and_supersedes_the_original() {
        let harness = reassignment_harness();
        let original = task(&harness, "key-task");

        let replaced = harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(original, replaced(None), 1, "key-reassign"),
            )
            .expect("the reassignment lands");

        assert_eq!(replaced["id"], json!(2), "a fresh row replaces the old");
        assert_eq!(
            replaced["number"],
            json!(2),
            "the replacement mints the next number"
        );
        assert_eq!(replaced["kind"], json!("task"));
        assert_eq!(replaced["priority"], json!("high"));
        assert_eq!(replaced["state"], json!("draft"));
        assert_eq!(
            replaced["predecessor_id"],
            json!(original),
            "the replacement references its predecessor (DR-DE-07)"
        );
        assert_eq!(replaced["title"], json!("Replan the register archive"));
        assert_eq!(replaced["version"], json!(1));

        let superseded = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": original }))
            .expect("the superseded Ticket still reads");
        assert_eq!(superseded["state"], json!("superseded"));
        assert_eq!(superseded["version"], json!(2), "the supersession counts");
        assert_eq!(
            superseded["number"],
            json!(1),
            "the superseded Ticket keeps its number"
        );
        assert!(
            superseded.get("predecessor_id").is_none(),
            "the original references no predecessor"
        );

        // Both rows landed with the counter move and two timeline rows:
        // the replacement's creation, then the original's supersession.
        let (tickets, timeline) = harness.rows.snapshot();
        assert_eq!(tickets.len(), 2);
        assert_eq!(
            harness.projects.rows()[0]
                .counters()
                .last(kanban_domain::NumberKind::Ticket),
            2,
            "the replacement consumes the Project's next number"
        );
        assert_eq!(timeline.len(), 3, "created, created, superseded");
        assert_eq!(
            timeline[1].detail(),
            &json!({
                "action": "created",
                "id": 2,
                "project_id": 1,
                "number": 2,
                "kind": "task",
                "predecessor": 1,
            })
        );
        assert_eq!(
            timeline[2].detail(),
            &json!({
                "action": "superseded",
                "id": 1,
                "from": "draft",
                "to": "superseded",
                "replacement": 2,
            })
        );
    }

    #[test]
    fn the_superseded_original_keeps_its_history_and_its_number_is_never_reused() {
        let harness = reassignment_harness();
        let original = task(&harness, "key-task");
        harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(original, replaced(None), 1, "key-reassign"),
            )
            .expect("the reassignment lands");

        // The original's creation row still stands on the timeline.
        let (_, timeline) = harness.rows.snapshot();
        assert_eq!(
            timeline[0].detail(),
            &json!({
                "action": "created",
                "id": 1,
                "project_id": 1,
                "number": 1,
                "kind": "task",
            }),
            "the history the original built stays exactly as it stood"
        );

        // Every later Ticket mints past both numbers: 1 is never
        // reused, and the replacement's 2 stands.
        let third = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": mutation(0, "key-next"),
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "normal",
                    "title": "Another defect",
                    "actual_behaviour": "It drops the branch.",
                    "reporter_evidence": "The log shows it.",
                }),
            )
            .expect("the next Ticket creates");
        assert_eq!(third["number"], json!(3), "numbers mint monotonically");
    }

    #[test]
    fn a_reassignment_restates_the_plan_under_another_kind() {
        let harness = reassignment_harness();
        let original = task(&harness, "key-task");
        let spec = crate::ticket::testing::authored_spec(&harness.core, "key-spec");

        let replaced = harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(
                    original,
                    json!({
                        "kind": "implementation",
                        "priority": "urgent",
                        "spec_id": spec,
                        "slice": "Spec authoring creates content versions end to end",
                        "criteria": [
                            { "outcome": "Specs mint unique numbers.", "stories": ["CORE-S1-US1"] }
                        ],
                    }),
                    1,
                    "key-reassign",
                ),
            )
            .expect("the replacement restates the plan under its own kind");

        assert_eq!(replaced["kind"], json!("implementation"));
        assert_eq!(replaced["priority"], json!("urgent"));
        assert_eq!(replaced["spec_id"], json!(spec));
        assert_eq!(replaced["predecessor_id"], json!(original));
        assert_eq!(replaced["state"], json!("draft"));
    }

    #[test]
    fn reassignment_serves_an_original_anywhere_along_its_lifecycle() {
        let harness = reassignment_harness();
        let original = task(&harness, "key-task");
        harness
            .rows
            .force_state(original, kanban_domain::TicketState::Active);

        let replaced = harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(original, replaced(None), 2, "key-reassign"),
            )
            .expect("active work is reassigned too");

        assert_eq!(replaced["predecessor_id"], json!(original));
        let superseded = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": original }))
            .expect("the superseded Ticket still reads");
        assert_eq!(superseded["state"], json!("superseded"));
        let (_, timeline) = harness.rows.snapshot();
        assert_eq!(
            timeline.last().expect("the supersession appended").detail(),
            &json!({
                "action": "superseded",
                "id": original,
                "from": "active",
                "to": "superseded",
                "replacement": 2,
            })
        );
    }

    #[test]
    fn landed_and_terminal_originals_are_refused() {
        let harness = reassignment_harness();

        let done = task(&harness, "key-done");
        harness
            .rows
            .force_state(done, kanban_domain::TicketState::Done);
        let refused = harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(done, replaced(None), 2, "key-1"),
            )
            .expect_err("done is final; landed work is not reassigned");
        assert_eq!(refused.code, ErrorCode::InvalidRequest);
        assert_eq!(
            refused.message,
            "done is final; landed work is not reassigned"
        );

        let cancelled = task(&harness, "key-cancelled");
        harness
            .core
            .command(
                "ticket.cancel",
                &json!({ "mutation": mutation(1, "key-cancel"), "ticket_id": cancelled }),
            )
            .expect("the Ticket cancels");
        let refused = harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(cancelled, replaced(None), 2, "key-2"),
            )
            .expect_err("cancelled is terminal");
        assert_eq!(refused.code, ErrorCode::InvalidRequest);
        assert_eq!(
            refused.message,
            "cancelled and superseded are terminal; the Ticket accepts no further changes"
        );

        let already = task(&harness, "key-replaced");
        harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(already, replaced(None), 1, "key-3"),
            )
            .expect("the first reassignment lands");
        let twice = harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(already, replaced(Some("Again")), 2, "key-4"),
            )
            .expect_err("a superseded original is replaced by nothing");
        assert_eq!(twice.code, ErrorCode::InvalidRequest);
        assert_eq!(
            twice.message,
            "cancelled and superseded are terminal; the Ticket accepts no further changes"
        );

        assert_eq!(
            harness.projects.rows()[0]
                .counters()
                .last(kanban_domain::NumberKind::Ticket),
            4,
            "the refusals consumed no number beyond the landed reassignment"
        );
        let (tickets, _) = harness.rows.snapshot();
        assert_eq!(tickets.len(), 4, "the refusals landed no rows");
    }

    #[test]
    fn a_refused_replacement_body_consumes_no_number_and_writes_nothing() {
        let harness = reassignment_harness();
        let original = task(&harness, "key-task");
        let before = harness.rows.snapshot().1.len();

        let refused = harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(original, replaced(Some("   ")), 1, "key-1"),
            )
            .expect_err("a blank title replaces nothing");
        assert_eq!(refused.code, ErrorCode::InvalidRequest);
        assert_eq!(refused.message, "a Ticket title cannot be blank");

        let unattached = harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(
                    original,
                    json!({
                        "kind": "implementation",
                        "slice": "A slice",
                        "criteria": [
                            { "outcome": "Done.", "stories": ["CORE-S1-US1"] }
                        ],
                    }),
                    1,
                    "key-2",
                ),
            )
            .expect_err("an Implementation attaches to exactly one Spec");
        assert_eq!(
            unattached.message,
            "an Implementation Ticket attaches to exactly one Spec"
        );

        assert_eq!(
            harness.projects.rows()[0]
                .counters()
                .last(kanban_domain::NumberKind::Ticket),
            1,
            "a refused reassignment consumes no number"
        );
        let (tickets, timeline) = harness.rows.snapshot();
        assert_eq!(tickets.len(), 1, "the refusals landed no rows");
        assert_eq!(
            timeline.len(),
            before,
            "the refusals appended no timeline row"
        );
        let untouched = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": original }))
            .expect("the original still reads");
        assert_eq!(
            untouched["state"],
            json!("draft"),
            "the original stood fast"
        );
    }

    #[test]
    fn an_unknown_ticket_an_archived_project_and_unknown_fields_are_refused() {
        let harness = reassignment_harness();

        let unknown = harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(99, replaced(None), 1, "key-1"),
            )
            .expect_err("an unknown Ticket is refused");
        assert_eq!(unknown.code, ErrorCode::NotFound);

        let original = task(&harness, "key-task");
        let mut project = harness.projects.rows()[0].clone();
        project.archive().expect("the fixture archives");
        harness.projects.replace(project);
        let archived = harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(original, replaced(None), 1, "key-2"),
            )
            .expect_err("an archived Project accepts no further changes");
        assert_eq!(archived.code, ErrorCode::InvalidRequest);
        assert!(archived.message.contains("archived"));

        let mut surprise = reassigning(original, replaced(None), 1, "key-3");
        surprise["surprise"] = json!(true);
        let refused = harness
            .core
            .command("ticket.reassign", &surprise)
            .expect_err("unknown fields are rejected");
        assert_eq!(refused.code, ErrorCode::UnknownField);
        assert_eq!(refused.message, "unknown field `surprise`");
    }

    #[test]
    fn a_stale_reassignment_is_refused_and_a_retry_replays() {
        let harness = reassignment_harness();
        let original = task(&harness, "key-task");

        let stale = harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(original, replaced(None), 0, "key-stale"),
            )
            .expect_err("the stale version is rejected");
        assert_eq!(stale.code, ErrorCode::StaleVersion);

        let request = reassigning(original, replaced(None), 1, "key-once");
        let first = harness
            .core
            .command("ticket.reassign", &request)
            .expect("the reassignment lands");
        let replay = harness
            .core
            .command("ticket.reassign", &request)
            .expect("the retry replays");
        assert_eq!(first, replay);
        let (tickets, _) = harness.rows.snapshot();
        assert_eq!(tickets.len(), 2, "the retry must not reapply");
    }

    #[test]
    fn reassignment_announces_both_sides_on_the_event_stream() {
        let sink = std::sync::Arc::new(crate::plan::testing::RecordingSink::default());
        let harness = reassignment_harness_with_sink(sink.clone());
        let original = task(&harness, "key-task");

        harness
            .core
            .command(
                "ticket.reassign",
                &reassigning(original, replaced(None), 1, "key-reassign"),
            )
            .expect("the reassignment lands");

        let events = sink.events.lock().expect("the recorder lock is sound");
        let superseded = events
            .iter()
            .find(|(name, payload)| {
                name == "ticket.state.changed" && payload["id"] == json!(original)
            })
            .expect("the supersession announces live");
        assert_eq!(superseded.1["state"], json!("superseded"));
        let created = events
            .iter()
            .find(|(name, payload)| name == "ticket.created" && payload["id"] == json!(2))
            .expect("the replacement's creation announces live");
        assert_eq!(created.1["predecessor_id"], json!(original));
    }
}
