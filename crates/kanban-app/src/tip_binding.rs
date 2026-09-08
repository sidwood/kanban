//! Criterion evidence binding at a reviewed code tip.

use std::sync::Arc;

use kanban_domain::{
    CriterionBinding, CriterionKind, EvidenceReview, TicketId, TicketKind, TipBindingError,
    attach_criterion_evidence, complete_task_criterion, invalidate_on_content_change,
    review_criterion_evidence, satisfy_at_approved_tip,
};
use kanban_dto::{
    ApiError, CriterionBindingListQuery, CriterionBindingListResponse, CriterionBindingRecord,
    CriterionCompleteRequest, CriterionEvidenceAttachRequest, CriterionEvidenceReviewRequest,
    CriterionInvalidateRequest, CriterionKindDto, CriterionSatisfyRequest, EvidenceReviewDto,
    TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::evidence::{EvidenceFilter, EvidenceStore};
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::ticket::TicketStore;
use crate::timeline::TimelineEnvelope;

pub trait CriterionBindingStore: Send + Sync {
    fn save(
        &self,
        ticket_id: u64,
        binding: &CriterionBinding,
        envelope: TimelineEnvelope,
    ) -> Result<(), ApiError>;
    fn find(
        &self,
        ticket_id: u64,
        criterion_index: u64,
    ) -> Result<Option<CriterionBinding>, ApiError>;
    fn list(&self, ticket_id: u64) -> Result<Vec<CriterionBinding>, ApiError>;
}

impl Core {
    pub fn register_criterion_bindings(
        &mut self,
        bindings: Arc<dyn CriterionBindingStore>,
        tickets: Arc<dyn TicketStore>,
        evidence: Arc<dyn EvidenceStore>,
    ) -> Result<(), RegistrationError> {
        let context = BindingContext {
            bindings,
            tickets,
            evidence,
        };
        self.register_command(
            "criterion.evidence.attach",
            Arc::new(AttachCriterionEvidence(context.clone())),
        )?;
        self.register_command(
            "criterion.evidence.review",
            Arc::new(ReviewCriterionEvidence(context.clone())),
        )?;
        self.register_command(
            "criterion.satisfy",
            Arc::new(SatisfyCriterion(context.clone())),
        )?;
        self.register_command(
            "criterion.complete",
            Arc::new(CompleteCriterion(context.clone())),
        )?;
        self.register_command(
            "criterion.invalidate",
            Arc::new(InvalidateCriteria(context.clone())),
        )?;
        self.register_query("criterion.bindings", Arc::new(ListBindings(context)))
    }
}

#[derive(Clone)]
struct BindingContext {
    bindings: Arc<dyn CriterionBindingStore>,
    tickets: Arc<dyn TicketStore>,
    evidence: Arc<dyn EvidenceStore>,
}

fn refuse(error: TipBindingError) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

fn envelope(ticket_id: u64, action: &str, facts: Value) -> TimelineEnvelope {
    TimelineEnvelope::project(
        1,
        TimelineEventKind::Evidence,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Ticket,
            id: ticket_id.to_string(),
        }),
        json!({ "action": action, "facts": facts }),
    )
}

fn record_of(ticket_id: u64, binding: &CriterionBinding) -> CriterionBindingRecord {
    CriterionBindingRecord {
        ticket_id,
        criterion_index: binding.criterion_index(),
        kind: match binding.kind() {
            CriterionKind::Acceptance => CriterionKindDto::Acceptance,
            CriterionKind::Task => CriterionKindDto::Task,
        },
        evidence_id: binding.evidence_id(),
        tip: binding.tip().to_owned(),
        review: match binding.review() {
            EvidenceReview::Pending => EvidenceReviewDto::Pending,
            EvidenceReview::Validated => EvidenceReviewDto::Validated,
            EvidenceReview::Rejected => EvidenceReviewDto::Rejected,
        },
        satisfied: binding.satisfied(),
        void: binding.void(),
    }
}

fn encode(ticket_id: u64, binding: &CriterionBinding) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(ticket_id, binding))
        .map_err(|error| ApiError::internal(&error.to_string()))
}

struct AttachCriterionEvidence(BindingContext);
impl CommandHandler for AttachCriterionEvidence {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<CriterionEvidenceAttachRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }
    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }
    fn apply(&self, command: &ParsedCommand, _: &dyn CommandEffects) -> Result<Value, ApiError> {
        let request: CriterionEvidenceAttachRequest = parse_payload(&command.payload)?;
        let ticket = self
            .0
            .tickets
            .find(TicketId::new(request.ticket_id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {}", request.ticket_id)))?;
        let kind = match ticket.kind() {
            TicketKind::Task => CriterionKind::Task,
            _ => CriterionKind::Acceptance,
        };
        let count = if kind == CriterionKind::Task {
            ticket.completion().len()
        } else {
            ticket.criteria().len()
        };
        if request.criterion_index as usize >= count {
            return Err(ApiError::invalid_request("the criterion index is unknown"));
        }
        let items = self.0.evidence.list(&EvidenceFilter {
            project_id: ticket.project().value(),
            entity_kind: Some("ticket".to_owned()),
            entity_id: Some(request.ticket_id.to_string()),
        })?;
        if !items
            .iter()
            .any(|item| item.id().value() == request.evidence_id)
        {
            return Err(ApiError::not_found(&format!(
                "evidence {}",
                request.evidence_id
            )));
        }
        let binding = attach_criterion_evidence(
            kind,
            request.criterion_index,
            request.evidence_id,
            request.tip,
        )
        .map_err(refuse)?;
        self.0.bindings.save(
            request.ticket_id,
            &binding,
            envelope(
                request.ticket_id,
                "bound",
                json!({"evidence_id": request.evidence_id}),
            ),
        )?;
        encode(request.ticket_id, &binding)
    }
}

struct ReviewCriterionEvidence(BindingContext);
impl CommandHandler for ReviewCriterionEvidence {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<CriterionEvidenceReviewRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }
    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }
    fn apply(&self, command: &ParsedCommand, _: &dyn CommandEffects) -> Result<Value, ApiError> {
        let request: CriterionEvidenceReviewRequest = parse_payload(&command.payload)?;
        let mut binding = self
            .0
            .bindings
            .find(request.ticket_id, request.criterion_index)?
            .ok_or_else(|| ApiError::not_found("criterion binding"))?;
        let review = match request.review {
            EvidenceReviewDto::Pending => EvidenceReview::Pending,
            EvidenceReviewDto::Validated => EvidenceReview::Validated,
            EvidenceReviewDto::Rejected => EvidenceReview::Rejected,
        };
        review_criterion_evidence(&mut binding, review).map_err(refuse)?;
        self.0.bindings.save(
            request.ticket_id,
            &binding,
            envelope(
                request.ticket_id,
                "reviewed",
                json!({"review": request.review}),
            ),
        )?;
        encode(request.ticket_id, &binding)
    }
}

struct SatisfyCriterion(BindingContext);
impl CommandHandler for SatisfyCriterion {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<CriterionSatisfyRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }
    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }
    fn apply(&self, command: &ParsedCommand, _: &dyn CommandEffects) -> Result<Value, ApiError> {
        let request: CriterionSatisfyRequest = parse_payload(&command.payload)?;
        let mut binding = self
            .0
            .bindings
            .find(request.ticket_id, request.criterion_index)?
            .ok_or_else(|| ApiError::not_found("criterion binding"))?;
        satisfy_at_approved_tip(&mut binding, &request.tip).map_err(refuse)?;
        self.0.bindings.save(
            request.ticket_id,
            &binding,
            envelope(request.ticket_id, "satisfied", json!({"tip": request.tip})),
        )?;
        encode(request.ticket_id, &binding)
    }
}

struct CompleteCriterion(BindingContext);
impl CommandHandler for CompleteCriterion {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<CriterionCompleteRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }
    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }
    fn apply(&self, command: &ParsedCommand, _: &dyn CommandEffects) -> Result<Value, ApiError> {
        let request: CriterionCompleteRequest = parse_payload(&command.payload)?;
        let ticket = self
            .0
            .tickets
            .find(TicketId::new(request.ticket_id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {}", request.ticket_id)))?;
        if ticket.kind() != TicketKind::Task {
            return Err(ApiError::invalid_request(
                "only humans complete Task criteria directly",
            ));
        }
        if request.criterion_index as usize >= ticket.completion().len() {
            return Err(ApiError::invalid_request("the criterion index is unknown"));
        }
        let binding = complete_task_criterion(request.criterion_index).map_err(refuse)?;
        self.0.bindings.save(
            request.ticket_id,
            &binding,
            envelope(request.ticket_id, "completed", json!({})),
        )?;
        encode(request.ticket_id, &binding)
    }
}

struct InvalidateCriteria(BindingContext);
impl CommandHandler for InvalidateCriteria {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<CriterionInvalidateRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }
    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }
    fn apply(&self, command: &ParsedCommand, _: &dyn CommandEffects) -> Result<Value, ApiError> {
        let request: CriterionInvalidateRequest = parse_payload(&command.payload)?;
        let mut listed = self.0.bindings.list(request.ticket_id)?;
        for binding in &mut listed {
            invalidate_on_content_change(binding, &request.observed_tip);
            self.0.bindings.save(
                request.ticket_id,
                binding,
                envelope(
                    request.ticket_id,
                    "invalidated",
                    json!({"observed_tip": request.observed_tip}),
                ),
            )?;
        }
        serde_json::to_value(CriterionBindingListResponse {
            bindings: listed
                .iter()
                .map(|binding| record_of(request.ticket_id, binding))
                .collect(),
        })
        .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

struct ListBindings(BindingContext);
impl QueryHandler for ListBindings {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query = parse_payload::<CriterionBindingListQuery>(payload)?;
        let listed = self.0.bindings.list(query.ticket_id)?;
        serde_json::to_value(CriterionBindingListResponse {
            bindings: listed
                .iter()
                .map(|binding| record_of(query.ticket_id, binding))
                .collect(),
        })
        .map_err(|error| ApiError::internal(&error.to_string()))
    }
}
