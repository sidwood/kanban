//! Operator recovery decisions reuse immutable Rulings, never workflow inference.
use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::events::emit_catalogued;
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::{CoordinatorWake, CoordinatorWakeRequest, ProjectStore};
use kanban_domain::{
    ProjectId, ResumeCustody, ResumeRefusal, RulingSummary, RunId, TicketId, TicketState,
    admit_run_resume,
};
use kanban_dto::RunRecoveryResumeRequest;
use kanban_dto::{
    ApiError, RunRecoveryListQuery, RunRecoveryListResponse, RunRecoveryRecord,
    RunRecoveryRetryRequest, RunRecoveryRuleRequest,
};
use kanban_dto::{LiveEventName, RulingIdentity};
use serde_json::Value;
use std::sync::Arc;

pub struct RunRecoveryContext {
    pub project: ProjectId,
    pub ticket: TicketId,
    /// Both read now rather than when the attempt started: a resume
    /// is judged against the lifecycle that holds today.
    pub project_archived: bool,
    pub ticket_state: TicketState,
    pub version: u64,
    pub superseded: bool,
    pub has_submission: bool,
    pub dispatch_request_id: u64,
    pub resumable: bool,
    pub review_eligible: bool,
    pub pending_resume: bool,
}

impl RunRecoveryContext {
    /// Whether current authoritative state admits resuming this
    /// attempt, naming the reason when it does not. Every seam that
    /// accepts or delivers a resume answers this one rule, so a stale
    /// intent and a fresh request are judged alike.
    pub fn admit_resume(&self) -> Result<(), ResumeRefusal> {
        admit_run_resume(ResumeCustody {
            project_archived: self.project_archived,
            ticket_state: self.ticket_state,
            superseded: self.superseded,
            has_submission: self.has_submission,
            request_active: self.review_eligible,
            authority_active: self.resumable,
        })
    }

    pub fn can_resume(&self) -> bool {
        self.admit_resume().is_ok()
    }
}

/// The typed resume refusal as the transport reports it. Acceptance
/// and delivery share it, so one refusal reads the same wherever it
/// is raised.
pub fn refuse_resume(refusal: ResumeRefusal) -> ApiError {
    ApiError::invalid_request(&refusal.to_string())
}

/// The authorised delivery a dispatch is handed: the identities its
/// prompt names, and nothing that would let a caller act on the
/// authorisation after the window that granted it closed.
pub struct PendingRunResume {
    pub recovery_id: u64,
    pub run_id: u64,
    pub dispatch_request_id: u64,
}

/// What the transport did with one authorised resume prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeDispatch {
    /// The Coordinator accepted the prompt.
    Accepted,
    /// The Coordinator answered and refused it.
    Declined,
    /// The transport failed, so whether the prompt arrived is
    /// unknown.
    Failed,
}

/// How one resume delivery ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeDelivery {
    /// No attempt was due: the intent is already closed, or its
    /// bounded retry delay has not elapsed.
    NotDue,
    /// Current custody refused the intent, which is closed as
    /// obsolete. Nothing was dispatched.
    Closed,
    /// The prompt was accepted, and the intent is closed as
    /// delivered.
    Delivered,
    /// The Coordinator refused the prompt; the intent stays pending
    /// for its bounded retry.
    Declined,
    /// The transport failed; the intent stays pending, and no
    /// delivery is claimed.
    Failed,
}

pub trait RunRecoveryStore: Send + Sync {
    fn context(&self, run: RunId) -> Result<Option<RunRecoveryContext>, ApiError>;
    fn rule(&self, run: RunId, summary: RulingSummary) -> Result<RunRecoveryRecord, ApiError>;
    fn retry(&self, run: RunId, summary: RulingSummary) -> Result<RunRecoveryRecord, ApiError>;
    fn resume(&self, run: RunId, summary: RulingSummary) -> Result<RunRecoveryRecord, ApiError>;
    fn list(&self, run: RunId) -> Result<Vec<RunRecoveryRecord>, ApiError>;
    fn pending_resume_ids(&self, project: ProjectId) -> Result<Vec<u64>, ApiError>;
    /// Deliver one pending resume intent: recheck current custody,
    /// reserve a bounded attempt, run `dispatch`, and record what the
    /// transport reported.
    ///
    /// Authorisation covers the whole call, `dispatch` included, so
    /// no mutation that could invalidate the custody just read can
    /// commit before the prompt goes out, and nothing outside this
    /// call can send that prompt or record its acknowledgement. The
    /// database connection is released before `dispatch` runs:
    /// `dispatch` may block on the transport for as long as its own
    /// deadline allows, but it must not mutate, because its thread
    /// already holds the authorisation a mutation would wait for.
    fn deliver_resume(
        &self,
        id: u64,
        dispatch: &mut dyn FnMut(&PendingRunResume) -> ResumeDispatch,
    ) -> Result<ResumeDelivery, ApiError>;
}

impl Core {
    pub fn register_run_recovery(
        &mut self,
        store: Arc<dyn RunRecoveryStore>,
        projects: Arc<dyn ProjectStore>,
        wake: Arc<dyn CoordinatorWake>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "run.recovery.rule",
            Arc::new(RecordRuling {
                store: store.clone(),
            }),
        )?;
        self.register_command(
            "run.recovery.retry",
            Arc::new(RetryRun {
                store: store.clone(),
                projects: projects.clone(),
                wake: wake.clone(),
            }),
        )?;
        self.register_command(
            "run.recovery.resume",
            Arc::new(ResumeRun {
                store: store.clone(),
                projects,
                wake,
            }),
        )?;
        self.register_query("run.recovery.list", Arc::new(ListRecovery { store }))?;
        Ok(())
    }
}

fn context(store: &dyn RunRecoveryStore, run: RunId) -> Result<RunRecoveryContext, ApiError> {
    store
        .context(run)?
        .ok_or_else(|| ApiError::not_found("run"))
}

struct RecordRuling {
    store: Arc<dyn RunRecoveryStore>,
}
impl CommandHandler for RecordRuling {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<RunRecoveryRuleRequest>(payload)?;
        ParsedCommand::lift("run_recovery", payload)
    }
    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: RunRecoveryRuleRequest = parse_payload(&command.payload)?;
        Ok(context(self.store.as_ref(), RunId::new(request.run_id))?.version)
    }
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: RunRecoveryRuleRequest = parse_payload(&command.payload)?;
        let summary = RulingSummary::new(&request.summary)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let record = self.store.rule(RunId::new(request.run_id), summary)?;
        emit_catalogued(
            effects,
            LiveEventName::RulingRecorded,
            &RulingIdentity {
                id: record.ruling_id,
            },
        );
        serde_json::to_value(record).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

struct RetryRun {
    store: Arc<dyn RunRecoveryStore>,
    projects: Arc<dyn ProjectStore>,
    wake: Arc<dyn CoordinatorWake>,
}
impl CommandHandler for RetryRun {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<RunRecoveryRetryRequest>(payload)?;
        ParsedCommand::lift("run_recovery", payload)
    }
    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: RunRecoveryRetryRequest = parse_payload(&command.payload)?;
        Ok(context(self.store.as_ref(), RunId::new(request.run_id))?.version)
    }
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: RunRecoveryRetryRequest = parse_payload(&command.payload)?;
        let run = RunId::new(request.run_id);
        let context = context(self.store.as_ref(), run)?;
        if context.superseded || context.has_submission || !context.review_eligible {
            return Err(ApiError::invalid_request(
                "a superseded, submitted or inactive review run cannot be retried through recovery",
            ));
        }
        let summary = RulingSummary::new(&request.summary)
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let project = self
            .projects
            .find(context.project)?
            .ok_or_else(|| ApiError::not_found("project"))?;
        let record = self.store.retry(run, summary)?;
        let request_id = record
            .replacement_dispatch_request_id
            .ok_or_else(|| ApiError::internal("a retry must identify its replacement request"))?;
        let wake = CoordinatorWakeRequest {
            project_id: project.id().value(),
            dispatch_request_id: request_id,
            resume_run_id: None,
            seed_workspace: project.registration().seed_workspace().to_owned(),
            herdr_workspace: project.registration().herdr_workspace().to_owned(),
            herdr_session: project.registration().herdr_session().map(str::to_owned),
        };
        let port = self.wake.clone();
        effects.after_commit(Box::new(move || port.wake(wake)));
        emit_catalogued(
            effects,
            LiveEventName::RulingRecorded,
            &RulingIdentity {
                id: record.ruling_id,
            },
        );
        serde_json::to_value(record).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

struct ResumeRun {
    store: Arc<dyn RunRecoveryStore>,
    projects: Arc<dyn ProjectStore>,
    wake: Arc<dyn CoordinatorWake>,
}
impl CommandHandler for ResumeRun {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<RunRecoveryResumeRequest>(payload)?;
        ParsedCommand::lift("run_recovery", payload)
    }
    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: RunRecoveryResumeRequest = parse_payload(&command.payload)?;
        Ok(context(self.store.as_ref(), RunId::new(request.run_id))?.version)
    }
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: RunRecoveryResumeRequest = parse_payload(&command.payload)?;
        let run = RunId::new(request.run_id);
        let context = context(self.store.as_ref(), run)?;
        context.admit_resume().map_err(refuse_resume)?;
        if context.pending_resume {
            return Err(ApiError::invalid_request(
                "a resume intent is already pending delivery for this attempt",
            ));
        }
        let project = self
            .projects
            .find(context.project)?
            .ok_or_else(|| ApiError::not_found("project"))?;
        let summary = RulingSummary::new(&request.summary)
            .map_err(|e| ApiError::invalid_request(&e.to_string()))?;
        let record = self.store.resume(run, summary)?;
        let wake = CoordinatorWakeRequest {
            project_id: project.id().value(),
            dispatch_request_id: context.dispatch_request_id,
            resume_run_id: Some(run.value()),
            seed_workspace: project.registration().seed_workspace().to_owned(),
            herdr_workspace: project.registration().herdr_workspace().to_owned(),
            herdr_session: project.registration().herdr_session().map(str::to_owned),
        };
        let port = self.wake.clone();
        effects.after_commit(Box::new(move || port.wake(wake)));
        emit_catalogued(
            effects,
            LiveEventName::RulingRecorded,
            &RulingIdentity {
                id: record.ruling_id,
            },
        );
        serde_json::to_value(record).map_err(|e| ApiError::internal(&e.to_string()))
    }
}

struct ListRecovery {
    store: Arc<dyn RunRecoveryStore>,
}
impl QueryHandler for ListRecovery {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let request: RunRecoveryListQuery = parse_payload(payload)?;
        let run = RunId::new(request.run_id);
        let records = self.store.list(run)?;
        let context = context(self.store.as_ref(), run)?;
        let response = RunRecoveryListResponse {
            run_id: request.run_id,
            version: records.last().map_or(0, |record| record.version),
            can_resume: context.can_resume() && !context.pending_resume,
            can_retry: !context.superseded && !context.has_submission && context.review_eligible,
            // Whether an accepted intent is still awaiting its
            // delivery disposition is a durable fact of its own: an
            // invalidation stops the delivery, but until that
            // delivery is authoritatively closed the row is really
            // still queued, and a consumer polling for its outcome
            // must not be told it vanished (KAN-T142-AC4).
            pending_resume: context.pending_resume,
            records,
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}
