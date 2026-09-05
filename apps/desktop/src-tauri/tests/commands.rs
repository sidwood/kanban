//! Shell command boundary: unknown fields are refused and catalogued
//! requests reach the core with their shape unchanged.

use std::sync::{Arc, Mutex};

use kanban_app::{
    CommandEffects, CommandHandler, Core, MemoryIdempotencyStore, OperationDescriptor,
    OperationKind, ParsedCommand, QueryHandler, exposed_operations,
};
use kanban_desktop_lib::Shell;
use kanban_desktop_lib::commands::{forward_command_value, forward_query_value, install_link};
use kanban_dto::{
    CommentCreateRequest, CommentEditRequest, CommentRevisionsQuery, DeferralListQuery,
    DeferralRecordRequest, DeferralSupersedeRequest, EvidenceAttachRequest, EvidenceListQuery,
    HealthQuery, HerdrDefaultsGetQuery, HerdrDefaultsUpdateRequest, HerdrSettingsGetQuery,
    HerdrSettingsUpdateRequest, InitiativeArchiveRequest, InitiativeCreateRequest,
    InitiativeListQuery, InitiativeRenameRequest, MutationContext, PlanActivateRequest,
    PlanArchiveRequest, PlanCancelRequest, PlanCompleteRequest, PlanCreateRequest,
    PlanEdgeAddRequest, PlanEdgeRemoveRequest, PlanGetQuery, PlanListQuery, PlanReplanRequest,
    PlanSpecAddRequest, PlanSpecMoveRequest, PlanSpecRemoveRequest, ProjectArchiveRequest,
    ProjectListQuery, ProjectRegisterRequest, RulingListQuery, RulingRecordRequest,
    RulingSupersedeRequest, SpecContent, SpecContentUpdateRequest, SpecCoverageCheckQuery,
    SpecCreateRequest, SpecExecutionMoveRequest, SpecGetQuery, SpecListQuery, SpecPlanJoinRequest,
    SpecVersionApproveRequest, SpecVersionGetQuery, SpecVersionSupersedeRequest,
    TimelineEntityKind, TimelineEntityRef, TimelineQuery, TimelineScope, WorkspaceListQuery,
    WorkspaceObserveRequest, WorkspaceRegisterRequest, WorkspaceRetireRequest,
};
use kanban_transport::SocketServer;
use serde_json::{Value, json};
use tempfile::TempDir;

/// Payloads the recording core saw, keyed by operation name.
#[derive(Default)]
struct Recorder {
    payloads: Mutex<Vec<(String, Value)>>,
}

impl Recorder {
    fn push(&self, operation: &str, payload: &Value) {
        self.payloads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((operation.to_owned(), payload.clone()));
    }

    fn last_for(&self, operation: &str) -> Option<Value> {
        self.payloads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .rev()
            .find(|(name, _)| name == operation)
            .map(|(_, payload)| payload.clone())
    }
}

struct RecordingQuery {
    recorder: Arc<Recorder>,
    operation: &'static str,
}

impl QueryHandler for RecordingQuery {
    fn handle(&self, payload: &Value) -> Result<Value, kanban_dto::ApiError> {
        self.recorder.push(self.operation, payload);
        Ok(json!({}))
    }
}

struct RecordingCommand {
    recorder: Arc<Recorder>,
    operation: &'static str,
}

impl CommandHandler for RecordingCommand {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, kanban_dto::ApiError> {
        ParsedCommand::lift("recording", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, kanban_dto::ApiError> {
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn CommandEffects,
    ) -> Result<Value, kanban_dto::ApiError> {
        self.recorder.push(self.operation, &command.payload);
        Ok(json!({}))
    }
}

fn mutation_for(schema: &str) -> MutationContext {
    MutationContext {
        optimistic_version: 0,
        idempotency_key: format!("key-{schema}"),
    }
}

fn entity() -> TimelineEntityRef {
    TimelineEntityRef {
        kind: TimelineEntityKind::Ticket,
        id: "KAN-T1".to_owned(),
    }
}

fn production_timeline_query_fixture() -> Value {
    serde_json::to_value(TimelineQuery {
        scope: TimelineScope::Project(1),
        entity: None,
        kinds: None,
        since: None,
        until: None,
    })
    .expect("the production timeline query encodes")
}

fn spec_content() -> SpecContent {
    SpecContent {
        name: "Plans and specifications".to_owned(),
        short_description: "Versioned Plan graphs of Specs".to_owned(),
        problem_statement: "Planning must survive change.".to_owned(),
        solution: "Immutable approved versions.".to_owned(),
        user_stories: "KAN-S3-US4".to_owned(),
        implementation_decisions: "Supersession is explicit.".to_owned(),
        testing_decisions: "Domain tests prove immutability.".to_owned(),
        out_of_scope: "The Ticket graph proposal.".to_owned(),
        further_notes: "None".to_owned(),
    }
}

fn sample_request(schema: &str) -> Value {
    let mutation = mutation_for(schema);
    match schema {
        "HealthQuery" | "InitiativeListQuery" | "ProjectListQuery" => json!({}),
        "InitiativeCreateRequest" => json!({ "mutation": mutation, "name": "Alpha" }),
        "InitiativeRenameRequest" => {
            json!({ "mutation": mutation, "initiative_id": 1, "name": "Beta" })
        }
        "InitiativeArchiveRequest" => json!({ "mutation": mutation, "initiative_id": 1 }),
        "ProjectRegisterRequest" => json!({
            "mutation": mutation,
            "code": "CORE",
            "name": "Control plane",
            "repository": "/repositories/kanban",
            "seed_workspace": "/workspaces/kanban.seed",
            "default_branch": "main",
            "herdr_workspace": "kanban.seed",
            "herdr_session": "kanban-main",
            "initiative_id": null,
        }),
        "ProjectArchiveRequest" => json!({ "mutation": mutation, "project_id": 1 }),
        "PlanCreateRequest" => json!({ "mutation": mutation, "project_id": 1 }),
        "PlanSpecAddRequest" => {
            json!({ "mutation": mutation, "plan_id": 1, "spec_number": 2 })
        }
        "PlanSpecRemoveRequest" => {
            json!({ "mutation": mutation, "plan_id": 1, "spec_number": 2 })
        }
        "PlanSpecMoveRequest" => json!({
            "mutation": mutation,
            "plan_id": 1,
            "spec_number": 2,
            "position": 0,
        }),
        "PlanEdgeAddRequest" => {
            json!({ "mutation": mutation, "plan_id": 1, "from_spec": 1, "to_spec": 2 })
        }
        "PlanEdgeRemoveRequest" => {
            json!({ "mutation": mutation, "plan_id": 1, "from_spec": 1, "to_spec": 2 })
        }
        "PlanActivateRequest" => json!({ "mutation": mutation, "plan_id": 1 }),
        "PlanReplanRequest" => json!({ "mutation": mutation, "plan_id": 1 }),
        "PlanCompleteRequest" => json!({ "mutation": mutation, "plan_id": 1 }),
        "PlanCancelRequest" => json!({ "mutation": mutation, "plan_id": 1 }),
        "PlanArchiveRequest" => json!({ "mutation": mutation, "plan_id": 1 }),
        "PlanListQuery" => json!({ "project_id": 1 }),
        "PlanGetQuery" => json!({ "plan_id": 1 }),
        "SpecCreateRequest" => {
            json!({ "mutation": mutation, "project_id": 1, "content": spec_content() })
        }
        "SpecContentUpdateRequest" => {
            json!({ "mutation": mutation, "spec_id": 1, "content": spec_content() })
        }
        "SpecVersionApproveRequest" => json!({ "mutation": mutation, "spec_id": 1 }),
        "SpecVersionSupersedeRequest" => {
            json!({ "mutation": mutation, "spec_id": 1, "version": 1 })
        }
        "SpecPlanJoinRequest" => json!({ "mutation": mutation, "spec_id": 1, "plan_id": 1 }),
        "SpecExecutionMoveRequest" => {
            json!({ "mutation": mutation, "spec_id": 1, "execution": "ready" })
        }
        "SpecListQuery" => json!({ "project_id": 1 }),
        "SpecGetQuery" => json!({ "spec_id": 1 }),
        "SpecVersionGetQuery" => json!({ "spec_id": 1, "number": 1 }),
        "SpecCoverageCheckQuery" => json!({
            "spec_id": 1,
            "version": 1,
            "criteria": [
                {
                    "outcome": "Every criterion links to one or more User Stories.",
                    "stories": ["CORE-S3-US6"],
                }
            ],
        }),
        "TimelineQuery" => production_timeline_query_fixture(),
        "CommentCreateRequest" => json!({
            "mutation": mutation,
            "project_id": 1,
            "target": entity(),
            "text": "hello",
        }),
        "CommentEditRequest" => json!({
            "mutation": mutation,
            "comment_id": 1,
            "text": "edited",
        }),
        "CommentRevisionsQuery" => json!({ "comment_id": 1 }),
        "RulingRecordRequest" => json!({
            "mutation": mutation,
            "project_id": 1,
            "summary": "ship it",
        }),
        "RulingSupersedeRequest" => json!({
            "mutation": mutation,
            "project_id": 1,
            "ruling_id": 1,
            "summary": "revise it",
        }),
        "RulingListQuery" => json!({ "project_id": 1 }),
        "DeferralRecordRequest" => json!({
            "mutation": mutation,
            "project_id": 1,
            "finding_id": "f-1",
            "reason": "later",
        }),
        "DeferralSupersedeRequest" => json!({
            "mutation": mutation,
            "project_id": 1,
            "deferral_id": 1,
            "reason": "still later",
        }),
        "DeferralListQuery" => json!({ "project_id": 1 }),
        "EvidenceAttachRequest" => json!({
            "mutation": mutation,
            "project_id": 1,
            "entity_kind": "ticket",
            "entity_id": "KAN-T1",
            "evidence_kind": "repository",
            "relative_path": "evidence/review.txt",
            "commit_identity": "c9eac24",
        }),
        "EvidenceListQuery" => json!({ "project_id": 1 }),
        "HerdrSettingsGetQuery" => json!({ "project_id": 1 }),
        "HerdrSettingsUpdateRequest" => json!({
            "mutation": mutation,
            "project_id": 1,
            "reconciliation_interval_secs": 300,
            "polling_fallback_enabled": false,
            "polling_fallback_interval_secs": 10,
            "stall_deadline_secs": 3600,
            "missing_result_deadline_secs": 7200,
        }),
        "HerdrDefaultsGetQuery" => json!({}),
        "HerdrDefaultsUpdateRequest" => json!({
            "mutation": mutation,
            "reconciliation_interval_secs": 300,
            "stall_deadline_secs": 3600,
            "missing_result_deadline_secs": 7200,
        }),
        "WorkspaceRegisterRequest" => json!({
            "mutation": mutation,
            "project_id": 1,
            "path": "/workspaces/kanban.seed",
        }),
        "WorkspaceObserveRequest" => json!({
            "mutation": mutation,
            "workspace_id": 1,
        }),
        "WorkspaceRetireRequest" => json!({
            "mutation": mutation,
            "workspace_id": 1,
        }),
        "WorkspaceListQuery" => json!({ "project_id": 1 }),
        other => panic!("no sample request fixture for {other}"),
    }
}

fn forward_catalogued(shell: &Arc<Shell>, operation: &OperationDescriptor) {
    let request = sample_request(operation.request_schema);
    match operation.kind {
        OperationKind::Query => {
            let _: Value = forward_query_value(shell, operation.name, operation.name, request)
                .expect("the query forwards");
        }
        OperationKind::Command => {
            let _: Value = forward_command_value(shell, operation.name, operation.name, request)
                .expect("the command forwards");
        }
    }
}

fn served(dir: &TempDir, recorder: Arc<Recorder>) -> (kanban_transport::ServerHandle, Arc<Shell>) {
    let server = SocketServer::bind(dir.path()).expect("the server binds");
    let broker = server.broker();
    let mut core = Core::new(
        exposed_operations(),
        Arc::new(MemoryIdempotencyStore::new()),
        broker,
    );
    for operation in exposed_operations() {
        match operation.kind {
            OperationKind::Query => core
                .register_query(
                    operation.name,
                    Arc::new(RecordingQuery {
                        recorder: recorder.clone(),
                        operation: operation.name,
                    }),
                )
                .expect("the query registers"),
            OperationKind::Command => core
                .register_command(
                    operation.name,
                    Arc::new(RecordingCommand {
                        recorder: recorder.clone(),
                        operation: operation.name,
                    }),
                )
                .expect("the command registers"),
        }
    }
    let handle = server.serve(Arc::new(core)).expect("the server serves");
    let shell = Arc::new(Shell::default());
    let link = kanban_desktop_lib::core_link::CoreLink::connect(handle.socket_path())
        .expect("the link connects");
    install_link(&shell, link);
    (handle, shell)
}

#[test]
fn commands_forward_catalogued_requests_unchanged() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let recorder = Arc::new(Recorder::default());
    let (handle, shell) = served(&dir, recorder.clone());

    for operation in exposed_operations() {
        forward_catalogued(&shell, operation);
        assert_eq!(
            recorder
                .last_for(operation.name)
                .expect("the core saw the request"),
            sample_request(operation.request_schema),
            "{} should forward its request unchanged",
            operation.name
        );
    }

    handle.shutdown();
}

fn assert_unknown_fields_refused(schema: &str, request: Value) {
    let envelope = json!({ "request": request });
    let refused = match schema {
        "HealthQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<HealthQuery>,
        >(envelope)
        .is_err(),
        "InitiativeCreateRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<InitiativeCreateRequest>,
        >(envelope)
        .is_err(),
        "InitiativeRenameRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<InitiativeRenameRequest>,
        >(envelope)
        .is_err(),
        "InitiativeArchiveRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<InitiativeArchiveRequest>,
        >(envelope)
        .is_err(),
        "InitiativeListQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<InitiativeListQuery>,
        >(envelope)
        .is_err(),
        "ProjectRegisterRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<ProjectRegisterRequest>,
        >(envelope)
        .is_err(),
        "ProjectArchiveRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<ProjectArchiveRequest>,
        >(envelope)
        .is_err(),
        "PlanCreateRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<PlanCreateRequest>,
        >(envelope)
        .is_err(),
        "PlanSpecAddRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<PlanSpecAddRequest>,
        >(envelope)
        .is_err(),
        "PlanSpecRemoveRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<PlanSpecRemoveRequest>,
        >(envelope)
        .is_err(),
        "PlanSpecMoveRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<PlanSpecMoveRequest>,
        >(envelope)
        .is_err(),
        "PlanEdgeAddRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<PlanEdgeAddRequest>,
        >(envelope)
        .is_err(),
        "PlanEdgeRemoveRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<PlanEdgeRemoveRequest>,
        >(envelope)
        .is_err(),
        "PlanActivateRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<PlanActivateRequest>,
        >(envelope)
        .is_err(),
        "PlanReplanRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<PlanReplanRequest>,
        >(envelope)
        .is_err(),
        "PlanCompleteRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<PlanCompleteRequest>,
        >(envelope)
        .is_err(),
        "PlanCancelRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<PlanCancelRequest>,
        >(envelope)
        .is_err(),
        "PlanArchiveRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<PlanArchiveRequest>,
        >(envelope)
        .is_err(),
        "PlanListQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<PlanListQuery>,
        >(envelope)
        .is_err(),
        "PlanGetQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<PlanGetQuery>,
        >(envelope)
        .is_err(),
        "ProjectListQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<ProjectListQuery>,
        >(envelope)
        .is_err(),
        "TimelineQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<TimelineQuery>,
        >(envelope)
        .is_err(),
        "CommentCreateRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<CommentCreateRequest>,
        >(envelope)
        .is_err(),
        "CommentEditRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<CommentEditRequest>,
        >(envelope)
        .is_err(),
        "CommentRevisionsQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<CommentRevisionsQuery>,
        >(envelope)
        .is_err(),
        "RulingRecordRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<RulingRecordRequest>,
        >(envelope)
        .is_err(),
        "RulingSupersedeRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<RulingSupersedeRequest>,
        >(envelope)
        .is_err(),
        "RulingListQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<RulingListQuery>,
        >(envelope)
        .is_err(),
        "DeferralRecordRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<DeferralRecordRequest>,
        >(envelope)
        .is_err(),
        "DeferralSupersedeRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<DeferralSupersedeRequest>,
        >(envelope)
        .is_err(),
        "DeferralListQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<DeferralListQuery>,
        >(envelope)
        .is_err(),
        "EvidenceAttachRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<EvidenceAttachRequest>,
        >(envelope)
        .is_err(),
        "EvidenceListQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<EvidenceListQuery>,
        >(envelope)
        .is_err(),
        "HerdrSettingsGetQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<HerdrSettingsGetQuery>,
        >(envelope)
        .is_err(),
        "HerdrSettingsUpdateRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<HerdrSettingsUpdateRequest>,
        >(envelope)
        .is_err(),
        "HerdrDefaultsGetQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<HerdrDefaultsGetQuery>,
        >(envelope)
        .is_err(),
        "HerdrDefaultsUpdateRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<HerdrDefaultsUpdateRequest>,
        >(envelope)
        .is_err(),
        "SpecCreateRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<SpecCreateRequest>,
        >(envelope)
        .is_err(),
        "SpecContentUpdateRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<SpecContentUpdateRequest>,
        >(envelope)
        .is_err(),
        "SpecVersionApproveRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<SpecVersionApproveRequest>,
        >(envelope)
        .is_err(),
        "SpecVersionSupersedeRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<SpecVersionSupersedeRequest>,
        >(envelope)
        .is_err(),
        "SpecPlanJoinRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<SpecPlanJoinRequest>,
        >(envelope)
        .is_err(),
        "SpecExecutionMoveRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<SpecExecutionMoveRequest>,
        >(envelope)
        .is_err(),
        "SpecListQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<SpecListQuery>,
        >(envelope)
        .is_err(),
        "SpecGetQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<SpecGetQuery>,
        >(envelope)
        .is_err(),
        "SpecVersionGetQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<SpecVersionGetQuery>,
        >(envelope)
        .is_err(),
        "SpecCoverageCheckQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<SpecCoverageCheckQuery>,
        >(envelope)
        .is_err(),
        "WorkspaceRegisterRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<WorkspaceRegisterRequest>,
        >(envelope)
        .is_err(),
        "WorkspaceObserveRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<WorkspaceObserveRequest>,
        >(envelope)
        .is_err(),
        "WorkspaceRetireRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<WorkspaceRetireRequest>,
        >(envelope)
        .is_err(),
        "WorkspaceListQuery" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<WorkspaceListQuery>,
        >(envelope)
        .is_err(),
        other => panic!("no unknown-field arm for {other}"),
    };
    assert!(refused, "{schema} should reject unknown request fields");
}

#[test]
fn commands_refuse_unknown_request_fields() {
    for operation in exposed_operations() {
        let mut request = sample_request(operation.request_schema);
        request
            .as_object_mut()
            .expect("sample requests are objects")
            .insert("definitely_unknown".to_owned(), json!(true));
        assert_unknown_fields_refused(operation.request_schema, request);
    }
}

#[test]
fn commands_timeline_query_fixture_matches_production_and_refuses_unknown_fields() {
    assert_eq!(
        sample_request("TimelineQuery"),
        production_timeline_query_fixture(),
        "the catalogue forwarding fixture must match the production TimelineQuery shape"
    );

    let mut request = production_timeline_query_fixture();
    request
        .as_object_mut()
        .expect("the production timeline query is an object")
        .insert("definitely_unknown".to_owned(), json!(true));
    assert_unknown_fields_refused("TimelineQuery", request);
}
