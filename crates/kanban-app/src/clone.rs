//! Guarded branch-clone commands (KAN-S6-US4). Creating and removing
//! branch clones goes through the fleet's `git bc-add` family — the
//! only sanctioned clone mechanism — so these commands wrap the fleet
//! skill, never reimplement it (DR-LW-09). Every precondition is
//! validated first: conflicting paths, branches, and Lane assignments
//! are refused before anything is invoked, with the conflict named
//! (DR-LW-10). Every invocation and every refusal appends a timeline
//! row, so the audit trail outlives both outcomes. Removal never
//! deletes the Workspace record (DR-LW-11); the next observation
//! reports the clone missing.

use std::sync::Arc;

use kanban_domain::{
    CloneConflict, ProjectId, WorkspaceCloneFacts, WorkspaceId, clone_create_conflict,
    clone_remove_conflict, validate_clone_target,
};
use kanban_dto::{
    ApiError, CloneCreateRequest, CloneCreatedRecord, CloneRemoveRequest, CloneRemovedRecord,
    ErrorCode, LiveEventName, TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, RegistrationError};
use crate::events::emit_catalogued;
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;
use crate::timeline::TimelineEnvelope;
use crate::workspace::WorkspaceStore;

/// The fleet branch-clone skill port: `git bc-add` and `git bc-rm`.
/// Kanban wraps the skill and never reimplements it, so this port is
/// the only way a guarded command touches the filesystem's clones —
/// and it is invoked only after every precondition has held.
pub trait FleetCloneTool: Send + Sync {
    /// Create the branch clone of `source` at `path` on `branch`.
    fn add_clone(&self, source: &str, path: &str, branch: &str) -> Result<(), ApiError>;

    /// Remove the branch clone at `path`.
    fn remove_clone(&self, path: &str) -> Result<(), ApiError>;
}

/// The storage port guarded clone commands append their timeline rows
/// through. A row appended while the command's mutation span is open
/// commits with it; a row a refused command still owes is appended
/// after the discard, in its own span.
pub trait CloneGuardStore: Send + Sync {
    /// Append one timeline row.
    fn append(&self, envelope: TimelineEnvelope) -> Result<(), ApiError>;
}

/// The reason code recorded when the fleet skill itself refuses the
/// request after Kanban's own guards passed: a refusal the caller
/// caused, surfaced as an invalid request.
pub const FLEET_TOOL_REFUSED: &str = "fleet_tool_refused";

/// The reason code recorded when the fleet skill fails its way rather
/// than refusing — it could not run, or it died without its own
/// deliberate report — surfaced as an internal failure.
pub const FLEET_TOOL_FAILED: &str = "fleet_tool_failed";

/// Which fleet tool outcome a durable row names, keeping the timeline
/// and the caller's error agreeing on what happened: the tool's error
/// code already carries the classification.
fn tool_failure_reason(error: &ApiError) -> &'static str {
    if error.code == ErrorCode::InvalidRequest {
        FLEET_TOOL_REFUSED
    } else {
        FLEET_TOOL_FAILED
    }
}

/// One clone-guard timeline row: on the Project's timeline, about the
/// entity the command touched, with `action` naming the outcome.
fn clone_transition(
    project_id: ProjectId,
    entity: TimelineEntityRef,
    action: &str,
    facts: Value,
) -> TimelineEnvelope {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("clone guard facts are a JSON object");
    object.insert("action".to_owned(), Value::from(action));
    TimelineEnvelope::project(
        project_id.value(),
        TimelineEventKind::Transition,
        Some(entity),
        detail,
    )
}

fn project_entity(project_id: ProjectId) -> TimelineEntityRef {
    TimelineEntityRef {
        kind: TimelineEntityKind::Project,
        id: project_id.value().to_string(),
    }
}

fn workspace_entity(workspace_id: WorkspaceId) -> TimelineEntityRef {
    TimelineEntityRef {
        kind: TimelineEntityKind::Workspace,
        id: workspace_id.value().to_string(),
    }
}

/// Defer one timeline row until the failed command's span has rolled
/// back — inside that span it would be discarded with the rejection it
/// records — so the refusal outlives the command that was refused.
fn record_after_discard(
    timeline: &Arc<dyn CloneGuardStore>,
    events: &dyn CommandEffects,
    envelope: TimelineEnvelope,
) {
    let timeline = timeline.clone();
    events.after_discard(Box::new(move || {
        if let Err(error) = timeline.append(envelope) {
            eprintln!(
                "kanban: the clone refusal could not be recorded: {}",
                error.message
            );
        }
    }));
}

/// Report a refused precondition as the stable invalid-request code.
fn refuse(error: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

fn encode(record: &impl serde::Serialize) -> Result<Value, ApiError> {
    serde_json::to_value(record).map_err(|error| ApiError::internal(&error.to_string()))
}

/// Announce one applied clone command to every live subscriber.
fn announce(events: &dyn CommandEffects, event: LiveEventName, record: &impl serde::Serialize) {
    emit_catalogued(events, event, record);
}

/// The facts a refused create records beside its outcome.
fn create_refusal_facts(path: &str, branch: &str, conflict: &CloneConflict) -> Value {
    let mut facts = json!({
        "path": path,
        "branch": branch,
        "reason": conflict.reason(),
        "message": conflict.to_string(),
    });
    match conflict {
        CloneConflict::PathTaken { workspace_id, .. }
        | CloneConflict::BranchCheckedOut { workspace_id, .. } => {
            facts["workspace_id"] = Value::from(*workspace_id);
        }
        _ => {}
    }
    facts
}

/// The facts a refused remove records beside its outcome.
fn remove_refusal_facts(workspace_id: u64, path: &str, conflict: &CloneConflict) -> Value {
    let mut facts = json!({
        "workspace_id": workspace_id,
        "path": path,
        "reason": conflict.reason(),
        "message": conflict.to_string(),
    });
    if let CloneConflict::LaneAssigned { lane_id, .. } = conflict {
        facts["lane_id"] = Value::from(*lane_id);
    }
    facts
}

/// The ports every guarded clone command calls through.
#[derive(Clone)]
struct CloneContext {
    tool: Arc<dyn FleetCloneTool>,
    projects: Arc<dyn ProjectStore>,
    workspaces: Arc<dyn WorkspaceStore>,
    timeline: Arc<dyn CloneGuardStore>,
}

impl Core {
    /// Register the guarded clone operations against the fleet `tool`,
    /// resolving Projects through `projects`, Workspace conflicts
    /// through `workspaces`, and appending timeline rows through
    /// `timeline`.
    pub fn register_clones(
        &mut self,
        tool: Arc<dyn FleetCloneTool>,
        projects: Arc<dyn ProjectStore>,
        workspaces: Arc<dyn WorkspaceStore>,
        timeline: Arc<dyn CloneGuardStore>,
    ) -> Result<(), RegistrationError> {
        let context = CloneContext {
            tool,
            projects,
            workspaces,
            timeline,
        };
        self.register_command("clone.create", Arc::new(CreateClone(context.clone())))?;
        self.register_command("clone.remove", Arc::new(RemoveClone(context)))?;
        Ok(())
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

fn load_workspace(
    store: &Arc<dyn WorkspaceStore>,
    id: u64,
) -> Result<kanban_domain::Workspace, ApiError> {
    store
        .find(WorkspaceId::new(id))?
        .ok_or_else(|| ApiError::not_found(&format!("workspace {id}")))
}

/// Serves `clone.create`: validate every precondition, then — and
/// only then — let the fleet skill create the clone.
struct CreateClone(CloneContext);

impl CommandHandler for CreateClone {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<CloneCreateRequest>(payload)?;
        ParsedCommand::lift("clone", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        // A fresh clone target is created at version 0.
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        events: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: CloneCreateRequest = parse_payload(&command.payload)?;
        let project_id = ProjectId::new(request.project_id);
        let project = load_project(&self.0.projects, project_id)?;
        if project.is_archived() {
            return Err(refuse("archived Projects accept no clone commands"));
        }
        let (path, branch) =
            validate_clone_target(&request.path, &request.branch).map_err(refuse)?;
        let registered = self.0.workspaces.list_for_project(project_id)?;
        let facts: Vec<_> = registered
            .iter()
            .map(WorkspaceCloneFacts::from_workspace)
            .collect();
        if let Some(conflict) = clone_create_conflict(
            &path,
            &branch,
            project.registration().seed_workspace(),
            &facts,
        ) {
            record_after_discard(
                &self.0.timeline,
                events,
                clone_transition(
                    project_id,
                    project_entity(project_id),
                    "clone_create_refused",
                    create_refusal_facts(&path, &branch, &conflict),
                ),
            );
            return Err(refuse(&conflict));
        }
        let source = project.registration().repository().to_owned();
        if let Err(error) = self.0.tool.add_clone(&source, &path, &branch) {
            record_after_discard(
                &self.0.timeline,
                events,
                clone_transition(
                    project_id,
                    project_entity(project_id),
                    "clone_create_refused",
                    json!({
                        "path": path,
                        "branch": branch,
                        "reason": tool_failure_reason(&error),
                        "error": error.message,
                    }),
                ),
            );
            return Err(error);
        }
        let record = CloneCreatedRecord {
            project_id: project_id.value(),
            path: path.clone(),
            branch: branch.clone(),
        };
        self.0.timeline.append(clone_transition(
            project_id,
            project_entity(project_id),
            "branch_clone_created",
            json!({
                "path": path,
                "branch": branch,
                "source": source,
            }),
        ))?;
        announce(events, LiveEventName::CloneCreated, &record);
        encode(&record)
    }
}

/// Serves `clone.remove`: validate every precondition, then — and only
/// then — let the fleet skill remove the clone. The Workspace record
/// is preserved untouched (DR-LW-11).
struct RemoveClone(CloneContext);

impl CommandHandler for RemoveClone {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<CloneRemoveRequest>(payload)?;
        ParsedCommand::lift("clone", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: CloneRemoveRequest = parse_payload(&command.payload)?;
        Ok(load_workspace(&self.0.workspaces, request.workspace_id)?.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        events: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: CloneRemoveRequest = parse_payload(&command.payload)?;
        let workspace = load_workspace(&self.0.workspaces, request.workspace_id)?;
        let project_id = workspace.registration().project_id();
        let project = load_project(&self.0.projects, project_id)?;
        if project.is_archived() {
            return Err(refuse("archived Projects accept no clone commands"));
        }
        let path = workspace.registration().path().to_owned();
        let facts = WorkspaceCloneFacts::from_workspace(&workspace);
        if let Some(conflict) = clone_remove_conflict(&facts) {
            record_after_discard(
                &self.0.timeline,
                events,
                clone_transition(
                    project_id,
                    workspace_entity(workspace.id()),
                    "clone_remove_refused",
                    remove_refusal_facts(workspace.id().value(), &path, &conflict),
                ),
            );
            return Err(refuse(&conflict));
        }
        if let Err(error) = self.0.tool.remove_clone(&path) {
            record_after_discard(
                &self.0.timeline,
                events,
                clone_transition(
                    project_id,
                    workspace_entity(workspace.id()),
                    "clone_remove_refused",
                    json!({
                        "workspace_id": workspace.id().value(),
                        "path": path,
                        "reason": tool_failure_reason(&error),
                        "error": error.message,
                    }),
                ),
            );
            return Err(error);
        }
        let record = CloneRemovedRecord {
            project_id: project_id.value(),
            workspace_id: workspace.id().value(),
            branch: workspace.observation().branch().map(str::to_owned),
            path,
        };
        self.0.timeline.append(clone_transition(
            project_id,
            workspace_entity(workspace.id()),
            "branch_clone_removed",
            json!({
                "workspace_id": record.workspace_id,
                "path": record.path,
                "branch": record.branch,
            }),
        ))?;
        announce(events, LiveEventName::CloneRemoved, &record);
        encode(&record)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex};

    use kanban_dto::ApiError;
    use serde_json::Value;

    use super::{CloneGuardStore, FleetCloneTool};
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::EventSink;
    use crate::lane::testing::MemoryLaneStore;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::plan::testing::MemoryProjects;
    use crate::spec::testing::MemorySpecs;
    use crate::ticket::testing::{MemoryTicketEvidence, MemoryTickets};
    use crate::timeline::TimelineEnvelope;
    use crate::workspace::testing::MemoryWorkspaceStore;

    /// One recorded fleet skill invocation.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum CloneCall {
        Add {
            source: String,
            path: String,
            branch: String,
        },
        Remove {
            path: String,
        },
    }

    /// The fleet tool the tests steer: it records every invocation and
    /// answers from a script of per-call outcomes. An empty script
    /// accepts everything, so a test that forgets to steer simply
    /// succeeds like the real skill would.
    #[derive(Default)]
    pub(crate) struct ScriptedCloneTool {
        pub(crate) calls: Mutex<Vec<CloneCall>>,
        pub(crate) outcomes: Mutex<Vec<Result<(), ApiError>>>,
    }

    impl ScriptedCloneTool {
        /// Answer the next call from the script.
        fn next_outcome(&self) -> Result<(), ApiError> {
            let mut outcomes = self.outcomes.lock().expect("the script lock is sound");
            if outcomes.is_empty() {
                Ok(())
            } else {
                outcomes.remove(0)
            }
        }

        /// Every invocation so far, in order.
        pub(crate) fn calls(&self) -> Vec<CloneCall> {
            self.calls.lock().expect("the calls lock is sound").clone()
        }
    }

    impl FleetCloneTool for ScriptedCloneTool {
        fn add_clone(&self, source: &str, path: &str, branch: &str) -> Result<(), ApiError> {
            self.calls
                .lock()
                .expect("the calls lock is sound")
                .push(CloneCall::Add {
                    source: source.to_owned(),
                    path: path.to_owned(),
                    branch: branch.to_owned(),
                });
            self.next_outcome()
        }

        fn remove_clone(&self, path: &str) -> Result<(), ApiError> {
            self.calls
                .lock()
                .expect("the calls lock is sound")
                .push(CloneCall::Remove {
                    path: path.to_owned(),
                });
            self.next_outcome()
        }
    }

    /// The in-memory clone guard timeline: the rows it was asked to
    /// land, invocation and refusal alike.
    #[derive(Default)]
    pub(crate) struct MemoryCloneGuardStore {
        rows: Mutex<Vec<TimelineEnvelope>>,
    }

    impl MemoryCloneGuardStore {
        /// The appended rows, in order.
        pub(crate) fn rows(&self) -> Vec<TimelineEnvelope> {
            self.rows.lock().expect("the rows lock is sound").clone()
        }
    }

    impl CloneGuardStore for MemoryCloneGuardStore {
        fn append(&self, envelope: TimelineEnvelope) -> Result<(), ApiError> {
            self.rows
                .lock()
                .expect("the rows lock is sound")
                .push(envelope);
            Ok(())
        }
    }

    /// Records every live event the core publishes.
    #[derive(Default)]
    pub(crate) struct RecordingSink {
        pub(crate) events: Mutex<Vec<(String, Value)>>,
    }

    impl EventSink for RecordingSink {
        fn emit(&self, event_type: &str, payload: Value) {
            self.events
                .lock()
                .expect("the sink lock is sound")
                .push((event_type.to_owned(), payload));
        }
    }

    /// A core with the Workspace, Lane, and guarded clone operations
    /// wired to in-memory stores over one active Project, whose fleet
    /// tool and published events the test steers and reads. The Lane
    /// store shares the Workspace rows, exactly as the SQLite store
    /// does.
    pub(crate) struct CloneHarness {
        pub(crate) projects: Arc<MemoryProjects>,
        pub(crate) workspaces: Arc<MemoryWorkspaceStore>,
        pub(crate) tool: Arc<ScriptedCloneTool>,
        pub(crate) timeline: Arc<MemoryCloneGuardStore>,
        pub(crate) sink: Arc<RecordingSink>,
        pub(crate) core: Core,
    }

    /// A harness whose git observer the test chooses.
    pub(crate) fn clone_harness_with_observer(
        observer: Arc<dyn crate::workspace::WorkspaceGitObserver>,
    ) -> CloneHarness {
        let projects = Arc::new(MemoryProjects::default());
        projects.seed(crate::plan::testing::active_project(
            1,
            "CORE",
            kanban_domain::ProjectCounters::zeroed(),
        ));
        let specs = Arc::new(MemorySpecs::sharing(projects.clone()));
        let tickets = Arc::new(MemoryTickets::sharing(projects.clone()));
        let workspaces = Arc::new(MemoryWorkspaceStore::default());
        let lanes = Arc::new(MemoryLaneStore::sharing(workspaces.clone()));
        let tool = Arc::new(ScriptedCloneTool::default());
        let timeline = Arc::new(MemoryCloneGuardStore::default());
        let sink = Arc::new(RecordingSink::default());
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            sink.clone(),
        );
        core.register_tickets(
            tickets.clone(),
            projects.clone(),
            specs,
            Arc::new(MemoryTicketEvidence::default()),
        )
        .expect("the ticket operations register");
        core.register_workspaces(workspaces.clone(), projects.clone(), observer)
            .expect("the workspace operations register");
        core.register_lanes(
            lanes.clone(),
            projects.clone(),
            workspaces.clone(),
            tickets.clone(),
        )
        .expect("the lane operations register");
        core.register_clones(
            tool.clone(),
            projects.clone(),
            workspaces.clone(),
            timeline.clone(),
        )
        .expect("the clone operations register");
        CloneHarness {
            projects,
            workspaces,
            tool,
            timeline,
            sink,
            core,
        }
    }

    /// A harness with a silent git observer: paths read as missing.
    pub(crate) fn clone_harness() -> CloneHarness {
        clone_harness_with_observer(Arc::new(
            crate::workspace::testing::ScriptedObserver::default(),
        ))
    }

    /// A harness whose observer reads `path` as a clean clone on
    /// `branch`, so a registered Workspace there reports that branch.
    pub(crate) fn observed_harness(path: &str, branch: &str) -> CloneHarness {
        use std::collections::HashMap;

        clone_harness_with_observer(Arc::new(crate::workspace::testing::ScriptedObserver {
            snapshots: HashMap::from([(
                path.to_owned(),
                crate::workspace::WorkspaceGitSnapshot {
                    present: true,
                    repository_identity: Some("identity".to_owned()),
                    checkout: Some(kanban_domain::WorkspaceCheckout::Branch(branch.to_owned())),
                    head: Some("abc123".to_owned()),
                    working_tree_clean: Some(true),
                    unique_unlanded_commits: Some(false),
                },
            )]),
        }))
    }
}

#[cfg(test)]
mod guarded_clone {
    use serde_json::{Value, json};

    use kanban_dto::ErrorCode;

    use super::testing::{CloneCall, clone_harness, observed_harness};
    use crate::clone::testing::CloneHarness;

    fn mutation(version: u64, key: &str) -> Value {
        json!({ "optimistic_version": version, "idempotency_key": key })
    }

    fn create(path: &str, branch: &str, key: &str) -> Value {
        json!({
            "mutation": mutation(0, key),
            "project_id": 1,
            "path": path,
            "branch": branch,
        })
    }

    fn create_in_project(project_id: u64, path: &str, branch: &str, key: &str) -> Value {
        json!({
            "mutation": mutation(0, key),
            "project_id": project_id,
            "path": path,
            "branch": branch,
        })
    }

    fn remove(workspace_id: u64, key: &str, version: u64) -> Value {
        json!({
            "mutation": mutation(version, key),
            "workspace_id": workspace_id,
        })
    }

    fn register_workspace(harness: &CloneHarness, path: &str, key: &str) -> Value {
        harness
            .core
            .command(
                "workspace.register",
                &json!({ "mutation": mutation(0, key), "project_id": 1, "path": path }),
            )
            .expect("the workspace registers")
    }

    fn register_and_observe(harness: &CloneHarness, path: &str, key: &str) -> Value {
        let workspace_id = register_workspace(harness, path, key)["id"]
            .as_u64()
            .expect("the identity is a number");
        harness
            .core
            .command(
                "workspace.observe",
                &json!({
                    "mutation": mutation(1, "observe-key"),
                    "workspace_id": workspace_id,
                }),
            )
            .expect("the observation applies")
    }

    #[test]
    fn creating_invokes_the_fleet_skill_with_the_registered_repository() {
        let harness = clone_harness();

        let response = harness
            .core
            .command(
                "clone.create",
                &create("/workspaces/kanban.fleet-t34", "fleet/kan-t34", "key-1"),
            )
            .expect("the guarded create applies");

        assert_eq!(response["project_id"], json!(1));
        assert_eq!(response["path"], json!("/workspaces/kanban.fleet-t34"));
        assert_eq!(response["branch"], json!("fleet/kan-t34"));
        assert_eq!(
            harness.tool.calls(),
            vec![CloneCall::Add {
                source: "/repositories/kanban".to_owned(),
                path: "/workspaces/kanban.fleet-t34".to_owned(),
                branch: "fleet/kan-t34".to_owned(),
            }],
            "the wrapper hands the fleet skill the Project's registered repository, never a client-supplied source"
        );
    }

    #[test]
    fn creating_appends_the_invocation_and_announces_the_live_event() {
        let harness = clone_harness();

        harness
            .core
            .command(
                "clone.create",
                &create("/workspaces/kanban.fleet-t34", "fleet/kan-t34", "key-1"),
            )
            .expect("the guarded create applies");

        let rows = harness.timeline.rows();
        assert_eq!(rows.len(), 1, "one invocation row, nothing else");
        assert_eq!(rows[0].detail()["action"], json!("branch_clone_created"));
        assert_eq!(
            rows[0].detail()["path"],
            json!("/workspaces/kanban.fleet-t34")
        );
        assert_eq!(rows[0].detail()["branch"], json!("fleet/kan-t34"));
        assert_eq!(
            rows[0].detail()["source"],
            json!("/repositories/kanban"),
            "the durable record names the repository it cloned from"
        );
        assert_eq!(
            rows[0].entity().map(|entity| entity.kind),
            Some(kanban_dto::TimelineEntityKind::Project)
        );
        let events = harness
            .sink
            .events
            .lock()
            .expect("the sink lock is sound")
            .clone();
        assert_eq!(events.len(), 1, "one live event, nothing else");
        assert_eq!(events[0].0, "clone.created");
        assert_eq!(events[0].1["path"], json!("/workspaces/kanban.fleet-t34"));
    }

    #[test]
    fn removing_invokes_the_fleet_skill_on_the_workspace_path() {
        let harness = observed_harness("/workspaces/kanban.fleet-t31", "fleet/kan-t31");
        register_and_observe(&harness, "/workspaces/kanban.fleet-t31", "key-1");

        let response = harness
            .core
            .command("clone.remove", &remove(1, "key-2", 2))
            .expect("the guarded remove applies");

        assert_eq!(response["project_id"], json!(1));
        assert_eq!(response["workspace_id"], json!(1));
        assert_eq!(response["path"], json!("/workspaces/kanban.fleet-t31"));
        assert_eq!(
            response["branch"],
            json!("fleet/kan-t31"),
            "the record reports the branch the Workspace last observed"
        );
        assert_eq!(
            harness.tool.calls(),
            vec![CloneCall::Remove {
                path: "/workspaces/kanban.fleet-t31".to_owned(),
            }]
        );
    }

    #[test]
    fn removing_appends_the_invocation_on_the_workspace_timeline() {
        let harness = observed_harness("/workspaces/kanban.fleet-t31", "fleet/kan-t31");
        register_and_observe(&harness, "/workspaces/kanban.fleet-t31", "key-1");

        harness
            .core
            .command("clone.remove", &remove(1, "key-2", 2))
            .expect("the guarded remove applies");

        let rows = harness.timeline.rows();
        assert_eq!(rows.len(), 1, "one invocation row, nothing else");
        assert_eq!(rows[0].detail()["action"], json!("branch_clone_removed"));
        assert_eq!(rows[0].detail()["workspace_id"], json!(1));
        assert_eq!(
            rows[0].detail()["path"],
            json!("/workspaces/kanban.fleet-t31")
        );
        assert_eq!(
            rows[0].entity().map(|entity| entity.kind),
            Some(kanban_dto::TimelineEntityKind::Workspace)
        );
    }

    #[test]
    fn removal_preserves_the_workspace_record() {
        let harness = observed_harness("/workspaces/kanban.fleet-t31", "fleet/kan-t31");
        register_and_observe(&harness, "/workspaces/kanban.fleet-t31", "key-1");

        harness
            .core
            .command("clone.remove", &remove(1, "key-2", 2))
            .expect("the guarded remove applies");

        let (stored, _) = harness.workspaces.snapshot();
        assert_eq!(
            stored.len(),
            1,
            "the Workspace record is never deleted (DR-LW-11)"
        );
        assert_eq!(stored[0].id().value(), 1);
        assert!(!stored[0].is_retired(), "removal retires nothing");
    }

    /// KAN-T121-AC3: an internal tool failure is a tool failure — it
    /// must not pose as an invalid request, and the durable row must
    /// agree with the caller's error about what happened.
    #[test]
    fn an_internal_tool_failure_is_not_mapped_as_a_refusal() {
        let harness = clone_harness();
        harness
            .tool
            .outcomes
            .lock()
            .expect("the script lock is sound")
            .push(Err(kanban_dto::ApiError::internal(
                "the fleet clone skill `git bc-add` failed: fatal: could not read from remote repository",
            )));

        let error = harness
            .core
            .command(
                "clone.create",
                &create("/workspaces/kanban.fleet-t34", "fleet/kan-t34", "key-1"),
            )
            .expect_err("the failed invocation refuses the command");

        assert_eq!(
            error.code,
            ErrorCode::Internal,
            "a tool failure must never surface as invalid_request"
        );
        assert!(
            error
                .message
                .contains("could not read from remote repository"),
            "the sanitised failure text survives the wrapping: {}",
            error.message
        );
        assert_eq!(harness.tool.calls().len(), 1, "the skill was invoked");
        let rows = harness.timeline.rows();
        assert_eq!(rows.len(), 1, "the failed invocation still records");
        assert_eq!(rows[0].detail()["action"], json!("clone_create_refused"));
        assert_eq!(rows[0].detail()["reason"], json!("fleet_tool_failed"));
        assert!(
            rows[0].detail()["error"]
                .as_str()
                .expect("the error text is recorded")
                .contains("could not read from remote repository"),
            "the durable row carries the sanitised failure"
        );
    }

    /// KAN-T121-AC3: the fleet skill's own refusal is the caller's
    /// refusal, and both surfaces — the caller's error and the durable
    /// row — classify it the same way.
    #[test]
    fn a_refused_invocation_classifies_the_caller_on_both_surfaces() {
        let harness = clone_harness();
        harness
            .tool
            .outcomes
            .lock()
            .expect("the script lock is sound")
            .push(Err(kanban_dto::ApiError::invalid_request(
                "the fleet clone skill `git bc-rm` refused: refusing to remove the clone holding unpushed work",
            )));

        let error = harness
            .core
            .command(
                "clone.create",
                &create("/workspaces/kanban.fleet-t34", "fleet/kan-t34", "key-1"),
            )
            .expect_err("the refused invocation refuses the command");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(harness.tool.calls().len(), 1, "the skill was invoked");
        let rows = harness.timeline.rows();
        assert_eq!(rows.len(), 1, "the refused invocation still records");
        assert_eq!(rows[0].detail()["reason"], json!("fleet_tool_refused"));
        assert_eq!(
            rows[0].detail()["error"],
            json!(
                "the fleet clone skill `git bc-rm` refused: refusing to remove the clone holding unpushed work"
            ),
            "the durable row carries the refusal text verbatim"
        );
    }

    #[test]
    fn a_failed_removal_is_refused_and_recorded() {
        let harness = observed_harness("/workspaces/kanban.fleet-t31", "fleet/kan-t31");
        register_and_observe(&harness, "/workspaces/kanban.fleet-t31", "key-1");
        harness
            .tool
            .outcomes
            .lock()
            .expect("the script lock is sound")
            .push(Err(kanban_dto::ApiError::internal(
                "the fleet clone skill `git bc-rm` failed: the clone holds unique commits",
            )));

        let error = harness
            .core
            .command("clone.remove", &remove(1, "key-2", 2))
            .expect_err("the failed invocation refuses the command");

        assert_eq!(error.code, ErrorCode::Internal);
        assert_eq!(harness.tool.calls().len(), 1, "the skill was invoked");
        let rows = harness.timeline.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].detail()["action"], json!("clone_remove_refused"));
        assert_eq!(rows[0].detail()["reason"], json!("fleet_tool_failed"));
        let (stored, _) = harness.workspaces.snapshot();
        assert_eq!(stored.len(), 1, "nothing was removed");
    }

    #[test]
    fn creating_refuses_an_unknown_project() {
        let harness = clone_harness();

        let error = harness
            .core
            .command(
                "clone.create",
                &create_in_project(9, "/workspaces/kanban.fleet-t34", "fleet/kan-t34", "key-1"),
            )
            .expect_err("the unknown Project is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(
            harness.tool.calls().is_empty(),
            "nothing is invoked for an unknown Project"
        );
        assert!(
            harness.timeline.rows().is_empty(),
            "no timeline row lands for an unknown Project"
        );
    }

    #[test]
    fn removing_an_unknown_workspace_is_not_found() {
        let harness = clone_harness();

        let error = harness
            .core
            .command("clone.remove", &remove(9, "key-1", 1))
            .expect_err("the unknown Workspace is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(harness.tool.calls().is_empty());
        assert!(harness.timeline.rows().is_empty());
    }

    #[test]
    fn creating_refuses_an_archived_project() {
        let harness = clone_harness();
        harness.projects.seed(kanban_domain::Project::restore(
            kanban_domain::ProjectId::new(2),
            kanban_domain::ProjectRegistration::new(
                "OLD",
                "Retired work",
                "/repositories/old",
                "/workspaces/old.seed",
                "main",
                "old.seed",
                Some("old-main"),
                None,
            )
            .expect("the fixture registration validates"),
            kanban_domain::ProjectState::Archived,
            kanban_domain::ProjectCounters::zeroed(),
            1,
        ));

        let error = harness
            .core
            .command(
                "clone.create",
                &create_in_project(2, "/workspaces/old.fleet", "fleet/old", "key-1"),
            )
            .expect_err("the archived Project is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("archived"));
        assert!(
            harness.tool.calls().is_empty(),
            "an archived Project invokes nothing"
        );
    }

    #[test]
    fn removing_refuses_an_archived_project() {
        let harness = observed_harness("/workspaces/kanban.fleet-t31", "fleet/kan-t31");
        register_and_observe(&harness, "/workspaces/kanban.fleet-t31", "key-1");
        // Archive the Project the Workspace belongs to, keeping its
        // identity, so the refusal is the archive rule alone.
        harness.projects.replace(kanban_domain::Project::restore(
            kanban_domain::ProjectId::new(1),
            kanban_domain::ProjectRegistration::new(
                "CORE",
                "Control plane",
                "/repositories/kanban",
                "/workspaces/kanban.seed",
                "main",
                "kanban.seed",
                Some("kanban-main"),
                None,
            )
            .expect("the fixture registration validates"),
            kanban_domain::ProjectState::Archived,
            kanban_domain::ProjectCounters::zeroed(),
            2,
        ));

        let error = harness
            .core
            .command("clone.remove", &remove(1, "key-2", 2))
            .expect_err("the archived Project is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("archived"));
        assert!(
            harness.tool.calls().is_empty(),
            "an archived Project invokes nothing"
        );
    }

    #[test]
    fn removing_with_a_stale_workspace_version_is_rejected() {
        let harness = observed_harness("/workspaces/kanban.fleet-t31", "fleet/kan-t31");
        register_and_observe(&harness, "/workspaces/kanban.fleet-t31", "key-1");

        let error = harness
            .core
            .command("clone.remove", &remove(1, "key-2", 1))
            .expect_err("the stale version is refused");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(2));
        assert!(harness.tool.calls().is_empty());
    }
}

#[cfg(test)]
mod clone_conflicts {
    use serde_json::{Value, json};

    use kanban_dto::ErrorCode;

    use super::testing::{clone_harness, observed_harness};

    fn mutation(version: u64, key: &str) -> Value {
        json!({ "optimistic_version": version, "idempotency_key": key })
    }

    fn create(path: &str, branch: &str, key: &str) -> Value {
        json!({
            "mutation": mutation(0, key),
            "project_id": 1,
            "path": path,
            "branch": branch,
        })
    }

    fn remove(workspace_id: u64, key: &str, version: u64) -> Value {
        json!({
            "mutation": mutation(version, key),
            "workspace_id": workspace_id,
        })
    }

    #[test]
    fn creating_refuses_the_declared_seed_path_before_any_invocation() {
        let harness = clone_harness();

        let error = harness
            .core
            .command(
                "clone.create",
                &create("/workspaces/kanban.seed", "fleet/kan-t34", "key-1"),
            )
            .expect_err("the Seed path is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("/workspaces/kanban.seed"),
            "the refusal names the path: {}",
            error.message
        );
        assert!(
            error.message.contains("Seed"),
            "the refusal names the rule: {}",
            error.message
        );
        assert!(
            harness.tool.calls().is_empty(),
            "the conflict is refused before anything is invoked"
        );
        let rows = harness.timeline.rows();
        assert_eq!(rows.len(), 1, "the refusal is recorded");
        assert_eq!(rows[0].detail()["action"], json!("clone_create_refused"));
        assert_eq!(rows[0].detail()["reason"], json!("seed_path"));
        assert_eq!(rows[0].detail()["path"], json!("/workspaces/kanban.seed"));
    }

    #[test]
    fn creating_refuses_an_equivalent_spelling_of_the_seed_path() {
        let harness = clone_harness();

        let error = harness
            .core
            .command(
                "clone.create",
                &create("/workspaces/kanban.seed/", "fleet/kan-t34", "key-1"),
            )
            .expect_err("a trailing separator still names the Seed path");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("Seed"),
            "the refusal names the rule: {}",
            error.message
        );
        assert!(
            harness.tool.calls().is_empty(),
            "the conflict is refused before anything is invoked"
        );
        let rows = harness.timeline.rows();
        assert_eq!(rows.len(), 1, "the refusal is recorded");
        assert_eq!(rows[0].detail()["action"], json!("clone_create_refused"));
        assert_eq!(rows[0].detail()["reason"], json!("seed_path"));
        assert_eq!(
            rows[0].detail()["path"],
            json!("/workspaces/kanban.seed/"),
            "the row records the spelling that was asked for"
        );
    }

    #[test]
    fn creating_refuses_a_registered_workspace_path_and_names_the_holder() {
        let harness = clone_harness();
        harness
            .core
            .command(
                "workspace.register",
                &json!({
                    "mutation": mutation(0, "key-1"),
                    "project_id": 1,
                    "path": "/workspaces/kanban.fleet-t31",
                }),
            )
            .expect("the workspace registers");

        let error = harness
            .core
            .command(
                "clone.create",
                &create("/workspaces/kanban.fleet-t31", "fleet/kan-t34", "key-2"),
            )
            .expect_err("the taken path is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("already registered as Workspace 1"),
            "the refusal names the holder: {}",
            error.message
        );
        assert!(harness.tool.calls().is_empty());
        let rows = harness.timeline.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].detail()["reason"], json!("path_taken"));
        assert_eq!(rows[0].detail()["workspace_id"], json!(1));
    }

    #[test]
    fn creating_refuses_a_branch_another_workspace_holds() {
        let harness = observed_harness("/workspaces/kanban.fleet-t31", "fleet/kan-t31");
        harness
            .core
            .command(
                "workspace.register",
                &json!({
                    "mutation": mutation(0, "key-1"),
                    "project_id": 1,
                    "path": "/workspaces/kanban.fleet-t31",
                }),
            )
            .expect("the workspace registers");
        harness
            .core
            .command(
                "workspace.observe",
                &json!({ "mutation": mutation(1, "key-2"), "workspace_id": 1 }),
            )
            .expect("the observation applies");

        let error = harness
            .core
            .command(
                "clone.create",
                &create("/workspaces/kanban.fleet-t34", "fleet/kan-t31", "key-3"),
            )
            .expect_err("the held branch is refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("already checked out"),
            "the refusal names the branch and its holder: {}",
            error.message
        );
        assert!(harness.tool.calls().is_empty());
        let rows = harness.timeline.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].detail()["reason"], json!("branch_checked_out"));
        assert_eq!(rows[0].detail()["branch"], json!("fleet/kan-t31"));
    }

    #[test]
    fn removing_refuses_the_seed_workspace() {
        let harness = clone_harness();
        harness
            .core
            .command(
                "workspace.register",
                &json!({
                    "mutation": mutation(0, "key-1"),
                    "project_id": 1,
                    "path": "/workspaces/kanban.seed",
                }),
            )
            .expect("the Seed workspace registers");

        let error = harness
            .core
            .command("clone.remove", &remove(1, "key-2", 1))
            .expect_err("the Seed is never a branch clone");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("Seed Workspace"),
            "the refusal names the rule: {}",
            error.message
        );
        assert!(harness.tool.calls().is_empty());
        let rows = harness.timeline.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].detail()["action"], json!("clone_remove_refused"));
        assert_eq!(rows[0].detail()["reason"], json!("seed_workspace"));
        assert_eq!(
            rows[0].entity().map(|entity| entity.kind),
            Some(kanban_dto::TimelineEntityKind::Workspace),
            "the refusal row is about the Workspace that was protected"
        );
        let (stored, _) = harness.workspaces.snapshot();
        assert_eq!(stored.len(), 1, "the Seed record survives the refusal");
    }

    #[test]
    fn removing_refuses_a_workspace_a_lane_claims() {
        let harness = observed_harness("/workspaces/kanban.fleet-t31", "fleet/kan-t31");
        harness
            .core
            .command(
                "workspace.register",
                &json!({
                    "mutation": mutation(0, "key-1"),
                    "project_id": 1,
                    "path": "/workspaces/kanban.fleet-t31",
                }),
            )
            .expect("the workspace registers");
        harness
            .core
            .command(
                "workspace.observe",
                &json!({ "mutation": mutation(1, "key-2"), "workspace_id": 1 }),
            )
            .expect("the observation applies");
        harness
            .core
            .command(
                "lane.create",
                &json!({ "mutation": mutation(0, "key-3"), "project_id": 1 }),
            )
            .expect("the lane creates");
        harness
            .core
            .command(
                "lane.workspace.assign",
                &json!({
                    "mutation": mutation(1, "key-4"),
                    "lane_id": 1,
                    "workspace_id": 1,
                }),
            )
            .expect("the Lane claims the Workspace");

        let error = harness
            .core
            .command("clone.remove", &remove(1, "key-5", 3))
            .expect_err("a claimed Workspace is never removed");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("assigned to Lane 1"),
            "the refusal names the Lane: {}",
            error.message
        );
        assert!(harness.tool.calls().is_empty());
        let rows = harness.timeline.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].detail()["reason"], json!("lane_assigned"));
        assert_eq!(rows[0].detail()["lane_id"], json!(1));
        let (stored, _) = harness.workspaces.snapshot();
        assert_eq!(
            stored[0].lane_id(),
            Some(1),
            "the claim survives the refused removal"
        );
    }

    #[test]
    fn blank_targets_are_refused_without_invoking_or_recording() {
        let harness = clone_harness();

        let blank_path = harness
            .core
            .command("clone.create", &create("   ", "fleet/kan-t34", "key-1"))
            .expect_err("a blank path is refused");
        let blank_branch = harness
            .core
            .command(
                "clone.create",
                &create("/workspaces/kanban.fleet-t34", " ", "key-2"),
            )
            .expect_err("a blank branch is refused");

        assert_eq!(blank_path.code, ErrorCode::InvalidRequest);
        assert_eq!(blank_branch.code, ErrorCode::InvalidRequest);
        assert!(
            harness.tool.calls().is_empty(),
            "payload validation invokes nothing"
        );
        assert!(
            harness.timeline.rows().is_empty(),
            "payload validation is not a guard conflict and records no row"
        );
    }
}
