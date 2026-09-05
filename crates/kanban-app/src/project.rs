//! Project commands and the query behind the register surface:
//! register, archive, and list (KAN-S1-US4, KAN-S1-US5, KAN-S1-US6).
//! Registration anchors a Project to exactly one Git repository, one
//! Seed Workspace, one default branch, and one required target Herdr
//! workspace with an optional Herdr session whose absence selects
//! Herdr's default session, optionally under one Initiative; every
//! change appends a timeline event in the same write, archived is
//! terminal, and no delete exists.

use std::sync::Arc;

use kanban_domain::{InitiativeId, NumberKind, Project, ProjectId, ProjectRegistration};
use kanban_dto::{
    ApiError, ProjectArchiveRequest, ProjectCounters, ProjectListQuery, ProjectListResponse,
    ProjectRecord, ProjectRegisterRequest, TimelineEntityKind, TimelineEntityRef,
    TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::event_catalog::{EventDescriptor, event_descriptor};
use crate::events::{EventSink, emit_catalogued};
use crate::herdr::{HerdrProjectObserver, HerdrSettingsStore};
use crate::initiative::InitiativeStore;
use crate::mutation::{CommandHandler, ParsedCommand, parse_payload};
use crate::timeline::TimelineEnvelope;

/// The git observation port: how registration confirms a target is a
/// Git repository. The service wires the real filesystem observation;
/// every implementation refusing everything else keeps non-Git
/// Projects out (DR-PH-08).
pub trait GitObservation: Send + Sync {
    /// Whether `repository` names a Git repository a Project may
    /// anchor to.
    fn is_repository(&self, repository: &str) -> bool;
}

/// The storage port Project commands call through. Implementations
/// insert the timeline envelope unchanged inside the same write as
/// the row.
pub trait ProjectStore: Send + Sync {
    /// Insert a fresh Project from a validated registration. Storage
    /// assigns its identity and asks `envelope` for the timeline row
    /// that identity belongs in. Duplicate codes are refused, so
    /// codes stay globally unique.
    fn create(
        &self,
        registration: &ProjectRegistration,
        envelope: &dyn Fn(ProjectId) -> TimelineEnvelope,
    ) -> Result<Project, ApiError>;
    /// Load one Project, if it exists.
    fn find(&self, id: ProjectId) -> Result<Option<Project>, ApiError>;
    /// Persist an applied transition and its timeline envelope.
    fn save(&self, project: &Project, envelope: TimelineEnvelope) -> Result<(), ApiError>;
    /// Every Project in id order, archived included.
    fn list(&self) -> Result<Vec<Project>, ApiError>;
}

/// The stable refusal for a code another Project already holds, so
/// every store refuses duplicates with one voice.
pub fn duplicate_code_error(code: &str) -> ApiError {
    ApiError::invalid_request(&format!("the project code `{code}` is already registered"))
}

/// The timeline row for one Project transition. A Project's own
/// history belongs on that Project's timeline, named by its identity,
/// and `action` names which transition it was inside the closed
/// `transition` kind, so every row decodes on the way back out.
fn transition(id: ProjectId, action: &str, facts: Value) -> TimelineEnvelope {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("Project transition facts are a JSON object");
    object.insert("action".to_owned(), Value::from(action));
    object.insert("id".to_owned(), Value::from(id.value()));
    let identity = id.value().to_string();
    TimelineEnvelope::project(
        &identity,
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Project,
            id: identity.clone(),
        }),
        detail,
    )
    .expect("a minted Project identity names a Project")
}

impl Core {
    /// Register the Project operations against `projects`, observing
    /// Git targets through `git` and resolving Initiative links
    /// through `initiatives`.
    pub fn register_projects(
        &mut self,
        projects: Arc<dyn ProjectStore>,
        git: Arc<dyn GitObservation>,
        initiatives: Arc<dyn InitiativeStore>,
        herdr_settings: Arc<dyn HerdrSettingsStore>,
        herdr_observer: Arc<dyn HerdrProjectObserver>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "project.register",
            Arc::new(RegisterProject {
                store: projects.clone(),
                git,
                initiatives,
                herdr_settings,
                herdr_observer: herdr_observer.clone(),
            }),
        )?;
        self.register_command(
            "project.archive",
            Arc::new(ArchiveProject {
                store: projects.clone(),
                herdr_observer,
            }),
        )?;
        self.register_query("project.list", Arc::new(ListProjects { store: projects }))?;
        Ok(())
    }
}

/// Serves `project.register`.
struct RegisterProject {
    store: Arc<dyn ProjectStore>,
    git: Arc<dyn GitObservation>,
    initiatives: Arc<dyn InitiativeStore>,
    herdr_settings: Arc<dyn HerdrSettingsStore>,
    herdr_observer: Arc<dyn HerdrProjectObserver>,
}

impl CommandHandler for RegisterProject {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ProjectRegisterRequest>(payload)?;
        ParsedCommand::lift("project", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        // A fresh aggregate is created at version 0.
        Ok(0)
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: ProjectRegisterRequest = parse_payload(&command.payload)?;
        let registration = ProjectRegistration::new(
            &request.code,
            &request.name,
            &request.repository,
            &request.seed_workspace,
            &request.default_branch,
            &request.herdr_workspace,
            request.herdr_session.as_deref(),
            request.initiative_id.map(InitiativeId::new),
        )
        .map_err(|error| ApiError::invalid_request(&error.to_string()))?;

        if !self.git.is_repository(registration.repository()) {
            return Err(ApiError::invalid_request(&format!(
                "the target repository at `{}` is not a Git repository",
                registration.repository()
            )));
        }
        if let Some(initiative) = registration.initiative()
            && self.initiatives.find(initiative)?.is_none()
        {
            return Err(ApiError::not_found(&format!(
                "initiative {}",
                initiative.value()
            )));
        }

        let facts = json!({
            "code": registration.code().as_str(),
            "name": registration.name(),
            "repository": registration.repository(),
            "seed_workspace": registration.seed_workspace(),
            "default_branch": registration.default_branch(),
            "herdr_workspace": registration.herdr_workspace(),
            "herdr_session": registration.herdr_session(),
            "initiative_id": registration.initiative().map(InitiativeId::value),
        });
        let project = self.store.create(&registration, &|id| {
            transition(id, "registered", facts.clone())
        })?;
        self.herdr_settings
            .seed_project_settings(project.id().value())?;
        self.herdr_observer.observe(&project);
        announce(events, event_descriptor("project.registered"), &project);
        encode_record(&project)
    }
}

/// Serves `project.archive`.
struct ArchiveProject {
    store: Arc<dyn ProjectStore>,
    herdr_observer: Arc<dyn HerdrProjectObserver>,
}

impl CommandHandler for ArchiveProject {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ProjectArchiveRequest>(payload)?;
        ParsedCommand::lift("project", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: ProjectArchiveRequest = parse_payload(&command.payload)?;
        let project = load(&self.store, request.project_id)?;
        Ok(project.version())
    }

    fn apply(&self, command: &ParsedCommand, events: &dyn EventSink) -> Result<Value, ApiError> {
        let request: ProjectArchiveRequest = parse_payload(&command.payload)?;
        let mut project = load(&self.store, request.project_id)?;
        project
            .archive()
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        self.store
            .save(&project, transition(project.id(), "archived", json!({})))?;
        // The archive landed, so the session it anchored stops being
        // observed: the owner releases its socket and thread.
        self.herdr_observer.stop_observing(project.id().value());
        announce(events, event_descriptor("project.archived"), &project);
        encode_record(&project)
    }
}

/// Serves `project.list`.
struct ListProjects {
    store: Arc<dyn ProjectStore>,
}

impl QueryHandler for ListProjects {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        parse_payload::<ProjectListQuery>(payload)?;
        let response = ProjectListResponse {
            projects: self.store.list()?.iter().map(record_of).collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// The Project a command addresses, or the stable not-found refusal.
fn load(store: &Arc<dyn ProjectStore>, id: u64) -> Result<Project, ApiError> {
    store
        .find(ProjectId::new(id))?
        .ok_or_else(|| ApiError::not_found(&format!("project {id}")))
}

/// The DTO record for one Project.
fn record_of(project: &Project) -> ProjectRecord {
    let registration = project.registration();
    ProjectRecord {
        id: project.id().value(),
        code: project.code().as_str().to_owned(),
        name: registration.name().to_owned(),
        repository: registration.repository().to_owned(),
        seed_workspace: registration.seed_workspace().to_owned(),
        default_branch: registration.default_branch().to_owned(),
        herdr_session: registration.herdr_session().map(str::to_owned),
        herdr_workspace: registration.herdr_workspace().to_owned(),
        initiative_id: registration.initiative().map(|id| id.value()),
        archived: project.is_archived(),
        counters: ProjectCounters {
            plan: project.counters().last(NumberKind::Plan),
            spec: project.counters().last(NumberKind::Spec),
            ticket: project.counters().last(NumberKind::Ticket),
        },
        version: project.version(),
    }
}

/// Encode a record for a command response.
fn encode_record(project: &Project) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(project)).map_err(|error| ApiError::internal(&error.to_string()))
}

/// Publish the change on the live event stream as exactly the record
/// the command returns, matching the durable timeline append.
fn announce(events: &dyn EventSink, event: &EventDescriptor, project: &Project) {
    emit_catalogued(events, event, &record_of(project));
}

#[cfg(test)]
pub(crate) mod testing {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use kanban_domain::{
        Initiative, InitiativeId, InitiativeName, Project, ProjectCounters, ProjectId,
        ProjectRegistration, ProjectState,
    };
    use kanban_dto::ApiError;
    use serde_json::{Value, json};

    use super::{GitObservation, ProjectStore, duplicate_code_error};
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::EventSink;
    use crate::herdr::{HerdrProjectObserver, HerdrSettingsStore};
    use crate::initiative::InitiativeStore;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::timeline::TimelineEnvelope;

    /// The git observation the tests steer: a fixed set of known
    /// repositories. An empty set refuses every target.
    #[derive(Default)]
    pub(crate) struct KnownRepositories {
        pub(crate) repositories: Vec<String>,
    }

    impl GitObservation for KnownRepositories {
        fn is_repository(&self, repository: &str) -> bool {
            self.repositories.iter().any(|known| known == repository)
        }
    }

    /// An in-memory Project store: rows by id, every uniqueness rule,
    /// plus every timeline envelope it was asked to land.
    #[derive(Default)]
    pub(crate) struct MemoryProjectStore {
        state: Mutex<MemoryState>,
    }

    #[derive(Default)]
    struct MemoryState {
        projects: Vec<Project>,
        next_id: u64,
        timeline: Vec<TimelineEnvelope>,
    }

    impl MemoryProjectStore {
        /// The stored rows and timeline envelopes, for assertions.
        pub(super) fn snapshot(&self) -> (Vec<Project>, Vec<TimelineEnvelope>) {
            let state = self.state.lock().expect("the memory store lock is sound");
            (state.projects.clone(), state.timeline.clone())
        }

        /// Insert a stored Project as-is, standing in for a Project
        /// with minted numbers.
        pub(crate) fn seed(&self, project: Project) {
            self.state
                .lock()
                .expect("the memory store lock is sound")
                .projects
                .push(project)
        }
    }

    impl ProjectStore for MemoryProjectStore {
        fn create(
            &self,
            registration: &ProjectRegistration,
            envelope: &dyn Fn(ProjectId) -> TimelineEnvelope,
        ) -> Result<Project, ApiError> {
            let mut state = self.state.lock().expect("the memory store lock is sound");
            if state
                .projects
                .iter()
                .any(|stored| stored.code() == registration.code())
            {
                return Err(duplicate_code_error(registration.code().as_str()));
            }
            state.next_id += 1;
            let id = ProjectId::new(state.next_id);
            let project = Project::new(id, registration.clone());
            state.projects.push(project.clone());
            state.timeline.push(envelope(id));
            Ok(project)
        }

        fn find(&self, id: ProjectId) -> Result<Option<Project>, ApiError> {
            let state = self.state.lock().expect("the memory store lock is sound");
            Ok(state.projects.iter().find(|row| row.id() == id).cloned())
        }

        fn save(&self, project: &Project, envelope: TimelineEnvelope) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory store lock is sound");
            let id = project.id();
            if let Some(row) = state.projects.iter_mut().find(|row| row.id() == id) {
                *row = project.clone();
            }
            state.timeline.push(envelope);
            Ok(())
        }

        fn list(&self) -> Result<Vec<Project>, ApiError> {
            let state = self.state.lock().expect("the memory store lock is sound");
            Ok(state.projects.clone())
        }
    }

    /// An in-memory Initiative store, so registrations can link to
    /// Initiatives that exist.
    #[derive(Default)]
    pub(crate) struct MemoryInitiatives {
        initiatives: Mutex<Vec<Initiative>>,
    }

    impl MemoryInitiatives {
        /// Create one Initiative and return its identity.
        pub(super) fn seed(&self, name: &str) -> InitiativeId {
            let mut initiatives = self
                .initiatives
                .lock()
                .expect("the memory initiatives lock is sound");
            let id = InitiativeId::new(initiatives.len() as u64 + 1);
            initiatives.push(Initiative::new(
                id,
                InitiativeName::new(name).expect("the fixture name validates"),
            ));
            id
        }
    }

    impl InitiativeStore for MemoryInitiatives {
        fn create(
            &self,
            name: &InitiativeName,
            _envelope: &dyn Fn(InitiativeId) -> TimelineEnvelope,
        ) -> Result<Initiative, ApiError> {
            let mut initiatives = self
                .initiatives
                .lock()
                .expect("the memory initiatives lock is sound");
            let id = InitiativeId::new(initiatives.len() as u64 + 1);
            let initiative = Initiative::new(id, name.clone());
            initiatives.push(initiative.clone());
            Ok(initiative)
        }

        fn find(&self, id: InitiativeId) -> Result<Option<Initiative>, ApiError> {
            let initiatives = self
                .initiatives
                .lock()
                .expect("the memory initiatives lock is sound");
            Ok(initiatives.iter().find(|row| row.id() == id).cloned())
        }

        fn save(
            &self,
            initiative: &Initiative,
            _envelope: TimelineEnvelope,
        ) -> Result<(), ApiError> {
            let mut initiatives = self
                .initiatives
                .lock()
                .expect("the memory initiatives lock is sound");
            if let Some(row) = initiatives
                .iter_mut()
                .find(|row| row.id() == initiative.id())
            {
                *row = initiative.clone();
            }
            Ok(())
        }

        fn list(&self) -> Result<Vec<Initiative>, ApiError> {
            let initiatives = self
                .initiatives
                .lock()
                .expect("the memory initiatives lock is sound");
            Ok(initiatives.clone())
        }
    }

    #[derive(Debug, Default)]
    pub(super) struct RecordingSink {
        pub(super) events: Mutex<Vec<(String, Value)>>,
    }

    impl EventSink for RecordingSink {
        fn emit(&self, event_type: &str, payload: Value) {
            self.events
                .lock()
                .expect("the recorder lock is sound")
                .push((event_type.to_owned(), payload));
        }
    }

    /// The Herdr observer the tests steer: it records every start and
    /// stop it was asked for.
    #[derive(Default)]
    pub(super) struct RecordingHerdrObserver {
        pub(super) calls: Mutex<Vec<(&'static str, u64)>>,
    }

    impl HerdrProjectObserver for RecordingHerdrObserver {
        fn observe(&self, project: &Project) {
            self.calls
                .lock()
                .expect("the recorder lock is sound")
                .push(("observe", project.id().value()));
        }

        fn stop_observing(&self, project_id: u64) {
            self.calls
                .lock()
                .expect("the recorder lock is sound")
                .push(("stop", project_id));
        }
    }

    /// A core with the Project and Initiative operations wired to
    /// in-memory stores and one known repository.
    pub(crate) struct Harness {
        pub(crate) projects: Arc<MemoryProjectStore>,
        pub(crate) initiatives: Arc<MemoryInitiatives>,
        pub(crate) core: Core,
    }

    pub(crate) struct MemoryHerdrSettings {
        projects: Mutex<HashMap<u64, kanban_dto::HerdrProjectSettings>>,
        defaults: kanban_dto::HerdrGlobalDefaults,
    }

    impl Default for MemoryHerdrSettings {
        fn default() -> Self {
            Self {
                projects: Mutex::new(HashMap::new()),
                defaults: kanban_dto::HerdrGlobalDefaults {
                    reconciliation_interval_secs: 300,
                    stall_deadline_secs: 3600,
                    missing_result_deadline_secs: 7200,
                    version: 1,
                },
            }
        }
    }

    impl HerdrSettingsStore for MemoryHerdrSettings {
        fn global_defaults(&self) -> Result<kanban_dto::HerdrGlobalDefaults, ApiError> {
            Ok(self.defaults.clone())
        }

        fn update_global_defaults(
            &self,
            _request: &kanban_dto::HerdrDefaultsUpdateRequest,
        ) -> Result<kanban_dto::HerdrGlobalDefaults, ApiError> {
            Err(ApiError::internal("not implemented in project tests"))
        }

        fn project_settings(
            &self,
            project_id: u64,
        ) -> Result<kanban_dto::HerdrProjectSettings, ApiError> {
            self.projects
                .lock()
                .unwrap()
                .get(&project_id)
                .cloned()
                .ok_or_else(|| {
                    ApiError::not_found(&format!("herdr settings for project {project_id}"))
                })
        }

        fn update_project_settings(
            &self,
            _request: &kanban_dto::HerdrSettingsUpdateRequest,
        ) -> Result<kanban_dto::HerdrProjectSettings, ApiError> {
            Err(ApiError::internal("not implemented in project tests"))
        }

        fn seed_project_settings(&self, project_id: u64) -> Result<(), ApiError> {
            let defaults = self.global_defaults()?;
            self.projects.lock().unwrap().insert(
                project_id,
                kanban_dto::HerdrProjectSettings {
                    reconciliation_interval_secs: defaults.reconciliation_interval_secs,
                    polling_fallback_enabled: false,
                    polling_fallback_interval_secs: 10,
                    stall_deadline_secs: defaults.stall_deadline_secs,
                    missing_result_deadline_secs: defaults.missing_result_deadline_secs,
                    version: 1,
                },
            );
            Ok(())
        }
    }

    pub(crate) fn harness() -> Harness {
        harness_with_observing(
            KnownRepositories {
                repositories: vec!["/repositories/kanban".to_owned()],
            },
            Arc::new(crate::events::NoopEventSink),
        )
    }

    /// A core whose git observation and event sink the test chooses.
    pub(super) fn harness_with_observing(
        git: KnownRepositories,
        events: Arc<dyn EventSink>,
    ) -> Harness {
        harness_with_parts(
            git,
            events,
            Arc::new(crate::herdr::NoopHerdrProjectObserver),
        )
    }

    /// A core observing the one known repository whose Herdr observer
    /// the test chooses.
    pub(super) fn harness_with_herdr(herdr: Arc<dyn HerdrProjectObserver>) -> Harness {
        harness_with_parts(
            KnownRepositories {
                repositories: vec!["/repositories/kanban".to_owned()],
            },
            Arc::new(crate::events::NoopEventSink),
            herdr,
        )
    }

    fn harness_with_parts(
        git: KnownRepositories,
        events: Arc<dyn EventSink>,
        herdr: Arc<dyn HerdrProjectObserver>,
    ) -> Harness {
        let projects = Arc::new(MemoryProjectStore::default());
        let initiatives = Arc::new(MemoryInitiatives::default());
        let herdr_settings = Arc::new(MemoryHerdrSettings::default());
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            events,
        );
        core.register_initiatives(initiatives.clone())
            .expect("the initiative operations register");
        core.register_projects(
            projects.clone(),
            Arc::new(git),
            initiatives.clone(),
            herdr_settings.clone(),
            herdr,
        )
        .expect("the project operations register");
        Harness {
            projects,
            initiatives,
            core,
        }
    }

    /// A register request observing the one known repository, with
    /// the fields tests vary.
    /// Mirrors the registration command's payload, so its parameters
    /// match the request fields exactly.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn register(
        code: &str,
        name: &str,
        seed_workspace: &str,
        default_branch: &str,
        herdr_workspace: &str,
        herdr_session: Option<&str>,
        initiative_id: Option<u64>,
        key: &str,
    ) -> Value {
        json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "code": code,
            "name": name,
            "repository": "/repositories/kanban",
            "seed_workspace": seed_workspace,
            "default_branch": default_branch,
            "herdr_workspace": herdr_workspace,
            "herdr_session": herdr_session,
            "initiative_id": initiative_id,
        })
    }

    /// The standard registration, with the code, session, and key
    /// tests vary: the session is named when given and omitted for
    /// `None`.
    pub(crate) fn registering(code: &str, session: Option<&str>, key: &str) -> Value {
        register(
            code,
            "Control plane",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            session,
            None,
            key,
        )
    }

    /// The standard registration's observable record.
    pub(super) fn registered() -> Value {
        json!({
            "id": 1,
            "code": "CORE",
            "name": "Control plane",
            "repository": "/repositories/kanban",
            "seed_workspace": "/workspaces/kanban.seed",
            "default_branch": "main",
            "herdr_session": "kanban-main",
            "herdr_workspace": "kanban.seed",
            "initiative_id": null,
            "archived": false,
            "counters": { "plan": 0, "spec": 0, "ticket": 0 },
            "version": 1,
        })
    }

    /// The standard registration's observable record, without a
    /// session: absence selects Herdr's default session.
    pub(super) fn registered_without_session() -> Value {
        let mut record = registered();
        record["herdr_session"] = Value::Null;
        record
    }

    /// One stored Project with minted counters, active at version 1.
    pub(super) fn stored_project(id: u64, code: &str, session: &str) -> Project {
        let registration = ProjectRegistration::new(
            code,
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some(session),
            None,
        )
        .expect("the fixture registration validates");
        Project::restore(
            ProjectId::new(id),
            registration,
            ProjectState::Active,
            ProjectCounters::restore(2, 0, 5),
            1,
        )
    }

    /// An archive request against `id`.
    pub(super) fn archive(id: u64, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "project_id": id,
        })
    }
}

#[cfg(test)]
mod project_registration {
    use kanban_dto::{
        ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineScope,
    };
    use serde_json::json;

    use std::sync::Arc;

    use super::testing::{
        Harness, KnownRepositories, RecordingSink, harness, harness_with_observing, register,
        registered, registered_without_session, registering,
    };
    use crate::catalog::exposed_operations;

    /// A core observing no repository, so every target is non-Git.
    fn blind_core() -> Harness {
        harness_with_observing(
            KnownRepositories::default(),
            Arc::new(crate::events::NoopEventSink),
        )
    }

    #[test]
    fn registering_returns_the_active_record_at_version_one() {
        let harness = harness();

        let response = harness
            .core
            .command(
                "project.register",
                &registering("CORE", Some("kanban-main"), "key-1"),
            )
            .expect("the registration applies");

        assert_eq!(response, registered());
    }

    #[test]
    fn registering_under_an_initiative_links_it() {
        let harness = harness();
        let initiative = harness.initiatives.seed("Reliability");

        let response = harness
            .core
            .command(
                "project.register",
                &register(
                    "CORE",
                    "Control plane",
                    "/workspaces/kanban.seed",
                    "main",
                    "kanban.seed",
                    Some("kanban-main"),
                    Some(initiative.value()),
                    "key-1",
                ),
            )
            .expect("the registration applies");

        assert_eq!(response["initiative_id"], json!(initiative.value()));
    }

    #[test]
    fn registering_an_unknown_initiative_is_not_found() {
        let harness = harness();

        let error = harness
            .core
            .command(
                "project.register",
                &register(
                    "CORE",
                    "Control plane",
                    "/workspaces/kanban.seed",
                    "main",
                    "kanban.seed",
                    Some("kanban-main"),
                    Some(9),
                    "key-1",
                ),
            )
            .expect_err("the unknown Initiative is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(error.message.contains("initiative 9"));
    }

    #[test]
    fn registering_refuses_a_non_git_target_without_recording_anything() {
        let harness = blind_core();

        let error = harness
            .core
            .command(
                "project.register",
                &registering("CORE", Some("kanban-main"), "key-1"),
            )
            .expect_err("a non-Git target is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the target repository at `/repositories/kanban` is not a Git repository"
        );
        let (rows, timeline) = harness.projects.snapshot();
        assert!(rows.is_empty(), "no row may be written");
        assert!(timeline.is_empty(), "no timeline event may be appended");
    }

    #[test]
    fn registering_allows_a_shared_session_name() {
        let harness = harness();
        harness
            .core
            .command(
                "project.register",
                &registering("CORE", Some("kanban-main"), "key-1"),
            )
            .expect("the first registration applies");

        let shared = harness
            .core
            .command(
                "project.register",
                &registering("WAVE", Some("kanban-main"), "key-2"),
            )
            .expect("session names are no longer exclusive");

        assert_eq!(shared["code"], json!("WAVE"));
        assert_eq!(shared["herdr_session"], json!("kanban-main"));
        let (rows, timeline) = harness.projects.snapshot();
        assert_eq!(rows.len(), 2);
        assert_eq!(timeline.len(), 2);
    }

    #[test]
    fn registering_without_a_session_selects_the_default_session() {
        let harness = harness();

        let response = harness
            .core
            .command("project.register", &registering("CORE", None, "key-1"))
            .expect("an omitted session registers");

        assert_eq!(response, registered_without_session());
    }

    #[test]
    fn registering_refuses_a_duplicate_code() {
        let harness = harness();
        harness
            .core
            .command(
                "project.register",
                &registering("CORE", Some("kanban-main"), "key-1"),
            )
            .expect("the first registration applies");

        let error = harness
            .core
            .command(
                "project.register",
                &registering("CORE", Some("wave-main"), "key-2"),
            )
            .expect_err("the duplicate code is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the project code `CORE` is already registered"
        );
        let (rows, _) = harness.projects.snapshot();
        assert_eq!(rows.len(), 1, "codes are minted once");
    }

    #[test]
    fn registering_refuses_the_reserved_code() {
        let harness = harness();

        let error = harness
            .core
            .command(
                "project.register",
                &registering("KAN", Some("kanban-main"), "key-1"),
            )
            .expect_err("the product code is reserved");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "KAN is reserved for this product");
    }

    #[test]
    fn registering_refuses_a_malformed_code() {
        let harness = harness();

        let error = harness
            .core
            .command(
                "project.register",
                &registering("core", Some("kanban-main"), "key-1"),
            )
            .expect_err("the malformed code is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "a Project code must match [A-Z][A-Z0-9]{1,7} in full"
        );
    }

    #[test]
    fn registering_refuses_a_blank_anchor() {
        let harness = harness();

        let error = harness
            .core
            .command(
                "project.register",
                &register(
                    "CORE",
                    " ",
                    "/workspaces/kanban.seed",
                    "main",
                    "kanban.seed",
                    Some("kanban-main"),
                    None,
                    "key-1",
                ),
            )
            .expect_err("a blank name is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "a Project name cannot be blank");
    }

    #[test]
    fn registering_rejects_unknown_fields() {
        let harness = harness();
        let mut request = registering("CORE", Some("kanban-main"), "key-1");
        request["surprise"] = json!(true);

        let error = harness
            .core
            .command("project.register", &request)
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }

    #[test]
    fn registering_replays_a_retry_without_reapplying() {
        let harness = harness();
        let request = registering("CORE", Some("kanban-main"), "key-1");

        let first = harness
            .core
            .command("project.register", &request)
            .expect("the first attempt applies");
        let replay = harness
            .core
            .command("project.register", &request)
            .expect("the retry replays");

        assert_eq!(first, replay);
        let (rows, timeline) = harness.projects.snapshot();
        assert_eq!(rows.len(), 1, "the retry must not have applied again");
        assert_eq!(timeline.len(), 1);
    }

    #[test]
    fn registering_records_a_transition_on_the_projects_own_timeline() {
        let harness = harness();
        harness
            .core
            .command(
                "project.register",
                &registering("CORE", Some("kanban-main"), "key-1"),
            )
            .expect("the registration applies");

        let (_, timeline) = harness.projects.snapshot();
        assert_eq!(
            timeline
                .iter()
                .map(|envelope| (
                    envelope.scope().clone(),
                    envelope.kind(),
                    envelope.entity().cloned(),
                    envelope.detail().clone(),
                ))
                .collect::<Vec<_>>(),
            vec![(
                TimelineScope::Project("1".to_owned()),
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Project,
                    id: "1".to_owned(),
                }),
                json!({
                    "code": "CORE",
                    "name": "Control plane",
                    "repository": "/repositories/kanban",
                    "seed_workspace": "/workspaces/kanban.seed",
                    "default_branch": "main",
                    "herdr_workspace": "kanban.seed",
                    "herdr_session": "kanban-main",
                    "initiative_id": null,
                    "action": "registered",
                    "id": 1,
                }),
            )],
            "the registration lands one closed-vocabulary row on the Project's timeline"
        );
    }

    #[test]
    fn registering_publishes_on_the_event_stream() {
        let sink = Arc::new(RecordingSink::default());
        let harness = harness_with_observing(
            KnownRepositories {
                repositories: vec!["/repositories/kanban".to_owned()],
            },
            sink.clone(),
        );

        harness
            .core
            .command(
                "project.register",
                &registering("CORE", Some("kanban-main"), "key-1"),
            )
            .expect("the registration applies");

        let events = sink.events.lock().expect("the recorder lock is sound");
        assert_eq!(
            events.as_slice(),
            [("project.registered".to_owned(), registered())],
            "the applied change announces itself live"
        );
    }

    #[test]
    fn no_project_delete_operation_is_catalogued() {
        for operation in exposed_operations() {
            assert!(
                !operation.name.starts_with("project.")
                    || (!operation.name.contains("delete") && !operation.name.contains("remove")),
                "`{}` must not exist; Projects are archived, never deleted",
                operation.name
            );
        }
    }
}

#[cfg(test)]
mod project_lifecycle {
    use std::sync::Arc;

    use kanban_dto::{
        ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineScope,
    };
    use serde_json::json;

    use super::testing::{
        RecordingHerdrObserver, archive, harness, harness_with_herdr, registering, stored_project,
    };

    #[test]
    fn archiving_stops_herdr_observation() {
        let observer = Arc::new(RecordingHerdrObserver::default());
        let harness = harness_with_herdr(observer.clone());
        harness
            .core
            .command(
                "project.register",
                &registering("CORE", "kanban-main", "key-1"),
            )
            .expect("the registration applies");

        harness
            .core
            .command("project.archive", &archive(1, "key-2", 1))
            .expect("the archive applies");

        let calls = observer.calls.lock().expect("the recorder lock is sound");
        assert_eq!(
            *calls,
            vec![("observe", 1), ("stop", 1)],
            "registration starts observation and the landed archive releases it"
        );
    }

    #[test]
    fn a_refused_archive_does_not_stop_observation_again() {
        let observer = Arc::new(RecordingHerdrObserver::default());
        let harness = harness_with_herdr(observer.clone());
        harness
            .projects
            .seed(stored_project(1, "CORE", "kanban-main"));
        harness
            .core
            .command("project.archive", &archive(1, "key-1", 1))
            .expect("the first archive applies");

        let refused = harness
            .core
            .command("project.archive", &archive(1, "key-2", 2))
            .expect_err("the second archive is refused");
        assert_eq!(refused.code, ErrorCode::InvalidRequest);
        let unknown = harness
            .core
            .command("project.archive", &archive(9, "key-3", 0))
            .expect_err("the unknown Project is refused");
        assert_eq!(unknown.code, ErrorCode::NotFound);

        let calls = observer.calls.lock().expect("the recorder lock is sound");
        assert_eq!(
            *calls,
            vec![("stop", 1)],
            "only the landed archive stops observation"
        );
    }

    #[test]
    fn archiving_is_terminal_and_preserves_every_fact() {
        let harness = harness();
        harness
            .core
            .command(
                "project.register",
                &registering("CORE", Some("kanban-main"), "key-1"),
            )
            .expect("the registration applies");

        let response = harness
            .core
            .command("project.archive", &archive(1, "key-2", 1))
            .expect("the archive applies");

        assert_eq!(response["archived"], json!(true));
        assert_eq!(response["code"], json!("CORE"), "the code survives");
        assert_eq!(
            response["counters"],
            json!({ "plan": 0, "spec": 0, "ticket": 0 }),
            "the counters survive"
        );
        let listed = harness
            .core
            .query("project.list", &json!({}))
            .expect("the list serves");
        assert_eq!(
            listed["projects"][0]["archived"],
            json!(true),
            "an archived Project stays listed"
        );
        assert_eq!(listed["projects"][0]["code"], json!("CORE"));
        assert_eq!(listed["projects"][0]["version"], json!(2));
    }

    #[test]
    fn archiving_preserves_minted_counters() {
        let harness = harness();
        harness
            .projects
            .seed(stored_project(1, "CORE", "kanban-main"));

        let response = harness
            .core
            .command("project.archive", &archive(1, "key-1", 1))
            .expect("the archive applies");

        assert_eq!(
            response["counters"],
            json!({ "plan": 2, "spec": 0, "ticket": 5 }),
            "archiving preserves the minted counters"
        );
    }

    #[test]
    fn archiving_twice_is_refused() {
        let harness = harness();
        harness
            .projects
            .seed(stored_project(1, "CORE", "kanban-main"));
        harness
            .core
            .command("project.archive", &archive(1, "key-1", 1))
            .expect("the first archive applies");

        let error = harness
            .core
            .command("project.archive", &archive(1, "key-2", 2))
            .expect_err("the second archive is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        let (rows, timeline) = harness.projects.snapshot();
        assert_eq!(rows[0].version(), 2, "the refusal changed nothing");
        assert_eq!(timeline.len(), 1, "no second append may land");
    }

    #[test]
    fn archiving_an_unknown_project_is_not_found() {
        let harness = harness();

        let error = harness
            .core
            .command("project.archive", &archive(9, "key-1", 0))
            .expect_err("the unknown Project is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn archiving_with_a_stale_version_is_rejected_with_the_current_one() {
        let harness = harness();
        harness
            .projects
            .seed(stored_project(1, "CORE", "kanban-main"));

        let error = harness
            .core
            .command("project.archive", &archive(1, "key-1", 0))
            .expect_err("the stale version is rejected");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(1));
    }

    #[test]
    fn archiving_records_a_transition_on_the_projects_own_timeline() {
        let harness = harness();
        harness
            .projects
            .seed(stored_project(1, "CORE", "kanban-main"));

        harness
            .core
            .command("project.archive", &archive(1, "key-1", 1))
            .expect("the archive applies");

        let (_, timeline) = harness.projects.snapshot();
        assert_eq!(
            timeline
                .iter()
                .map(|envelope| (
                    envelope.scope().clone(),
                    envelope.kind(),
                    envelope.entity().cloned(),
                    envelope.detail().clone(),
                ))
                .collect::<Vec<_>>(),
            vec![(
                TimelineScope::Project("1".to_owned()),
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Project,
                    id: "1".to_owned(),
                }),
                json!({ "action": "archived", "id": 1 }),
            )],
            "the archive lands exactly one closed-vocabulary row"
        );
    }

    #[test]
    fn archiving_publishes_on_the_event_stream() {
        let sink = std::sync::Arc::new(super::testing::RecordingSink::default());
        let harness = super::testing::harness_with_observing(
            super::testing::KnownRepositories {
                repositories: vec!["/repositories/kanban".to_owned()],
            },
            sink.clone(),
        );
        harness
            .projects
            .seed(stored_project(1, "CORE", "kanban-main"));

        harness
            .core
            .command("project.archive", &archive(1, "key-1", 1))
            .expect("the archive applies");

        let events = sink.events.lock().expect("the recorder lock is sound");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "project.archived");
        assert_eq!(events[0].1["archived"], json!(true));
        assert_eq!(
            events[0].1["counters"],
            json!({ "plan": 2, "spec": 0, "ticket": 5 })
        );
    }

    #[test]
    fn listing_returns_every_project_in_id_order() {
        let harness = harness();
        harness
            .projects
            .seed(stored_project(1, "CORE", "kanban-main"));
        harness
            .projects
            .seed(stored_project(2, "WAVE", "wave-main"));

        let listed = harness
            .core
            .query("project.list", &json!({}))
            .expect("the list serves");

        let codes: Vec<_> = listed["projects"]
            .as_array()
            .expect("the projects are a list")
            .iter()
            .map(|project| project["code"].clone())
            .collect();
        assert_eq!(codes, vec![json!("CORE"), json!("WAVE")]);
    }

    #[test]
    fn listing_rejects_unknown_fields() {
        let harness = harness();

        let error = harness
            .core
            .query("project.list", &json!({ "include_archived": true }))
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
    }
}
