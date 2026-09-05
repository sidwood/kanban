//! The Execution Profile catalogue commands and queries: define,
//! update, retire, and list the named entries, and assign Tickets to
//! a profile by name (KAN-S7-US1, DR-EP-01, DR-EP-03). Every
//! catalogue change appends a timeline row on the global scope in
//! the same write as the row it changes and never rewrites an
//! earlier row, so past assignments keep the names they referenced
//! (DR-EP-05).

use std::sync::Arc;

use kanban_domain::{
    ExecutionProfile, ProfileCatalogue, ProfileDefinition, ProfileError, ProfileName, Project,
    ProjectId,
};
use kanban_dto::{
    ApiError, LiveEventName, ProfileDefineRequest, ProfileGetQuery, ProfileListQuery,
    ProfileListResponse, ProfileRecord, ProfileRetireRequest, ProfileUpdateRequest,
    TicketAssignRequest, TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::events::emit_catalogued;
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;
use crate::ticket::TicketStore;
use crate::timeline::TimelineEnvelope;

/// The storage port the catalogue commands call through.
/// Implementations land the row change and the timeline envelope
/// unchanged inside one write.
pub trait ProfileStore: Send + Sync {
    /// Insert a fresh entry. Storage keeps names unique; a duplicate
    /// name that raced past the application check is refused here.
    fn define(
        &self,
        profile: &ExecutionProfile,
        envelope: &TimelineEnvelope,
    ) -> Result<(), ApiError>;
    /// Persist an applied change to one entry, guarded by its
    /// version.
    fn save(&self, profile: &ExecutionProfile, envelope: &TimelineEnvelope)
    -> Result<(), ApiError>;
    /// Load one entry by name, retired ones included.
    fn find(&self, name: &ProfileName) -> Result<Option<ExecutionProfile>, ApiError>;
    /// Every entry in definition order, retired ones included.
    fn list(&self) -> Result<Vec<ExecutionProfile>, ApiError>;
}

/// The timeline row for one catalogue change: on the global scope —
/// the catalogue sits above every Project — about the entry, with
/// `action` naming the change inside the closed `transition` kind.
fn transition(name: &ProfileName, action: &str, facts: Value) -> TimelineEnvelope {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("profile transition facts are a JSON object");
    object.insert("action".to_owned(), Value::from(action));
    object.insert("name".to_owned(), Value::from(name.as_str()));
    TimelineEnvelope::global(
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Profile,
            id: name.as_str().to_owned(),
        }),
        detail,
    )
}

/// The catalogue behind one command: the stored entries as the
/// domain aggregate, so the collection rules decide the change.
fn catalogue_of(store: &dyn ProfileStore) -> Result<ProfileCatalogue, ApiError> {
    Ok(ProfileCatalogue::restore(store.list()?))
}

/// Report a refused domain rule as the stable invalid-request code.
fn refuse(error: ProfileError) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

/// One validated name from a request's raw text.
fn name_of(raw: &str) -> Result<ProfileName, ApiError> {
    ProfileName::new(raw).map_err(refuse)
}

/// One validated definition from a request's raw fields.
fn definition_of(
    harness: &str,
    model: &str,
    effort: &str,
    usage_pool: &str,
    fallback: Option<&str>,
) -> Result<ProfileDefinition, ApiError> {
    let fallback = fallback.map(name_of).transpose()?;
    ProfileDefinition::new(harness, model, effort, usage_pool, fallback).map_err(refuse)
}

/// The entry a command addresses, or the stable not-found refusal.
fn load(store: &dyn ProfileStore, name: &ProfileName) -> Result<ExecutionProfile, ApiError> {
    store
        .find(name)?
        .ok_or_else(|| ApiError::not_found(&format!("profile {}", name)))
}

impl Core {
    /// Register the catalogue operations against `profiles`, and the
    /// Ticket assignment against `tickets` resolving Projects through
    /// `projects`.
    pub fn register_profiles(
        &mut self,
        profiles: Arc<dyn ProfileStore>,
        tickets: Arc<dyn TicketStore>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "profile.define",
            Arc::new(DefineProfile {
                store: profiles.clone(),
            }),
        )?;
        self.register_command(
            "profile.update",
            Arc::new(UpdateProfile {
                store: profiles.clone(),
            }),
        )?;
        self.register_command(
            "profile.retire",
            Arc::new(RetireProfile {
                store: profiles.clone(),
            }),
        )?;
        self.register_query(
            "profile.list",
            Arc::new(ListProfiles {
                store: profiles.clone(),
            }),
        )?;
        self.register_query(
            "profile.get",
            Arc::new(GetProfile {
                store: profiles.clone(),
            }),
        )?;
        self.register_command(
            "ticket.assign",
            Arc::new(AssignTicket {
                profiles,
                tickets,
                projects,
            }),
        )?;
        Ok(())
    }
}

/// Serves `profile.define`.
struct DefineProfile {
    store: Arc<dyn ProfileStore>,
}

impl CommandHandler for DefineProfile {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ProfileDefineRequest>(payload)?;
        ParsedCommand::lift("profile", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        // A fresh aggregate is created at version 0.
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: ProfileDefineRequest = parse_payload(&command.payload)?;
        let name = name_of(&request.name)?;
        let definition = definition_of(
            &request.harness,
            &request.model,
            &request.effort,
            &request.usage_pool,
            request.fallback.as_deref(),
        )?;
        let mut catalogue = catalogue_of(self.store.as_ref())?;
        let entry = catalogue.define(name, definition).map_err(refuse)?;
        self.store
            .define(entry, &transition(entry.name(), "defined", json!({})))?;
        announce(effects, LiveEventName::ProfileDefined, entry);
        encode_record(entry)
    }
}

/// Serves `profile.update`.
struct UpdateProfile {
    store: Arc<dyn ProfileStore>,
}

impl CommandHandler for UpdateProfile {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ProfileUpdateRequest>(payload)?;
        ParsedCommand::lift("profile", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: ProfileUpdateRequest = parse_payload(&command.payload)?;
        let name = name_of(&request.name)?;
        Ok(load(self.store.as_ref(), &name)?.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: ProfileUpdateRequest = parse_payload(&command.payload)?;
        let name = name_of(&request.name)?;
        let definition = definition_of(
            &request.harness,
            &request.model,
            &request.effort,
            &request.usage_pool,
            request.fallback.as_deref(),
        )?;
        let mut catalogue = catalogue_of(self.store.as_ref())?;
        let entry = catalogue.redefine(&name, definition).map_err(refuse)?;
        self.store.save(
            entry,
            &transition(&name, "updated", json!({ "version": entry.version() })),
        )?;
        announce(effects, LiveEventName::ProfileUpdated, entry);
        encode_record(entry)
    }
}

/// Serves `profile.retire`.
struct RetireProfile {
    store: Arc<dyn ProfileStore>,
}

impl CommandHandler for RetireProfile {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ProfileRetireRequest>(payload)?;
        ParsedCommand::lift("profile", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: ProfileRetireRequest = parse_payload(&command.payload)?;
        let name = name_of(&request.name)?;
        Ok(load(self.store.as_ref(), &name)?.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: ProfileRetireRequest = parse_payload(&command.payload)?;
        let name = name_of(&request.name)?;
        let mut catalogue = catalogue_of(self.store.as_ref())?;
        let entry = catalogue.retire(&name).map_err(refuse)?;
        self.store
            .save(entry, &transition(&name, "retired", json!({})))?;
        announce(effects, LiveEventName::ProfileRetired, entry);
        encode_record(entry)
    }
}

/// Serves `profile.list`.
struct ListProfiles {
    store: Arc<dyn ProfileStore>,
}

impl QueryHandler for ListProfiles {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        parse_payload::<ProfileListQuery>(payload)?;
        let mut profiles: Vec<ProfileRecord> = self.store.list()?.iter().map(record_of).collect();
        profiles.sort_by(|left, right| left.name.cmp(&right.name));
        let response = ProfileListResponse { profiles };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `profile.get`.
struct GetProfile {
    store: Arc<dyn ProfileStore>,
}

impl QueryHandler for GetProfile {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: ProfileGetQuery = parse_payload(payload)?;
        let name = name_of(&query.name)?;
        let entry = load(self.store.as_ref(), &name)?;
        serde_json::to_value(record_of(&entry))
            .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `ticket.assign`.
struct AssignTicket {
    profiles: Arc<dyn ProfileStore>,
    tickets: Arc<dyn TicketStore>,
    projects: Arc<dyn ProjectStore>,
}

impl CommandHandler for AssignTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketAssignRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketAssignRequest = parse_payload(&command.payload)?;
        self.tickets
            .find(kanban_domain::TicketId::new(request.ticket_id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {}", request.ticket_id)))
            .map(|ticket| ticket.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketAssignRequest = parse_payload(&command.payload)?;
        let mut ticket = self
            .tickets
            .find(kanban_domain::TicketId::new(request.ticket_id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {}", request.ticket_id)))?;
        let project: Project = self
            .projects
            .find(ProjectId::new(ticket.project().value()))?
            .ok_or_else(|| {
                ApiError::internal(&format!(
                    "ticket {} belongs to no stored Project",
                    request.ticket_id
                ))
            })?;
        if project.is_archived() {
            return Err(ApiError::invalid_request(
                "archived is terminal; the Project accepts no further changes",
            ));
        }
        let name = name_of(&request.profile)?;
        let catalogue = catalogue_of(self.profiles.as_ref())?;
        if !catalogue.assignable(&name) {
            return Err(refuse(ProfileError::UnknownName {
                name: name.as_str().to_owned(),
            }));
        }
        ticket
            .assign(name.clone())
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        self.tickets
            .save(&ticket, assignment_row(&ticket, &project, &name))?;
        emit_catalogued(
            effects,
            LiveEventName::TicketAssigned,
            &crate::ticket::record_of(&ticket, project.code()),
        );
        serde_json::to_value(crate::ticket::record_of(&ticket, project.code()))
            .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// The timeline row for one assignment: on the Ticket's own Project
/// timeline, about the Ticket, naming the profile the assignment
/// references.
fn assignment_row(
    ticket: &kanban_domain::Ticket,
    project: &Project,
    name: &ProfileName,
) -> TimelineEnvelope {
    TimelineEnvelope::project(
        project.id().value(),
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Ticket,
            id: ticket.id().value().to_string(),
        }),
        json!({
            "action": "assigned",
            "id": ticket.id().value(),
            "profile": name.as_str(),
        }),
    )
}

/// The DTO record for one entry.
fn record_of(entry: &ExecutionProfile) -> ProfileRecord {
    ProfileRecord {
        name: entry.name().as_str().to_owned(),
        harness: entry.harness().to_owned(),
        model: entry.model().to_owned(),
        effort: entry.effort().to_owned(),
        usage_pool: entry.usage_pool().to_owned(),
        fallback: entry.fallback().map(|name| name.as_str().to_owned()),
        retired: entry.is_retired(),
        version: entry.version(),
    }
}

/// Encode a record for a command response.
fn encode_record(entry: &ExecutionProfile) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(entry)).map_err(|error| ApiError::internal(&error.to_string()))
}

/// Publish the change on the live event stream, matching the durable
/// timeline append.
fn announce(effects: &dyn CommandEffects, event: LiveEventName, entry: &ExecutionProfile) {
    emit_catalogued(effects, event, &record_of(entry));
}

/// The duplicate-name refusal a racing define reports.
pub fn duplicate_profile_name_error(name: &str) -> ApiError {
    ApiError::invalid_request(&format!("the profile name `{name}` is already defined"))
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex};

    use kanban_domain::ExecutionProfile;
    use kanban_dto::ApiError;

    use super::ProfileStore;
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::EventSink;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::plan::testing::{MemoryPlans, MemoryProjects};
    use crate::spec::testing::MemorySpecs;
    use crate::ticket::testing::MemoryTickets;
    use crate::timeline::TimelineEnvelope;

    /// An in-memory profile store: rows by name, plus every timeline
    /// append it was asked to land, for assertions.
    #[derive(Default)]
    pub(crate) struct MemoryProfiles {
        state: Mutex<MemoryProfileState>,
    }

    #[derive(Default)]
    struct MemoryProfileState {
        entries: Vec<ExecutionProfile>,
        timeline: Vec<TimelineEnvelope>,
    }

    impl MemoryProfiles {
        /// The stored rows and timeline envelopes, for assertions.
        pub(crate) fn snapshot(&self) -> (Vec<ExecutionProfile>, Vec<TimelineEnvelope>) {
            let state = self.state.lock().expect("the memory profile lock is sound");
            (state.entries.clone(), state.timeline.clone())
        }
    }

    impl ProfileStore for MemoryProfiles {
        fn define(
            &self,
            profile: &ExecutionProfile,
            envelope: &TimelineEnvelope,
        ) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory profile lock is sound");
            if state
                .entries
                .iter()
                .any(|entry| entry.name() == profile.name())
            {
                return Err(super::duplicate_profile_name_error(profile.name().as_str()));
            }
            state.entries.push(profile.clone());
            state.timeline.push(envelope.clone());
            Ok(())
        }

        fn save(
            &self,
            profile: &ExecutionProfile,
            envelope: &TimelineEnvelope,
        ) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory profile lock is sound");
            let preceding = profile.version() - 1;
            let current = state
                .entries
                .iter()
                .position(|entry| entry.name() == profile.name());
            match current {
                Some(index) if state.entries[index].version() == preceding => {
                    state.entries[index] = profile.clone();
                    state.timeline.push(envelope.clone());
                    Ok(())
                }
                Some(index) => Err(ApiError::stale_version(
                    preceding,
                    state.entries[index].version(),
                )),
                None => Err(ApiError::not_found(&format!("profile {}", profile.name()))),
            }
        }

        fn find(
            &self,
            name: &kanban_domain::ProfileName,
        ) -> Result<Option<ExecutionProfile>, ApiError> {
            let state = self.state.lock().expect("the memory profile lock is sound");
            Ok(state
                .entries
                .iter()
                .find(|entry| entry.name() == name)
                .cloned())
        }

        fn list(&self) -> Result<Vec<ExecutionProfile>, ApiError> {
            let state = self.state.lock().expect("the memory profile lock is sound");
            Ok(state.entries.clone())
        }
    }

    /// The recorder the live-event assertions read.
    #[derive(Debug, Default)]
    pub(crate) struct RecordingSink {
        pub(crate) events: Mutex<Vec<(String, serde_json::Value)>>,
    }

    impl EventSink for RecordingSink {
        fn emit(&self, event_type: &str, payload: serde_json::Value) {
            self.events
                .lock()
                .expect("the recorder lock is sound")
                .push((event_type.to_owned(), payload));
        }
    }

    /// A core with the Plan, Spec, Ticket, and catalogue operations
    /// wired to in-memory stores over one active Project.
    fn catalogue_core(
        events: Arc<dyn EventSink>,
    ) -> (Core, Arc<MemoryProfiles>, Arc<MemoryTickets>) {
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
            events,
        );
        core.register_plans(plans.clone(), projects.clone(), specs.clone())
            .expect("the plan operations register");
        core.register_specs(specs.clone(), projects.clone(), plans)
            .expect("the spec operations register");
        core.register_tickets(tickets.clone(), projects.clone(), specs.clone())
            .expect("the ticket operations register");
        core.register_profiles(profiles.clone(), tickets.clone(), projects)
            .expect("the profile operations register");
        (core, profiles, tickets)
    }

    /// The harness the catalogue tests drive.
    pub(crate) struct CatalogueHarness {
        pub(crate) core: Core,
        pub(crate) profiles: Arc<MemoryProfiles>,
    }

    /// A harness with a silent event sink.
    pub(crate) fn catalogue_harness() -> CatalogueHarness {
        let (core, profiles, _) = catalogue_core(Arc::new(crate::events::NoopEventSink));
        CatalogueHarness { core, profiles }
    }

    /// A core and the sink its events land in.
    pub(crate) fn catalogue_core_with_sink() -> (Core, Arc<RecordingSink>) {
        let sink = Arc::new(RecordingSink::default());
        let (core, _, _) = catalogue_core(sink.clone());
        (core, sink)
    }
}

#[cfg(test)]
mod catalogue {
    use serde_json::{Value, json};

    use kanban_dto::ErrorCode;

    /// The catalogue harness: the full ticket surface plus a memory
    /// profile store wired through the profile operations.
    fn harness() -> super::testing::CatalogueHarness {
        super::testing::catalogue_harness()
    }

    fn define(name: &str, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "name": name,
            "harness": "claude-code",
            "model": "opus",
            "effort": "high",
            "usage_pool": "operator",
        })
    }

    fn update(name: &str, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "name": name,
            "harness": "shell-agent",
            "model": "sonnet",
            "effort": "medium",
            "usage_pool": "operator",
        })
    }

    fn retire(name: &str, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "name": name,
        })
    }

    fn assign(profile: &str, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "ticket_id": 1,
            "profile": profile,
        })
    }

    /// One Bug Ticket on the seeded Project, returning its identity.
    fn one_ticket(core: &crate::dispatch::Core) -> u64 {
        let created = core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-ticket" },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "normal",
                    "title": "Landing drops the integration branch",
                }),
            )
            .expect("the Ticket creates");
        created["id"].as_u64().expect("the identity is a number")
    }

    #[test]
    fn defining_returns_the_record_at_version_one() {
        let harness = harness();

        let response = harness
            .core
            .command("profile.define", &define("standard", "key-1", 0))
            .expect("the define applies");

        assert_eq!(
            response,
            json!({
                "name": "standard",
                "harness": "claude-code",
                "model": "opus",
                "effort": "high",
                "usage_pool": "operator",
                "fallback": null,
                "retired": false,
                "version": 1,
            })
        );
    }

    #[test]
    fn defining_refuses_blank_or_incomplete_entries_without_recording() {
        let harness = harness();
        let mut blank_name = define("  ", "key-1", 0);
        blank_name["name"] = json!("   ");
        let error = harness
            .core
            .command("profile.define", &blank_name)
            .expect_err("a blank name is refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest);

        let mut blank_harness_field = define("standard", "key-2", 0);
        blank_harness_field["harness"] = json!(" ");
        let error = harness
            .core
            .command("profile.define", &blank_harness_field)
            .expect_err("a blank harness is refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest);

        assert!(
            harness.profiles.snapshot().0.is_empty(),
            "no row may be written"
        );
        assert!(
            harness.profiles.snapshot().1.is_empty(),
            "no timeline row may be appended"
        );
    }

    #[test]
    fn defining_a_duplicate_name_is_refused() {
        let harness = harness();
        harness
            .core
            .command("profile.define", &define("standard", "key-1", 0))
            .expect("the first define applies");

        let error = harness
            .core
            .command("profile.define", &define("standard", "key-2", 0))
            .expect_err("names are unique");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the profile name `standard` is already defined"
        );
    }

    #[test]
    fn a_fallback_names_an_active_entry_or_nothing() {
        let harness = harness();
        harness
            .core
            .command("profile.define", &define("standard", "key-1", 0))
            .expect("the primary lands");

        let mut unknown = define("nightly", "key-2", 0);
        unknown["fallback"] = json!("ghost");
        let error = harness
            .core
            .command("profile.define", &unknown)
            .expect_err("an unknown fallback is refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the profile name `ghost` is not in the catalogue"
        );

        let mut self_named = define("nightly", "key-3", 0);
        self_named["fallback"] = json!("nightly");
        let error = harness
            .core
            .command("profile.define", &self_named)
            .expect_err("a self fallback is refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest);

        let mut named = define("nightly", "key-4", 0);
        named["fallback"] = json!("standard");
        let response = harness
            .core
            .command("profile.define", &named)
            .expect("a named fallback lands");
        assert_eq!(response["fallback"], json!("standard"));
    }

    #[test]
    fn updating_replaces_the_definition_under_the_same_name() {
        let harness = harness();
        harness
            .core
            .command("profile.define", &define("standard", "key-1", 0))
            .expect("the define applies");

        let response = harness
            .core
            .command("profile.update", &update("standard", "key-2", 1))
            .expect("the update applies");

        assert_eq!(response["harness"], json!("shell-agent"));
        assert_eq!(response["model"], json!("sonnet"));
        assert_eq!(response["version"], json!(2));
        assert_eq!(response["name"], json!("standard"), "names never rename");
    }

    #[test]
    fn updating_with_a_stale_version_is_rejected_with_the_current_one() {
        let harness = harness();
        harness
            .core
            .command("profile.define", &define("standard", "key-1", 0))
            .expect("the define applies");

        let error = harness
            .core
            .command("profile.update", &update("standard", "key-2", 0))
            .expect_err("the stale version is rejected");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(1));
    }

    #[test]
    fn updating_or_retiring_an_unknown_profile_is_not_found() {
        let harness = harness();

        let error = harness
            .core
            .command("profile.update", &update("ghost", "key-1", 1))
            .expect_err("an unknown profile is refused");
        assert_eq!(error.code, ErrorCode::NotFound);

        let error = harness
            .core
            .command("profile.retire", &retire("ghost", "key-2", 1))
            .expect_err("an unknown profile is refused");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn retiring_is_terminal_and_the_list_keeps_the_entry() {
        let harness = harness();
        harness
            .core
            .command("profile.define", &define("standard", "key-1", 0))
            .expect("the define applies");

        let response = harness
            .core
            .command("profile.retire", &retire("standard", "key-2", 1))
            .expect("the retire applies");
        assert_eq!(response["retired"], json!(true));
        assert_eq!(response["version"], json!(2));

        let error = harness
            .core
            .command("profile.retire", &retire("standard", "key-3", 2))
            .expect_err("retiring twice is refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest);

        let listed = harness
            .core
            .query("profile.list", &json!({}))
            .expect("the list serves");
        assert_eq!(
            listed,
            json!({
                "profiles": [
                    {
                        "name": "standard",
                        "harness": "claude-code",
                        "model": "opus",
                        "effort": "high",
                        "usage_pool": "operator",
                        "fallback": null,
                        "retired": true,
                        "version": 2,
                    }
                ]
            }),
            "a retired entry stays listed with every fact"
        );

        let read = harness
            .core
            .query("profile.get", &json!({ "name": "standard" }))
            .expect("the get serves");
        assert_eq!(read["retired"], json!(true));
    }

    #[test]
    fn retiring_a_fallback_target_is_refused() {
        let harness = harness();
        harness
            .core
            .command("profile.define", &define("standard", "key-1", 0))
            .expect("the primary lands");
        let mut named = define("nightly", "key-2", 0);
        named["fallback"] = json!("standard");
        harness
            .core
            .command("profile.define", &named)
            .expect("the secondary names the primary");

        let error = harness
            .core
            .command("profile.retire", &retire("standard", "key-3", 1))
            .expect_err("a fallback target may not retire");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the profile `standard` is still the fallback of another profile"
        );
    }

    #[test]
    fn listing_covers_every_entry_in_name_order() {
        let harness = harness();
        for (name, key) in [("nightly", "key-1"), ("standard", "key-2")] {
            harness
                .core
                .command("profile.define", &define(name, key, 0))
                .expect("the define applies");
        }

        let listed = harness
            .core
            .query("profile.list", &json!({}))
            .expect("the list serves");

        let names: Vec<_> = listed["profiles"]
            .as_array()
            .expect("the profiles are a list")
            .iter()
            .map(|profile| profile["name"].clone())
            .collect();
        assert_eq!(names, vec![json!("nightly"), json!("standard")]);
    }

    #[test]
    fn catalogue_changes_append_rows_and_never_rewrite_history() {
        let harness = harness();
        harness
            .core
            .command("profile.define", &define("standard", "key-1", 0))
            .expect("the define applies");
        harness
            .core
            .command("profile.update", &update("standard", "key-2", 1))
            .expect("the update applies");
        harness
            .core
            .command("profile.retire", &retire("standard", "key-3", 2))
            .expect("the retire applies");

        let recorded: Vec<_> = harness
            .profiles
            .snapshot()
            .1
            .iter()
            .map(|envelope| {
                (
                    *envelope.scope(),
                    envelope.kind(),
                    envelope.entity().cloned(),
                    envelope.detail()["action"].clone(),
                )
            })
            .collect();
        assert_eq!(
            recorded,
            vec![
                (
                    kanban_dto::TimelineScope::Global,
                    kanban_dto::TimelineEventKind::Transition,
                    Some(kanban_dto::TimelineEntityRef {
                        kind: kanban_dto::TimelineEntityKind::Profile,
                        id: "standard".to_owned(),
                    }),
                    json!("defined"),
                ),
                (
                    kanban_dto::TimelineScope::Global,
                    kanban_dto::TimelineEventKind::Transition,
                    Some(kanban_dto::TimelineEntityRef {
                        kind: kanban_dto::TimelineEntityKind::Profile,
                        id: "standard".to_owned(),
                    }),
                    json!("updated"),
                ),
                (
                    kanban_dto::TimelineScope::Global,
                    kanban_dto::TimelineEventKind::Transition,
                    Some(kanban_dto::TimelineEntityRef {
                        kind: kanban_dto::TimelineEntityKind::Profile,
                        id: "standard".to_owned(),
                    }),
                    json!("retired"),
                ),
            ],
            "every change appends its own row; none rewrites another"
        );
    }

    #[test]
    fn catalogue_changes_publish_on_the_event_stream() {
        let (core, sink) = super::testing::catalogue_core_with_sink();
        core.command("profile.define", &define("standard", "key-1", 0))
            .expect("the define applies");

        let events = sink.events.lock().expect("the recorder lock is sound");
        assert_eq!(
            events
                .iter()
                .map(|(name, payload)| (
                    name.as_str(),
                    payload["name"].clone(),
                    payload["retired"].clone(),
                ))
                .collect::<Vec<_>>(),
            vec![("profile.defined", json!("standard"), json!(false))],
            "the applied change announces itself live"
        );
    }

    #[test]
    fn assigning_names_the_profile_and_returns_the_ticket() {
        let harness = harness();
        let ticket = one_ticket(&harness.core);
        harness
            .core
            .command("profile.define", &define("standard", "key-profile", 0))
            .expect("the profile lands");

        let response = harness
            .core
            .command("ticket.assign", &assign("standard", "key-1", 1))
            .expect("the assignment applies");

        assert_eq!(response["id"], json!(ticket));
        assert_eq!(response["profile"], json!("standard"));
        assert_eq!(response["version"], json!(2));
    }

    #[test]
    fn assigning_an_unknown_name_is_refused() {
        let harness = harness();
        one_ticket(&harness.core);

        let error = harness
            .core
            .command("ticket.assign", &assign("ghost", "key-1", 1))
            .expect_err("an unknown name is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the profile name `ghost` is not in the catalogue"
        );
        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": 1 }))
            .expect("the get serves");
        assert_eq!(read["profile"], json!(null), "the refusal changed nothing");
    }

    #[test]
    fn assigning_a_retired_name_is_refused() {
        let harness = harness();
        one_ticket(&harness.core);
        harness
            .core
            .command("profile.define", &define("standard", "key-profile", 0))
            .expect("the profile lands");
        harness
            .core
            .command("profile.retire", &retire("standard", "key-retire", 1))
            .expect("the profile retires");

        let error = harness
            .core
            .command("ticket.assign", &assign("standard", "key-1", 1))
            .expect_err("a retired name is not assignable");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the profile name `standard` is not in the catalogue"
        );
    }

    #[test]
    fn past_assignments_keep_their_names_after_the_catalogue_changes() {
        let harness = harness();
        one_ticket(&harness.core);
        harness
            .core
            .command("profile.define", &define("standard", "key-profile", 0))
            .expect("the profile lands");
        harness
            .core
            .command("ticket.assign", &assign("standard", "key-assign", 1))
            .expect("the assignment applies");

        harness
            .core
            .command(
                "profile.update",
                &json!({
                    "mutation": { "optimistic_version": 1, "idempotency_key": "key-update" },
                    "name": "standard",
                    "harness": "shell-agent",
                    "model": "sonnet",
                    "effort": "medium",
                    "usage_pool": "operator",
                }),
            )
            .expect("the definition changes");
        harness
            .core
            .command("profile.retire", &retire("standard", "key-retire", 2))
            .expect("the entry retires");

        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": 1 }))
            .expect("the get serves");
        assert_eq!(
            read["profile"],
            json!("standard"),
            "the assignment keeps the name it referenced through every catalogue change"
        );
    }

    #[test]
    fn assigning_to_an_unknown_ticket_is_not_found_and_a_stale_version_is_rejected() {
        let harness = harness();
        harness
            .core
            .command("profile.define", &define("standard", "key-profile", 0))
            .expect("the profile lands");

        let mut elsewhere = assign("standard", "key-1", 1);
        elsewhere["ticket_id"] = json!(9);
        let error = harness
            .core
            .command("ticket.assign", &elsewhere)
            .expect_err("the unknown Ticket is refused");
        assert_eq!(error.code, ErrorCode::NotFound);

        one_ticket(&harness.core);
        let error = harness
            .core
            .command("ticket.assign", &assign("standard", "key-2", 0))
            .expect_err("the stale version is rejected");
        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(1));
    }

    #[test]
    fn assignment_appends_a_project_timeline_row_and_publishes_live() {
        let (core, sink) = super::testing::catalogue_core_with_sink();
        one_ticket(&core);
        core.command("profile.define", &define("standard", "key-profile", 0))
            .expect("the profile lands");
        core.command("ticket.assign", &assign("standard", "key-1", 1))
            .expect("the assignment applies");

        let events = sink.events.lock().expect("the recorder lock is sound");
        assert_eq!(
            events
                .iter()
                .filter(|(name, _)| name == "ticket.assigned")
                .map(|(_, payload)| (payload["id"].clone(), payload["profile"].clone()))
                .collect::<Vec<_>>(),
            vec![(json!(1), json!("standard"))],
            "the assignment announces itself live with the named profile"
        );
    }

    #[test]
    fn a_retry_replays_without_reapplying() {
        let harness = harness();
        one_ticket(&harness.core);
        harness
            .core
            .command("profile.define", &define("standard", "key-profile", 0))
            .expect("the profile lands");
        let request = assign("standard", "key-1", 1);

        let first = harness
            .core
            .command("ticket.assign", &request)
            .expect("the assignment applies");
        let replay = harness
            .core
            .command("ticket.assign", &request)
            .expect("the retry replays");

        assert_eq!(first, replay);
        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": 1 }))
            .expect("the get serves");
        assert_eq!(read["version"], json!(2), "the retry must not reapply");
    }

    #[test]
    fn every_command_rejects_unknown_fields() {
        let harness = harness();
        let mut request = define("standard", "key-1", 0);
        request["surprise"] = json!(true);

        let error = harness
            .core
            .command("profile.define", &request)
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }

    #[test]
    fn no_profile_delete_operation_is_catalogued() {
        let names: Vec<_> = crate::catalog::exposed_operations()
            .iter()
            .map(|operation| operation.name)
            .collect();
        assert!(
            !names.contains(&"profile.delete") && !names.contains(&"profile.remove"),
            "profiles are retired, never deleted"
        );
    }
}
