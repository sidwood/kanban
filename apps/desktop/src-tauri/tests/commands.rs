//! Shell command boundary: unknown fields are refused and catalogued
//! requests reach the core with their shape unchanged.

use std::sync::{Arc, Mutex};

use kanban_app::{
    CommandHandler, Core, EventSink, MemoryIdempotencyStore, OperationDescriptor, OperationKind,
    ParsedCommand, QueryHandler, exposed_operations,
};
use kanban_desktop_lib::Shell;
use kanban_desktop_lib::commands::{forward_command_value, forward_query_value, install_link};
use kanban_dto::{
    CommentCreateRequest, CommentEditRequest, CommentRevisionsQuery, DeferralListQuery,
    DeferralRecordRequest, DeferralSupersedeRequest, EvidenceAttachRequest, EvidenceListRequest,
    HealthQuery, HerdrDefaultsGetQuery, HerdrDefaultsUpdateRequest, HerdrSettingsGetQuery,
    HerdrSettingsUpdateRequest, InitiativeArchiveRequest, InitiativeCreateRequest,
    InitiativeListQuery, InitiativeRenameRequest, MutationContext, ProjectArchiveRequest,
    ProjectListQuery, ProjectRegisterRequest, RulingListQuery, RulingRecordRequest,
    RulingSupersedeRequest, TimelineEntityKind, TimelineEntityRef, TimelineQuery, TimelineScope,
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
        _events: &dyn EventSink,
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
        scope: TimelineScope::Project("kan".to_owned()),
        entity: None,
        kinds: None,
        since: None,
        until: None,
    })
    .expect("the production timeline query encodes")
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
            "herdr_session": "kanban-main",
            "initiative_id": null,
        }),
        "ProjectArchiveRequest" => json!({ "mutation": mutation, "project_id": 1 }),
        "TimelineQuery" => production_timeline_query_fixture(),
        "CommentCreateRequest" => json!({
            "mutation": mutation,
            "project_id": "kan",
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
            "project_id": "kan",
            "summary": "ship it",
        }),
        "RulingSupersedeRequest" => json!({
            "mutation": mutation,
            "project_id": "kan",
            "ruling_id": 1,
            "summary": "revise it",
        }),
        "RulingListQuery" => json!({ "project_id": "kan" }),
        "DeferralRecordRequest" => json!({
            "mutation": mutation,
            "project_id": "kan",
            "finding_id": "f-1",
            "reason": "later",
        }),
        "DeferralSupersedeRequest" => json!({
            "mutation": mutation,
            "project_id": "kan",
            "deferral_id": 1,
            "reason": "still later",
        }),
        "DeferralListQuery" => json!({ "project_id": "kan" }),
        "EvidenceAttachRequest" => json!({
            "mutation": mutation,
            "project_id": "kan",
            "entity_kind": "ticket",
            "entity_id": "KAN-T1",
            "evidence_kind": "repository",
            "relative_path": "evidence/review.txt",
            "commit_identity": "c9eac24",
        }),
        "EvidenceListRequest" => json!({ "mutation": mutation, "project_id": "kan" }),
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
        "EvidenceListRequest" => serde_json::from_value::<
            kanban_desktop_lib::commands::ShellInvokeArgs<EvidenceListRequest>,
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
