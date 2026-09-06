//! Thin Tauri shell: window, lifecycle, and the core socket client
//! (ADR-0001). It starts the core on demand, exposes only typed
//! commands, and forwards the core's ordered events; quitting the
//! window never takes the core down with it. Domain rules never live
//! here.

use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kanban_dto::{
    ApiError, CapacityDefaultsGetQuery, CapacityDefaultsGetResponse, CapacityDefaultsUpdateRequest,
    CapacityGlobalDefaults, CapacityProjectCaps, CapacitySettingsGetQuery,
    CapacitySettingsGetResponse, CapacitySettingsUpdateRequest, CloneCreateRequest,
    CloneCreatedRecord, CloneRemoveRequest, CloneRemovedRecord, CommentCreateRequest,
    CommentEditRequest, CommentRecord, CommentRevisionsQuery, CommentRevisionsResponse,
    DeferralListQuery, DeferralListResponse, DeferralRecord, DeferralRecordRequest,
    DeferralSupersedeRequest, DiagnosticsExportQuery, DiagnosticsExportResponse,
    DispatchClaimRequest, DispatchClaimResponse, DispatchQueueQuery, DispatchQueueResponse,
    DispatchRequestCreateRequest, DispatchRequestRecord, EvidenceAttachRequest, EvidenceListQuery,
    EvidenceListResponse, EvidenceRecord, ExportDriftQuery, ExportDriftResponse,
    ExportRenderRequest, ExportRenderResponse, HealthQuery, HealthResponse, HerdrDefaultsGetQuery,
    HerdrDefaultsGetResponse, HerdrDefaultsUpdateRequest, HerdrGlobalDefaults,
    HerdrProjectSettings, HerdrSettingsGetQuery, HerdrSettingsGetResponse,
    HerdrSettingsUpdateRequest, InitiativeArchiveRequest, InitiativeCreateRequest,
    InitiativeListQuery, InitiativeListResponse, InitiativeRecord, InitiativeRenameRequest,
    LaneCreateRequest, LaneListQuery, LaneListResponse, LaneRecord, LaneTicketAssignRequest,
    LaneTicketReleaseRequest, LaneWorkspaceAssignRequest, LaneWorkspaceReleaseRequest,
    PlanActivateRequest, PlanArchiveRequest, PlanCancelRequest, PlanCompleteRequest,
    PlanCreateRequest, PlanDiagnosticsQuery, PlanDiagnosticsResponse, PlanEdgeAddRequest,
    PlanEdgeRemoveRequest, PlanGetQuery, PlanGetResponse, PlanListQuery, PlanListResponse,
    PlanRecord, PlanReplanRequest, PlanSpecAddRequest, PlanSpecMoveRequest, PlanSpecRemoveRequest,
    ProfileDefineRequest, ProfileGetQuery, ProfileListQuery, ProfileListResponse, ProfileRecord,
    ProfileRetireRequest, ProfileUpdateRequest, ProjectArchiveRequest, ProjectListQuery,
    ProjectListResponse, ProjectRecord, ProjectRegisterRequest, RulingListQuery,
    RulingListResponse, RulingRecord, RulingRecordRequest, RulingSupersedeRequest,
    SpecContentUpdateRequest, SpecCoverageCheckQuery, SpecCoverageCheckResponse,
    SpecCoverageMatrixQuery, SpecCoverageMatrixResponse, SpecCreateRequest,
    SpecExecutionMoveRequest, SpecGetQuery, SpecGetResponse, SpecListQuery, SpecListResponse,
    SpecPlanJoinRequest, SpecRecord, SpecVersionApproveRequest, SpecVersionGetQuery,
    SpecVersionRecord, SpecVersionSupersedeRequest, TicketAssignRequest, TicketBlockerAddRequest,
    TicketBlockerRemoveRequest, TicketBugFactsRequest, TicketBugQualifyRequest,
    TicketCancelRequest, TicketCreateRequest, TicketDependenciesQuery, TicketDependenciesResponse,
    TicketDependencyAddRequest, TicketDependencyRemoveRequest, TicketEditRequest,
    TicketEmergencyOverrideRequest, TicketGetQuery, TicketGraphApproveRequest,
    TicketGraphListQuery, TicketGraphListResponse, TicketGraphProposeRequest, TicketGraphRecord,
    TicketListQuery, TicketListResponse, TicketParkRequest, TicketPrioritiseRequest,
    TicketReadinessQuery, TicketReadinessResponse, TicketReassignRequest, TicketRecord,
    TicketReviewRequest, TicketScheduleRequest, TicketSpecMoveRequest, TicketTransitionRequest,
    TicketUnparkRequest, TimelineQuery, TimelineQueryResponse, WorkspaceListQuery,
    WorkspaceListResponse, WorkspaceObserveRequest, WorkspaceRecord, WorkspaceRegisterRequest,
    WorkspaceRetireRequest,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

pub mod commands;
pub mod core_link;
mod shell_handlers;

use commands::{decode_invoke_args, forward_command, forward_query};

/// The shell emits the core's ordered events under this name.
pub const CORE_EVENT: &str = "core://event";

/// The shell announces its connection to the core under this name.
/// This is shell-level plumbing, not a domain contract: the WebView
/// confirms real state through the generated client's health query.
pub const CONNECTION_EVENT: &str = "core://connection";

/// How long the on-demand start waits for the spawned core to serve.
const CORE_START_TIMEOUT: Duration = Duration::from_secs(15);

/// How often the on-demand start polls for the socket.
const CORE_START_POLL: Duration = Duration::from_millis(100);

/// The shell's connection to the core, when it has one.
#[derive(Default)]
pub struct Shell {
    pub(crate) link: Mutex<Option<core_link::CoreLink>>,
}

/// The connection the shell announces to the WebView.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectionState {
    /// The socket is serving and the event stream is attached.
    Connected,
    /// No core is reachable.
    Disconnected,
}

/// Run one operation on the shell's link, from a blocking task so
/// the link's mutex never blocks the async runtime, and decode the
/// answer into its contract type. The typed commands below are the
/// only wrappers the WebView may call.
async fn run_blocking<T, F>(shell: Arc<Shell>, subject: &str, run: F) -> Result<T, ApiError>
where
    T: serde::de::DeserializeOwned + Send + 'static,
    F: FnOnce(&Arc<Shell>) -> Result<T, ApiError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || run(&shell))
        .await
        .map_err(|_| ApiError::internal(&format!("the {subject} task did not finish")))?
}

#[tauri::command]
async fn health_get(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<HealthResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<HealthQuery>(request)?;
    run_blocking(shell, "health", |shell| {
        forward_query(shell, "health.get", "health", request)
    })
    .await
}

#[tauri::command]
async fn diagnostics_export(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<DiagnosticsExportResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<DiagnosticsExportQuery>(request)?;
    run_blocking(shell, "diagnostics export", |shell| {
        forward_query(shell, "diagnostics.export", "diagnostics export", request)
    })
    .await
}

#[tauri::command]
async fn initiative_create(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<InitiativeRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<InitiativeCreateRequest>(request)?;
    run_blocking(shell, "create", |shell| {
        forward_command(shell, "initiative.create", "created Initiative", request)
    })
    .await
}

#[tauri::command]
async fn initiative_rename(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<InitiativeRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<InitiativeRenameRequest>(request)?;
    run_blocking(shell, "rename", |shell| {
        forward_command(shell, "initiative.rename", "renamed Initiative", request)
    })
    .await
}

#[tauri::command]
async fn initiative_archive(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<InitiativeRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<InitiativeArchiveRequest>(request)?;
    run_blocking(shell, "archive", |shell| {
        forward_command(shell, "initiative.archive", "archived Initiative", request)
    })
    .await
}

#[tauri::command]
async fn initiative_list(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<InitiativeListResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<InitiativeListQuery>(request)?;
    run_blocking(shell, "initiative list", |shell| {
        forward_query(shell, "initiative.list", "initiative list", request)
    })
    .await
}

#[tauri::command]
async fn project_register(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<ProjectRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<ProjectRegisterRequest>(request)?;
    run_blocking(shell, "project register", |shell| {
        forward_command(shell, "project.register", "registered Project", request)
    })
    .await
}

#[tauri::command]
async fn project_archive(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<ProjectRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<ProjectArchiveRequest>(request)?;
    run_blocking(shell, "project archive", |shell| {
        forward_command(shell, "project.archive", "archived Project", request)
    })
    .await
}

#[tauri::command]
async fn project_list(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<ProjectListResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<ProjectListQuery>(request)?;
    run_blocking(shell, "project list", |shell| {
        forward_query(shell, "project.list", "project list", request)
    })
    .await
}

#[tauri::command]
async fn plan_create(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanCreateRequest>(request)?;
    run_blocking(shell, "plan create", |shell| {
        forward_command(shell, "plan.create", "created Plan", request)
    })
    .await
}

#[tauri::command]
async fn plan_spec_add(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanSpecAddRequest>(request)?;
    run_blocking(shell, "plan spec add", |shell| {
        forward_command(shell, "plan.spec.add", "added Spec to Plan", request)
    })
    .await
}

#[tauri::command]
async fn plan_spec_remove(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanSpecRemoveRequest>(request)?;
    run_blocking(shell, "plan spec remove", |shell| {
        forward_command(shell, "plan.spec.remove", "removed Spec from Plan", request)
    })
    .await
}

#[tauri::command]
async fn plan_spec_move(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanSpecMoveRequest>(request)?;
    run_blocking(shell, "plan spec move", |shell| {
        forward_command(shell, "plan.spec.move", "moved Spec within Plan", request)
    })
    .await
}

#[tauri::command]
async fn plan_edge_add(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanEdgeAddRequest>(request)?;
    run_blocking(shell, "plan edge add", |shell| {
        forward_command(shell, "plan.edge.add", "added Plan edge", request)
    })
    .await
}

#[tauri::command]
async fn plan_edge_remove(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanEdgeRemoveRequest>(request)?;
    run_blocking(shell, "plan edge remove", |shell| {
        forward_command(shell, "plan.edge.remove", "removed Plan edge", request)
    })
    .await
}

#[tauri::command]
async fn plan_activate(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanActivateRequest>(request)?;
    run_blocking(shell, "plan activate", |shell| {
        forward_command(shell, "plan.activate", "activated Plan", request)
    })
    .await
}

#[tauri::command]
async fn plan_replan(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanReplanRequest>(request)?;
    run_blocking(shell, "plan replan", |shell| {
        forward_command(shell, "plan.replan", "replanned Plan", request)
    })
    .await
}

#[tauri::command]
async fn plan_complete(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanCompleteRequest>(request)?;
    run_blocking(shell, "plan complete", |shell| {
        forward_command(shell, "plan.complete", "completed Plan", request)
    })
    .await
}

#[tauri::command]
async fn plan_cancel(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanCancelRequest>(request)?;
    run_blocking(shell, "plan cancel", |shell| {
        forward_command(shell, "plan.cancel", "cancelled Plan", request)
    })
    .await
}

#[tauri::command]
async fn plan_archive(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanArchiveRequest>(request)?;
    run_blocking(shell, "plan archive", |shell| {
        forward_command(shell, "plan.archive", "archived Plan", request)
    })
    .await
}

#[tauri::command]
async fn plan_list(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanListResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanListQuery>(request)?;
    run_blocking(shell, "plan list", |shell| {
        forward_query(shell, "plan.list", "plan list", request)
    })
    .await
}

#[tauri::command]
async fn plan_get(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanGetResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanGetQuery>(request)?;
    run_blocking(shell, "plan get", |shell| {
        forward_query(shell, "plan.get", "plan get", request)
    })
    .await
}

#[tauri::command]
async fn plan_diagnostics(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<PlanDiagnosticsResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<PlanDiagnosticsQuery>(request)?;
    run_blocking(shell, "plan diagnostics", |shell| {
        forward_query(shell, "plan.diagnostics", "plan diagnostics", request)
    })
    .await
}

#[tauri::command]
async fn spec_create(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<SpecRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<SpecCreateRequest>(request)?;
    run_blocking(shell, "spec create", |shell| {
        forward_command(shell, "spec.create", "authored Spec", request)
    })
    .await
}

#[tauri::command]
async fn spec_content_update(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<SpecRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<SpecContentUpdateRequest>(request)?;
    run_blocking(shell, "spec content update", |shell| {
        forward_command(
            shell,
            "spec.content.update",
            "updated Spec content",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn spec_version_approve(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<SpecRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<SpecVersionApproveRequest>(request)?;
    run_blocking(shell, "spec version approve", |shell| {
        forward_command(
            shell,
            "spec.version.approve",
            "approved Spec version",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn spec_version_supersede(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<SpecRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<SpecVersionSupersedeRequest>(request)?;
    run_blocking(shell, "spec version supersede", |shell| {
        forward_command(
            shell,
            "spec.version.supersede",
            "superseded Spec version",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn spec_plan_join(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<SpecRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<SpecPlanJoinRequest>(request)?;
    run_blocking(shell, "spec plan join", |shell| {
        forward_command(shell, "spec.plan.join", "planned Spec", request)
    })
    .await
}

#[tauri::command]
async fn spec_execution_move(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<SpecRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<SpecExecutionMoveRequest>(request)?;
    run_blocking(shell, "spec execution move", |shell| {
        forward_command(
            shell,
            "spec.execution.move",
            "moved Spec execution",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn spec_list(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<SpecListResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<SpecListQuery>(request)?;
    run_blocking(shell, "spec list", |shell| {
        forward_query(shell, "spec.list", "spec list", request)
    })
    .await
}

#[tauri::command]
async fn spec_get(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<SpecGetResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<SpecGetQuery>(request)?;
    run_blocking(shell, "spec get", |shell| {
        forward_query(shell, "spec.get", "spec get", request)
    })
    .await
}

#[tauri::command]
async fn spec_version_get(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<SpecVersionRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<SpecVersionGetQuery>(request)?;
    run_blocking(shell, "spec version get", |shell| {
        forward_query(shell, "spec.version.get", "spec version get", request)
    })
    .await
}

#[tauri::command]
async fn spec_coverage_check(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<SpecCoverageCheckResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<SpecCoverageCheckQuery>(request)?;
    run_blocking(shell, "spec coverage check", |shell| {
        forward_query(shell, "spec.coverage.check", "spec coverage check", request)
    })
    .await
}

#[tauri::command]
async fn spec_coverage_matrix(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<SpecCoverageMatrixResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<SpecCoverageMatrixQuery>(request)?;
    run_blocking(shell, "spec coverage matrix", |shell| {
        forward_query(
            shell,
            "spec.coverage.matrix",
            "spec coverage matrix",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn ticket_create(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketCreateRequest>(request)?;
    run_blocking(shell, "ticket create", |shell| {
        forward_command(shell, "ticket.create", "created Ticket", request)
    })
    .await
}

#[tauri::command]
async fn ticket_bug_qualify(
    shell: State<'_, Arc<Shell>>,
    request: TicketBugQualifyRequest,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "ticket bug qualify", |shell| {
        forward_command(shell, "ticket.bug.qualify", "qualified Bug", request)
    })
    .await
}

#[tauri::command]
async fn ticket_bug_facts(
    shell: State<'_, Arc<Shell>>,
    request: TicketBugFactsRequest,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "ticket bug facts", |shell| {
        forward_command(shell, "ticket.bug.facts", "recorded Bug facts", request)
    })
    .await
}

#[tauri::command]
async fn ticket_list(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketListResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketListQuery>(request)?;
    run_blocking(shell, "ticket list", |shell| {
        forward_query(shell, "ticket.list", "ticket list", request)
    })
    .await
}

#[tauri::command]
async fn ticket_get(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketGetQuery>(request)?;
    run_blocking(shell, "ticket get", |shell| {
        forward_query(shell, "ticket.get", "ticket get", request)
    })
    .await
}

#[tauri::command]
async fn ticket_dependency_add(
    shell: State<'_, Arc<Shell>>,
    request: TicketDependencyAddRequest,
) -> Result<TicketDependenciesResponse, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "ticket dependency add", |shell| {
        forward_command(
            shell,
            "ticket.dependency.add",
            "added Ticket dependency",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn ticket_dependency_remove(
    shell: State<'_, Arc<Shell>>,
    request: TicketDependencyRemoveRequest,
) -> Result<TicketDependenciesResponse, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "ticket dependency remove", |shell| {
        forward_command(
            shell,
            "ticket.dependency.remove",
            "removed Ticket dependency",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn ticket_blocker_add(
    shell: State<'_, Arc<Shell>>,
    request: TicketBlockerAddRequest,
) -> Result<TicketDependenciesResponse, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "ticket blocker add", |shell| {
        forward_command(
            shell,
            "ticket.blocker.add",
            "added external blocker",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn ticket_blocker_remove(
    shell: State<'_, Arc<Shell>>,
    request: TicketBlockerRemoveRequest,
) -> Result<TicketDependenciesResponse, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "ticket blocker remove", |shell| {
        forward_command(
            shell,
            "ticket.blocker.remove",
            "removed external blocker",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn ticket_dependencies(
    shell: State<'_, Arc<Shell>>,
    request: TicketDependenciesQuery,
) -> Result<TicketDependenciesResponse, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "ticket dependencies", |shell| {
        forward_query(shell, "ticket.dependencies", "ticket dependencies", request)
    })
    .await
}

#[tauri::command]
async fn ticket_readiness(
    shell: State<'_, Arc<Shell>>,
    request: TicketReadinessQuery,
) -> Result<TicketReadinessResponse, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "ticket readiness", |shell| {
        forward_query(shell, "ticket.readiness", "ticket readiness", request)
    })
    .await
}

#[tauri::command]
async fn ticket_assign(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketAssignRequest>(request)?;
    run_blocking(shell, "ticket assign", |shell| {
        forward_command(shell, "ticket.assign", "assigned Ticket", request)
    })
    .await
}

#[tauri::command]
async fn ticket_transition(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketTransitionRequest>(request)?;
    run_blocking(shell, "ticket transition", |shell| {
        forward_command(shell, "ticket.transition", "moved Ticket", request)
    })
    .await
}

#[tauri::command]
async fn ticket_park(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketParkRequest>(request)?;
    run_blocking(shell, "ticket park", |shell| {
        forward_command(shell, "ticket.park", "parked Ticket", request)
    })
    .await
}

#[tauri::command]
async fn ticket_unpark(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketUnparkRequest>(request)?;
    run_blocking(shell, "ticket unpark", |shell| {
        forward_command(shell, "ticket.unpark", "unparked Ticket", request)
    })
    .await
}

#[tauri::command]
async fn ticket_schedule(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketScheduleRequest>(request)?;
    run_blocking(shell, "ticket schedule", |shell| {
        forward_command(shell, "ticket.schedule", "scheduled Ticket", request)
    })
    .await
}

#[tauri::command]
async fn ticket_cancel(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketCancelRequest>(request)?;
    run_blocking(shell, "ticket cancel", |shell| {
        forward_command(shell, "ticket.cancel", "cancelled Ticket", request)
    })
    .await
}

#[tauri::command]
async fn ticket_review(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketReviewRequest>(request)?;
    run_blocking(shell, "ticket review", |shell| {
        forward_command(shell, "ticket.review", "reviewed Ticket", request)
    })
    .await
}

#[tauri::command]
async fn ticket_spec_move(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketSpecMoveRequest>(request)?;
    run_blocking(shell, "ticket spec move", |shell| {
        forward_command(
            shell,
            "ticket.spec.move",
            "moved Ticket between Specs",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn ticket_prioritise(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketPrioritiseRequest>(request)?;
    run_blocking(shell, "ticket prioritise", |shell| {
        forward_command(shell, "ticket.prioritise", "prioritised Ticket", request)
    })
    .await
}

#[tauri::command]
async fn ticket_graph_propose(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketGraphRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketGraphProposeRequest>(request)?;
    run_blocking(shell, "ticket graph propose", |shell| {
        forward_command(
            shell,
            "ticket.graph.propose",
            "recorded Ticket graph",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn ticket_edit(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketEditRequest>(request)?;
    run_blocking(shell, "ticket edit", |shell| {
        forward_command(shell, "ticket.edit", "edited Ticket", request)
    })
    .await
}

#[tauri::command]
async fn ticket_graph_approve(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketGraphRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketGraphApproveRequest>(request)?;
    run_blocking(shell, "ticket graph approve", |shell| {
        forward_command(
            shell,
            "ticket.graph.approve",
            "approved Ticket graph",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn ticket_emergency_override(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketEmergencyOverrideRequest>(request)?;
    run_blocking(shell, "ticket emergency override", |shell| {
        forward_command(
            shell,
            "ticket.emergency.override",
            "overrode Ticket lifecycle",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn ticket_graph_list(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketGraphListResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketGraphListQuery>(request)?;
    run_blocking(shell, "ticket graph list", |shell| {
        forward_query(shell, "ticket.graph.list", "ticket graph list", request)
    })
    .await
}

#[tauri::command]
async fn ticket_reassign(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TicketRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TicketReassignRequest>(request)?;
    run_blocking(shell, "ticket reassign", |shell| {
        forward_command(shell, "ticket.reassign", "reassigned Ticket", request)
    })
    .await
}

#[tauri::command]
async fn profile_define(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<ProfileRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<ProfileDefineRequest>(request)?;
    run_blocking(shell, "profile define", |shell| {
        forward_command(shell, "profile.define", "defined profile", request)
    })
    .await
}

#[tauri::command]
async fn profile_update(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<ProfileRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<ProfileUpdateRequest>(request)?;
    run_blocking(shell, "profile update", |shell| {
        forward_command(shell, "profile.update", "updated profile", request)
    })
    .await
}

#[tauri::command]
async fn profile_retire(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<ProfileRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<ProfileRetireRequest>(request)?;
    run_blocking(shell, "profile retire", |shell| {
        forward_command(shell, "profile.retire", "retired profile", request)
    })
    .await
}

#[tauri::command]
async fn profile_list(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<ProfileListResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<ProfileListQuery>(request)?;
    run_blocking(shell, "profile list", |shell| {
        forward_query(shell, "profile.list", "profile list", request)
    })
    .await
}

#[tauri::command]
async fn profile_get(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<ProfileRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<ProfileGetQuery>(request)?;
    run_blocking(shell, "profile get", |shell| {
        forward_query(shell, "profile.get", "profile get", request)
    })
    .await
}

#[tauri::command]
async fn timeline_query(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<TimelineQueryResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<TimelineQuery>(request)?;
    run_blocking(shell, "timeline", |shell| {
        forward_query(shell, "timeline.query", "timeline", request)
    })
    .await
}

#[tauri::command]
async fn comment_create(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<CommentRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<CommentCreateRequest>(request)?;
    run_blocking(shell, "comment create", |shell| {
        forward_command(shell, "comment.create", "created Comment", request)
    })
    .await
}

#[tauri::command]
async fn comment_edit(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<CommentRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<CommentEditRequest>(request)?;
    run_blocking(shell, "comment edit", |shell| {
        forward_command(shell, "comment.edit", "edited Comment", request)
    })
    .await
}

#[tauri::command]
async fn comment_revisions(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<CommentRevisionsResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<CommentRevisionsQuery>(request)?;
    run_blocking(shell, "comment revisions", |shell| {
        forward_query(shell, "comment.revisions", "comment revisions", request)
    })
    .await
}

#[tauri::command]
async fn ruling_record(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<RulingRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<RulingRecordRequest>(request)?;
    run_blocking(shell, "ruling record", |shell| {
        forward_command(shell, "ruling.record", "recorded ruling", request)
    })
    .await
}

#[tauri::command]
async fn ruling_supersede(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<RulingRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<RulingSupersedeRequest>(request)?;
    run_blocking(shell, "ruling supersede", |shell| {
        forward_command(shell, "ruling.supersede", "superseded ruling", request)
    })
    .await
}

#[tauri::command]
async fn ruling_list(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<RulingListResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<RulingListQuery>(request)?;
    run_blocking(shell, "ruling list", |shell| {
        forward_query(shell, "ruling.list", "ruling list", request)
    })
    .await
}

#[tauri::command]
async fn deferral_record(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<DeferralRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<DeferralRecordRequest>(request)?;
    run_blocking(shell, "deferral record", |shell| {
        forward_command(shell, "deferral.record", "recorded deferral", request)
    })
    .await
}

#[tauri::command]
async fn deferral_supersede(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<DeferralRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<DeferralSupersedeRequest>(request)?;
    run_blocking(shell, "deferral supersede", |shell| {
        forward_command(shell, "deferral.supersede", "superseded deferral", request)
    })
    .await
}

#[tauri::command]
async fn deferral_list(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<DeferralListResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<DeferralListQuery>(request)?;
    run_blocking(shell, "deferral list", |shell| {
        forward_query(shell, "deferral.list", "deferral list", request)
    })
    .await
}

#[tauri::command]
async fn evidence_attach(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<EvidenceRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<EvidenceAttachRequest>(request)?;
    run_blocking(shell, "evidence attach", |shell| {
        forward_command(shell, "evidence.attach", "attached evidence", request)
    })
    .await
}

#[tauri::command]
async fn evidence_list(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<EvidenceListResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<EvidenceListQuery>(request)?;
    run_blocking(shell, "evidence list", |shell| {
        forward_query(shell, "evidence.list", "evidence list", request)
    })
    .await
}

#[tauri::command]
async fn herdr_settings_get(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<HerdrSettingsGetResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<HerdrSettingsGetQuery>(request)?;
    run_blocking(shell, "herdr settings", |shell| {
        forward_query(shell, "herdr.settings.get", "herdr settings", request)
    })
    .await
}

#[tauri::command]
async fn workspace_register(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<WorkspaceRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<WorkspaceRegisterRequest>(request)?;
    run_blocking(shell, "workspace register", |shell| {
        forward_command(shell, "workspace.register", "registered Workspace", request)
    })
    .await
}

#[tauri::command]
async fn herdr_settings_update(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<HerdrProjectSettings, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<HerdrSettingsUpdateRequest>(request)?;
    run_blocking(shell, "herdr settings update", |shell| {
        forward_command(
            shell,
            "herdr.settings.update",
            "updated herdr settings",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn workspace_observe(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<WorkspaceRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<WorkspaceObserveRequest>(request)?;
    run_blocking(shell, "workspace observe", |shell| {
        forward_command(shell, "workspace.observe", "observed Workspace", request)
    })
    .await
}

#[tauri::command]
async fn herdr_defaults_get(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<HerdrDefaultsGetResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<HerdrDefaultsGetQuery>(request)?;
    run_blocking(shell, "herdr defaults", |shell| {
        forward_query(shell, "herdr.defaults.get", "herdr defaults", request)
    })
    .await
}

#[tauri::command]
async fn herdr_defaults_update(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<HerdrGlobalDefaults, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<HerdrDefaultsUpdateRequest>(request)?;
    run_blocking(shell, "herdr defaults update", |shell| {
        forward_command(
            shell,
            "herdr.defaults.update",
            "updated herdr defaults",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn capacity_defaults_get(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<CapacityDefaultsGetResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<CapacityDefaultsGetQuery>(request)?;
    run_blocking(shell, "capacity defaults", move |shell| {
        forward_query(shell, "capacity.defaults.get", "capacity defaults", request)
    })
    .await
}

#[tauri::command]
async fn capacity_defaults_update(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<CapacityGlobalDefaults, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<CapacityDefaultsUpdateRequest>(request)?;
    run_blocking(shell, "capacity defaults update", |shell| {
        forward_command(
            shell,
            "capacity.defaults.update",
            "updated capacity defaults",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn capacity_settings_get(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<CapacitySettingsGetResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<CapacitySettingsGetQuery>(request)?;
    run_blocking(shell, "capacity settings", move |shell| {
        forward_query(shell, "capacity.settings.get", "capacity settings", request)
    })
    .await
}

#[tauri::command]
async fn capacity_settings_update(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<CapacityProjectCaps, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<CapacitySettingsUpdateRequest>(request)?;
    run_blocking(shell, "capacity settings update", |shell| {
        forward_command(
            shell,
            "capacity.settings.update",
            "updated capacity caps",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn dispatch_request(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<DispatchRequestRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<DispatchRequestCreateRequest>(request)?;
    run_blocking(shell, "dispatch request", |shell| {
        forward_command(
            shell,
            "dispatch.request",
            "created Dispatch Request",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn dispatch_claim(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<DispatchClaimResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<DispatchClaimRequest>(request)?;
    run_blocking(shell, "dispatch claim", |shell| {
        forward_command(shell, "dispatch.claim", "claimed Dispatch Request", request)
    })
    .await
}

#[tauri::command]
async fn dispatch_queue(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<DispatchQueueResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<DispatchQueueQuery>(request)?;
    run_blocking(shell, "dispatch queue", move |shell| {
        forward_query(shell, "dispatch.queue", "dispatch queue", request)
    })
    .await
}

#[tauri::command]
async fn workspace_retire(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<WorkspaceRecord, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<WorkspaceRetireRequest>(request)?;
    run_blocking(shell, "workspace retire", |shell| {
        forward_command(shell, "workspace.retire", "retired Workspace", request)
    })
    .await
}

#[tauri::command]
async fn workspace_list(
    shell: State<'_, Arc<Shell>>,
    request: serde_json::Value,
) -> Result<WorkspaceListResponse, ApiError> {
    let shell = shell.inner().clone();
    let request = decode_invoke_args::<WorkspaceListQuery>(request)?;
    run_blocking(shell, "workspace list", |shell| {
        forward_query(shell, "workspace.list", "workspace list", request)
    })
    .await
}

#[tauri::command]
async fn lane_create(
    shell: State<'_, Arc<Shell>>,
    request: LaneCreateRequest,
) -> Result<LaneRecord, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "lane create", |shell| {
        forward_command(shell, "lane.create", "created Lane", request)
    })
    .await
}

#[tauri::command]
async fn lane_workspace_assign(
    shell: State<'_, Arc<Shell>>,
    request: LaneWorkspaceAssignRequest,
) -> Result<LaneRecord, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "lane workspace assign", |shell| {
        forward_command(
            shell,
            "lane.workspace.assign",
            "assigned Workspace to Lane",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn lane_workspace_release(
    shell: State<'_, Arc<Shell>>,
    request: LaneWorkspaceReleaseRequest,
) -> Result<LaneRecord, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "lane workspace release", |shell| {
        forward_command(
            shell,
            "lane.workspace.release",
            "released Workspace from Lane",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn lane_ticket_assign(
    shell: State<'_, Arc<Shell>>,
    request: LaneTicketAssignRequest,
) -> Result<LaneRecord, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "lane ticket assign", |shell| {
        forward_command(
            shell,
            "lane.ticket.assign",
            "assigned Ticket to Lane",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn lane_ticket_release(
    shell: State<'_, Arc<Shell>>,
    request: LaneTicketReleaseRequest,
) -> Result<LaneRecord, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "lane ticket release", |shell| {
        forward_command(
            shell,
            "lane.ticket.release",
            "released Ticket from Lane",
            request,
        )
    })
    .await
}

#[tauri::command]
async fn lane_list(
    shell: State<'_, Arc<Shell>>,
    request: LaneListQuery,
) -> Result<LaneListResponse, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "lane list", |shell| {
        forward_query(shell, "lane.list", "lane list", request)
    })
    .await
}

#[tauri::command]
async fn clone_create(
    shell: State<'_, Arc<Shell>>,
    request: CloneCreateRequest,
) -> Result<CloneCreatedRecord, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "clone create", |shell| {
        forward_command(shell, "clone.create", "created clone", request)
    })
    .await
}

#[tauri::command]
async fn clone_remove(
    shell: State<'_, Arc<Shell>>,
    request: CloneRemoveRequest,
) -> Result<CloneRemovedRecord, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "clone remove", |shell| {
        forward_command(shell, "clone.remove", "removed clone", request)
    })
    .await
}

#[tauri::command]
async fn export_render(
    shell: State<'_, Arc<Shell>>,
    request: ExportRenderRequest,
) -> Result<ExportRenderResponse, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "export render", |shell| {
        forward_command(shell, "export.render", "rendered export", request)
    })
    .await
}

#[tauri::command]
async fn export_drift(
    shell: State<'_, Arc<Shell>>,
    request: ExportDriftQuery,
) -> Result<ExportDriftResponse, ApiError> {
    let shell = shell.inner().clone();
    run_blocking(shell, "export drift", |shell| {
        forward_query(shell, "export.drift", "export drift", request)
    })
    .await
}

shell_handlers::shell_handler_catalogue! {
    health_get,
    diagnostics_export,
    initiative_create,
    initiative_rename,
    initiative_archive,
    initiative_list,
    project_register,
    project_archive,
    project_list,
    plan_create,
    plan_spec_add,
    plan_spec_remove,
    plan_spec_move,
    plan_edge_add,
    plan_edge_remove,
    plan_activate,
    plan_replan,
    plan_complete,
    plan_cancel,
    plan_archive,
    plan_list,
    plan_get,
    plan_diagnostics,
    spec_create,
    spec_content_update,
    spec_version_approve,
    spec_version_supersede,
    spec_plan_join,
    spec_execution_move,
    spec_list,
    spec_get,
    spec_version_get,
    spec_coverage_check,
    spec_coverage_matrix,
    ticket_create,
    ticket_bug_qualify,
    ticket_bug_facts,
    ticket_list,
    ticket_get,
    ticket_dependency_add,
    ticket_dependency_remove,
    ticket_blocker_add,
    ticket_blocker_remove,
    ticket_dependencies,
    ticket_readiness,
    ticket_assign,
    ticket_transition,
    ticket_park,
    ticket_unpark,
    ticket_schedule,
    ticket_cancel,
    ticket_review,
    ticket_prioritise,
    ticket_edit,
    ticket_emergency_override,
    ticket_spec_move,
    ticket_graph_propose,
    ticket_graph_approve,
    ticket_graph_list,
    ticket_reassign,
    profile_define,
    profile_update,
    profile_retire,
    profile_list,
    profile_get,
    timeline_query,
    comment_create,
    comment_edit,
    comment_revisions,
    ruling_record,
    ruling_supersede,
    ruling_list,
    deferral_record,
    deferral_supersede,
    deferral_list,
    evidence_attach,
    evidence_list,
    herdr_settings_get,
    herdr_settings_update,
    herdr_defaults_get,
    herdr_defaults_update,
    capacity_defaults_get,
    capacity_defaults_update,
    capacity_settings_get,
    capacity_settings_update,
    dispatch_request,
    dispatch_claim,
    dispatch_queue,
    workspace_register,
    workspace_observe,
    workspace_retire,
    workspace_list,
    lane_create,
    lane_workspace_assign,
    lane_workspace_release,
    lane_ticket_assign,
    lane_ticket_release,
    lane_list,
    clone_create,
    clone_remove,
    export_render,
    export_drift,
}

/// Build the window, start the core on demand, and supervise the
/// connection for as long as this shell process lives.
pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .setup(|app| {
            let socket_path = managed_socket_path()?;
            let shell = Arc::new(Shell::default());
            app.manage(shell.clone());
            let supervisor = app.handle().clone();
            let spawn = std::thread::Builder::new()
                .name("kanban-shell-supervisor".to_owned())
                .spawn(move || supervise(socket_path, shell, supervisor));
            match spawn {
                Ok(_) => Ok(()),
                Err(failure) => Err(Box::new(failure) as Box<dyn std::error::Error>),
            }
        })
        .invoke_handler(catalogue_invoke_handler())
        .run(tauri::generate_context!())
}

/// Keep the shell's view of the core honest: start it if it is not
/// serving, connect, forward events, and announce loss.
fn supervise(socket_path: PathBuf, shell: Arc<Shell>, app: AppHandle) {
    let spawned = match ensure_core_running(&socket_path) {
        Ok(spawned) => spawned,
        Err(failure) => {
            eprintln!("kanban shell: {failure}");
            let _ = app.emit(CONNECTION_EVENT, ConnectionState::Disconnected);
            return;
        }
    };
    let link = match core_link::CoreLink::connect(&socket_path) {
        Ok(link) => link,
        Err(failure) => {
            eprintln!("kanban shell: the core socket is unreachable: {failure}");
            let _ = app.emit(CONNECTION_EVENT, ConnectionState::Disconnected);
            return;
        }
    };
    eprintln!("kanban shell: connected to {}", socket_path.display());
    *shell
        .link
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(link);
    let _ = app.emit(CONNECTION_EVENT, ConnectionState::Connected);

    // Blocks until the core closes the socket or the connection
    // dies; one reader thread keeps event order intact.
    let event_app = app.clone();
    let forward = core_link::forward_events(&socket_path, move |envelope| {
        let _ = event_app.emit(CORE_EVENT, envelope);
    });
    if let Err(failure) = forward {
        eprintln!("kanban shell: the event stream ended: {failure}");
    }
    // The socket is gone: drop the request link, say so, and reap
    // the core we spawned if it was ours and has exited. The shell
    // never kills a live core; reconnecting is KAN-S13 hardening.
    *shell
        .link
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let _ = app.emit(CONNECTION_EVENT, ConnectionState::Disconnected);
    if let Some(mut child) = spawned {
        let _ = child.try_wait();
    }
}

/// The socket the core serves inside managed application data.
fn managed_socket_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let data_dir = dirs::data_dir()
        .map(|dir| dir.join("Kanban"))
        .ok_or("the home directory is unknown")?;
    Ok(data_dir.join(kanban_transport::SOCKET_FILE_NAME))
}

/// True when a core is already serving `socket_path`.
pub fn socket_serving(socket_path: &Path) -> bool {
    UnixStream::connect(socket_path).is_ok()
}

/// Make sure exactly one core is serving `socket_path`: reuse a live
/// core, otherwise spawn one detached and wait for it to serve. The
/// child is returned for reaping, never for killing — quitting the
/// UI must leave the core running (DR-RB-02).
pub fn ensure_core_running(socket_path: &Path) -> Result<Option<Child>, String> {
    if socket_serving(socket_path) {
        return Ok(None);
    }
    let binary = locate_core_binary()?;
    let data_dir = socket_path.parent().unwrap_or_else(|| Path::new("."));
    let child = Command::new(&binary)
        .arg(data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Own process group: signals aimed at the shell's group (a
        // Ctrl-C on `tauri dev`) must not reach the durable core.
        .process_group(0)
        .spawn()
        .map_err(|failure| {
            format!(
                "could not start the core at {}: {failure}",
                binary.display()
            )
        })?;
    let deadline = std::time::Instant::now() + CORE_START_TIMEOUT;
    while std::time::Instant::now() < deadline {
        if socket_serving(socket_path) {
            return Ok(Some(child));
        }
        std::thread::sleep(CORE_START_POLL);
    }
    Err(format!(
        "the core at {} did not serve its socket within {} seconds",
        binary.display(),
        CORE_START_TIMEOUT.as_secs()
    ))
}

/// Where the core binary is: in debug and test builds an explicit
/// override first; in every configuration the copy packaged beside
/// the shell second; in debug builds the workspace build `just dev`
/// produces third.
pub fn locate_core_binary() -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(override_path) = std::env::var_os("KANBAN_CORE_BIN") {
        return Ok(PathBuf::from(override_path));
    }
    if let Ok(exe) = std::env::current_exe() {
        let beside = exe
            .parent()
            .map(|dir| dir.join("kanban-service"))
            .filter(|path| path.is_file());
        if let Some(beside) = beside {
            return Ok(beside);
        }
    }
    #[cfg(debug_assertions)]
    {
        let dev =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/debug/kanban-service");
        if dev.is_file() {
            return Ok(dev);
        }
    }
    #[cfg(debug_assertions)]
    {
        Err(
            "no kanban-service binary found; set KANBAN_CORE_BIN or build it with `cargo build -p kanban-service`"
                .to_owned(),
        )
    }
    #[cfg(not(debug_assertions))]
    {
        Err(
            "no kanban-service binary found; build it with `cargo build -p kanban-service`"
                .to_owned(),
        )
    }
}
