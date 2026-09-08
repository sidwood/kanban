//! Authoritative result intake; telemetry cannot write a submission.
use crate::QueryHandler;
use crate::{
    CapabilityStore, CommandEffects, CommandHandler, Core, ParsedCommand, RegistrationError,
    TimelineEnvelope, encode_capability, parse_payload,
};
use kanban_domain::{CapabilityId, CapabilityRole};
use kanban_dto::submission::{SubmissionListQuery, SubmissionListResponse};
use kanban_dto::{
    ApiError, SubmissionRecord, SubmissionResult, SubmissionSubmitRequest, TimelineEntityKind,
    TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Run facts read within the command transaction, never supplied by an agent.
#[derive(Debug, Clone)]
pub struct SubmissionContext {
    pub project_id: u64,
    pub ticket_id: u64,
    pub dispatch_request_id: u64,
    pub version: u64,
}

pub trait SubmissionStore: Send + Sync {
    fn list(&self, project_id: u64) -> Result<Vec<SubmissionRecord>, ApiError>;
    fn context(&self, run: u64) -> Result<SubmissionContext, ApiError>;
    fn append(
        &self,
        record: SubmissionRecord,
        envelope: &dyn Fn(&SubmissionRecord) -> TimelineEnvelope,
        reviewed: &crate::review_execution::ReviewChangeObserver<'_>,
    ) -> Result<SubmissionRecord, ApiError>;
}

/// Only persisted intake can satisfy a run's required result. A Herdr
/// result frame cannot claim that intake accepted anything.
pub fn missing_submission_signal(
    store: &dyn SubmissionStore,
    project_id: u64,
    event: &Value,
) -> Result<Option<crate::AttentionSignal>, ApiError> {
    if event.get("kind").and_then(Value::as_str) != Some("role.settled") {
        return Ok(None);
    }
    let Some(run_id) = event.get("run").and_then(crate::telemetry::observed_run_id) else {
        return Ok(None);
    };
    let context = match store.context(run_id) {
        Ok(context) => context,
        Err(error) if error.code == kanban_dto::ErrorCode::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if context.project_id != project_id
        || store
            .list(project_id)?
            .iter()
            .any(|record| record.run_id == run_id)
    {
        return Ok(None);
    }
    Ok(Some(crate::AttentionSignal {
        project_id,
        reason: "missing_submission".to_owned(),
        detail: json!({"run_id": run_id, "ticket_id": context.ticket_id,
            "dispatch_request_id": context.dispatch_request_id}),
    }))
}

/// Keep historical telemetry in the timeline, but clear current missing-result
/// attention once authoritative intake exists. A failed read must not hide it.
pub fn unresolved_submission_signals(
    store: &dyn SubmissionStore,
    project_id: u64,
    signals: &[crate::AttentionSignal],
) -> Result<Vec<crate::AttentionSignal>, ApiError> {
    let submitted: std::collections::HashSet<_> = store
        .list(project_id)?
        .into_iter()
        .map(|record| record.run_id)
        .collect();
    Ok(signals
        .iter()
        .filter(|signal| {
            signal.reason != "missing_submission"
                || !signal.detail["run_id"]
                    .as_u64()
                    .is_some_and(|run| submitted.contains(&run))
        })
        .cloned()
        .collect())
}

impl Core {
    pub fn register_submissions(
        &mut self,
        store: Arc<dyn SubmissionStore>,
        capabilities: Arc<dyn CapabilityStore>,
        projects: Arc<dyn crate::ProjectStore>,
        wake: Arc<dyn crate::CoordinatorWake>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "submission.submit",
            Arc::new(Submit {
                store: store.clone(),
                capabilities,
                projects,
                wake,
            }),
        )?;
        self.register_query("submission.list", Arc::new(List { store }))
    }
}

struct List {
    store: Arc<dyn SubmissionStore>,
}
impl QueryHandler for List {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: SubmissionListQuery = parse_payload(payload)?;
        serde_json::to_value(SubmissionListResponse {
            project_id: query.project_id,
            submissions: self.store.list(query.project_id)?,
        })
        .map_err(|e| ApiError::internal(&e.to_string()))
    }
}

struct Submit {
    store: Arc<dyn SubmissionStore>,
    capabilities: Arc<dyn CapabilityStore>,
    projects: Arc<dyn crate::ProjectStore>,
    wake: Arc<dyn crate::CoordinatorWake>,
}
impl CommandHandler for Submit {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        let request: SubmissionSubmitRequest = parse_payload(payload)?;
        if let SubmissionResult::Review { findings, .. } = &request.result {
            crate::findings::validate_findings(findings)?;
        }
        ParsedCommand::lift("submission", payload)
    }
    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: SubmissionSubmitRequest = parse_payload(&command.payload)?;
        Ok(self.store.context(request.run_id)?.version)
    }
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: SubmissionSubmitRequest = parse_payload(&command.payload)?;
        let context = self.store.context(request.run_id)?;
        let capability = self
            .capabilities
            .find(CapabilityId::new(request.capability_id))?
            .ok_or_else(|| ApiError::not_found("capability"))?;
        capability
            .permits("submission.submit")
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        if capability.dispatch().value() != context.dispatch_request_id
            || capability.scope().ticket().value() != context.ticket_id
        {
            return Err(ApiError::invalid_request(
                "the capability does not belong to this run",
            ));
        }
        let (role, tip, summary) = match &request.result {
            SubmissionResult::Implementation { tip, summary } => {
                (CapabilityRole::Implementer, tip, summary)
            }
            SubmissionResult::Review { tip, summary, .. } => {
                (CapabilityRole::Reviewer, tip, summary)
            }
        };
        if capability.scope().role() != role {
            return Err(ApiError::invalid_request(
                "the result does not match the run role",
            ));
        }
        if !matches!(tip.len(), 40 | 64) || !tip.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ApiError::invalid_request(
                "a result requires a full commit or tree hash",
            ));
        }
        if summary.trim().is_empty() {
            return Err(ApiError::invalid_request(
                "a result summary cannot be blank",
            ));
        }
        let scope = encode_capability(&capability);
        let record = SubmissionRecord {
            id: 0,
            project_id: context.project_id,
            ticket_id: context.ticket_id,
            run_id: request.run_id,
            capability_id: request.capability_id,
            role: scope.role,
            reviewer_slot_id: scope.reviewer_slot_id,
            result: request.result,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        let envelope = |record: &SubmissionRecord| {
            TimelineEnvelope::project(
                record.project_id,
                TimelineEventKind::Transition,
                Some(TimelineEntityRef {
                    kind: TimelineEntityKind::Ticket,
                    id: record.ticket_id.to_string(),
                }),
                json!({"action": "submission_received", "submission_id": record.id, "run_id": record.run_id,
                "capability_id": record.capability_id, "role": record.role,
                "reviewer_slot_id": record.reviewer_slot_id}),
            )
        };
        let reviewed = |before: &kanban_dto::ReviewExecutionRecord,
                        after: &kanban_dto::ReviewExecutionRecord| {
            let project = self
                .projects
                .find(kanban_domain::ProjectId::new(after.project_id))?
                .ok_or_else(|| ApiError::not_found("project"))?;
            crate::review_execution::schedule_review_dispatches(
                &project,
                self.wake.clone(),
                effects,
                Some(before),
                after,
            );
            Ok(())
        };
        let record = self.store.append(record, &envelope, &reviewed)?;
        serde_json::to_value(record).map_err(|e| ApiError::internal(&e.to_string()))
    }
}
