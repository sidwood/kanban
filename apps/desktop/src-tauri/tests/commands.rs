//! Shell command boundary: unknown fields are refused and catalogued
//! requests reach the core with their shape unchanged.

use std::sync::{Arc, Mutex};

use kanban_app::{
    CommandEffects, CommandHandler, Core, MemoryIdempotencyStore, OperationDescriptor,
    OperationKind, ParsedCommand, QueryHandler, exposed_operations,
};
use kanban_desktop_lib::Shell;
use kanban_desktop_lib::commands::{
    decode_invoke_args, forward_command_value, forward_query_value, install_link,
};
use kanban_dto::{
    CommentCreateRequest, CommentEditRequest, CommentRevisionsQuery, DeferralListQuery,
    DeferralRecordRequest,
    DeferralSupersedeRequest,
    DiagnosticsExportQuery,
    EvidenceAttachRequest,
    EvidenceListQuery,
    HealthQuery,
    HerdrDefaultsGetQuery,
    HerdrDefaultsUpdateRequest,
    HerdrSettingsGetQuery,
    HerdrSettingsUpdateRequest,
    InitiativeArchiveRequest,
    InitiativeCreateRequest,
    InitiativeListQuery,
    InitiativeRenameRequest,
    LaneCreateRequest,
    LaneListQuery,
    LaneTicketAssignRequest,
    LaneTicketReleaseRequest,
    LaneWorkspaceAssignRequest,
    LaneWorkspaceReleaseRequest,
    MutationContext,
    PlanActivateRequest,
    PlanArchiveRequest,
    PlanCancelRequest,
    PlanCompleteRequest,
    PlanCreateRequest,
    PlanDiagnosticsQuery,
    PlanEdgeAddRequest,
    PlanEdgeRemoveRequest,
    PlanGetQuery,
    PlanListQuery,
    PlanReplanRequest,
    PlanSpecAddRequest,
    PlanSpecMoveRequest,
    PlanSpecRemoveRequest,
    ProjectArchiveRequest,
    ProjectListQuery,
    ProjectRegisterRequest,
    RulingListQuery,
    RulingRecordRequest,
    RulingSupersedeRequest,
    SpecContent,
    SpecContentUpdateRequest,
    SpecCoverageCheckQuery,
    SpecCreateRequest,
    SpecExecutionMoveRequest,
    SpecGetQuery,
    SpecListQuery,
    SpecPlanJoinRequest,
    SpecVersionApproveRequest,
    SpecVersionGetQuery,
    SpecVersionSupersedeRequest,
    TicketBlockerAddRequest,
    TicketBlockerRemoveRequest,
    TicketBugFactsRequest,
    TicketBugQualifyRequest,
    TicketCreateRequest,
    TicketDependenciesQuery,
    TicketDependencyAddRequest,
    TicketDependencyRemoveRequest,
    TicketGetQuery,
    TicketListQuery,
    TicketReadinessQuery,
    TimelineEntityKind,
    TimelineEntityRef,
    TimelineQuery,
    TimelineScope,
    WorkspaceListQuery,
    WorkspaceObserveRequest,
    WorkspaceRegisterRequest,
    WorkspaceRetireRequest,
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
        "HealthQuery" | "DiagnosticsExportQuery" | "InitiativeListQuery" | "ProjectListQuery" => {
            json!({})
        }
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
        "PlanDiagnosticsQuery" => json!({ "plan_id": 1, "version": null }),
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
        "TicketCreateRequest" => json!({
            "mutation": mutation,
            "project_id": 1,
            "kind": "implementation",
            "priority": "high",
            "spec_id": 1,
            "slice": "Registration creates Projects end to end",
            "criteria": [
                {
                    "outcome": "Projects register with unique codes.",
                    "stories": ["CORE-S1-US1"],
                }
            ],
        }),
        "TicketListQuery" => json!({ "project_id": 1 }),
        "TicketBugQualifyRequest" => json!({
            "mutation": mutation,
            "ticket_id": 1,
            "qualification": {
                "expected_behaviour": "The integration branch survives every landing.",
                "reproduction": "Re land a reviewed change.",
                "environment": "macOS 26.",
                "severity": "critical",
                "frequency": "Every landing.",
                "affected_scope": "Landings.",
                "risk": "Lost review state.",
                "criteria": [
                    {
                        "outcome": "The integration branch survives a landing.",
                        "stories": ["CORE-S1-US1"],
                    }
                ],
                "verification_steps": [
                    { "command": "cargo test -p kanban-storage tickets" }
                ],
            },
        }),
        "TicketBugFactsRequest" => json!({
            "mutation": mutation,
            "ticket_id": 1,
            "external_references": [
                { "uri": "https://example.invalid/issues/12", "label": "The report" }
            ],
            "occurrence_snapshots": [
                {
                    "observed_at": "2026-09-05T07:41:00Z",
                    "observation": "The log shows the drop.",
                }
            ],
            "evidence_ids": [2],
        }),
        "TicketGetQuery" => json!({ "ticket_id": 1 }),
        "TicketDependencyAddRequest" => {
            json!({ "mutation": mutation, "from_ticket": 1, "to_ticket": 2 })
        }
        "TicketDependencyRemoveRequest" => {
            json!({ "mutation": mutation, "from_ticket": 1, "to_ticket": 2 })
        }
        "TicketBlockerAddRequest" => {
            json!({ "mutation": mutation, "ticket_id": 2, "description": "Design sign-off" })
        }
        "TicketBlockerRemoveRequest" => {
            json!({ "mutation": mutation, "ticket_id": 2, "blocker_id": 1 })
        }
        "TicketDependenciesQuery" => json!({ "ticket_id": 2 }),
        "TicketReadinessQuery" => json!({ "ticket_id": 2 }),
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
        "LaneCreateRequest" => json!({ "mutation": mutation, "project_id": 1 }),
        "LaneWorkspaceAssignRequest" => {
            json!({ "mutation": mutation, "lane_id": 1, "workspace_id": 2 })
        }
        "LaneWorkspaceReleaseRequest" => json!({ "mutation": mutation, "lane_id": 1 }),
        "LaneTicketAssignRequest" => {
            json!({ "mutation": mutation, "lane_id": 1, "ticket_id": 3 })
        }
        "LaneTicketReleaseRequest" => json!({ "mutation": mutation, "lane_id": 1 }),
        "LaneListQuery" => json!({ "project_id": 1 }),
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
    let refused = match schema {
        "HealthQuery" => decode_invoke_args::<HealthQuery>(request).is_err(),
        "DiagnosticsExportQuery" => decode_invoke_args::<DiagnosticsExportQuery>(request).is_err(),
        "InitiativeCreateRequest" => {
            decode_invoke_args::<InitiativeCreateRequest>(request).is_err()
        }
        "InitiativeRenameRequest" => {
            decode_invoke_args::<InitiativeRenameRequest>(request).is_err()
        }
        "InitiativeArchiveRequest" => {
            decode_invoke_args::<InitiativeArchiveRequest>(request).is_err()
        }
        "InitiativeListQuery" => decode_invoke_args::<InitiativeListQuery>(request).is_err(),
        "ProjectRegisterRequest" => decode_invoke_args::<ProjectRegisterRequest>(request).is_err(),
        "ProjectArchiveRequest" => decode_invoke_args::<ProjectArchiveRequest>(request).is_err(),
        "PlanCreateRequest" => decode_invoke_args::<PlanCreateRequest>(request).is_err(),
        "PlanSpecAddRequest" => decode_invoke_args::<PlanSpecAddRequest>(request).is_err(),
        "PlanSpecRemoveRequest" => decode_invoke_args::<PlanSpecRemoveRequest>(request).is_err(),
        "PlanSpecMoveRequest" => decode_invoke_args::<PlanSpecMoveRequest>(request).is_err(),
        "PlanEdgeAddRequest" => decode_invoke_args::<PlanEdgeAddRequest>(request).is_err(),
        "PlanEdgeRemoveRequest" => decode_invoke_args::<PlanEdgeRemoveRequest>(request).is_err(),
        "PlanActivateRequest" => decode_invoke_args::<PlanActivateRequest>(request).is_err(),
        "PlanReplanRequest" => decode_invoke_args::<PlanReplanRequest>(request).is_err(),
        "PlanCompleteRequest" => decode_invoke_args::<PlanCompleteRequest>(request).is_err(),
        "PlanCancelRequest" => decode_invoke_args::<PlanCancelRequest>(request).is_err(),
        "PlanArchiveRequest" => decode_invoke_args::<PlanArchiveRequest>(request).is_err(),
        "PlanListQuery" => decode_invoke_args::<PlanListQuery>(request).is_err(),
        "PlanGetQuery" => decode_invoke_args::<PlanGetQuery>(request).is_err(),
        "PlanDiagnosticsQuery" => decode_invoke_args::<PlanDiagnosticsQuery>(request).is_err(),
        "ProjectListQuery" => decode_invoke_args::<ProjectListQuery>(request).is_err(),
        "TimelineQuery" => decode_invoke_args::<TimelineQuery>(request).is_err(),
        "CommentCreateRequest" => decode_invoke_args::<CommentCreateRequest>(request).is_err(),
        "CommentEditRequest" => decode_invoke_args::<CommentEditRequest>(request).is_err(),
        "CommentRevisionsQuery" => decode_invoke_args::<CommentRevisionsQuery>(request).is_err(),
        "RulingRecordRequest" => decode_invoke_args::<RulingRecordRequest>(request).is_err(),
        "RulingSupersedeRequest" => decode_invoke_args::<RulingSupersedeRequest>(request).is_err(),
        "RulingListQuery" => decode_invoke_args::<RulingListQuery>(request).is_err(),
        "DeferralRecordRequest" => decode_invoke_args::<DeferralRecordRequest>(request).is_err(),
        "DeferralSupersedeRequest" => {
            decode_invoke_args::<DeferralSupersedeRequest>(request).is_err()
        }
        "DeferralListQuery" => decode_invoke_args::<DeferralListQuery>(request).is_err(),
        "EvidenceAttachRequest" => decode_invoke_args::<EvidenceAttachRequest>(request).is_err(),
        "EvidenceListQuery" => decode_invoke_args::<EvidenceListQuery>(request).is_err(),
        "HerdrSettingsGetQuery" => decode_invoke_args::<HerdrSettingsGetQuery>(request).is_err(),
        "HerdrSettingsUpdateRequest" => {
            decode_invoke_args::<HerdrSettingsUpdateRequest>(request).is_err()
        }
        "HerdrDefaultsGetQuery" => decode_invoke_args::<HerdrDefaultsGetQuery>(request).is_err(),
        "HerdrDefaultsUpdateRequest" => {
            decode_invoke_args::<HerdrDefaultsUpdateRequest>(request).is_err()
        }
        "SpecCreateRequest" => decode_invoke_args::<SpecCreateRequest>(request).is_err(),
        "SpecContentUpdateRequest" => {
            decode_invoke_args::<SpecContentUpdateRequest>(request).is_err()
        }
        "SpecVersionApproveRequest" => {
            decode_invoke_args::<SpecVersionApproveRequest>(request).is_err()
        }
        "SpecVersionSupersedeRequest" => {
            decode_invoke_args::<SpecVersionSupersedeRequest>(request).is_err()
        }
        "SpecPlanJoinRequest" => decode_invoke_args::<SpecPlanJoinRequest>(request).is_err(),
        "SpecExecutionMoveRequest" => {
            decode_invoke_args::<SpecExecutionMoveRequest>(request).is_err()
        }
        "SpecListQuery" => decode_invoke_args::<SpecListQuery>(request).is_err(),
        "SpecGetQuery" => decode_invoke_args::<SpecGetQuery>(request).is_err(),
        "SpecVersionGetQuery" => decode_invoke_args::<SpecVersionGetQuery>(request).is_err(),
        "SpecCoverageCheckQuery" => decode_invoke_args::<SpecCoverageCheckQuery>(request).is_err(),
        "TicketCreateRequest" => decode_invoke_args::<TicketCreateRequest>(request).is_err(),
        "TicketBugQualifyRequest" => {
            decode_invoke_args::<TicketBugQualifyRequest>(request).is_err()
        }
        "TicketBugFactsRequest" => decode_invoke_args::<TicketBugFactsRequest>(request).is_err(),
        "TicketListQuery" => decode_invoke_args::<TicketListQuery>(request).is_err(),
        "TicketGetQuery" => decode_invoke_args::<TicketGetQuery>(request).is_err(),
        "TicketDependencyAddRequest" => {
            decode_invoke_args::<TicketDependencyAddRequest>(request).is_err()
        }
        "TicketDependencyRemoveRequest" => {
            decode_invoke_args::<TicketDependencyRemoveRequest>(request).is_err()
        }
        "TicketBlockerAddRequest" => {
            decode_invoke_args::<TicketBlockerAddRequest>(request).is_err()
        }
        "TicketBlockerRemoveRequest" => {
            decode_invoke_args::<TicketBlockerRemoveRequest>(request).is_err()
        }
        "TicketDependenciesQuery" => {
            decode_invoke_args::<TicketDependenciesQuery>(request).is_err()
        }
        "TicketReadinessQuery" => decode_invoke_args::<TicketReadinessQuery>(request).is_err(),
        "WorkspaceRegisterRequest" => {
            decode_invoke_args::<WorkspaceRegisterRequest>(request).is_err()
        }
        "WorkspaceObserveRequest" => {
            decode_invoke_args::<WorkspaceObserveRequest>(request).is_err()
        }
        "WorkspaceRetireRequest" => decode_invoke_args::<WorkspaceRetireRequest>(request).is_err(),
        "LaneCreateRequest" => decode_invoke_args::<LaneCreateRequest>(request).is_err(),
        "LaneWorkspaceAssignRequest" => {
            decode_invoke_args::<LaneWorkspaceAssignRequest>(request).is_err()
        }
        "LaneWorkspaceReleaseRequest" => {
            decode_invoke_args::<LaneWorkspaceReleaseRequest>(request).is_err()
        }
        "LaneTicketAssignRequest" => {
            decode_invoke_args::<LaneTicketAssignRequest>(request).is_err()
        }
        "LaneTicketReleaseRequest" => {
            decode_invoke_args::<LaneTicketReleaseRequest>(request).is_err()
        }
        "LaneListQuery" => decode_invoke_args::<LaneListQuery>(request).is_err(),
        "WorkspaceListQuery" => decode_invoke_args::<WorkspaceListQuery>(request).is_err(),
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
