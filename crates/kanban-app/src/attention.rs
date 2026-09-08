//! Materialize source facts as attention, never as workflow verdicts.
use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::mutation::parse_payload;
use kanban_dto::{
    ApiError, AttentionListQuery, AttentionListResponse, AttentionState, AttentionSubjectKind,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxSignal {
    pub project_id: u64,
    pub kind: AttentionState,
    pub subject_kind: AttentionSubjectKind,
    pub subject_id: String,
    pub revision: String,
    pub summary: String,
    pub detail: Value,
}
impl InboxSignal {
    pub fn key(&self) -> Result<String, ApiError> {
        serde_json::to_string(&(
            self.project_id,
            self.kind,
            self.subject_kind,
            &self.subject_id,
        ))
        .map_err(internal)
    }
}

pub trait AttentionSource: Send + Sync {
    fn snapshot(&self) -> Result<Vec<InboxSignal>, ApiError>;
}
pub trait AttentionStore: Send + Sync {
    fn get(&self, id: &str) -> Result<kanban_dto::AttentionItemRecord, ApiError>;
    fn acknowledge(
        &self,
        request: &kanban_dto::AttentionAcknowledgeRequest,
        envelope: &dyn Fn(&kanban_dto::AttentionItemRecord) -> crate::TimelineEnvelope,
    ) -> Result<kanban_dto::AttentionItemRecord, ApiError>;

    fn reconcile(&self, signals: &[InboxSignal], observed_at: &str) -> Result<(), ApiError>;
    fn list(&self, query: &AttentionListQuery) -> Result<AttentionListResponse, ApiError>;
}

pub struct AttentionProjector {
    source: Arc<dyn AttentionSource>,
    store: Arc<dyn AttentionStore>,
}
impl AttentionProjector {
    pub fn new(source: Arc<dyn AttentionSource>, store: Arc<dyn AttentionStore>) -> Self {
        Self { source, store }
    }
    pub fn refresh(&self, observed_at: &str) -> Result<(), ApiError> {
        time::OffsetDateTime::parse(observed_at, &time::format_description::well_known::Rfc3339)
            .map_err(|_| {
                ApiError::invalid_request("attention observation time must be RFC 3339")
            })?;
        let signals = consolidate(self.source.snapshot()?)?;
        self.store.reconcile(&signals, observed_at)
    }
}

fn consolidate(signals: Vec<InboxSignal>) -> Result<Vec<InboxSignal>, ApiError> {
    let mut groups: BTreeMap<String, Vec<InboxSignal>> = BTreeMap::new();
    for signal in signals {
        groups.entry(signal.key()?).or_default().push(signal);
    }
    let mut result = Vec::new();
    for mut group in groups.into_values() {
        group.sort_by(|a, b| (&a.revision, &a.summary).cmp(&(&b.revision, &b.summary)));
        group.dedup();
        let mut signal = group[0].clone();
        let revisions: BTreeSet<_> = group.iter().map(|s| (&s.revision, &s.summary)).collect();
        signal.revision = serde_json::to_string(&revisions).map_err(internal)?;
        signal.summary = group
            .iter()
            .map(|s| s.summary.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        signal.detail = json!({"sources":group.iter().map(|s|&s.detail).collect::<Vec<_>>()});
        result.push(signal);
    }
    Ok(result)
}

impl Core {
    pub fn register_attention(
        &mut self,
        store: Arc<dyn AttentionStore>,
    ) -> Result<(), RegistrationError> {
        self.register_command(
            "attention.acknowledge",
            Arc::new(AcknowledgeAttention(store.clone())),
        )?;
        self.register_query("attention.list", Arc::new(ListAttention(store)))
    }
}
struct ListAttention(Arc<dyn AttentionStore>);
impl QueryHandler for ListAttention {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: AttentionListQuery = parse_payload(payload)?;
        serde_json::to_value(self.0.list(&query)?).map_err(internal)
    }
}
fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&error.to_string())
}

struct AcknowledgeAttention(Arc<dyn AttentionStore>);
impl crate::CommandHandler for AcknowledgeAttention {
    fn parse(&self, payload: &Value) -> Result<crate::ParsedCommand, ApiError> {
        let request: kanban_dto::AttentionAcknowledgeRequest = parse_payload(payload)?;
        if request.who.trim().is_empty() {
            return Err(ApiError::invalid_request(
                "name the operator acknowledging this item",
            ));
        }
        crate::ParsedCommand::lift("attention", payload)
    }
    fn current_version(&self, command: &crate::ParsedCommand) -> Result<u64, ApiError> {
        let request: kanban_dto::AttentionAcknowledgeRequest = parse_payload(&command.payload)?;
        Ok(self.0.get(&request.item_id)?.version)
    }
    fn apply(
        &self,
        command: &crate::ParsedCommand,
        _effects: &dyn crate::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: kanban_dto::AttentionAcknowledgeRequest = parse_payload(&command.payload)?;
        let record = self.0.acknowledge(&request, &|record| {
            crate::TimelineEnvelope::project(
                record.project_id,
                kanban_dto::TimelineEventKind::Transition,
                Some(kanban_dto::TimelineEntityRef {
                    kind: kanban_dto::TimelineEntityKind::AttentionItem,
                    id: record.id.clone(),
                }),
                json!({"action":"attention_acknowledged","item_id":record.id,
                "who":record.acknowledged_by,"at":record.acknowledged_at,"version":record.version}),
            )
        })?;
        serde_json::to_value(record).map_err(internal)
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeAttentionSnapshot {
    pub diagnostics: kanban_dto::HerdrConnectionDiagnostics,
    pub signals: Vec<crate::AttentionSignal>,
}
pub trait RuntimeAttentionFeed: Send + Sync {
    fn snapshot(
        &self,
        project: &kanban_domain::Project,
    ) -> Result<RuntimeAttentionSnapshot, ApiError>;
}

pub fn runtime_attention(
    project: &kanban_domain::Project,
    snapshot: &RuntimeAttentionSnapshot,
    submissions: &dyn crate::submission::SubmissionStore,
) -> Result<Vec<InboxSignal>, ApiError> {
    let mut signals = Vec::new();
    let diagnostics = &snapshot.diagnostics;
    if !diagnostics.connected
        && (diagnostics.last_error.is_some() || diagnostics.last_snapshot_at.is_some())
    {
        let registration = project.registration();
        let revision = serde_json::to_string(&(
            registration.herdr_session(),
            registration.herdr_workspace(),
            registration.seed_workspace(),
            &diagnostics.last_snapshot_at,
        ))
        .map_err(internal)?;
        signals.push(InboxSignal {project_id:project.id().value(),kind:AttentionState::DisconnectedSession,
            subject_kind:AttentionSubjectKind::Project,subject_id:project.id().value().to_string(),revision,
            summary:"Herdr session is disconnected for this Project.".to_owned(),
            detail:json!({"source":"herdr_connection","session_name":registration.herdr_session(),
                "herdr_workspace":registration.herdr_workspace(),"product_workspace":registration.seed_workspace(),
                "last_snapshot_at":diagnostics.last_snapshot_at,"last_error":diagnostics.last_error})});
    }
    let submitted: BTreeSet<_> = submissions
        .list(project.id().value())?
        .into_iter()
        .map(|s| s.run_id)
        .collect();
    for signal in &snapshot.signals {
        if signal.project_id != project.id().value() {
            continue;
        }
        if signal.reason == crate::deadlines::STALL_DEADLINE_REASON {
            let Some(anchor) = signal.detail["last_activity_unix_secs"].as_u64() else {
                continue;
            };
            if let Some(run) = signal.detail["run_id"].as_u64() {
                if submitted.contains(&run) {
                    continue;
                }
                let context = match submissions.context(run) {
                    Ok(context) => context,
                    Err(error) if error.code == kanban_dto::ErrorCode::NotFound => continue,
                    Err(error) => return Err(error),
                };
                if context.project_id != project.id().value() {
                    continue;
                }
                signals.push(InboxSignal {project_id:project.id().value(),kind:AttentionState::StaleRun,
                    subject_kind:AttentionSubjectKind::Run,subject_id:run.to_string(),revision:format!("stale_run:{run}:{anchor}"),
                    summary:"A run breached its stall deadline.".to_owned(),
                    detail:json!({"source":signal.reason,"run_id":run,"ticket_id":context.ticket_id,
                        "dispatch_request_id":context.dispatch_request_id,"last_activity_unix_secs":anchor})});
            } else if let Some(role) = signal.detail["role"]
                .as_str()
                .filter(|role| !role.is_empty())
            {
                signals.push(InboxSignal {project_id:project.id().value(),kind:AttentionState::StaleRun,
                    subject_kind:AttentionSubjectKind::Role,subject_id:role.to_owned(),revision:format!("stale_role:{role}:{anchor}"),
                    summary:"An observed role breached its stall deadline.".to_owned(),
                    detail:json!({"source":signal.reason,"role":role,"last_activity_unix_secs":anchor})});
            }
            continue;
        }
        if signal.reason != "missing_submission"
            && signal.reason != crate::deadlines::MISSING_RESULT_DEADLINE_REASON
        {
            continue;
        }
        if let Some(run) = signal.detail["run_id"].as_u64() {
            if submitted.contains(&run) {
                continue;
            }
            let context = match submissions.context(run) {
                Ok(context) => context,
                Err(error) if error.code == kanban_dto::ErrorCode::NotFound => continue,
                Err(error) => return Err(error),
            };
            if context.project_id != project.id().value() {
                continue;
            }
            signals.push(InboxSignal {project_id:project.id().value(),kind:AttentionState::MissingResult,
                subject_kind:AttentionSubjectKind::Run,subject_id:run.to_string(),revision:format!("missing_run:{run}"),
                summary:"A required structured result is missing.".to_owned(),
                detail:json!({"source":signal.reason,"run_id":run,"ticket_id":context.ticket_id,
                    "dispatch_request_id":context.dispatch_request_id,"observed_role":signal.detail.get("role")})});
        } else if signal.reason == crate::deadlines::MISSING_RESULT_DEADLINE_REASON {
            let (Some(role), Some(anchor)) = (
                signal.detail["role"].as_str(),
                signal.detail["settled_unix_secs"].as_u64(),
            ) else {
                continue;
            };
            if role.is_empty() {
                continue;
            }
            signals.push(InboxSignal {
                project_id: project.id().value(),
                kind: AttentionState::MissingResult,
                subject_kind: AttentionSubjectKind::Role,
                subject_id: role.to_owned(),
                revision: format!("missing_role:{role}:{anchor}"),
                summary: "A settled role has not supplied its required result.".to_owned(),
                detail: json!({"source":signal.reason,"role":role,"settled_unix_secs":anchor}),
            });
        }
    }
    Ok(signals)
}
