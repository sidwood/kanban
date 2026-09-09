//! Authoritative review execution, separated from agent observation.
use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::submission::SubmissionStore;
use crate::{
    ProfileStore, ProjectStore, ReviewConfigStore, RunStore, TicketStore, TimelineEnvelope,
};
use kanban_domain::{
    ProjectId, ReviewSlotAssignment, SlotRequirement, TicketId, resolve_effective,
};
use kanban_dto::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSlotDraft {
    pub requirement: TicketReviewSlotRequirement,
    pub occupant: TicketReviewOccupant,
    pub requested: Option<ProfileSnapshotRecord>,
    pub effective: Option<ProfileSnapshotRecord>,
    pub fallback_path: Vec<String>,
}

pub struct ReviewExecutionDraft {
    pub project_id: u64,
    pub ticket_id: u64,
    pub submission_id: u64,
    pub tip: String,
    pub configuration_version: u64,
    pub priority: String,
    pub stages: Vec<Vec<ReviewSlotDraft>>,
}

pub trait ReviewExecutionStore: Send + Sync {
    fn start(&self, draft: &ReviewExecutionDraft) -> Result<ReviewExecutionRecord, ApiError>;
    fn find(&self, review_id: u64) -> Result<Option<ReviewExecutionRecord>, ApiError>;
    fn latest_for_ticket(&self, ticket_id: u64) -> Result<Option<ReviewExecutionRecord>, ApiError>;
    fn human_verdict(
        &self,
        request: &ReviewHumanSubmitRequest,
    ) -> Result<ReviewExecutionRecord, ApiError>;
    fn revalidate(&self, ticket_id: u64) -> Result<ReviewHistoryResponse, ApiError>;
    fn expire(&self, review_id: u64) -> Result<ReviewExecutionRecord, ApiError>;
    fn history(&self, ticket_id: u64) -> Result<ReviewHistoryResponse, ApiError>;
    fn needs_revalidation(&self, ticket_id: u64) -> Result<bool, ApiError>;
}

#[derive(Clone)]
struct Context {
    store: Arc<dyn ReviewExecutionStore>,
    configs: Arc<dyn ReviewConfigStore>,
    tickets: Arc<dyn TicketStore>,
    profiles: Arc<dyn ProfileStore>,
    projects: Arc<dyn ProjectStore>,
    submissions: Arc<dyn SubmissionStore>,
    runs: Arc<dyn RunStore>,
    wake: Arc<dyn crate::CoordinatorWake>,
}

impl Core {
    #[allow(clippy::too_many_arguments)]
    pub fn register_reviews(
        &mut self,
        store: Arc<dyn ReviewExecutionStore>,
        configs: Arc<dyn ReviewConfigStore>,
        tickets: Arc<dyn TicketStore>,
        profiles: Arc<dyn ProfileStore>,
        projects: Arc<dyn ProjectStore>,
        submissions: Arc<dyn SubmissionStore>,
        runs: Arc<dyn RunStore>,
        wake: Arc<dyn crate::CoordinatorWake>,
    ) -> Result<(), RegistrationError> {
        let context = Context {
            store,
            configs,
            tickets,
            profiles,
            projects,
            submissions,
            runs,
            wake,
        };
        self.register_command("review.start", Arc::new(Start(context.clone())))?;
        self.register_command("review.human.submit", Arc::new(Human(context.clone())))?;
        self.register_command("review.revalidate", Arc::new(Revalidate(context.clone())))?;
        self.register_command("review.expire", Arc::new(Expire(context.clone())))?;
        self.register_query("review.get", Arc::new(Get(context.clone())))?;
        self.register_query("review.history", Arc::new(History(context)))
    }
}

fn encode(value: &impl Serialize) -> Result<Value, ApiError> {
    serde_json::to_value(value).map_err(|e| ApiError::internal(&e.to_string()))
}
fn snapshot(profile: &kanban_domain::ExecutionProfile) -> ProfileSnapshotRecord {
    ProfileSnapshotRecord {
        name: profile.name().as_str().to_owned(),
        harness: profile.harness().to_owned(),
        model: profile.model().to_owned(),
        effort: profile.effort().to_owned(),
        usage_pool: profile.usage_pool().to_owned(),
    }
}

struct Start(Context);
impl CommandHandler for Start {
    fn parse(&self, value: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ReviewStartRequest>(value)?;
        ParsedCommand::lift("review_execution", value)
    }
    fn current_version(&self, _: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: ReviewStartRequest = parse_payload(&command.payload)?;
        let ticket = self
            .0
            .tickets
            .find(TicketId::new(request.ticket_id))?
            .ok_or_else(|| ApiError::not_found("ticket"))?;
        let project = self
            .0
            .projects
            .find(ticket.project())?
            .ok_or_else(|| ApiError::not_found("project"))?;
        if project.is_archived() || ticket.state().is_terminal() {
            return Err(ApiError::invalid_request(
                "terminal work cannot start review",
            ));
        }
        let submissions = self.0.submissions.list(project.id().value())?;
        let submission = submissions
            .iter()
            .find(|s| s.id == request.submission_id && s.ticket_id == request.ticket_id)
            .ok_or_else(|| ApiError::not_found("implementation submission"))?;
        let SubmissionResult::Implementation { tip, .. } = &submission.result else {
            return Err(ApiError::invalid_request(
                "review requires an implementation result",
            ));
        };
        if submission.role != CapabilityRole::Implementer {
            return Err(ApiError::invalid_request(
                "review requires the implementer's result",
            ));
        }
        if self.0.store.needs_revalidation(request.ticket_id)? {
            return Err(ApiError::invalid_request(
                "failed or expired gates require revalidation before approval can proceed again",
            ));
        }
        let runs = self.0.runs.list_for_project(project.id())?;
        let implemented = runs
            .iter()
            .find(|run| run.id().value() == submission.run_id)
            .ok_or_else(|| ApiError::internal("submission run is missing"))?;
        let configuration =
            self.0.configs.find(ticket.id())?.ok_or_else(|| {
                ApiError::invalid_request("configure required review stages first")
            })?;
        let catalogue = self.0.profiles.list()?;
        let mut harness_separated = false;
        let mut stages = Vec::new();
        for stage in configuration.stages() {
            let mut slots = Vec::new();
            for slot in stage.slots() {
                let requirement = match slot.requirement() {
                    SlotRequirement::Required => TicketReviewSlotRequirement::Required,
                    SlotRequirement::Optional => TicketReviewSlotRequirement::Optional,
                };
                let (occupant, requested, effective, fallback_path) = match slot.assignment() {
                    ReviewSlotAssignment::Human => {
                        (TicketReviewOccupant::Human {}, None, None, Vec::new())
                    }
                    ReviewSlotAssignment::Profile(name) => {
                        let requested =
                            catalogue.iter().find(|p| p.name() == name).ok_or_else(|| {
                                ApiError::invalid_request("reviewer profile is missing")
                            })?;
                        let (effective, path) = resolve_effective(&catalogue, name)
                            .map_err(|e| ApiError::invalid_request(&e.to_string()))?;
                        if effective.model() == implemented.effective().model() {
                            return Err(ApiError::invalid_request(
                                "a model family cannot review its own work",
                            ));
                        }
                        harness_separated |=
                            effective.harness() != implemented.effective().harness();
                        (
                            TicketReviewOccupant::Profile {
                                name: name.as_str().to_owned(),
                            },
                            Some(snapshot(requested)),
                            Some(snapshot(effective)),
                            path.iter().map(|p| p.as_str().to_owned()).collect(),
                        )
                    }
                };
                slots.push(ReviewSlotDraft {
                    requirement,
                    occupant,
                    requested,
                    effective,
                    fallback_path,
                });
            }
            stages.push(slots);
        }
        if !harness_separated {
            return Err(ApiError::invalid_request(
                "at least one reviewer must use a different harness family",
            ));
        }
        let record = self.0.store.start(&ReviewExecutionDraft {
            project_id: project.id().value(),
            ticket_id: ticket.id().value(),
            submission_id: submission.id,
            tip: tip.clone(),
            configuration_version: configuration.version(),
            priority: ticket.priority().wire_name().to_owned(),
            stages,
        })?;
        schedule_review_dispatches(&project, self.0.wake.clone(), effects, None, &record);
        encode(&record)
    }
}

struct Human(Context);
impl CommandHandler for Human {
    fn parse(&self, value: &Value) -> Result<ParsedCommand, ApiError> {
        let request: ReviewHumanSubmitRequest = parse_payload(value)?;
        crate::findings::validate_findings(&request.findings)?;
        if request.summary.trim().is_empty() {
            return Err(ApiError::invalid_request("a review summary is required"));
        }
        ParsedCommand::lift("review_execution", value)
    }
    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: ReviewHumanSubmitRequest = parse_payload(&command.payload)?;
        Ok(self
            .0
            .store
            .find(request.review_id)?
            .ok_or_else(|| ApiError::not_found("review"))?
            .version)
    }
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: ReviewHumanSubmitRequest = parse_payload(&command.payload)?;
        let review = self
            .0
            .store
            .find(request.review_id)?
            .ok_or_else(|| ApiError::not_found("review"))?;
        let project = self
            .0
            .projects
            .find(ProjectId::new(review.project_id))?
            .ok_or_else(|| ApiError::not_found("project"))?;
        if project.is_archived() {
            return Err(ApiError::invalid_request(
                "archived projects cannot accept review",
            ));
        }
        let updated = self.0.store.human_verdict(&request)?;
        schedule_review_dispatches(
            &project,
            self.0.wake.clone(),
            effects,
            Some(&review),
            &updated,
        );
        encode(&updated)
    }
}

struct Revalidate(Context);
impl CommandHandler for Revalidate {
    fn parse(&self, value: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ReviewRevalidateRequest>(value)?;
        ParsedCommand::lift("review_execution", value)
    }
    fn current_version(&self, _: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(0)
    }
    fn apply(&self, command: &ParsedCommand, _: &dyn CommandEffects) -> Result<Value, ApiError> {
        let request: ReviewRevalidateRequest = parse_payload(&command.payload)?;
        encode(&self.0.store.revalidate(request.ticket_id)?)
    }
}

struct Expire(Context);
impl CommandHandler for Expire {
    fn parse(&self, value: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ReviewExpireRequest>(value)?;
        ParsedCommand::lift("review_execution", value)
    }
    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: ReviewExpireRequest = parse_payload(&command.payload)?;
        Ok(self
            .0
            .store
            .find(request.review_id)?
            .ok_or_else(|| ApiError::not_found("review"))?
            .version)
    }
    fn apply(&self, command: &ParsedCommand, _: &dyn CommandEffects) -> Result<Value, ApiError> {
        let request: ReviewExpireRequest = parse_payload(&command.payload)?;
        encode(&self.0.store.expire(request.review_id)?)
    }
}

struct Get(Context);
impl QueryHandler for Get {
    fn handle(&self, value: &Value) -> Result<Value, ApiError> {
        let query: ReviewGetQuery = parse_payload(value)?;
        encode(
            &self
                .0
                .store
                .find(query.review_id)?
                .ok_or_else(|| ApiError::not_found("review"))?,
        )
    }
}

struct History(Context);
impl QueryHandler for History {
    fn handle(&self, value: &Value) -> Result<Value, ApiError> {
        let query: ReviewHistoryQuery = parse_payload(value)?;
        encode(&self.0.store.history(query.ticket_id)?)
    }
}

pub fn review_stage_status(tip: &str, slots: &[ReviewSlotRecord]) -> ReviewStageStatus {
    let votes: Vec<_> = slots
        .iter()
        .map(|slot| kanban_domain::review_execution::ReviewVote {
            required: slot.requirement == TicketReviewSlotRequirement::Required,
            verdict: slot
                .verdict
                .as_ref()
                .filter(|v| v.counts_for_resolution)
                .map(|v| (v.tip.as_str(), v.approve)),
        })
        .collect();
    match kanban_domain::review_execution::resolve_review_stage(tip, &votes) {
        kanban_domain::review_execution::ReviewStageResolution::Waiting => {
            ReviewStageStatus::Waiting
        }
        kanban_domain::review_execution::ReviewStageResolution::Approved => {
            ReviewStageStatus::Approved
        }
        kanban_domain::review_execution::ReviewStageResolution::Rejected => {
            ReviewStageStatus::Rejected
        }
    }
}

pub fn review_transition(review: &ReviewExecutionRecord, action: &str) -> TimelineEnvelope {
    TimelineEnvelope::project(
        review.project_id,
        TimelineEventKind::Review,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Ticket,
            id: review.ticket_id.to_string(),
        }),
        json!({"action":action,"review_id":review.id,"ticket_id":review.ticket_id,"tip":review.tip,"status":review.status,"version":review.version}),
    )
}

pub fn active_review_slot(
    review: &ReviewExecutionRecord,
    id: u64,
) -> Result<&ReviewSlotRecord, ApiError> {
    let (stage, slot) = review
        .stages
        .iter()
        .find_map(|stage| {
            stage
                .slots
                .iter()
                .find(|slot| slot.id == id)
                .map(|slot| (stage, slot))
        })
        .ok_or_else(|| ApiError::not_found("review slot"))?;
    if slot.verdict.is_some() {
        return Err(ApiError::invalid_request("review slot already submitted"));
    }
    let active = review
        .stages
        .iter()
        .find(|stage| stage.status != ReviewStageStatus::Approved)
        .map(|stage| stage.index);
    let currently_active = review.status == ReviewExecutionStatus::InProgress
        && stage.status == ReviewStageStatus::Waiting
        && active == Some(stage.index);
    let completed_optional = slot.requirement == TicketReviewSlotRequirement::Optional
        && active.is_none_or(|index| stage.index <= index)
        && (stage.status != ReviewStageStatus::Waiting
            || review.status != ReviewExecutionStatus::InProgress);
    if !currently_active && !completed_optional {
        return Err(ApiError::invalid_request(
            "review slot is not in an active or completed optional stage",
        ));
    }
    Ok(slot)
}

pub fn counts_for_resolution(review: &ReviewExecutionRecord, slot_id: u64) -> bool {
    review.status == ReviewExecutionStatus::InProgress
        && review
            .stages
            .iter()
            .find(|stage| stage.status != ReviewStageStatus::Approved)
            .is_some_and(|stage| {
                stage.status == ReviewStageStatus::Waiting
                    && stage.slots.iter().any(|slot| slot.id == slot_id)
            })
}

pub fn validate_review_tip(review: &ReviewExecutionRecord, tip: &str) -> Result<(), ApiError> {
    if tip != review.tip {
        return Err(ApiError::invalid_request(
            "review verdict must bind the exact implementation tip",
        ));
    }
    Ok(())
}

pub fn review_bounce(tip: &str, stages: &[ReviewStageRecord]) -> Option<ReviewBounceRecord> {
    let stage = stages
        .iter()
        .find(|stage| stage.status == ReviewStageStatus::Rejected)?;
    let findings = stage
        .slots
        .iter()
        .flat_map(|slot| {
            slot.verdict
                .iter()
                .filter(|v| v.counts_for_resolution)
                .flat_map(move |verdict| {
                    verdict
                        .findings
                        .iter()
                        .enumerate()
                        .map(move |(index, finding)| ReviewFindingReference {
                            slot_id: slot.id,
                            submission_id: verdict.submission_id,
                            finding_index: index as u64,
                            finding: finding.clone(),
                        })
                })
        })
        .collect();
    Some(ReviewBounceRecord {
        stage_index: stage.index,
        tip: tip.to_owned(),
        findings,
    })
}

pub fn schedule_review_dispatches(
    project: &kanban_domain::Project,
    wake: Arc<dyn crate::CoordinatorWake>,
    effects: &dyn CommandEffects,
    before: Option<&ReviewExecutionRecord>,
    after: &ReviewExecutionRecord,
) {
    let previous: std::collections::BTreeSet<u64> = before
        .into_iter()
        .flat_map(|review| review.stages.iter())
        .flat_map(|stage| stage.slots.iter())
        .filter_map(|slot| slot.dispatch_request_id)
        .collect();
    for id in after
        .stages
        .iter()
        .flat_map(|stage| stage.slots.iter())
        .filter_map(|slot| slot.dispatch_request_id)
    {
        if previous.contains(&id) {
            continue;
        }
        let request = crate::CoordinatorWakeRequest {
            project_id: project.id().value(),
            dispatch_request_id: id,
            resume_run_id: None,
            seed_workspace: project.registration().seed_workspace().to_owned(),
            herdr_workspace: project.registration().herdr_workspace().to_owned(),
            herdr_session: project.registration().herdr_session().map(str::to_owned),
        };
        let port = wake.clone();
        effects.after_commit(Box::new(move || port.wake(request)));
    }
}

/// Observes an accepted review projection while its effects remain staged.
pub type ReviewChangeObserver<'a> =
    dyn Fn(&ReviewExecutionRecord, &ReviewExecutionRecord) -> Result<(), ApiError> + 'a;

pub fn review_progress(review: &ReviewExecutionRecord) -> (ReviewExecutionStatus, Option<usize>) {
    use kanban_domain::review_execution::{
        ReviewSequenceResolution as Sequence, ReviewStageResolution as Stage,
        resolve_review_sequence,
    };
    let stages: Vec<_> = review
        .stages
        .iter()
        .map(|stage| match stage.status {
            ReviewStageStatus::Waiting => Stage::Waiting,
            ReviewStageStatus::Approved => Stage::Approved,
            ReviewStageStatus::Rejected => Stage::Rejected,
        })
        .collect();
    match resolve_review_sequence(&stages) {
        Sequence::Waiting(index) => (ReviewExecutionStatus::InProgress, Some(index)),
        Sequence::Approved => (ReviewExecutionStatus::Approved, None),
        Sequence::Rejected => (ReviewExecutionStatus::Rejected, None),
    }
}
