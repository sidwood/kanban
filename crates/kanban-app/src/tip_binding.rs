//! Criterion evidence binding at a reviewed code tip. Every event
//! lands on the owning Ticket's Project timeline, resolved from the
//! Ticket itself (KAN-T138-AC4, KAN-S2-US1).

use std::sync::Arc;

use kanban_domain::{
    CriterionBinding, CriterionKind, EvidenceReview, ReviewExecutionState, ReviewedContent, Ticket,
    TicketId, TicketKind, TipBindingError, attach_criterion_evidence, complete_task_criterion,
    invalidate_on_content_change, invalidate_on_criterion_replacement, refuse_spent_approval,
    require_completed_required_stage_review, review_criterion_evidence, satisfy_at_approved_tip,
};
use kanban_dto::{
    ApiError, CriterionBindingListQuery, CriterionBindingListResponse, CriterionBindingRecord,
    CriterionCompleteRequest, CriterionEvidenceAttachRequest, CriterionEvidenceReviewRequest,
    CriterionInvalidateRequest, CriterionKindDto, CriterionSatisfyRequest, EvidenceReviewDto,
    LiveEventName, ReviewExecutionRecord, ReviewExecutionStatus, TimelineEntityKind,
    TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{
    Core, LandingCriteriaReplacement, ObservedWorkspaceHead, QueryHandler, RegistrationError,
};
use crate::events::emit_catalogued;
use crate::evidence::{EvidenceFilter, EvidenceStore};
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::review_execution::ReviewExecutionStore;
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
    fn list_for_workspace(
        &self,
        workspace_id: u64,
    ) -> Result<Vec<(u64, CriterionBinding)>, ApiError>;
    fn record_voided_approval(
        &self,
        ticket_id: u64,
        review_id: u64,
        tip: &str,
    ) -> Result<(), ApiError>;
    fn approval_is_voided(&self, review_id: u64) -> Result<bool, ApiError>;
}

impl Core {
    pub fn register_criterion_bindings(
        &mut self,
        bindings: Arc<dyn CriterionBindingStore>,
        tickets: Arc<dyn TicketStore>,
        evidence: Arc<dyn EvidenceStore>,
        reviews: Arc<dyn ReviewExecutionStore>,
    ) -> Result<(), RegistrationError> {
        let context = BindingContext {
            bindings,
            tickets,
            evidence,
            reviews,
        };
        *self
            .observed_workspace_head
            .lock()
            .expect("the observed-head lock is sound") = Some(Arc::new(context.clone()));
        *self
            .landing_criteria_replacement
            .lock()
            .expect("the replacement lock is sound") = Some(Arc::new(context.clone()));
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
    reviews: Arc<dyn ReviewExecutionStore>,
}

impl BindingContext {
    /// The Ticket a criterion command addresses; its Project owns
    /// every event the command records.
    fn owning(&self, ticket_id: u64) -> Result<Ticket, ApiError> {
        self.tickets
            .find(TicketId::new(ticket_id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {ticket_id}")))
    }

    fn remember_spent_approval(&self, ticket_id: u64, tip: &str) -> Result<(), ApiError> {
        let Some(review) = self.reviews.latest_approved_for_tip(ticket_id, tip)? else {
            return Ok(());
        };
        self.bindings
            .record_voided_approval(ticket_id, review.id, tip)
    }

    fn refuse_historical_approval(
        &self,
        ticket_id: u64,
        requested_tip: Option<&str>,
    ) -> Result<(), ApiError> {
        let Some(review) = self.reviews.latest_for_ticket(ticket_id)? else {
            return Ok(());
        };
        let spent = self.bindings.approval_is_voided(review.id)?
            && requested_tip.is_none_or(|tip| tip == review.tip);
        refuse_spent_approval(spent).map_err(refuse)
    }
}

impl ObservedWorkspaceHead for BindingContext {
    fn on_observed_head(
        &self,
        _project_id: u64,
        workspace_id: u64,
        observed: &ReviewedContent,
        effects: &dyn CommandEffects,
    ) -> Result<(), ApiError> {
        for (ticket_id, mut binding) in self.bindings.list_for_workspace(workspace_id)? {
            let was_void = binding.void();
            invalidate_on_content_change(&mut binding, observed);
            if binding.void() == was_void {
                continue;
            }
            self.remember_spent_approval(ticket_id, binding.tip())?;
            let ticket = self.owning(ticket_id)?;
            self.bindings.save(
                ticket_id,
                &binding,
                envelope(&ticket, "invalidated", json!({ "observed_content": true })),
            )?;
            announce(effects, ticket_id, &binding);
        }
        Ok(())
    }
}

impl LandingCriteriaReplacement for BindingContext {
    fn on_replaced(&self, ticket: &Ticket, effects: &dyn CommandEffects) -> Result<(), ApiError> {
        let ticket_id = ticket.id().value();
        for mut binding in self.bindings.list(ticket_id)? {
            if binding.void() {
                continue;
            }
            invalidate_on_criterion_replacement(&mut binding);
            self.bindings.save(
                ticket_id,
                &binding,
                envelope(
                    ticket,
                    "invalidated",
                    json!({ "qualification_replaced": true }),
                ),
            )?;
            announce(effects, ticket_id, &binding);
        }
        Ok(())
    }
}

fn review_execution_state(review: Option<&ReviewExecutionRecord>) -> ReviewExecutionState {
    match review.map(|review| review.status) {
        None => ReviewExecutionState::Absent,
        Some(ReviewExecutionStatus::InProgress) => ReviewExecutionState::Incomplete,
        Some(ReviewExecutionStatus::Rejected) => ReviewExecutionState::Rejected,
        Some(ReviewExecutionStatus::Expired) => ReviewExecutionState::Expired,
        Some(ReviewExecutionStatus::Approved) => ReviewExecutionState::Approved,
    }
}

fn refuse(error: TipBindingError) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

fn envelope(ticket: &Ticket, action: &str, facts: Value) -> TimelineEnvelope {
    let ticket_id = ticket.id().value();
    TimelineEnvelope::project(
        ticket.project().value(),
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

/// Announce one binding this command wrote. The guard holds the
/// announcement until the mutation commits, so a refused or
/// rolled-back write says nothing; what lands carries the Ticket and
/// the criterion, which is what a surface counting progress from these
/// bindings needs to read them again (KAN-T137-AC7).
fn announce(effects: &dyn CommandEffects, ticket_id: u64, binding: &CriterionBinding) {
    emit_catalogued(
        effects,
        LiveEventName::CriterionBindingChanged,
        &record_of(ticket_id, binding),
    );
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
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: CriterionEvidenceAttachRequest = parse_payload(&command.payload)?;
        let ticket = self.0.owning(request.ticket_id)?;
        self.0
            .refuse_historical_approval(request.ticket_id, Some(&request.tip))?;
        let kind = match ticket.kind() {
            TicketKind::Task => CriterionKind::Task,
            _ => CriterionKind::Acceptance,
        };
        let count = if kind == CriterionKind::Task {
            ticket.completion().len()
        } else {
            // Same list ordinary landing counts, so a Bug qualification
            // criterion can be bound and satisfied at the source tip.
            ticket.landing_criteria().len()
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
                &ticket,
                "bound",
                json!({"evidence_id": request.evidence_id}),
            ),
        )?;
        announce(effects, request.ticket_id, &binding);
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
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: CriterionEvidenceReviewRequest = parse_payload(&command.payload)?;
        let ticket = self.0.owning(request.ticket_id)?;
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
            envelope(&ticket, "reviewed", json!({"review": request.review})),
        )?;
        announce(effects, request.ticket_id, &binding);
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
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: CriterionSatisfyRequest = parse_payload(&command.payload)?;
        let ticket = self.0.owning(request.ticket_id)?;
        let mut binding = self
            .0
            .bindings
            .find(request.ticket_id, request.criterion_index)?
            .ok_or_else(|| ApiError::not_found("criterion binding"))?;
        let review = self.0.reviews.latest_for_ticket(request.ticket_id)?;
        self.0.refuse_historical_approval(request.ticket_id, None)?;
        require_completed_required_stage_review(
            review_execution_state(review.as_ref()),
            review.as_ref().map(|review| review.tip.as_str()),
            &request.tip,
        )
        .map_err(refuse)?;
        satisfy_at_approved_tip(&mut binding, &request.tip).map_err(refuse)?;
        self.0.bindings.save(
            request.ticket_id,
            &binding,
            envelope(&ticket, "satisfied", json!({"tip": request.tip})),
        )?;
        announce(effects, request.ticket_id, &binding);
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
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: CriterionCompleteRequest = parse_payload(&command.payload)?;
        let ticket = self.0.owning(request.ticket_id)?;
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
            envelope(&ticket, "completed", json!({})),
        )?;
        announce(effects, request.ticket_id, &binding);
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
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: CriterionInvalidateRequest = parse_payload(&command.payload)?;
        let ticket = self.0.owning(request.ticket_id)?;
        let mut listed = self.0.bindings.list(request.ticket_id)?;
        let observed = ReviewedContent::clean([request.observed_tip.clone()]);
        for binding in &mut listed {
            let was_void = binding.void();
            invalidate_on_content_change(binding, &observed);
            if binding.void() && !was_void {
                self.0
                    .remember_spent_approval(request.ticket_id, binding.tip())?;
            }
            self.0.bindings.save(
                request.ticket_id,
                binding,
                envelope(
                    &ticket,
                    "invalidated",
                    json!({"observed_tip": request.observed_tip}),
                ),
            )?;
            announce(effects, request.ticket_id, binding);
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
