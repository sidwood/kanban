//! Structured finding admission and governance.
use kanban_dto::{ApiError, ReviewFindingRecord};

pub fn validate_findings(findings: &[ReviewFindingRecord]) -> Result<(), ApiError> {
    for finding in findings {
        kanban_domain::finding::validate_finding_details(
            &finding.summary,
            &finding.evidence,
            &finding.location,
            &finding.proposed_resolution,
        )
        .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
    }
    Ok(())
}

pub fn blocks_approval(finding: &ReviewFindingRecord) -> bool {
    use kanban_domain::finding::FindingSeverity as Domain;
    let severity = match finding.severity {
        kanban_dto::FindingSeverity::P0 => Domain::P0,
        kanban_dto::FindingSeverity::P1 => Domain::P1,
        kanban_dto::FindingSeverity::P2 => Domain::P2,
        kanban_dto::FindingSeverity::P3 => Domain::P3,
    };
    kanban_domain::finding::finding_blocks(severity, finding.in_scope)
}

pub fn validate_review_verdict(
    approve: bool,
    findings: &[ReviewFindingRecord],
    counts_for_resolution: bool,
) -> Result<(), ApiError> {
    validate_findings(findings)?;
    kanban_domain::finding::validate_finding_verdict(
        approve,
        findings.iter().any(blocks_approval),
        counts_for_resolution,
    )
    .map_err(|error| ApiError::invalid_request(&error.to_string()))
}

use crate::{Core, ProjectStore, QueryHandler, RegistrationError, parse_payload};
use kanban_domain::{ProjectId, finding::FindingIdentity};
use kanban_dto::{FindingGetQuery, FindingListQuery, FindingListResponse, FindingRecord};
use serde_json::Value;
use std::sync::Arc;

pub trait FindingStore: Send + Sync {
    fn promotion(
        &self,
        project_id: u64,
        id: FindingIdentity,
    ) -> Result<Option<kanban_dto::DeferralPromotionRecord>, ApiError>;
    fn record_promotion(
        &self,
        record: &kanban_dto::DeferralPromotionRecord,
        envelope: crate::TimelineEnvelope,
    ) -> Result<(), ApiError>;
    fn list(&self, query: &FindingListQuery) -> Result<Vec<FindingRecord>, ApiError>;
    fn find(&self, project_id: u64, id: FindingIdentity)
    -> Result<Option<FindingRecord>, ApiError>;
}

impl Core {
    pub fn register_findings(
        &mut self,
        findings: Arc<dyn FindingStore>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), RegistrationError> {
        self.register_query(
            "finding.list",
            Arc::new(ListFindings {
                findings: findings.clone(),
                projects: projects.clone(),
            }),
        )?;
        self.register_query("finding.get", Arc::new(GetFinding { findings, projects }))
    }
}
struct ListFindings {
    findings: Arc<dyn FindingStore>,
    projects: Arc<dyn ProjectStore>,
}
impl QueryHandler for ListFindings {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: FindingListQuery = parse_payload(payload)?;
        self.projects
            .find(ProjectId::new(query.project_id))?
            .ok_or_else(|| ApiError::not_found("project"))?;
        serde_json::to_value(FindingListResponse {
            project_id: query.project_id,
            findings: self.findings.list(&query)?,
        })
        .map_err(|e| ApiError::internal(&e.to_string()))
    }
}
struct GetFinding {
    findings: Arc<dyn FindingStore>,
    projects: Arc<dyn ProjectStore>,
}
impl QueryHandler for GetFinding {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: FindingGetQuery = parse_payload(payload)?;
        let id = FindingIdentity::parse(&query.finding_id)
            .map_err(|e| ApiError::invalid_request(&e.to_string()))?;
        self.projects
            .find(ProjectId::new(query.project_id))?
            .ok_or_else(|| ApiError::not_found("project"))?;
        let finding = self
            .findings
            .find(query.project_id, id)?
            .ok_or_else(|| ApiError::not_found("finding"))?;
        serde_json::to_value(finding).map_err(|e| ApiError::internal(&e.to_string()))
    }
}

use crate::{CommandEffects, CommandHandler, DeferralStore, ParsedCommand, SpecStore, TicketStore};
use kanban_dto::{
    DeferralPromoteRequest, DeferralPromoteResponse, DeferralPromotionRecord,
    DeferralPromotionTarget, TicketCreateRequest, TicketKind, TicketRecord, TimelineEntityKind,
    TimelineEntityRef, TimelineEventKind,
};

impl Core {
    pub fn register_deferral_promotions(
        &mut self,
        findings: Arc<dyn FindingStore>,
        deferrals: Arc<dyn DeferralStore>,
        projects: Arc<dyn ProjectStore>,
        tickets: Arc<dyn TicketStore>,
        specs: Arc<dyn SpecStore>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "deferral.promote",
            Arc::new(PromoteDeferral {
                findings,
                deferrals,
                projects,
                tickets,
                specs,
            }),
        )
    }
}
struct PromoteDeferral {
    findings: Arc<dyn FindingStore>,
    deferrals: Arc<dyn DeferralStore>,
    projects: Arc<dyn ProjectStore>,
    tickets: Arc<dyn TicketStore>,
    specs: Arc<dyn SpecStore>,
}
impl CommandHandler for PromoteDeferral {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<DeferralPromoteRequest>(payload)?;
        ParsedCommand::lift("finding_promotion", payload)
    }
    fn current_version(&self, _: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: DeferralPromoteRequest = parse_payload(&command.payload)?;
        let project = self
            .projects
            .find(ProjectId::new(request.project_id))?
            .ok_or_else(|| ApiError::not_found("project"))?;
        if project.is_archived() {
            return Err(ApiError::invalid_request(
                "archived projects cannot promote findings",
            ));
        }
        let deferred = self
            .deferrals
            .find(
                request.project_id,
                kanban_domain::DeferralId::new(request.deferral_id),
            )?
            .ok_or_else(|| ApiError::not_found("deferral"))?;
        if self
            .deferrals
            .has_successor(request.project_id, deferred.id())?
        {
            return Err(ApiError::invalid_request(
                "only the current deferral may be promoted",
            ));
        }
        let identity = FindingIdentity::parse(deferred.finding_id())
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let finding = self
            .findings
            .find(request.project_id, identity)?
            .ok_or_else(|| ApiError::not_found("finding"))?;
        if self
            .findings
            .promotion(request.project_id, identity)?
            .is_some()
        {
            return Err(ApiError::invalid_request("the finding is already promoted"));
        }
        let create = promoted_ticket_request(&request, &finding);
        let value = crate::ticket::create_ticket(
            &create,
            self.projects.as_ref(),
            self.tickets.as_ref(),
            self.specs.as_ref(),
            effects,
        )?;
        let ticket: TicketRecord = serde_json::from_value(value)
            .map_err(|error| ApiError::internal(&error.to_string()))?;
        let promotion = DeferralPromotionRecord {
            project_id: request.project_id,
            deferral_id: deferred.id().value(),
            finding_id: finding.id.clone(),
            ticket_id: ticket.id,
        };
        self.findings.record_promotion(&promotion,crate::TimelineEnvelope::project(request.project_id,TimelineEventKind::Deferral,
            Some(TimelineEntityRef{kind:TimelineEntityKind::Ticket,id:ticket.id.to_string()}),
            serde_json::json!({"action":"finding_promoted","deferral_id":promotion.deferral_id,"finding_id":promotion.finding_id,"ticket_id":promotion.ticket_id})))?;
        serde_json::to_value(DeferralPromoteResponse { promotion, ticket })
            .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

fn promoted_ticket_request(
    request: &DeferralPromoteRequest,
    record: &FindingRecord,
) -> TicketCreateRequest {
    let (kind, actual_behaviour, reporter_evidence, subtype, mode, completion) =
        match &request.target {
            DeferralPromotionTarget::Bug {} => (
                TicketKind::Bug,
                Some(record.finding.summary.clone()),
                Some(format!(
                    "{}\n\nLocation: {}\nSource finding: {} at {}",
                    record.finding.evidence, record.finding.location, record.id, record.tip
                )),
                None,
                None,
                None,
            ),
            DeferralPromotionTarget::Task {
                subtype,
                mode,
                completion,
            } => (
                TicketKind::Task,
                None,
                None,
                Some(*subtype),
                Some(*mode),
                Some(completion.clone()),
            ),
        };
    TicketCreateRequest {
        mutation: request.mutation.clone(),
        project_id: request.project_id,
        kind,
        priority: request.priority,
        spec_id: None,
        title: Some(record.finding.summary.clone()),
        actual_behaviour,
        reporter_evidence,
        slice: None,
        criteria: None,
        subtype,
        mode,
        completion,
        scheduled_for: None,
        due: None,
    }
}
