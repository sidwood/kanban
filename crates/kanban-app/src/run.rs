//! Run commands and queries: acknowledge the run of a claimed Dispatch
//! Request with its requested and effective profile snapshots, and
//! list a Project's runs (KAN-S9-US3, DR-EP-04). The snapshots freeze
//! at the mint — the effective resolution applies the fallback policy
//! over the catalogue as it stands — and a later catalogue change
//! never rewrites them (DR-EP-05).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use kanban_domain::{
    DispatchRequest, DispatchRequestId, DispatchStatus, ExecutionProfile, ProfileSnapshot,
    ProjectId, Run, RunError, RunId, RunStatus, resolve_effective,
};
use kanban_dto::{
    ApiError, LiveEventName, ProfileSnapshotRecord, RunAcknowledgeRequest, RunListQuery,
    RunListResponse, RunRecord, RunStatus as WireStatus, TimelineEntityKind, TimelineEntityRef,
    TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::dispatch_request::DispatchStore;
use crate::events::emit_catalogued;
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::profile::ProfileStore;
use crate::project::ProjectStore;
use crate::ticket::TicketStore;
use crate::timeline::TimelineEnvelope;

/// The facts one mint writes: the claimed request the run executes and
/// the snapshots the effective resolution produced.
#[derive(Debug, Clone)]
pub struct RunMint {
    /// The claimed Dispatch Request the run executes. Storage calls
    /// the domain mint, which refuses an unclaimed request.
    pub request: DispatchRequest,
    /// The requested profile snapshot: what the assignment named.
    pub requested: ProfileSnapshot,
    /// The effective profile snapshot: what actually runs.
    pub effective: ProfileSnapshot,
    /// The names the fallback walk touched, requested first.
    pub fallback_path: Vec<String>,
    /// When the run mints, as unix seconds.
    pub created_at: u64,
}

/// The storage port Run operations call through. `mint` lands the run
/// row and the timeline envelope inside one write, and the partial
/// unique index keeps one executing run per claimed request.
pub trait RunStore: Send + Sync {
    /// Insert a fresh executing run. Storage assigns the identity and
    /// asks `envelope` for the timeline row that identity belongs in.
    /// A request already holding an executing run is refused.
    fn mint(
        &self,
        draft: &RunMint,
        envelope: &dyn Fn(RunId) -> TimelineEnvelope,
    ) -> Result<Run, ApiError>;
    /// Every run of one Project, oldest first.
    fn list_for_project(&self, project: ProjectId) -> Result<Vec<Run>, ApiError>;
    /// The executing run of one claimed Dispatch Request, if it has
    /// one.
    fn executing_for_request(&self, request: DispatchRequestId) -> Result<Option<Run>, ApiError>;
}

impl Core {
    /// Register the run operations.
    pub fn register_runs(
        &mut self,
        runs: Arc<dyn RunStore>,
        requests: Arc<dyn DispatchStore>,
        tickets: Arc<dyn TicketStore>,
        profiles: Arc<dyn ProfileStore>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "run.acknowledge",
            Arc::new(AcknowledgeRun {
                runs: runs.clone(),
                requests: requests.clone(),
                tickets: tickets.clone(),
                profiles: profiles.clone(),
            }),
        )?;
        self.register_query("run.list", Arc::new(ListRuns { runs, projects }))?;
        Ok(())
    }
}

/// Serves `run.acknowledge` (DR-HB-15): the Coordinator acknowledges
/// the run of a claim it holds, and the core freezes the requested and
/// effective profile snapshots with the fallback transitions between
/// them (DR-EP-04).
struct AcknowledgeRun {
    runs: Arc<dyn RunStore>,
    requests: Arc<dyn DispatchStore>,
    tickets: Arc<dyn TicketStore>,
    profiles: Arc<dyn ProfileStore>,
}

impl CommandHandler for AcknowledgeRun {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<RunAcknowledgeRequest>(payload)?;
        ParsedCommand::lift("run", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: RunAcknowledgeRequest = parse_payload(&command.payload)?;
        Ok(self.request(request.dispatch_request_id)?.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: RunAcknowledgeRequest = parse_payload(&command.payload)?;
        let claim = self.request(request.dispatch_request_id)?;
        if claim.status() != DispatchStatus::Claimed {
            return Err(refuse(RunError::UnclaimedRequest));
        }
        if let Some(existing) = self.runs.executing_for_request(claim.id())? {
            return Err(ApiError::invalid_request(&format!(
                "Dispatch Request {} already holds an executing run",
                existing.dispatch_request().value()
            )));
        }
        // The snapshots freeze the catalogue as it stands at the mint:
        // the requested entry under the assignment's own name, and the
        // entry the fallback policy resolves to (DR-EP-04, DR-EP-05).
        let catalogue = self.profiles.list()?;
        let assigned = self
            .tickets
            .find(claim.ticket())?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {}", claim.ticket().value())))?
            .profile()
            .cloned()
            .ok_or_else(|| {
                ApiError::invalid_request("a run requires the Ticket's assigned Execution Profile")
            })?;
        let (effective, path) = resolve_effective(&catalogue, &assigned).map_err(refuse)?;
        let requested_entry = catalogue
            .iter()
            .find(|entry| entry.name() == &assigned)
            .ok_or_else(|| ApiError::internal("the requested name resolved and then vanished"))?;
        let requested = snapshot_of(requested_entry).map_err(refuse)?;
        let effective_snapshot = snapshot_of(effective).map_err(refuse)?;
        let fallback_path: Vec<String> = path.iter().map(|name| name.as_str().to_owned()).collect();
        let facts = json!({
            "ticket_id": claim.ticket().value(),
            "dispatch_request_id": claim.id().value(),
            "requested": assigned.as_str(),
            "effective": effective.name().as_str(),
            "fallback": effective.name() != requested_entry.name(),
        });
        let created_at = unix_now();
        let project = claim.project();
        let run = self.runs.mint(
            &RunMint {
                request: claim,
                requested,
                effective: effective_snapshot,
                fallback_path,
                created_at,
            },
            &|id| acknowledge_envelope(id, project, &facts),
        )?;
        let record = encode_run(&run);
        emit_catalogued(effects, LiveEventName::RunAcknowledged, &record);
        serde_json::to_value(record).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

impl AcknowledgeRun {
    fn request(&self, id: u64) -> Result<DispatchRequest, ApiError> {
        self.requests
            .find(DispatchRequestId::new(id))?
            .ok_or_else(|| ApiError::not_found(&format!("dispatch request {id}")))
    }
}

/// Serves `run.list`.
struct ListRuns {
    runs: Arc<dyn RunStore>,
    projects: Arc<dyn ProjectStore>,
}

impl QueryHandler for ListRuns {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: RunListQuery = parse_payload(payload)?;
        self.projects
            .find(ProjectId::new(query.project_id))?
            .ok_or_else(|| ApiError::not_found(&format!("project {}", query.project_id)))?;
        let runs = self
            .runs
            .list_for_project(ProjectId::new(query.project_id))?;
        let response = RunListResponse {
            project_id: query.project_id,
            runs: runs.iter().map(encode_run).collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// The timeline row for one acknowledged run: on the Project's
/// timeline, about the run, with `action` naming the mint. The entity
/// is the run; the Ticket and the Dispatch Request it executes are
/// facts beside it.
fn acknowledge_envelope(run: RunId, project: ProjectId, facts: &Value) -> TimelineEnvelope {
    let mut detail = facts.clone();
    let object = detail
        .as_object_mut()
        .expect("run acknowledge facts are a JSON object");
    object.insert("action".to_owned(), Value::from("acknowledged"));
    object.insert("run_id".to_owned(), json!(run.value()));
    TimelineEnvelope::project(
        project.value(),
        TimelineEventKind::Run,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Run,
            id: run.value().to_string(),
        }),
        detail,
    )
}

/// One snapshot of an entry's five decisions as they stand.
fn snapshot_of(entry: &ExecutionProfile) -> Result<ProfileSnapshot, RunError> {
    ProfileSnapshot::new(
        entry.name().as_str(),
        entry.harness(),
        entry.model(),
        entry.effort(),
        entry.usage_pool(),
    )
}

/// Report a refused run rule as the stable invalid-request code.
fn refuse(error: RunError) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

fn encode_snapshot(snapshot: &ProfileSnapshot) -> ProfileSnapshotRecord {
    ProfileSnapshotRecord {
        name: snapshot.name().to_owned(),
        harness: snapshot.harness().to_owned(),
        model: snapshot.model().to_owned(),
        effort: snapshot.effort().to_owned(),
        usage_pool: snapshot.usage_pool().to_owned(),
    }
}

fn encode_run(run: &Run) -> RunRecord {
    RunRecord {
        id: run.id().value(),
        project_id: run.project().value(),
        ticket_id: run.ticket().value(),
        dispatch_request_id: run.dispatch_request().value(),
        status: match run.status() {
            RunStatus::Executing => WireStatus::Executing,
        },
        requested: encode_snapshot(run.requested()),
        effective: encode_snapshot(run.effective()),
        fallback: run.fell_back(),
        fallback_path: run.fallback_path().to_vec(),
        created_at: run.created_at(),
        version: run.version(),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
