//! Shared per-operation authorization for authenticated run transports.
use crate::{CapabilityStore, Core, parse_payload};
use kanban_domain::CapabilityId;
use kanban_dto::{ApiError, TicketGetQuery};
use serde_json::Value;
use std::sync::Arc;

pub struct RunAuthority {
    capabilities: Arc<dyn CapabilityStore>,
    tickets: Arc<dyn crate::TicketStore>,
    runs: Arc<dyn crate::RunStore>,
}
impl RunAuthority {
    fn check_spec(
        &self,
        grant: &kanban_domain::Capability,
        spec: u64,
        number: Option<u64>,
    ) -> Result<(), ApiError> {
        let ticket = self
            .tickets
            .find(grant.scope().ticket())?
            .ok_or_else(denied)?;
        let pin = ticket.pinned_version().ok_or_else(denied)?;
        if ticket.spec().map(|id| id.value()) != Some(spec)
            || number.is_some_and(|number| number != pin)
        {
            return Err(denied());
        }
        Ok(())
    }
    pub(crate) fn project_response(
        &self,
        id: CapabilityId,
        name: &str,
        value: Value,
    ) -> Result<Value, ApiError> {
        if name != "spec.get" {
            return Ok(value);
        }
        let grant = self.capabilities.find(id)?.ok_or_else(denied)?;
        let ticket = self
            .tickets
            .find(grant.scope().ticket())?
            .ok_or_else(denied)?;
        let mut response: kanban_dto::SpecGetResponse = serde_json::from_value(value)
            .map_err(|_| ApiError::internal("invalid Spec response"))?;
        response
            .versions
            .retain(|version| Some(version.number) == ticket.pinned_version());
        response.spec.name = response
            .versions
            .first()
            .ok_or_else(denied)?
            .content
            .name
            .clone();
        serde_json::to_value(response).map_err(|_| ApiError::internal("invalid Spec response"))
    }
    pub fn operations(&self, id: CapabilityId) -> Result<Vec<String>, ApiError> {
        let grant = self.capabilities.find(id)?.ok_or_else(denied)?;
        if !grant.status().is_live() || self.runs.executing_for_request(grant.dispatch())?.is_none()
        {
            return Err(denied());
        }
        Ok(grant
            .operations()
            .iter()
            .filter(|name| role_allows(grant.scope().role(), name))
            .map(str::to_owned)
            .collect())
    }
    fn ticket_target(
        &self,
        grant: &kanban_domain::Capability,
        project: u64,
        kind: &str,
        id: &str,
    ) -> Result<(), ApiError> {
        let ticket = self
            .tickets
            .find(grant.scope().ticket())?
            .ok_or_else(denied)?;
        if project != ticket.project().value() || kind != "ticket" || id != ticket.id().to_string()
        {
            return Err(denied());
        }
        Ok(())
    }
    pub fn new(
        capabilities: Arc<dyn CapabilityStore>,
        tickets: Arc<dyn crate::TicketStore>,
        runs: Arc<dyn crate::RunStore>,
    ) -> Self {
        Self {
            capabilities,
            tickets,
            runs,
        }
    }
    pub fn authorize(&self, id: CapabilityId, name: &str, payload: &Value) -> Result<(), ApiError> {
        let grant = self.capabilities.find(id)?.ok_or_else(denied)?;
        grant.permits(name).map_err(|_| denied())?;
        if !role_allows(grant.scope().role(), name) {
            return Err(denied());
        }
        let run = self
            .runs
            .executing_for_request(grant.dispatch())?
            .ok_or_else(denied)?;
        if run.ticket() != grant.scope().ticket() {
            return Err(denied());
        }
        match name {
            "health.get" => {
                parse_payload::<kanban_dto::HealthQuery>(payload)?;
            }
            "ticket.get" => {
                let request: TicketGetQuery = parse_payload(payload)?;
                if request.ticket_id != grant.scope().ticket().value() {
                    return Err(denied());
                }
            }
            "comment.create" => {
                let request: kanban_dto::CommentCreateRequest = parse_payload(payload)?;
                let ticket = self
                    .tickets
                    .find(grant.scope().ticket())?
                    .ok_or_else(denied)?;
                if request.project_id != ticket.project().value()
                    || request.target.kind != kanban_dto::TimelineEntityKind::Ticket
                    || request.target.id != ticket.id().to_string()
                {
                    return Err(denied());
                }
            }
            "evidence.attach" => {
                let request: kanban_dto::EvidenceAttachRequest = parse_payload(payload)?;
                self.ticket_target(
                    &grant,
                    request.project_id,
                    &request.entity_kind,
                    &request.entity_id,
                )?;
            }
            "evidence.list" => {
                let request: kanban_dto::EvidenceListQuery = parse_payload(payload)?;
                self.ticket_target(
                    &grant,
                    request.project_id,
                    request.entity_kind.as_deref().ok_or_else(denied)?,
                    request.entity_id.as_deref().ok_or_else(denied)?,
                )?;
            }
            "submission.submit" => {
                let request: kanban_dto::SubmissionSubmitRequest = parse_payload(payload)?;
                if request.capability_id != id.value() || request.run_id != run.id().value() {
                    return Err(denied());
                }
                match (&request.result, grant.scope().role()) {
                    (
                        kanban_dto::SubmissionResult::Implementation { .. },
                        kanban_domain::CapabilityRole::Implementer,
                    )
                    | (
                        kanban_dto::SubmissionResult::Review { .. },
                        kanban_domain::CapabilityRole::Reviewer,
                    ) => (),
                    _ => return Err(denied()),
                }
            }
            "timeline.query" => {
                let request: kanban_dto::TimelineQuery = parse_payload(payload)?;
                let entity = request.entity.as_ref().ok_or_else(denied)?;
                let project = request.scope.project_id().ok_or_else(denied)?;
                self.ticket_target(&grant, project, entity.kind.as_str(), &entity.id)?;
            }
            "criterion.evidence.attach" => {
                let request: kanban_dto::CriterionEvidenceAttachRequest = parse_payload(payload)?;
                if request.ticket_id != grant.scope().ticket().value() {
                    return Err(denied());
                }
            }
            "criterion.evidence.review" => {
                let request: kanban_dto::CriterionEvidenceReviewRequest = parse_payload(payload)?;
                if request.ticket_id != grant.scope().ticket().value()
                    || grant.scope().reviewer_slot().is_none()
                {
                    return Err(denied());
                }
            }
            "criterion.satisfy" => {
                let request: kanban_dto::CriterionSatisfyRequest = parse_payload(payload)?;
                if request.ticket_id != grant.scope().ticket().value()
                    || grant.scope().reviewer_slot().is_none()
                {
                    return Err(denied());
                }
            }
            "spec.get" => {
                let request: kanban_dto::SpecGetQuery = parse_payload(payload)?;
                self.check_spec(&grant, request.spec_id, None)?;
            }
            "spec.version.get" => {
                let request: kanban_dto::SpecVersionGetQuery = parse_payload(payload)?;
                self.check_spec(&grant, request.spec_id, Some(request.number))?;
            }
            _ => return Err(denied()),
        }
        Ok(())
    }
}
impl Core {
    pub fn agent_operations(&self, id: CapabilityId) -> Result<Vec<String>, ApiError> {
        self.agent_authority
            .as_ref()
            .ok_or_else(denied)?
            .operations(id)
    }
    /// The service installs this once; clients never supply their own authority.
    pub fn register_agent_authority(&mut self, authority: Arc<RunAuthority>) {
        self.agent_authority = Some(authority);
    }
}
fn denied() -> ApiError {
    ApiError::invalid_request("operation is outside the authenticated run capability")
}

/// A service-created binding; no request can replace its capability identity.
pub struct AgentSession {
    core: Arc<Core>,
    capability: CapabilityId,
}
impl AgentSession {
    pub fn new(core: Arc<Core>, capability: CapabilityId) -> Result<Self, ApiError> {
        core.agent_operations(capability)?;
        Ok(Self { core, capability })
    }
    pub fn operations(&self) -> Result<Vec<String>, ApiError> {
        self.core.agent_operations(self.capability)
    }
    pub fn call(&self, name: &str, payload: &Value) -> Result<Value, ApiError> {
        let operation = crate::catalog::exposed_operations()
            .iter()
            .find(|op| op.name == name)
            .ok_or_else(denied)?;
        match operation.kind {
            crate::catalog::OperationKind::Query => {
                self.core.agent_query(self.capability, name, payload)
            }
            crate::catalog::OperationKind::Command => {
                self.core.agent_command(self.capability, name, payload)
            }
        }
    }
}

fn role_allows(role: kanban_domain::CapabilityRole, name: &str) -> bool {
    match name {
        "criterion.evidence.attach" => role == kanban_domain::CapabilityRole::Implementer,
        "criterion.evidence.review" | "criterion.satisfy" => {
            role == kanban_domain::CapabilityRole::Reviewer
        }
        _ => true,
    }
}
