//! Workspace commands: register, observe, and list (KAN-S6-US1).
//! Observation reads git state through the git observer port and never
//! mutates the repository; health transitions append timeline rows.

use kanban_dto::LiveEventName;
use std::sync::Arc;

use kanban_domain::{
    ProjectId, Workspace, WorkspaceCheckout, WorkspaceHealth, WorkspaceId, WorkspaceRegistration,
};
use kanban_dto::{
    ApiError, TimelineEntityKind, TimelineEntityRef, TimelineEventKind, WorkspaceCheckoutDto,
    WorkspaceHealthDto, WorkspaceListQuery, WorkspaceListResponse, WorkspaceObservationDto,
    WorkspaceObserveRequest, WorkspaceRecord, WorkspaceRegisterRequest, WorkspaceRetireRequest,
    WorkspaceReuseDto,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::events::{EventSink, emit_catalogued};
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;
use crate::timeline::TimelineEnvelope;

/// The git facts one observation read returns without mutating the
/// clone. `checkout` carries the closed state; a detached HEAD never
/// appears as a branch name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceGitSnapshot {
    pub present: bool,
    pub repository_identity: Option<String>,
    pub checkout: Option<WorkspaceCheckout>,
    pub head: Option<String>,
    pub working_tree_clean: bool,
    /// Whether the Workspace holds unique unlanded commits; `None`
    /// when the observer could not decide (DR-LW-06).
    pub unique_unlanded_commits: Option<bool>,
}

/// The git observation port: read-only workspace state. The service
/// wires the real filesystem observer; tests steer fixture repositories
/// initialised in temp directories.
pub trait WorkspaceGitObserver: Send + Sync {
    /// Read git state for `workspace_path`, expecting it to belong to
    /// `repository_path`. Implementations must not mutate either path.
    fn observe(&self, workspace_path: &str, repository_path: &str) -> WorkspaceGitSnapshot;
}

/// The storage port Workspace commands call through.
pub trait WorkspaceStore: Send + Sync {
    /// Insert a fresh Workspace from a validated registration.
    fn create(
        &self,
        registration: &WorkspaceRegistration,
        envelope: &dyn Fn(WorkspaceId) -> TimelineEnvelope,
    ) -> Result<Workspace, ApiError>;

    /// Load one Workspace, if it exists.
    fn find(&self, id: WorkspaceId) -> Result<Option<Workspace>, ApiError>;

    /// Persist an applied transition and its timeline envelope.
    fn save(&self, workspace: &Workspace, envelope: TimelineEnvelope) -> Result<(), ApiError>;

    /// Every Workspace for one Project, in id order.
    fn list_for_project(&self, project_id: ProjectId) -> Result<Vec<Workspace>, ApiError>;

    /// Whether `path` is already registered for `project_id`.
    fn path_taken(&self, project_id: ProjectId, path: &str) -> Result<bool, ApiError>;
}

/// The stable refusal for a path another Workspace on the same Project
/// already holds.
pub fn duplicate_path_error(path: &str) -> ApiError {
    ApiError::invalid_request(&format!(
        "the Workspace path `{path}` is already registered for this Project"
    ))
}

fn transition(
    project_id: ProjectId,
    workspace_id: WorkspaceId,
    action: &str,
    facts: Value,
) -> Result<TimelineEnvelope, ApiError> {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("Workspace transition facts are a JSON object");
    object.insert("action".to_owned(), Value::from(action));
    object.insert("id".to_owned(), Value::from(workspace_id.value()));
    let identity = workspace_id.value().to_string();
    Ok(TimelineEnvelope::project(
        project_id.value(),
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Workspace,
            id: identity.clone(),
        }),
        detail,
    ))
}

fn health_transition(
    project_id: ProjectId,
    workspace_id: WorkspaceId,
    from: WorkspaceHealth,
    to: WorkspaceHealth,
    facts: Value,
) -> Result<TimelineEnvelope, ApiError> {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("Workspace health facts are a JSON object");
    object.insert("from".to_owned(), Value::from(from.as_str()));
    object.insert("to".to_owned(), Value::from(to.as_str()));
    transition(project_id, workspace_id, "health_changed", detail)
}

impl Core {
    /// Register the Workspace operations against `workspaces`, resolving
    /// Projects through `projects` and observing git through `git`.
    pub fn register_workspaces(
        &mut self,
        workspaces: Arc<dyn WorkspaceStore>,
        projects: Arc<dyn ProjectStore>,
        git: Arc<dyn WorkspaceGitObserver>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "workspace.register",
            Arc::new(RegisterWorkspace {
                store: workspaces.clone(),
                projects: projects.clone(),
            }),
        )?;
        self.register_command(
            "workspace.observe",
            Arc::new(ObserveWorkspace {
                store: workspaces.clone(),
                projects,
                git,
            }),
        )?;
        self.register_command(
            "workspace.retire",
            Arc::new(RetireWorkspace {
                store: workspaces.clone(),
            }),
        )?;
        self.register_query(
            "workspace.list",
            Arc::new(ListWorkspaces { store: workspaces }),
        )?;
        Ok(())
    }
}

struct RegisterWorkspace {
    store: Arc<dyn WorkspaceStore>,
    projects: Arc<dyn ProjectStore>,
}

impl CommandHandler for RegisterWorkspace {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<WorkspaceRegisterRequest>(payload)?;
        ParsedCommand::lift("workspace", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        events: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: WorkspaceRegisterRequest = parse_payload(&command.payload)?;
        let project_id = ProjectId::new(request.project_id);
        let project = load_project(&self.projects, project_id)?;
        if project.is_archived() {
            return Err(ApiError::invalid_request(
                "archived Projects cannot register Workspaces",
            ));
        }
        let is_seed = project.registration().seed_workspace() == request.path.trim();
        let registration = WorkspaceRegistration::new(project_id, &request.path, is_seed)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        if self.store.path_taken(project_id, registration.path())? {
            return Err(duplicate_path_error(registration.path()));
        }
        let facts = json!({
            "path": registration.path(),
            "is_seed": registration.is_seed(),
            "project_id": project_id.value(),
        });
        let workspace = self.store.create(&registration, &|id| {
            transition(project_id, id, "registered", facts.clone())
                .expect("a minted Workspace identity names a Project")
        })?;
        announce(events, LiveEventName::WorkspaceRegistered, &workspace);
        encode_record(&workspace)
    }
}

struct ObserveWorkspace {
    store: Arc<dyn WorkspaceStore>,
    projects: Arc<dyn ProjectStore>,
    git: Arc<dyn WorkspaceGitObserver>,
}

impl CommandHandler for ObserveWorkspace {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<WorkspaceObserveRequest>(payload)?;
        ParsedCommand::lift("workspace", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: WorkspaceObserveRequest = parse_payload(&command.payload)?;
        let workspace = load_workspace(&self.store, request.workspace_id)?;
        Ok(workspace.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        events: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: WorkspaceObserveRequest = parse_payload(&command.payload)?;
        let mut workspace = load_workspace(&self.store, request.workspace_id)?;
        let project_id = workspace.registration().project_id();
        let project = load_project(&self.projects, project_id)?;
        let snapshot = self.git.observe(
            workspace.registration().path(),
            project.registration().repository(),
        );
        let health_change = workspace.observe(
            snapshot.present,
            snapshot.repository_identity,
            snapshot.checkout,
            snapshot.head,
            snapshot.working_tree_clean,
            snapshot.unique_unlanded_commits,
        );
        let envelope = if let Some((from, to)) = health_change {
            health_transition(
                project_id,
                workspace.id(),
                from,
                to,
                observation_facts(&workspace),
            )?
        } else {
            transition(
                project_id,
                workspace.id(),
                "observed",
                observation_facts(&workspace),
            )?
        };
        self.store.save(&workspace, envelope)?;
        announce(events, LiveEventName::WorkspaceObserved, &workspace);
        encode_record(&workspace)
    }
}

/// Retirement is the explicit operator action that ends reuse. No
/// command or automated path deletes a Workspace (DR-LW-11); the
/// record and every observed fact survive retirement.
struct RetireWorkspace {
    store: Arc<dyn WorkspaceStore>,
}

impl CommandHandler for RetireWorkspace {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<WorkspaceRetireRequest>(payload)?;
        ParsedCommand::lift("workspace", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: WorkspaceRetireRequest = parse_payload(&command.payload)?;
        let workspace = load_workspace(&self.store, request.workspace_id)?;
        Ok(workspace.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        events: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: WorkspaceRetireRequest = parse_payload(&command.payload)?;
        let mut workspace = load_workspace(&self.store, request.workspace_id)?;
        let project_id = workspace.registration().project_id();
        let health_change = workspace
            .retire()
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let envelope = if let Some((from, to)) = health_change {
            health_transition(
                project_id,
                workspace.id(),
                from,
                to,
                observation_facts(&workspace),
            )?
        } else {
            transition(
                project_id,
                workspace.id(),
                "retired",
                observation_facts(&workspace),
            )?
        };
        self.store.save(&workspace, envelope)?;
        announce(events, LiveEventName::WorkspaceRetired, &workspace);
        encode_record(&workspace)
    }
}

struct ListWorkspaces {
    store: Arc<dyn WorkspaceStore>,
}

impl QueryHandler for ListWorkspaces {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let request: WorkspaceListQuery = parse_payload(payload)?;
        let workspaces = self
            .store
            .list_for_project(ProjectId::new(request.project_id))?;
        let response = WorkspaceListResponse {
            workspaces: workspaces.iter().map(record_of).collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

fn load_project(
    store: &Arc<dyn ProjectStore>,
    id: ProjectId,
) -> Result<kanban_domain::Project, ApiError> {
    store
        .find(id)?
        .ok_or_else(|| ApiError::not_found(&format!("project {}", id.value())))
}

fn load_workspace(store: &Arc<dyn WorkspaceStore>, id: u64) -> Result<Workspace, ApiError> {
    store
        .find(WorkspaceId::new(id))?
        .ok_or_else(|| ApiError::not_found(&format!("workspace {id}")))
}

fn observation_facts(workspace: &Workspace) -> Value {
    let observation = workspace.observation();
    json!({
        "path": workspace.registration().path(),
        "health": workspace.health().as_str(),
        "repository_identity": observation.repository_identity(),
        "checkout": observation.checkout().map(checkout_dto).map(WorkspaceCheckoutDto::as_str),
        "branch": observation.branch(),
        "head": observation.head(),
        "working_tree_clean": observation.working_tree_clean(),
        "unique_unlanded_commits": observation.unique_unlanded_commits(),
        "lane_assignment": observation.lane_assignment(),
    })
}

fn checkout_dto(checkout: &WorkspaceCheckout) -> WorkspaceCheckoutDto {
    match checkout {
        WorkspaceCheckout::Branch(_) => WorkspaceCheckoutDto::Branch,
        WorkspaceCheckout::Detached => WorkspaceCheckoutDto::Detached,
    }
}

fn record_of(workspace: &Workspace) -> WorkspaceRecord {
    let observation = workspace.observation();
    WorkspaceRecord {
        id: workspace.id().value(),
        project_id: workspace.registration().project_id().value(),
        path: workspace.registration().path().to_owned(),
        is_seed: workspace.registration().is_seed(),
        health: health_dto(workspace.health()),
        observation: WorkspaceObservationDto {
            repository_identity: observation.repository_identity().map(str::to_owned),
            checkout: observation.checkout().map(checkout_dto),
            branch: observation.branch().map(str::to_owned),
            head: observation.head().map(str::to_owned),
            working_tree_clean: observation.working_tree_clean(),
            unique_unlanded_commits: observation.unique_unlanded_commits(),
            lane_assignment: observation.lane_assignment(),
        },
        reuse: reuse_dto(workspace.reuse_evaluation()),
        version: workspace.version(),
    }
}

fn reuse_dto(evaluation: kanban_domain::ReuseEvaluation) -> WorkspaceReuseDto {
    WorkspaceReuseDto {
        reusable: evaluation.reusable(),
        clean: evaluation.clean(),
        unassigned: evaluation.unassigned(),
        free_of_unlanded_commits: evaluation.free_of_unlanded_commits(),
    }
}

fn health_dto(health: WorkspaceHealth) -> WorkspaceHealthDto {
    match health {
        WorkspaceHealth::Available => WorkspaceHealthDto::Available,
        WorkspaceHealth::Assigned => WorkspaceHealthDto::Assigned,
        WorkspaceHealth::Dirty => WorkspaceHealthDto::Dirty,
        WorkspaceHealth::Missing => WorkspaceHealthDto::Missing,
        WorkspaceHealth::Retired => WorkspaceHealthDto::Retired,
    }
}

fn encode_record(workspace: &Workspace) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(workspace))
        .map_err(|error| ApiError::internal(&error.to_string()))
}

fn announce(events: &dyn EventSink, event: LiveEventName, workspace: &Workspace) {
    emit_catalogued(events, event, &record_of(workspace));
}

#[cfg(test)]
mod testing {
    use std::sync::{Arc, Mutex};

    use kanban_domain::{
        Project, ProjectCounters, ProjectId, ProjectRegistration, ProjectState, Workspace,
        WorkspaceId, WorkspaceRegistration,
    };
    use kanban_dto::ApiError;
    use serde_json::{Value, json};

    use super::{WorkspaceGitObserver, WorkspaceGitSnapshot, WorkspaceStore, duplicate_path_error};
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::project::testing::MemoryProjectStore;
    use crate::timeline::TimelineEnvelope;

    #[derive(Default)]
    pub(super) struct MemoryWorkspaceStore {
        state: Mutex<MemoryState>,
    }

    #[derive(Default)]
    struct MemoryState {
        workspaces: Vec<Workspace>,
        next_id: u64,
        timeline: Vec<TimelineEnvelope>,
    }

    impl MemoryWorkspaceStore {
        pub(super) fn snapshot(&self) -> (Vec<Workspace>, Vec<TimelineEnvelope>) {
            let state = self.state.lock().expect("the memory store lock is sound");
            (state.workspaces.clone(), state.timeline.clone())
        }

        pub(super) fn seed(&self, workspace: Workspace) {
            self.state
                .lock()
                .expect("the memory store lock is sound")
                .workspaces
                .push(workspace);
        }
    }

    impl WorkspaceStore for MemoryWorkspaceStore {
        fn create(
            &self,
            registration: &WorkspaceRegistration,
            envelope: &dyn Fn(WorkspaceId) -> TimelineEnvelope,
        ) -> Result<Workspace, ApiError> {
            let mut state = self.state.lock().expect("the memory store lock is sound");
            if state.workspaces.iter().any(|row| {
                row.registration().project_id() == registration.project_id()
                    && row.registration().path() == registration.path()
            }) {
                return Err(duplicate_path_error(registration.path()));
            }
            state.next_id += 1;
            let id = WorkspaceId::new(state.next_id);
            let workspace = Workspace::new(id, registration.clone());
            state.workspaces.push(workspace.clone());
            state.timeline.push(envelope(id));
            Ok(workspace)
        }

        fn find(&self, id: WorkspaceId) -> Result<Option<Workspace>, ApiError> {
            let state = self.state.lock().expect("the memory store lock is sound");
            Ok(state.workspaces.iter().find(|row| row.id() == id).cloned())
        }

        fn save(&self, workspace: &Workspace, envelope: TimelineEnvelope) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory store lock is sound");
            let id = workspace.id();
            if let Some(row) = state.workspaces.iter_mut().find(|row| row.id() == id) {
                *row = workspace.clone();
            }
            state.timeline.push(envelope);
            Ok(())
        }

        fn list_for_project(&self, project_id: ProjectId) -> Result<Vec<Workspace>, ApiError> {
            let state = self.state.lock().expect("the memory store lock is sound");
            Ok(state
                .workspaces
                .iter()
                .filter(|row| row.registration().project_id() == project_id)
                .cloned()
                .collect())
        }

        fn path_taken(&self, project_id: ProjectId, path: &str) -> Result<bool, ApiError> {
            let state = self.state.lock().expect("the memory store lock is sound");
            Ok(state.workspaces.iter().any(|row| {
                row.registration().project_id() == project_id && row.registration().path() == path
            }))
        }
    }

    /// A git observer the tests steer with fixed snapshots per path.
    #[derive(Default)]
    pub(super) struct ScriptedObserver {
        pub(super) snapshots: std::collections::HashMap<String, WorkspaceGitSnapshot>,
    }

    impl WorkspaceGitObserver for ScriptedObserver {
        fn observe(&self, workspace_path: &str, _repository_path: &str) -> WorkspaceGitSnapshot {
            self.snapshots
                .get(workspace_path)
                .cloned()
                .unwrap_or(WorkspaceGitSnapshot {
                    present: false,
                    working_tree_clean: true,
                    ..WorkspaceGitSnapshot::default()
                })
        }
    }

    pub(crate) struct Harness {
        pub(crate) projects: Arc<MemoryProjectStore>,
        pub(crate) workspaces: Arc<MemoryWorkspaceStore>,
        pub(crate) core: Core,
    }

    pub(super) fn harness(observer: Arc<dyn WorkspaceGitObserver>) -> Harness {
        let projects = Arc::new(MemoryProjectStore::default());
        let workspaces = Arc::new(MemoryWorkspaceStore::default());
        let initiatives = Arc::new(crate::project::testing::MemoryInitiatives::default());
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            Arc::new(crate::events::NoopEventSink),
        );
        core.register_initiatives(initiatives.clone())
            .expect("the initiative operations register");
        core.register_projects(
            projects.clone(),
            Arc::new(crate::project::testing::KnownRepositories {
                repositories: vec!["/repositories/kanban".to_owned()],
            }),
            initiatives,
            Arc::new(crate::project::testing::MemoryHerdrSettings::default()),
            Arc::new(crate::herdr::NoopHerdrProjectObserver),
        )
        .expect("the project operations register");
        core.register_workspaces(workspaces.clone(), projects.clone(), observer)
            .expect("the workspace operations register");
        Harness {
            projects,
            workspaces,
            core,
        }
    }

    pub(super) fn stored_project() -> Project {
        let registration = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            None,
        )
        .expect("the fixture registration validates");
        Project::restore(
            ProjectId::new(1),
            registration,
            ProjectState::Active,
            ProjectCounters::zeroed(),
            1,
        )
    }

    pub(super) fn register(project_id: u64, path: &str, key: &str) -> Value {
        json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": project_id,
            "path": path,
        })
    }

    pub(super) fn observe(workspace_id: u64, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "workspace_id": workspace_id,
        })
    }

    pub(super) fn retire(workspace_id: u64, key: &str, version: u64) -> Value {
        json!({
            "mutation": { "optimistic_version": version, "idempotency_key": key },
            "workspace_id": workspace_id,
        })
    }
}

#[cfg(test)]
mod workspace_registration {
    use std::sync::Arc;

    use kanban_dto::ErrorCode;
    use serde_json::json;

    use super::testing::{ScriptedObserver, harness, register, stored_project};

    #[test]
    fn registering_marks_the_seed_when_the_path_matches() {
        let harness = harness(Arc::new(ScriptedObserver::default()));
        harness.projects.seed(stored_project());

        let response = harness
            .core
            .command(
                "workspace.register",
                &register(1, "/workspaces/kanban.seed", "key-1"),
            )
            .expect("the workspace registers");

        assert_eq!(response["is_seed"], json!(true));
        assert_eq!(response["path"], json!("/workspaces/kanban.seed"));
    }

    #[test]
    fn registering_refuses_a_duplicate_path_for_the_same_project() {
        let harness = harness(Arc::new(ScriptedObserver::default()));
        harness.projects.seed(stored_project());
        harness
            .core
            .command(
                "workspace.register",
                &register(1, "/workspaces/kanban.feature", "key-1"),
            )
            .expect("the first registration applies");

        let error = harness
            .core
            .command(
                "workspace.register",
                &register(1, "/workspaces/kanban.feature", "key-2"),
            )
            .expect_err("the duplicate path is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("/workspaces/kanban.feature"));
    }

    #[test]
    fn registering_refuses_an_unknown_project() {
        let harness = harness(Arc::new(ScriptedObserver::default()));

        let error = harness
            .core
            .command(
                "workspace.register",
                &register(9, "/workspaces/kanban.feature", "key-1"),
            )
            .expect_err("the unknown Project is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }
}

#[cfg(test)]
mod workspace_observe {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use kanban_domain::{
        Workspace, WorkspaceCheckout, WorkspaceHealth, WorkspaceId, WorkspaceObservation,
        WorkspaceRegistration,
    };
    use kanban_dto::{
        ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineScope,
    };
    use serde_json::json;

    use super::testing::{ScriptedObserver, harness, observe, register, stored_project};
    use super::{WorkspaceGitObserver, WorkspaceGitSnapshot};

    /// Return successive snapshots for repeated observations.
    struct SteppedObserver {
        steps: Mutex<Vec<WorkspaceGitSnapshot>>,
    }

    impl WorkspaceGitObserver for SteppedObserver {
        fn observe(&self, _workspace_path: &str, _repository_path: &str) -> WorkspaceGitSnapshot {
            let mut steps = self.steps.lock().expect("the steps lock is sound");
            if steps.is_empty() {
                WorkspaceGitSnapshot {
                    present: false,
                    ..WorkspaceGitSnapshot::default()
                }
            } else {
                steps.remove(0)
            }
        }
    }

    #[test]
    fn observing_records_a_health_transition_on_the_project_timeline() {
        let harness = harness(Arc::new(SteppedObserver {
            steps: Mutex::new(vec![
                WorkspaceGitSnapshot {
                    present: true,
                    repository_identity: Some("identity".to_owned()),
                    checkout: Some(WorkspaceCheckout::Branch("feature".to_owned())),
                    head: Some("abc".to_owned()),
                    working_tree_clean: true,
                    unique_unlanded_commits: Some(false),
                },
                WorkspaceGitSnapshot {
                    present: true,
                    repository_identity: Some("identity".to_owned()),
                    checkout: Some(WorkspaceCheckout::Branch("feature".to_owned())),
                    head: Some("def".to_owned()),
                    working_tree_clean: false,
                    unique_unlanded_commits: Some(false),
                },
            ]),
        }));
        harness.projects.seed(stored_project());
        harness
            .core
            .command(
                "workspace.register",
                &register(1, "/workspaces/kanban.feature", "key-1"),
            )
            .expect("the workspace registers");
        harness
            .core
            .command("workspace.observe", &observe(1, "key-2", 1))
            .expect("the first observation applies");
        harness
            .core
            .command("workspace.observe", &observe(1, "key-3", 2))
            .expect("the dirty observation applies");

        let (_, timeline) = harness.workspaces.snapshot();
        let health_rows: Vec<_> = timeline
            .iter()
            .filter(|row| row.detail().get("action") == Some(&json!("health_changed")))
            .map(|row| {
                (
                    row.scope().clone(),
                    row.kind(),
                    row.entity().cloned(),
                    row.detail().clone(),
                )
            })
            .collect();

        assert_eq!(health_rows.len(), 2);
        assert_eq!(
            health_rows[1].3.get("from").and_then(|v| v.as_str()),
            Some("available")
        );
        assert_eq!(
            health_rows[1].3.get("to").and_then(|v| v.as_str()),
            Some("dirty")
        );
        assert_eq!(
            health_rows[1].1,
            TimelineEventKind::Transition,
            "health transitions use the transition kind"
        );
        assert_eq!(
            health_rows[1].2,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Workspace,
                id: "1".to_owned(),
            })
        );
        assert!(matches!(health_rows[1].0, TimelineScope::Project(_)));
    }

    #[test]
    fn observing_a_detached_head_reports_the_closed_state() {
        let harness = harness(Arc::new(ScriptedObserver {
            snapshots: HashMap::from([(
                "/workspaces/kanban.detached".to_owned(),
                WorkspaceGitSnapshot {
                    present: true,
                    repository_identity: Some("identity".to_owned()),
                    checkout: Some(kanban_domain::WorkspaceCheckout::Detached),
                    head: Some("abc123".to_owned()),
                    working_tree_clean: true,
                    unique_unlanded_commits: Some(false),
                },
            )]),
        }));
        harness.projects.seed(stored_project());
        harness
            .core
            .command(
                "workspace.register",
                &register(1, "/workspaces/kanban.detached", "key-1"),
            )
            .expect("the workspace registers");

        let response = harness
            .core
            .command("workspace.observe", &observe(1, "key-2", 1))
            .expect("the observation applies");

        assert_eq!(response["health"], json!("available"));
        assert_eq!(
            response["observation"]["branch"],
            json!(null),
            "a detached checkout has no branch name"
        );
        let (_, timeline) = harness.workspaces.snapshot();
        let observed: Vec<_> = timeline
            .iter()
            .filter(|row| row.detail().get("action") == Some(&json!("health_changed")))
            .collect();
        assert_eq!(observed.len(), 1);
        assert_eq!(
            observed[0].detail().get("checkout"),
            Some(&json!("detached")),
            "timeline facts carry the closed checkout state"
        );
    }

    #[test]
    fn observing_a_missing_path_marks_the_workspace_missing() {
        let harness = harness(Arc::new(ScriptedObserver::default()));
        harness.projects.seed(stored_project());
        harness
            .core
            .command(
                "workspace.register",
                &register(1, "/workspaces/absent", "key-1"),
            )
            .expect("the workspace registers");

        let response = harness
            .core
            .command("workspace.observe", &observe(1, "key-2", 1))
            .expect("the observation applies");

        assert_eq!(response["health"], json!("missing"));
        assert_eq!(response["observation"]["branch"], json!(null));
    }

    #[test]
    fn observing_reports_the_reuse_verdict_with_every_condition() {
        let harness = harness(Arc::new(ScriptedObserver {
            snapshots: HashMap::from([(
                "/workspaces/kanban.feature".to_owned(),
                WorkspaceGitSnapshot {
                    present: true,
                    repository_identity: Some("identity".to_owned()),
                    checkout: Some(kanban_domain::WorkspaceCheckout::Branch(
                        "feature".to_owned(),
                    )),
                    head: Some("def456".to_owned()),
                    working_tree_clean: true,
                    unique_unlanded_commits: Some(true),
                },
            )]),
        }));
        harness.projects.seed(stored_project());
        harness
            .core
            .command(
                "workspace.register",
                &register(1, "/workspaces/kanban.feature", "key-1"),
            )
            .expect("the workspace registers");

        let response = harness
            .core
            .command("workspace.observe", &observe(1, "key-2", 1))
            .expect("the observation applies");

        assert_eq!(
            response["observation"]["unique_unlanded_commits"],
            json!(true)
        );
        assert_eq!(
            response["reuse"],
            json!({
                "reusable": false,
                "clean": true,
                "unassigned": true,
                "free_of_unlanded_commits": false,
            }),
            "every condition reports even when the verdict refuses reuse"
        );
    }

    #[test]
    fn observing_a_landed_clean_workspace_reports_it_reusable() {
        let harness = harness(Arc::new(ScriptedObserver {
            snapshots: HashMap::from([(
                "/workspaces/kanban.feature".to_owned(),
                WorkspaceGitSnapshot {
                    present: true,
                    repository_identity: Some("identity".to_owned()),
                    checkout: Some(kanban_domain::WorkspaceCheckout::Branch(
                        "feature".to_owned(),
                    )),
                    head: Some("abc123".to_owned()),
                    working_tree_clean: true,
                    unique_unlanded_commits: Some(false),
                },
            )]),
        }));
        harness.projects.seed(stored_project());
        harness
            .core
            .command(
                "workspace.register",
                &register(1, "/workspaces/kanban.feature", "key-1"),
            )
            .expect("the workspace registers");

        let response = harness
            .core
            .command("workspace.observe", &observe(1, "key-2", 1))
            .expect("the observation applies");

        assert_eq!(response["reuse"]["reusable"], json!(true));
        assert_eq!(response["reuse"]["free_of_unlanded_commits"], json!(true));
    }

    #[test]
    fn listing_reports_unobserved_workspaces_as_not_reusable() {
        let harness = harness(Arc::new(ScriptedObserver::default()));
        harness.projects.seed(stored_project());
        harness
            .core
            .command(
                "workspace.register",
                &register(1, "/workspaces/kanban.feature", "key-1"),
            )
            .expect("the workspace registers");

        let listing = harness
            .core
            .query("workspace.list", &json!({ "project_id": 1 }))
            .expect("the listing serves");

        assert_eq!(listing["workspaces"][0]["reuse"]["reusable"], json!(false));
        assert_eq!(
            listing["workspaces"][0]["reuse"]["free_of_unlanded_commits"],
            json!(false),
            "an undecided unlanded guard refuses reuse"
        );
    }

    #[test]
    fn observing_an_assigned_lane_reports_assigned_health() {
        let harness = harness(Arc::new(ScriptedObserver {
            snapshots: HashMap::from([(
                "/workspaces/kanban.feature".to_owned(),
                WorkspaceGitSnapshot {
                    present: true,
                    repository_identity: Some("identity".to_owned()),
                    checkout: Some(WorkspaceCheckout::Branch("feature".to_owned())),
                    head: Some("abc".to_owned()),
                    working_tree_clean: false,
                    unique_unlanded_commits: Some(false),
                },
            )]),
        }));
        harness.projects.seed(stored_project());
        let workspace = Workspace::restore(
            WorkspaceId::new(1),
            WorkspaceRegistration::new(
                kanban_domain::ProjectId::new(1),
                "/workspaces/kanban.feature",
                false,
            )
            .expect("the registration validates"),
            false,
            Some(7),
            WorkspaceHealth::Missing,
            WorkspaceObservation::empty(),
            1,
        );
        harness.workspaces.seed(workspace);

        let response = harness
            .core
            .command("workspace.observe", &observe(1, "key-1", 1))
            .expect("the observation applies");

        assert_eq!(response["health"], json!("assigned"));
        assert_eq!(response["observation"]["lane_assignment"], json!(7));
    }

    #[test]
    fn observing_with_a_stale_version_is_rejected() {
        let harness = harness(Arc::new(ScriptedObserver::default()));
        harness.projects.seed(stored_project());
        harness.workspaces.seed(Workspace::restore(
            WorkspaceId::new(1),
            WorkspaceRegistration::new(
                kanban_domain::ProjectId::new(1),
                "/workspaces/kanban.feature",
                false,
            )
            .expect("the registration validates"),
            false,
            None,
            WorkspaceHealth::Missing,
            WorkspaceObservation::empty(),
            2,
        ));

        let error = harness
            .core
            .command("workspace.observe", &observe(1, "key-1", 1))
            .expect_err("the stale version is refused");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(2));
    }
}

#[cfg(test)]
mod workspace_retire {
    use std::collections::HashMap;
    use std::sync::Arc;

    use kanban_dto::{
        ErrorCode, TimelineEntityKind, TimelineEntityRef, TimelineEventKind, TimelineScope,
    };
    use serde_json::json;

    use super::WorkspaceGitSnapshot;
    use super::testing::{ScriptedObserver, harness, observe, register, retire, stored_project};

    fn clean_observer() -> Arc<ScriptedObserver> {
        Arc::new(ScriptedObserver {
            snapshots: HashMap::from([(
                "/workspaces/kanban.feature".to_owned(),
                WorkspaceGitSnapshot {
                    present: true,
                    repository_identity: Some("identity".to_owned()),
                    checkout: Some(kanban_domain::WorkspaceCheckout::Branch(
                        "feature".to_owned(),
                    )),
                    head: Some("abc".to_owned()),
                    working_tree_clean: true,
                    unique_unlanded_commits: Some(false),
                },
            )]),
        })
    }

    fn registered_and_observed(harness: &super::testing::Harness) -> serde_json::Value {
        harness
            .core
            .command(
                "workspace.register",
                &register(1, "/workspaces/kanban.feature", "key-1"),
            )
            .expect("the workspace registers");
        harness
            .core
            .command("workspace.observe", &observe(1, "key-2", 1))
            .expect("the observation applies")
    }

    #[test]
    fn retiring_moves_health_to_retired_and_appends_a_timeline_transition() {
        let harness = harness(clean_observer());
        harness.projects.seed(stored_project());
        registered_and_observed(&harness);

        let response = harness
            .core
            .command("workspace.retire", &retire(1, "key-3", 2))
            .expect("the retirement applies");

        assert_eq!(response["health"], json!("retired"));
        let (_, timeline) = harness.workspaces.snapshot();
        let retirement = timeline
            .iter()
            .find(|row| {
                row.detail().get("action") == Some(&json!("health_changed"))
                    && row.detail().get("to") == Some(&json!("retired"))
            })
            .expect("retirement appends a health transition");
        assert_eq!(
            retirement.detail().get("from"),
            Some(&json!("available")),
            "the transition names the state it left"
        );
        assert_eq!(retirement.kind(), TimelineEventKind::Transition);
        assert_eq!(
            retirement.entity(),
            Some(&TimelineEntityRef {
                kind: TimelineEntityKind::Workspace,
                id: "1".to_owned(),
            })
        );
        assert!(matches!(retirement.scope(), TimelineScope::Project(_)));
    }

    #[test]
    fn retirement_preserves_the_record_in_the_project_listing() {
        let harness = harness(clean_observer());
        harness.projects.seed(stored_project());
        registered_and_observed(&harness);

        harness
            .core
            .command("workspace.retire", &retire(1, "key-3", 2))
            .expect("the retirement applies");

        let listing = harness
            .core
            .query("workspace.list", &json!({ "project_id": 1 }))
            .expect("the listing serves");
        assert_eq!(
            listing["workspaces"]
                .as_array()
                .expect("the listing is an array")
                .len(),
            1,
            "a retired Workspace stays listed; retirement never deletes"
        );
        assert_eq!(listing["workspaces"][0]["health"], json!("retired"));
        assert_eq!(
            listing["workspaces"][0]["path"],
            json!("/workspaces/kanban.feature")
        );
        assert_eq!(
            listing["workspaces"][0]["reuse"]["reusable"],
            json!(false),
            "a retired record never reports reusable"
        );

        let (stored, _) = harness.workspaces.snapshot();
        assert!(
            stored.iter().any(|row| row.is_retired()),
            "the stored record survives retirement"
        );
    }

    #[test]
    fn retiring_twice_is_refused() {
        let harness = harness(clean_observer());
        harness.projects.seed(stored_project());
        registered_and_observed(&harness);
        harness
            .core
            .command("workspace.retire", &retire(1, "key-3", 2))
            .expect("the first retirement applies");

        let error = harness
            .core
            .command("workspace.retire", &retire(1, "key-4", 3))
            .expect_err("the second retirement is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("already retired"));
    }

    #[test]
    fn retiring_an_unknown_workspace_is_not_found() {
        let harness = harness(clean_observer());
        harness.projects.seed(stored_project());

        let error = harness
            .core
            .command("workspace.retire", &retire(9, "key-1", 1))
            .expect_err("the unknown Workspace is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn retiring_with_a_stale_version_is_rejected() {
        let harness = harness(clean_observer());
        harness.projects.seed(stored_project());
        registered_and_observed(&harness);

        let error = harness
            .core
            .command("workspace.retire", &retire(1, "key-3", 1))
            .expect_err("the stale version is refused");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(2));
    }

    #[test]
    fn no_exposed_operation_deletes_a_workspace() {
        let destructive = ["delete", "remove", "unregister", "destroy", "drop", "purge"];
        for operation in crate::catalog::exposed_operations() {
            if let Some(suffix) = operation.name.strip_prefix("workspace.") {
                assert!(
                    !destructive.contains(&suffix),
                    "`{}` must not exist: Workspaces are retired, never deleted",
                    operation.name
                );
            }
        }
        assert!(
            crate::catalog::exposed_operations()
                .iter()
                .any(|operation| operation.name == "workspace.retire"),
            "retirement is the explicit operator action that ends a Workspace"
        );
    }

    #[test]
    fn dispatching_a_fabricated_delete_is_refused() {
        let harness = harness(clean_observer());
        harness.projects.seed(stored_project());
        registered_and_observed(&harness);

        let error = harness
            .core
            .command("workspace.delete", &retire(1, "key-9", 2))
            .expect_err("no destructive operation is registered");

        assert_eq!(error.code, ErrorCode::NotFound);
        let (stored, _) = harness.workspaces.snapshot();
        assert_eq!(stored.len(), 1, "nothing was removed");
    }
}
