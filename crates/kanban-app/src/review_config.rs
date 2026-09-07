//! Staged review commands and queries: configure one Ticket's review
//! — ordered stages of parallel slots, each required or optional and
//! occupied by a human or a named profile — and read the stored
//! configuration back (KAN-S10-US1, DR-EP-09). Separation validates
//! here, at configuration time, against the Ticket's implementer
//! assignment and the live catalogue: a model family never reviews
//! its own work, at least one reviewer uses a different harness
//! family from the implementer, and human slots join neither check
//! (DR-EP-13 to DR-EP-15). Reviewer runs dispatch through KAN-T42's
//! mechanics in the next ticket; nothing here dispatches anything.

use std::sync::Arc;

use kanban_domain::{
    ProfileCatalogue, ProfileName, Project, ReviewConfiguration, ReviewSlot, ReviewSlotAssignment,
    ReviewStage as DomainStage, SlotRequirement, Ticket, TicketId,
};
use kanban_dto::{
    ApiError, LiveEventName, TicketReviewConfigQuery, TicketReviewConfigRecord,
    TicketReviewConfigResponse, TicketReviewConfigureRequest, TicketReviewOccupant,
    TicketReviewSlot, TicketReviewSlotRequirement, TicketReviewStage, TimelineEntityKind,
    TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::events::emit_catalogued;
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::profile::ProfileStore;
use crate::project::ProjectStore;
use crate::ticket::TicketStore;
use crate::timeline::TimelineEnvelope;

/// The storage port the review configuration commands call through.
/// Implementations land the row change and the timeline envelope
/// unchanged inside one write.
pub trait ReviewConfigStore: Send + Sync {
    /// Insert a first configuration for `ticket`. A row that already
    /// stands is refused, so a create that raced past the
    /// application's version check lands nowhere.
    fn insert(
        &self,
        ticket: TicketId,
        configuration: &ReviewConfiguration,
        envelope: &TimelineEnvelope,
    ) -> Result<(), ApiError>;
    /// Persist an applied replacement, guarded by the version the
    /// aggregate moved from.
    fn save(
        &self,
        ticket: TicketId,
        configuration: &ReviewConfiguration,
        envelope: &TimelineEnvelope,
    ) -> Result<(), ApiError>;
    /// The stored configuration of `ticket`, if one stands.
    fn find(&self, ticket: TicketId) -> Result<Option<ReviewConfiguration>, ApiError>;
}

/// The timeline row for one configuration change: on the Project's
/// own timeline, about the Ticket, with `action` naming the change
/// inside the closed `transition` kind.
fn transition(
    project: kanban_domain::ProjectId,
    ticket: TicketId,
    configuration: &ReviewConfiguration,
) -> TimelineEnvelope {
    TimelineEnvelope::project(
        project.value(),
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Ticket,
            id: ticket.value().to_string(),
        }),
        json!({
            "action": "review_configured",
            "id": ticket.value(),
            "stages": configuration.stages().len(),
            "slots": configuration.stages().iter().map(|stage| stage.slots().len()).sum::<usize>(),
            "version": configuration.version(),
        }),
    )
}

/// Report a refused domain rule as the stable invalid-request code.
fn refuse(error: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

/// The catalogue behind one command: the stored entries as the
/// domain aggregate, so the separation rules decide the change.
fn catalogue_of(store: &dyn ProfileStore) -> Result<ProfileCatalogue, ApiError> {
    Ok(ProfileCatalogue::restore(store.list()?))
}

/// One validated name from a request's raw text.
fn name_of(raw: &str) -> Result<ProfileName, ApiError> {
    ProfileName::new(raw).map_err(refuse)
}

/// The requirement the wire names, on the domain's vocabulary.
fn requirement_of(wire: TicketReviewSlotRequirement) -> SlotRequirement {
    match wire {
        TicketReviewSlotRequirement::Required => SlotRequirement::Required,
        TicketReviewSlotRequirement::Optional => SlotRequirement::Optional,
    }
}

/// The requirement the domain carries, on the wire's vocabulary.
fn wire_requirement_of(domain: SlotRequirement) -> TicketReviewSlotRequirement {
    match domain {
        SlotRequirement::Required => TicketReviewSlotRequirement::Required,
        SlotRequirement::Optional => TicketReviewSlotRequirement::Optional,
    }
}

/// The stages a request names, as the domain's ordered stages of
/// parallel slots. Only the names validate here — the shape and the
/// separation rules are the configuration's own to refuse.
fn stages_of(wire: &[TicketReviewStage]) -> Result<Vec<DomainStage>, ApiError> {
    wire.iter()
        .map(|stage| {
            let slots = stage
                .slots
                .iter()
                .map(|slot| {
                    let requirement = requirement_of(slot.requirement);
                    match &slot.occupant {
                        TicketReviewOccupant::Human => Ok(ReviewSlot::human(requirement)),
                        TicketReviewOccupant::Profile { name } => {
                            Ok(ReviewSlot::profile(name_of(name)?, requirement))
                        }
                    }
                })
                .collect::<Result<Vec<_>, ApiError>>()?;
            Ok(DomainStage::new(slots))
        })
        .collect()
}

/// The record every client sees for one stored configuration.
fn record_of(ticket: TicketId, configuration: &ReviewConfiguration) -> TicketReviewConfigRecord {
    TicketReviewConfigRecord {
        ticket_id: ticket.value(),
        stages: configuration
            .stages()
            .iter()
            .map(|stage| TicketReviewStage {
                slots: stage
                    .slots()
                    .iter()
                    .map(|slot| TicketReviewSlot {
                        occupant: match slot.assignment() {
                            ReviewSlotAssignment::Human => TicketReviewOccupant::Human,
                            ReviewSlotAssignment::Profile(name) => TicketReviewOccupant::Profile {
                                name: name.as_str().to_owned(),
                            },
                        },
                        requirement: wire_requirement_of(slot.requirement()),
                    })
                    .collect(),
            })
            .collect(),
        version: configuration.version(),
    }
}

/// The stores the review configuration operations read and write
/// through.
#[derive(Clone)]
struct ReviewContext {
    configurations: Arc<dyn ReviewConfigStore>,
    tickets: Arc<dyn TicketStore>,
    profiles: Arc<dyn ProfileStore>,
    projects: Arc<dyn ProjectStore>,
}

impl ReviewContext {
    /// The Ticket a command addresses with its Project, refusing an
    /// unknown Ticket, the terminal Ticket states, and the terminal
    /// archived-Project state — the same open guards the dispatch
    /// commands apply.
    fn open(&self, ticket_id: u64) -> Result<(Project, Ticket), ApiError> {
        let ticket = self
            .tickets
            .find(TicketId::new(ticket_id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {ticket_id}")))?;
        let project = self.projects.find(ticket.project())?.ok_or_else(|| {
            ApiError::internal(&format!("ticket {ticket_id} belongs to no stored Project"))
        })?;
        if project.is_archived() {
            return Err(ApiError::invalid_request(
                "archived is terminal; the Project accepts no further changes",
            ));
        }
        if ticket.state().is_terminal() {
            return Err(ApiError::invalid_request(
                "cancelled and superseded are terminal; the Ticket accepts no further changes",
            ));
        }
        Ok((project, ticket))
    }
}

impl Core {
    /// Register the review configuration operations against
    /// `configurations`, resolving Tickets through `tickets`, the
    /// separation catalogue through `profiles`, and Projects through
    /// `projects`.
    pub fn register_review_config(
        &mut self,
        configurations: Arc<dyn ReviewConfigStore>,
        tickets: Arc<dyn TicketStore>,
        profiles: Arc<dyn ProfileStore>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), RegistrationError> {
        let context = ReviewContext {
            configurations,
            tickets,
            profiles,
            projects,
        };
        self.register_command(
            "ticket.review.configure",
            Arc::new(ConfigureTicketReview(context.clone())),
        )?;
        self.register_query(
            "ticket.review.config",
            Arc::new(GetTicketReviewConfig {
                configurations: context.configurations,
                tickets: context.tickets,
            }),
        )?;
        Ok(())
    }
}

/// Serves `ticket.review.configure`.
struct ConfigureTicketReview(ReviewContext);

impl CommandHandler for ConfigureTicketReview {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketReviewConfigureRequest>(payload)?;
        ParsedCommand::lift("review", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketReviewConfigureRequest = parse_payload(&command.payload)?;
        self.0
            .tickets
            .find(TicketId::new(request.ticket_id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {}", request.ticket_id)))?;
        Ok(self
            .0
            .configurations
            .find(TicketId::new(request.ticket_id))?
            .map_or(0, |configuration| configuration.version()))
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketReviewConfigureRequest = parse_payload(&command.payload)?;
        let (project, ticket) = self.0.open(request.ticket_id)?;
        // Separation needs the implementer's model and harness
        // families, so the assignment this configuration separates
        // from must already stand (DR-EP-13, DR-EP-14).
        let implementer = ticket.profile().ok_or_else(|| {
            ApiError::invalid_request(
                "a staged review is configured against the Ticket's assigned \
                 implementer profile",
            )
        })?;
        let stages = stages_of(&request.stages)?;
        let configuration = match self.0.configurations.find(ticket.id())? {
            Some(mut standing) => {
                standing.replace(stages).map_err(refuse)?;
                standing
            }
            None => ReviewConfiguration::new(stages).map_err(refuse)?,
        };
        let catalogue = catalogue_of(self.0.profiles.as_ref())?;
        configuration
            .validate_separation(implementer, &catalogue)
            .map_err(refuse)?;
        let envelope = transition(project.id(), ticket.id(), &configuration);
        if configuration.version() == 1 {
            self.0
                .configurations
                .insert(ticket.id(), &configuration, &envelope)?;
        } else {
            self.0
                .configurations
                .save(ticket.id(), &configuration, &envelope)?;
        }
        let record = record_of(ticket.id(), &configuration);
        emit_catalogued(effects, LiveEventName::TicketReviewConfigured, &record);
        serde_json::to_value(record).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `ticket.review.config`.
struct GetTicketReviewConfig {
    configurations: Arc<dyn ReviewConfigStore>,
    tickets: Arc<dyn TicketStore>,
}

impl QueryHandler for GetTicketReviewConfig {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: TicketReviewConfigQuery = parse_payload(payload)?;
        let ticket = TicketId::new(query.ticket_id);
        self.tickets
            .find(ticket)?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {}", query.ticket_id)))?;
        let response = TicketReviewConfigResponse {
            config: self
                .configurations
                .find(ticket)?
                .map(|configuration| record_of(ticket, &configuration)),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Mutex;

    use kanban_domain::{ReviewConfiguration, TicketId};
    use kanban_dto::ApiError;

    use super::ReviewConfigStore;
    use crate::timeline::TimelineEnvelope;

    /// An in-memory review configuration store: rows by Ticket, plus
    /// every timeline append it was asked to land, for assertions.
    #[derive(Default)]
    pub(crate) struct MemoryReviewConfigs {
        state: Mutex<MemoryReviewConfigState>,
    }

    #[derive(Default)]
    struct MemoryReviewConfigState {
        rows: Vec<(TicketId, ReviewConfiguration)>,
        timeline: Vec<TimelineEnvelope>,
    }

    impl MemoryReviewConfigs {
        /// The stored rows and timeline envelopes, for assertions.
        pub(crate) fn snapshot(
            &self,
        ) -> (Vec<(TicketId, ReviewConfiguration)>, Vec<TimelineEnvelope>) {
            let state = self.state.lock().expect("the memory review lock is sound");
            (state.rows.clone(), state.timeline.clone())
        }
    }

    impl ReviewConfigStore for MemoryReviewConfigs {
        fn insert(
            &self,
            ticket: TicketId,
            configuration: &ReviewConfiguration,
            envelope: &TimelineEnvelope,
        ) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory review lock is sound");
            if let Some((_, standing)) = state.rows.iter().find(|(held, _)| *held == ticket) {
                return Err(ApiError::stale_version(0, standing.version()));
            }
            state.rows.push((ticket, configuration.clone()));
            state.timeline.push(envelope.clone());
            Ok(())
        }

        fn save(
            &self,
            ticket: TicketId,
            configuration: &ReviewConfiguration,
            envelope: &TimelineEnvelope,
        ) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory review lock is sound");
            let preceding = configuration.version() - 1;
            match state.rows.iter().position(|(held, _)| *held == ticket) {
                Some(index) if state.rows[index].1.version() == preceding => {
                    state.rows[index].1 = configuration.clone();
                    state.timeline.push(envelope.clone());
                    Ok(())
                }
                Some(index) => Err(ApiError::stale_version(
                    preceding,
                    state.rows[index].1.version(),
                )),
                None => Err(ApiError::not_found(&format!(
                    "review configuration of ticket {}",
                    ticket.value()
                ))),
            }
        }

        fn find(&self, ticket: TicketId) -> Result<Option<ReviewConfiguration>, ApiError> {
            let state = self.state.lock().expect("the memory review lock is sound");
            Ok(state
                .rows
                .iter()
                .find(|(held, _)| *held == ticket)
                .map(|(_, configuration)| configuration.clone()))
        }
    }
}

#[cfg(test)]
mod review_configuration {
    use std::sync::Arc;

    use serde_json::{Value, json};

    use kanban_dto::ErrorCode;

    /// The review harness: the full ticket and catalogue surface plus
    /// a memory review configuration store wired through the review
    /// operations.
    struct ReviewHarness {
        core: crate::dispatch::Core,
        configurations: Arc<super::testing::MemoryReviewConfigs>,
    }

    fn harness() -> ReviewHarness {
        let (core, configurations) = review_core(Arc::new(crate::events::NoopEventSink));
        ReviewHarness {
            core,
            configurations,
        }
    }

    /// A core, its memory review configuration store, and the sink
    /// its events land in.
    fn harness_with_sink() -> (
        crate::dispatch::Core,
        Arc<super::testing::MemoryReviewConfigs>,
        Arc<crate::profile::testing::RecordingSink>,
    ) {
        let sink = Arc::new(crate::profile::testing::RecordingSink::default());
        let (core, configurations) = review_core(sink.clone());
        (core, configurations, sink)
    }

    /// A core with the Plan, Spec, Ticket, and catalogue operations
    /// plus the review configuration operations, over in-memory
    /// stores and one active Project.
    fn review_core(
        events: Arc<dyn crate::events::EventSink>,
    ) -> (
        crate::dispatch::Core,
        Arc<super::testing::MemoryReviewConfigs>,
    ) {
        use crate::catalog::exposed_operations;
        use crate::mutation::MemoryIdempotencyStore;
        use crate::plan::testing::{MemoryPlans, MemoryProjects};
        use crate::profile::testing::MemoryProfiles;
        use crate::spec::testing::MemorySpecs;
        use crate::ticket::testing::{MemoryTicketEvidence, MemoryTickets};

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
        let configurations = Arc::new(super::testing::MemoryReviewConfigs::default());
        let mut core = crate::dispatch::Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            events,
        );
        core.register_plans(plans.clone(), projects.clone(), specs.clone())
            .expect("the plan operations register");
        core.register_specs(specs.clone(), projects.clone(), plans)
            .expect("the spec operations register");
        core.register_tickets(
            tickets.clone(),
            projects.clone(),
            specs.clone(),
            Arc::new(MemoryTicketEvidence::default()),
        )
        .expect("the ticket operations register");
        core.register_profiles(profiles.clone(), tickets.clone(), projects.clone())
            .expect("the profile operations register");
        core.register_review_config(
            configurations.clone(),
            tickets.clone(),
            profiles.clone(),
            projects,
        )
        .expect("the review operations register");
        (core, configurations)
    }

    fn define(core: &crate::dispatch::Core, name: &str, harness: &str, model: &str, key: &str) {
        core.command(
            "profile.define",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": key },
                "name": name,
                "harness": harness,
                "model": model,
                "effort": "high",
                "usage_pool": "operator",
            }),
        )
        .expect("the profile lands");
    }

    /// One Bug Ticket on the seeded Project with the implementer
    /// assignment standing, returning its identity.
    fn assigned_ticket(core: &crate::dispatch::Core) -> u64 {
        let created = core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-ticket" },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "normal",
                    "title": "Landing drops the integration branch",
                    "actual_behaviour": "The integration branch is dropped after a review lands.",
                    "reporter_evidence":
                        "The landing log names the drop immediately after the merge.",
                }),
            )
            .expect("the Ticket creates");
        let id = created["id"].as_u64().expect("the identity is a number");
        core.command(
            "ticket.assign",
            &json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "key-assign" },
                "ticket_id": id,
                "profile": "implementer",
            }),
        )
        .expect("the implementer assignment lands");
        id
    }

    fn configure(ticket: u64, stage_slots: Value, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "ticket_id": ticket,
            "stages": [{ "slots": stage_slots }],
        })
    }

    /// One separating slot set: a required outsider profile and an
    /// optional human in parallel.
    fn separating() -> Value {
        json!([
            { "occupant": { "kind": "profile", "name": "outsider" }, "requirement": "required" },
            { "occupant": { "kind": "human" }, "requirement": "optional" },
        ])
    }

    #[test]
    fn configuring_creates_the_configuration_at_version_one() {
        let harness = harness();
        define(
            &harness.core,
            "implementer",
            "claude-code",
            "opus",
            "key-implementer",
        );
        define(
            &harness.core,
            "outsider",
            "gemini-cli",
            "flash",
            "key-outsider",
        );
        let ticket = assigned_ticket(&harness.core);

        let response = harness
            .core
            .command(
                "ticket.review.configure",
                &configure(ticket, separating(), "key-configure", 0),
            )
            .expect("the configuration applies");

        assert_eq!(response["ticket_id"], json!(ticket));
        assert_eq!(response["version"], json!(1));
        assert_eq!(
            response["stages"],
            json!([{
                "slots": [
                    { "occupant": { "kind": "profile", "name": "outsider" }, "requirement": "required" },
                    { "occupant": { "kind": "human" }, "requirement": "optional" },
                ],
            }])
        );

        let read = harness
            .core
            .query("ticket.review.config", &json!({ "ticket_id": ticket }))
            .expect("the query serves");
        assert_eq!(read["config"], response);
    }

    #[test]
    fn the_query_serves_nothing_until_a_configuration_stands() {
        let harness = harness();
        define(
            &harness.core,
            "implementer",
            "claude-code",
            "opus",
            "key-implementer",
        );
        let ticket = assigned_ticket(&harness.core);

        let read = harness
            .core
            .query("ticket.review.config", &json!({ "ticket_id": ticket }))
            .expect("the query serves");

        assert_eq!(read, json!({ "config": null }));

        let unknown = harness
            .core
            .query("ticket.review.config", &json!({ "ticket_id": 9 }))
            .expect_err("an unknown Ticket is refused");
        assert_eq!(unknown.code, ErrorCode::NotFound);
    }

    #[test]
    fn configuring_refuses_a_reviewer_sharing_the_implementers_model_family() {
        let harness = harness();
        define(
            &harness.core,
            "implementer",
            "claude-code",
            "opus",
            "key-implementer",
        );
        define(
            &harness.core,
            "same-model",
            "shell-agent",
            "opus",
            "key-same-model",
        );
        let ticket = assigned_ticket(&harness.core);

        let error = harness
            .core
            .command(
                "ticket.review.configure",
                &configure(
                    ticket,
                    json!([
                        { "occupant": { "kind": "profile", "name": "same-model" }, "requirement": "required" },
                    ]),
                    "key-configure",
                    0,
                ),
            )
            .expect_err("a shared model family is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the reviewer profile `same-model` shares the implementer's `opus` model family; \
             a model family never reviews its own work"
        );
        let read = harness
            .core
            .query("ticket.review.config", &json!({ "ticket_id": ticket }))
            .expect("the query serves");
        assert_eq!(read["config"], json!(null), "the refusal recorded nothing");
        assert!(
            harness.configurations.snapshot().1.is_empty(),
            "no timeline row may be appended"
        );
    }

    #[test]
    fn configuring_refuses_a_review_with_no_different_harness_family() {
        let harness = harness();
        define(
            &harness.core,
            "implementer",
            "claude-code",
            "opus",
            "key-implementer",
        );
        define(
            &harness.core,
            "same-harness",
            "claude-code",
            "sonnet",
            "key-same-harness",
        );
        let ticket = assigned_ticket(&harness.core);

        let error = harness
            .core
            .command(
                "ticket.review.configure",
                &configure(
                    ticket,
                    json!([
                        { "occupant": { "kind": "profile", "name": "same-harness" }, "requirement": "required" },
                        { "occupant": { "kind": "human" }, "requirement": "required" },
                    ]),
                    "key-configure",
                    0,
                ),
            )
            .expect_err("no different harness family is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "at least one reviewer must use a different harness family \
             than the implementer's `claude-code`"
        );
    }

    #[test]
    fn configuring_requires_the_implementer_assignment_to_stand() {
        let harness = harness();
        define(
            &harness.core,
            "implementer",
            "claude-code",
            "opus",
            "key-implementer",
        );
        define(
            &harness.core,
            "outsider",
            "gemini-cli",
            "flash",
            "key-outsider",
        );
        let created = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-ticket" },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "normal",
                    "title": "Landing drops the integration branch",
                    "actual_behaviour": "The integration branch is dropped.",
                    "reporter_evidence": "The landing log names the drop.",
                }),
            )
            .expect("the Ticket creates");
        let unassigned = created["id"].as_u64().expect("the identity is a number");

        let error = harness
            .core
            .command(
                "ticket.review.configure",
                &configure(unassigned, separating(), "key-configure", 0),
            )
            .expect_err("an unassigned Ticket accepts no review configuration");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "a staged review is configured against the Ticket's assigned implementer profile"
        );
    }

    #[test]
    fn configuring_refuses_unknown_profiles_blank_names_and_empty_shapes() {
        let harness = harness();
        define(
            &harness.core,
            "implementer",
            "claude-code",
            "opus",
            "key-implementer",
        );
        define(
            &harness.core,
            "outsider",
            "gemini-cli",
            "flash",
            "key-outsider",
        );
        let ticket = assigned_ticket(&harness.core);

        let unknown = harness
            .core
            .command(
                "ticket.review.configure",
                &configure(
                    ticket,
                    json!([
                        { "occupant": { "kind": "profile", "name": "ghost" }, "requirement": "required" },
                    ]),
                    "key-unknown",
                    0,
                ),
            )
            .expect_err("an unknown profile is refused");
        assert_eq!(unknown.code, ErrorCode::InvalidRequest);
        assert_eq!(
            unknown.message,
            "the profile name `ghost` is not in the catalogue"
        );

        let blank = harness
            .core
            .command(
                "ticket.review.configure",
                &configure(
                    ticket,
                    json!([
                        { "occupant": { "kind": "profile", "name": "  " }, "requirement": "required" },
                    ]),
                    "key-blank",
                    0,
                ),
            )
            .expect_err("a blank name is refused");
        assert_eq!(blank.code, ErrorCode::InvalidRequest);

        let empty = harness
            .core
            .command(
                "ticket.review.configure",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-empty" },
                    "ticket_id": ticket,
                    "stages": [{ "slots": [] }],
                }),
            )
            .expect_err("an empty stage is refused");
        assert_eq!(empty.code, ErrorCode::InvalidRequest);
        assert_eq!(empty.message, "stage 1 carries at least one review slot");

        let bare = harness
            .core
            .command(
                "ticket.review.configure",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-bare" },
                    "ticket_id": ticket,
                    "stages": [],
                }),
            )
            .expect_err("a configuration with no stage is refused");
        assert_eq!(bare.code, ErrorCode::InvalidRequest);
        assert_eq!(
            bare.message,
            "a review configuration carries at least one stage"
        );
    }

    #[test]
    fn replacing_the_whole_configuration_bumps_the_version_under_guard() {
        let harness = harness();
        define(
            &harness.core,
            "implementer",
            "claude-code",
            "opus",
            "key-implementer",
        );
        define(
            &harness.core,
            "outsider",
            "gemini-cli",
            "flash",
            "key-outsider",
        );
        let ticket = assigned_ticket(&harness.core);

        harness
            .core
            .command(
                "ticket.review.configure",
                &configure(ticket, separating(), "key-configure", 0),
            )
            .expect("the first configuration applies");

        let replaced = harness
            .core
            .command(
                "ticket.review.configure",
                &configure(
                    ticket,
                    json!([
                        { "occupant": { "kind": "human" }, "requirement": "required" },
                        { "occupant": { "kind": "profile", "name": "outsider" }, "requirement": "required" },
                    ]),
                    "key-replace",
                    1,
                ),
            )
            .expect("the replacement applies");
        assert_eq!(replaced["version"], json!(2));
        assert_eq!(
            replaced["stages"][0]["slots"].as_array().map(Vec::len),
            Some(2)
        );

        let stale = harness
            .core
            .command(
                "ticket.review.configure",
                &configure(ticket, separating(), "key-stale", 0),
            )
            .expect_err("the stale version is rejected");
        assert_eq!(stale.code, ErrorCode::StaleVersion);
        assert_eq!(stale.current_version, Some(2));

        let unknown = harness
            .core
            .command(
                "ticket.review.configure",
                &configure(9, separating(), "key-unknown", 0),
            )
            .expect_err("the unknown Ticket is refused");
        assert_eq!(unknown.code, ErrorCode::NotFound);
    }

    #[test]
    fn a_retry_replays_without_reapplying() {
        let harness = harness();
        define(
            &harness.core,
            "implementer",
            "claude-code",
            "opus",
            "key-implementer",
        );
        define(
            &harness.core,
            "outsider",
            "gemini-cli",
            "flash",
            "key-outsider",
        );
        let ticket = assigned_ticket(&harness.core);
        let request = configure(ticket, separating(), "key-configure", 0);

        let first = harness
            .core
            .command("ticket.review.configure", &request)
            .expect("the configuration applies");
        let replay = harness
            .core
            .command("ticket.review.configure", &request)
            .expect("the retry replays");

        assert_eq!(first, replay);
        assert_eq!(
            harness.configurations.snapshot().0.len(),
            1,
            "the retry must not reapply"
        );
    }

    #[test]
    fn configuring_appends_one_timeline_row_and_publishes_live() {
        let (core, configurations, sink) = harness_with_sink();
        define(
            &core,
            "implementer",
            "claude-code",
            "opus",
            "key-implementer",
        );
        define(&core, "outsider", "gemini-cli", "flash", "key-outsider");
        let ticket = assigned_ticket(&core);
        core.command(
            "ticket.review.configure",
            &configure(ticket, separating(), "key-configure", 0),
        )
        .expect("the configuration applies");

        let events = sink.events.lock().expect("the recorder lock is sound");
        assert_eq!(
            events
                .iter()
                .filter(|(name, _)| name == "ticket.review.configured")
                .map(|(_, payload)| (payload["ticket_id"].clone(), payload["version"].clone(),))
                .collect::<Vec<_>>(),
            vec![(json!(ticket), json!(1))],
            "the configuration announces itself live"
        );
        drop(events);

        let recorded: Vec<_> = configurations
            .snapshot()
            .1
            .iter()
            .map(|envelope| {
                (
                    envelope.kind(),
                    envelope.entity().cloned(),
                    envelope.detail()["action"].clone(),
                )
            })
            .collect();
        assert_eq!(
            recorded,
            vec![(
                kanban_dto::TimelineEventKind::Transition,
                Some(kanban_dto::TimelineEntityRef {
                    kind: kanban_dto::TimelineEntityKind::Ticket,
                    id: ticket.to_string(),
                }),
                json!("review_configured"),
            )],
            "the change appends one row on the Ticket's timeline"
        );
    }

    #[test]
    fn every_command_rejects_unknown_fields() {
        let harness = harness();
        define(
            &harness.core,
            "implementer",
            "claude-code",
            "opus",
            "key-implementer",
        );
        define(
            &harness.core,
            "outsider",
            "gemini-cli",
            "flash",
            "key-outsider",
        );
        let ticket = assigned_ticket(&harness.core);
        let mut request = configure(ticket, separating(), "key-configure", 0);
        request["surprise"] = json!(true);

        let error = harness
            .core
            .command("ticket.review.configure", &request)
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }
}
