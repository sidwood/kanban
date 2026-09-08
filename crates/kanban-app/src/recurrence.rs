//! Atomic recurring occurrence orchestration.
use std::sync::Arc;

use kanban_domain::recurrence::{CatchUpPolicy, RecurrenceAdvance};
use kanban_domain::{ScheduleId, TaskTiming, Ticket, TicketBody, TicketId, TicketNumber};
use kanban_dto::{
    ApiError, LiveEventName, TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
};
use serde_json::json;

use crate::events::emit_catalogued;
use crate::ticket::record_of;
use crate::{DueActivation, EventSink, TimelineEnvelope};

pub struct DueRecurrence {
    pub activation: DueActivation,
    pub catch_up: CatchUpPolicy,
    pub has_open_occurrence: bool,
    pub readiness: kanban_domain::Readiness,
}

pub struct CommittedRecurrence {
    pub activation: DueActivation,
    pub ticket: Option<Ticket>,
}

/// Storage reads fresh facts and applies the decision under the same write lock.
pub trait RecurrenceStore: Send + Sync {
    fn due(&self, now: &str) -> Result<Vec<ScheduleId>, ApiError>;
    fn advance(
        &self,
        id: ScheduleId,
        now: &str,
        decide: &dyn Fn(&DueRecurrence) -> Result<Option<RecurrenceAdvance>, ApiError>,
    ) -> Result<Option<CommittedRecurrence>, ApiError>;
}

#[derive(Default)]
pub struct RecurrenceReport {
    pub minted: Vec<TicketId>,
}

pub struct RecurrencePass {
    store: Arc<dyn RecurrenceStore>,
}
impl RecurrencePass {
    pub fn new(store: Arc<dyn RecurrenceStore>) -> Self {
        Self { store }
    }
    pub fn tick(&self, now: &str, events: &dyn EventSink) -> Result<RecurrenceReport, ApiError> {
        let mut report = RecurrenceReport::default();
        for id in self.store.due(now)? {
            let committed = self.store.advance(id, now, &|due| {
                let Some(mut decision) = kanban_domain::recurrence::advance_recurring(
                    &due.activation.schedule,
                    now,
                    due.has_open_occurrence,
                    due.catch_up,
                )
                .map_err(invalid)?
                else {
                    return Ok(None);
                };
                if !due.readiness.is_ready()
                    && let Some(window) = decision.occurrence_window.take()
                {
                    decision.skipped = Some(kanban_domain::recurrence::SkippedWindows {
                        from: due.activation.schedule.next_activation().to_owned(),
                        through: window,
                        reason: kanban_domain::recurrence::SkippedWindowReason::Blocked,
                    });
                }
                Ok(Some(decision))
            })?;
            if let Some(CommittedRecurrence {
                activation,
                ticket: Some(ticket),
            }) = committed
            {
                emit_catalogued(
                    events,
                    LiveEventName::TicketCreated,
                    &record_of(&ticket, activation.project.code()),
                );
                report.minted.push(ticket.id());
            }
        }
        Ok(report)
    }
}

/// A recurring Task is new work, not a replacement, pin, or revived template.
pub fn occurrence_ticket(
    due: &DueRecurrence,
    id: TicketId,
    number: TicketNumber,
) -> Result<Ticket, ApiError> {
    let activation = &due.activation;
    let TicketBody::Task(task) = activation.ticket.body() else {
        return Err(ApiError::invalid_request(
            "only a Task mints recurring occurrences",
        ));
    };
    let body = TicketBody::task(
        task.title(),
        task.spec(),
        Some(task.subtype()),
        Some(task.mode()),
        task.completion().to_vec(),
        TaskTiming::none(),
    )
    .map_err(invalid)?;
    let mut ticket = Ticket::new(
        id,
        activation.ticket.project(),
        number,
        activation.ticket.priority(),
        body,
    );
    ticket
        .assign(activation.schedule.profile().clone())
        .map_err(invalid)?;
    kanban_domain::apply_drag(
        &mut ticket,
        kanban_domain::TicketState::Ready,
        kanban_domain::Actor::Agent,
        &due.readiness,
    )
    .map_err(invalid)?;
    Ok(ticket)
}

pub fn recurrence_envelopes(
    due: &DueActivation,
    decision: &RecurrenceAdvance,
    ticket: Option<&Ticket>,
) -> Vec<TimelineEnvelope> {
    let mut envelopes = vec![TimelineEnvelope::project(
        due.project.id().value(),
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Ticket,
            id: due.ticket.id().value().to_string(),
        }),
        json!({"action":"recurrence_advanced", "schedule_id":due.id().value(),
            "template_ticket_id":due.ticket.id().value(), "from":due.schedule.next_activation(),
            "next_activation":decision.next_activation, "occurrence_window":decision.occurrence_window,
            "skipped":decision.skipped.as_ref().map(|s| json!({"from":s.from,"through":s.through,"reason":s.reason.wire_name()}))}),
    )];
    if let Some(ticket) = ticket {
        envelopes.push(TimelineEnvelope::project(
            due.project.id().value(),
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Ticket,
                id: ticket.id().value().to_string(),
            }),
            json!({"action":"occurrence_created", "template_ticket_id":due.ticket.id().value(),
                "schedule_id":due.id().value(), "window_at":decision.occurrence_window,
                "from":null, "to":ticket.state().wire_name()}),
        ));
    }
    envelopes
}

fn invalid(error: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

pub trait SchedulePolicyStore: Send + Sync {
    fn list_attention(
        &self,
        project_id: u64,
    ) -> Result<kanban_dto::recurrence::ScheduleAttentionListResponse, ApiError>;
    fn get_policy(
        &self,
        project_id: u64,
    ) -> Result<kanban_dto::ProjectSchedulePolicyRecord, ApiError>;
    fn set_policy(
        &self,
        request: &kanban_dto::ProjectSchedulePolicySetRequest,
        envelope: &TimelineEnvelope,
    ) -> Result<kanban_dto::ProjectSchedulePolicyRecord, ApiError>;
}

impl crate::Core {
    pub fn register_scheduling_policy(
        &mut self,
        store: Arc<dyn SchedulePolicyStore>,
        projects: Arc<dyn crate::ProjectStore>,
    ) -> Result<(), crate::RegistrationError> {
        self.register_query(
            "schedule.attention.list",
            Arc::new(ScheduleAttention {
                store: store.clone(),
                projects: projects.clone(),
            }),
        )?;
        let handler = Arc::new(SchedulePolicy { store, projects });
        self.register_query("project.schedule_policy.get", handler.clone())?;
        self.register_command("project.schedule_policy.set", handler)
    }
}

struct SchedulePolicy {
    store: Arc<dyn SchedulePolicyStore>,
    projects: Arc<dyn crate::ProjectStore>,
}
impl SchedulePolicy {
    fn project(&self, id: u64) -> Result<kanban_domain::Project, ApiError> {
        self.projects
            .find(kanban_domain::ProjectId::new(id))?
            .ok_or_else(|| ApiError::not_found("project"))
    }
}
impl crate::dispatch::QueryHandler for SchedulePolicy {
    fn handle(&self, payload: &serde_json::Value) -> Result<serde_json::Value, ApiError> {
        let query: kanban_dto::ProjectSchedulePolicyQuery =
            crate::mutation::parse_payload(payload)?;
        self.project(query.project_id)?;
        serde_json::to_value(self.store.get_policy(query.project_id)?)
            .map_err(|e| ApiError::internal(&e.to_string()))
    }
}
impl crate::mutation::CommandHandler for SchedulePolicy {
    fn parse(
        &self,
        payload: &serde_json::Value,
    ) -> Result<crate::mutation::ParsedCommand, ApiError> {
        crate::mutation::parse_payload::<kanban_dto::ProjectSchedulePolicySetRequest>(payload)?;
        crate::mutation::ParsedCommand::lift("project schedule policy", payload)
    }
    fn current_version(&self, command: &crate::mutation::ParsedCommand) -> Result<u64, ApiError> {
        let request: kanban_dto::ProjectSchedulePolicySetRequest =
            crate::mutation::parse_payload(&command.payload)?;
        self.project(request.project_id)?;
        Ok(self.store.get_policy(request.project_id)?.version)
    }
    fn apply(
        &self,
        command: &crate::mutation::ParsedCommand,
        _: &dyn crate::CommandEffects,
    ) -> Result<serde_json::Value, ApiError> {
        let request: kanban_dto::ProjectSchedulePolicySetRequest =
            crate::mutation::parse_payload(&command.payload)?;
        if self.project(request.project_id)?.is_archived() {
            return Err(ApiError::invalid_request(
                "an archived Project cannot change scheduling policy",
            ));
        }
        let envelope = TimelineEnvelope::project(
            request.project_id,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Project,
                id: request.project_id.to_string(),
            }),
            json!({"action":"schedule_policy_changed", "catch_up_one":request.catch_up_one}),
        );
        serde_json::to_value(self.store.set_policy(&request, &envelope)?)
            .map_err(|e| ApiError::internal(&e.to_string()))
    }
}

struct ScheduleAttention {
    store: Arc<dyn SchedulePolicyStore>,
    projects: Arc<dyn crate::ProjectStore>,
}
impl crate::dispatch::QueryHandler for ScheduleAttention {
    fn handle(&self, payload: &serde_json::Value) -> Result<serde_json::Value, ApiError> {
        let query: kanban_dto::recurrence::ScheduleAttentionListQuery =
            crate::mutation::parse_payload(payload)?;
        self.projects
            .find(kanban_domain::ProjectId::new(query.project_id))?
            .ok_or_else(|| ApiError::not_found("project"))?;
        serde_json::to_value(self.store.list_attention(query.project_id)?)
            .map_err(|e| ApiError::internal(&e.to_string()))
    }
}
