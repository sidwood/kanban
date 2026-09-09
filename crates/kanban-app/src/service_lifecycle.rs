use crate::{QueryHandler, parse_payload};
use kanban_dto::{ApiError, HealthResponse, ServiceStopWarning, ServiceStopWarningQuery};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::{CommandEffects, CommandHandler, ParsedCommand};
use kanban_dto::{ServiceStopRequest, ServiceStopResponse, ServiceStopStatus};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub struct StopControl(AtomicBool);
impl StopControl {
    pub fn requested(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub trait LoginLaunchPort: Send + Sync {
    fn enabled(&self) -> Result<bool, ApiError>;
    fn set_enabled(&self, enabled: bool) -> Result<(), ApiError>;
}

pub struct StopWarningHandler {
    pub health: Arc<dyn QueryHandler>,
    pub control: Arc<StopControl>,
}
impl StopWarningHandler {
    fn current(&self) -> Result<ServiceStopWarning, ApiError> {
        let health: HealthResponse = parse_payload(&self.health.handle(&serde_json::json!({}))?)?;
        Ok(compose_stop_warning(
            &health,
            u64::from(self.control.requested()),
        ))
    }
}
impl CommandHandler for StopWarningHandler {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        let request: ServiceStopRequest = parse_payload(payload)?;
        if !request.confirmed {
            return Err(ApiError::invalid_request(
                "deliberate stop confirmation is required",
            ));
        }
        ParsedCommand::lift("service_lifecycle", payload)
    }
    fn current_version(&self, _: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(u64::from(self.control.requested()))
    }
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: ServiceStopRequest = parse_payload(&command.payload)?;
        let warning = self.current()?;
        if request.instance_id != warning.instance_id || request.warning_id != warning.warning_id {
            return Err(ApiError::invalid_request(
                "the stop warning is stale; review the current capabilities before confirming again",
            ));
        }
        let control = self.control.clone();
        effects.after_commit(Box::new(move || control.0.store(true, Ordering::Release)));
        serde_json::to_value(ServiceStopResponse {
            instance_id: warning.instance_id,
            status: ServiceStopStatus::StopRequested,
        })
        .map_err(|e| ApiError::internal(&e.to_string()))
    }
}
impl QueryHandler for StopWarningHandler {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        parse_payload::<ServiceStopWarningQuery>(payload)?;
        serde_json::to_value(self.current()?).map_err(|e| ApiError::internal(&e.to_string()))
    }
}

pub fn compose_stop_warning(health: &HealthResponse, version: u64) -> ServiceStopWarning {
    let mut capabilities = vec![
        "Application commands, database access, and live updates".to_owned(),
        "Schedule activation, daily backups, Attention Items, and notifications".to_owned(),
    ];
    if health.mcp.exposed_tools > 0 {
        capabilities.push(format!(
            "MCP access to {} application operations and active adapter connections",
            health.mcp.exposed_tools
        ));
    }
    if !health.herdr.sessions.is_empty() {
        let connected = health
            .herdr
            .sessions
            .iter()
            .filter(|session| session.diagnostics.connected)
            .count();
        capabilities.push(format!(
            "Herdr observation and reconnection for {} Projects ({} connected)",
            health.herdr.sessions.len(),
            connected
        ));
    }
    let census = &health.workspaces.by_health;
    let workspaces = u64::from(census.available)
        + u64::from(census.assigned)
        + u64::from(census.dirty)
        + u64::from(census.missing)
        + u64::from(census.retired)
        + u64::from(census.unobserved);
    if workspaces > 0 {
        capabilities.push(format!(
            "Health reporting for {workspaces} registered Workspaces"
        ));
    }
    let instance_id = health.service.started_at.clone();
    let warning_id = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(&instance_id, version, &capabilities))
                .expect("warning values serialise")
        )
    );
    ServiceStopWarning {
        instance_id,
        version,
        warning_id,
        capabilities,
    }
}
