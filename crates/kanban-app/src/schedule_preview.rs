//! Read-only preview uses the same pure calendar function as execution.
use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::mutation::parse_payload;
use kanban_dto::{
    ApiError, ScheduleActivationPreview, ScheduleDstBehaviour, ScheduleDstKind,
    SchedulePreviewQuery, SchedulePreviewResponse,
};
use serde_json::Value;
use std::sync::Arc;

impl Core {
    pub fn register_schedule_preview(&mut self) -> Result<(), RegistrationError> {
        self.register_query("schedule.preview", Arc::new(PreviewSchedule))
    }
}
struct PreviewSchedule;
impl QueryHandler for PreviewSchedule {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: SchedulePreviewQuery = parse_payload(payload)?;
        if !(1..=20).contains(&query.count) {
            return Err(ApiError::invalid_request(
                "preview count must be between 1 and 20",
            ));
        }
        let preview = match (query.activation.as_deref(), query.cron.as_deref()) {
            (Some(activation), None) => {
                kanban_domain::schedule_preview::preview_one_time(activation, &query.timezone)
            }
            (None, Some(cron)) => kanban_domain::schedule_preview::preview_recurring(
                cron,
                &query.timezone,
                &query.after,
                query.count,
            ),
            _ => {
                return Err(ApiError::invalid_request(
                    "preview exactly one activation or cron expression",
                ));
            }
        }
        .map_err(|e| ApiError::invalid_request(&e.to_string()))?;
        let kind = match preview.kind {
            kanban_domain::schedule_preview::DstKind::FixedInstant => ScheduleDstKind::FixedInstant,
            kanban_domain::schedule_preview::DstKind::FixedTime => ScheduleDstKind::FixedTime,
            kanban_domain::schedule_preview::DstKind::IntervalWildcard => {
                ScheduleDstKind::IntervalWildcard
            }
        };
        let response = SchedulePreviewResponse {
            activations: preview
                .activations
                .into_iter()
                .map(|a| ScheduleActivationPreview {
                    utc: a.utc,
                    local: a.local,
                })
                .collect(),
            dst_behaviour: ScheduleDstBehaviour {
                kind,
                spring_forward: preview.spring_forward.to_owned(),
                fall_back: preview.fall_back.to_owned(),
            },
        };
        serde_json::to_value(response).map_err(|e| ApiError::internal(&e.to_string()))
    }
}

pub trait ScheduleReadStore: Send + Sync {
    fn get(
        &self,
        ticket: kanban_domain::TicketId,
    ) -> Result<Option<kanban_domain::Schedule>, ApiError>;
}
impl Core {
    pub fn register_schedule_reads(
        &mut self,
        schedules: Arc<dyn ScheduleReadStore>,
        tickets: Arc<dyn crate::ticket::TicketStore>,
    ) -> Result<(), RegistrationError> {
        self.register_query("schedule.get", Arc::new(GetSchedule { schedules, tickets }))
    }
}
struct GetSchedule {
    schedules: Arc<dyn ScheduleReadStore>,
    tickets: Arc<dyn crate::ticket::TicketStore>,
}
impl QueryHandler for GetSchedule {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: kanban_dto::ScheduleGetQuery = parse_payload(payload)?;
        let ticket = kanban_domain::TicketId::new(query.ticket_id);
        self.tickets
            .find(ticket)?
            .ok_or_else(|| ApiError::not_found("ticket"))?;
        let schedule = self
            .schedules
            .get(ticket)?
            .map(|schedule| {
                let (activation, cron) = match schedule.trigger() {
                    kanban_domain::ScheduleTrigger::OneTime { activation } => {
                        (Some(activation.clone()), None)
                    }
                    kanban_domain::ScheduleTrigger::Recurring { expression } => {
                        (None, Some(expression.as_str().to_owned()))
                    }
                };
                Ok::<_, ApiError>(kanban_dto::ScheduleRecord {
                    id: schedule
                        .id()
                        .ok_or_else(|| ApiError::internal("stored Schedule has no identity"))?
                        .value(),
                    activation,
                    cron,
                    timezone: schedule.timezone().as_str().to_owned(),
                    profile: schedule.profile().as_str().to_owned(),
                    next_activation: schedule.next_activation().to_owned(),
                })
            })
            .transpose()?;
        serde_json::to_value(kanban_dto::ScheduleGetResponse {
            ticket_id: query.ticket_id,
            schedule,
        })
        .map_err(|e| ApiError::internal(&e.to_string()))
    }
}
