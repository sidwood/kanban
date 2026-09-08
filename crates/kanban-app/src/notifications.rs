//! Notification policy is separate from acknowledgement authority.
use crate::dispatch::RegistrationError;
use crate::mutation::parse_payload;
use crate::{
    CommandEffects, CommandHandler, Core, ParsedCommand, ProjectStore, QueryHandler,
    TimelineEnvelope,
};
use kanban_dto::{
    ApiError, NotificationSettingsQuery, NotificationSettingsRecord,
    NotificationSettingsUpdateRequest, TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};
use std::sync::Arc;

pub trait NotificationSettingsStore: Send + Sync {
    fn get(&self, project_id: u64) -> Result<NotificationSettingsRecord, ApiError>;
    fn update(
        &self,
        request: &NotificationSettingsUpdateRequest,
        envelope: TimelineEnvelope,
    ) -> Result<NotificationSettingsRecord, ApiError>;
}
#[derive(Clone)]
struct Settings {
    store: Arc<dyn NotificationSettingsStore>,
    projects: Arc<dyn ProjectStore>,
}
impl Core {
    pub fn register_notification_settings(
        &mut self,
        store: Arc<dyn NotificationSettingsStore>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), RegistrationError> {
        let settings = Settings { store, projects };
        self.register_query("notification.settings.get", Arc::new(settings.clone()))?;
        self.register_command("notification.settings.update", Arc::new(settings))
    }
}
impl Settings {
    fn project(&self, id: u64) -> Result<kanban_domain::Project, ApiError> {
        self.projects
            .find(kanban_domain::ProjectId::new(id))?
            .ok_or_else(|| ApiError::not_found("project"))
    }
}
impl QueryHandler for Settings {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: NotificationSettingsQuery = parse_payload(payload)?;
        self.project(query.project_id)?;
        serde_json::to_value(self.store.get(query.project_id)?).map_err(internal)
    }
}
impl CommandHandler for Settings {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        let request: NotificationSettingsUpdateRequest = parse_payload(payload)?;
        if request
            .mirror_role
            .as_ref()
            .is_some_and(|role| role.trim().is_empty() || role.len() > 128)
        {
            return Err(ApiError::invalid_request(
                "a mirror role must be a non-blank name of at most 128 bytes, or null to disable mirrors",
            ));
        }
        ParsedCommand::lift("notification_settings", payload)
    }
    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: NotificationSettingsUpdateRequest = parse_payload(&command.payload)?;
        self.project(request.project_id)?;
        Ok(self.store.get(request.project_id)?.version)
    }
    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: NotificationSettingsUpdateRequest = parse_payload(&command.payload)?;
        if self.project(request.project_id)?.is_archived() {
            return Err(ApiError::invalid_request(
                "an archived Project cannot change notification policy",
            ));
        }
        let envelope = TimelineEnvelope::project(
            request.project_id,
            TimelineEventKind::Transition,
            Some(TimelineEntityRef {
                kind: TimelineEntityKind::Project,
                id: request.project_id.to_string(),
            }),
            json!({"action":"notification_settings_updated","local_enabled":request.local_enabled,"mirror_role":request.mirror_role}),
        );
        serde_json::to_value(self.store.update(&request, envelope)?).map_err(internal)
    }
}
fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&error.to_string())
}

#[derive(Debug, Clone)]
pub struct NotificationMessage {
    pub delivery_id: u64,
    pub target: kanban_dto::NotificationTarget,
    pub title: String,
    pub body: String,
}
#[derive(Debug, Clone)]
pub enum NotificationOutcome {
    Submitted { receipt: String },
    NotSent { reason: String },
    Uncertain { reason: String },
}
/// This port can deliver information, but it has no acknowledgement operation.
pub trait NotificationSink: Send + Sync {
    fn deliver(&self, message: &NotificationMessage) -> NotificationOutcome;
}
pub struct NotificationDeliveryDraft {
    pub project_id: u64,
    pub item_id: String,
    pub item_version: u64,
    pub settings_version: u64,
    pub channel: kanban_dto::NotificationChannel,
    pub target: kanban_dto::NotificationTarget,
}
pub trait NotificationDeliveryStore: Send + Sync {
    fn get(&self, id: u64) -> Result<kanban_dto::NotificationDeliveryRecord, ApiError>;
    fn list(
        &self,
        project_id: u64,
    ) -> Result<Vec<kanban_dto::NotificationDeliveryRecord>, ApiError>;
    fn retry(
        &self,
        request: &kanban_dto::NotificationRetryRequest,
    ) -> Result<kanban_dto::NotificationDeliveryRecord, ApiError>;

    fn prepare(
        &self,
        draft: &NotificationDeliveryDraft,
        at: &str,
    ) -> Result<Option<kanban_dto::NotificationDeliveryRecord>, ApiError>;
    fn finish(&self, id: u64, outcome: &NotificationOutcome) -> Result<(), ApiError>;
}
#[derive(Debug, Default)]
pub struct NotificationReport {
    pub submitted: usize,
    pub failed: usize,
    pub uncertain: usize,
}

pub struct NotificationDispatcher {
    inbox: Arc<dyn crate::attention::AttentionStore>,
    settings: Arc<dyn NotificationSettingsStore>,
    deliveries: Arc<dyn NotificationDeliveryStore>,
    projects: Arc<dyn ProjectStore>,
    sink: Arc<dyn NotificationSink>,
}
impl NotificationDispatcher {
    pub fn new(
        inbox: Arc<dyn crate::attention::AttentionStore>,
        settings: Arc<dyn NotificationSettingsStore>,
        deliveries: Arc<dyn NotificationDeliveryStore>,
        projects: Arc<dyn ProjectStore>,
        sink: Arc<dyn NotificationSink>,
    ) -> Self {
        Self {
            inbox,
            settings,
            deliveries,
            projects,
            sink,
        }
    }
    pub fn dispatch_once(&self, at: &str) -> Result<NotificationReport, ApiError> {
        use kanban_dto::{AttentionListQuery, NotificationChannel, NotificationTarget};
        let mut report = NotificationReport::default();
        for item in self.inbox.list(&AttentionListQuery::default())?.items {
            let Some(project) = self
                .projects
                .find(kanban_domain::ProjectId::new(item.project_id))?
            else {
                continue;
            };
            if project.is_archived() {
                continue;
            }
            let settings = self.settings.get(item.project_id)?;
            let mut targets = Vec::new();
            if settings.local_enabled {
                targets.push((NotificationChannel::Local, NotificationTarget::Local {}));
            }
            if let Some(role) = &settings.mirror_role {
                let registration = project.registration();
                targets.push((
                    NotificationChannel::HerdrMirror,
                    NotificationTarget::HerdrMirror {
                        session_name: registration.herdr_session().map(str::to_owned),
                        product_workspace: registration.seed_workspace().to_owned(),
                        herdr_workspace: registration.herdr_workspace().to_owned(),
                        role: role.clone(),
                    },
                ));
            }
            for (channel, target) in targets {
                let draft = NotificationDeliveryDraft {
                    project_id: item.project_id,
                    item_id: item.id.clone(),
                    item_version: item.version,
                    settings_version: settings.version,
                    channel,
                    target,
                };
                let Some(delivery) = self.deliveries.prepare(&draft, at)? else {
                    continue;
                };
                let message = NotificationMessage {
                    delivery_id: delivery.id,
                    target: delivery.target,
                    title: "Kanban attention".to_owned(),
                    body: format!(
                        "Project {}: {} needs operator attention. Open the Kanban Inbox. Informational delivery #{}; no workflow action requested.",
                        project.code(),
                        item.kind.wire_name(),
                        delivery.id
                    ),
                };
                let outcome = self.sink.deliver(&message);
                self.deliveries.finish(delivery.id, &outcome)?;
                match outcome {
                    NotificationOutcome::Submitted { .. } => report.submitted += 1,
                    NotificationOutcome::NotSent { .. } => report.failed += 1,
                    NotificationOutcome::Uncertain { .. } => report.uncertain += 1,
                }
            }
        }
        Ok(report)
    }
}

/// Platform consent is observable, but this port cannot acknowledge an Inbox item.
pub trait NotificationPermissionPort: Send + Sync {
    fn status(&self) -> kanban_dto::NotificationPermissionRecord;
    /// Queue an explicit OS permission request; do not block the Core on a dialog.
    fn request(&self) -> Result<bool, ApiError>;
}
impl Core {
    pub fn register_notification_permissions(
        &mut self,
        port: Arc<dyn NotificationPermissionPort>,
    ) -> Result<(), RegistrationError> {
        self.register_query(
            "notification.permission.get",
            Arc::new(Permission(port.clone())),
        )?;
        self.register_command(
            "notification.permission.request",
            Arc::new(Permission(port)),
        )
    }
}
struct Permission(Arc<dyn NotificationPermissionPort>);
impl QueryHandler for Permission {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        parse_payload::<kanban_dto::NotificationPermissionQuery>(payload)?;
        serde_json::to_value(self.0.status()).map_err(internal)
    }
}
impl CommandHandler for Permission {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<kanban_dto::NotificationPermissionRequest>(payload)?;
        ParsedCommand::lift("notification_permission", payload)
    }
    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }
    fn apply(
        &self,
        _command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        use kanban_dto::NotificationPermissionState;
        let status = self.0.status();
        let accepted = match status.state {
            NotificationPermissionState::Granted => false,
            NotificationPermissionState::Denied => {
                return Err(ApiError::invalid_request(
                    "notification permission was denied; change it in macOS System Settings",
                ));
            }
            NotificationPermissionState::Unavailable => {
                return Err(ApiError::invalid_request(
                    status
                        .reason
                        .as_deref()
                        .unwrap_or("native notifications are unavailable"),
                ));
            }
            NotificationPermissionState::NotDetermined => {
                let port = self.0.clone();
                effects.after_commit(Box::new(move || {
                    let _ = port.request();
                }));
                true
            }
        };
        serde_json::to_value(kanban_dto::NotificationPermissionResponse { accepted })
            .map_err(internal)
    }
}

#[derive(Clone)]
struct Deliveries {
    store: Arc<dyn NotificationDeliveryStore>,
    projects: Arc<dyn ProjectStore>,
}
impl Core {
    pub fn register_notification_deliveries(
        &mut self,
        store: Arc<dyn NotificationDeliveryStore>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), RegistrationError> {
        let context = Deliveries { store, projects };
        self.register_query("notification.deliveries", Arc::new(context.clone()))?;
        self.register_command("notification.retry", Arc::new(context))
    }
}
impl QueryHandler for Deliveries {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: kanban_dto::NotificationDeliveriesQuery = parse_payload(payload)?;
        self.projects
            .find(kanban_domain::ProjectId::new(query.project_id))?
            .ok_or_else(|| ApiError::not_found("project"))?;
        serde_json::to_value(kanban_dto::NotificationDeliveriesResponse {
            deliveries: self.store.list(query.project_id)?,
        })
        .map_err(internal)
    }
}
impl CommandHandler for Deliveries {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<kanban_dto::NotificationRetryRequest>(payload)?;
        ParsedCommand::lift("notification_delivery", payload)
    }
    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: kanban_dto::NotificationRetryRequest = parse_payload(&command.payload)?;
        Ok(self.store.get(request.delivery_id)?.version)
    }
    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: kanban_dto::NotificationRetryRequest = parse_payload(&command.payload)?;
        serde_json::to_value(self.store.retry(&request)?).map_err(internal)
    }
}
