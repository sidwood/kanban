//! Ticket commands and queries: create a Ticket under its kind's
//! schema — an Implementation attached to exactly one Spec with its
//! slice and story-linked criteria, a Bug quick-captured with title,
//! actual behaviour, and reporter evidence, a Task with a title, one
//! subtype of the closed set, a human-or-agent mode, completion
//! criteria, and optional schedule or due-date timing stored for
//! KAN-S11 — qualify a Bug and record its vendor-neutral facts, and
//! read Tickets back per Project (KAN-S4-US1 through KAN-S4-US4).
//! Creation mints the Project's next Ticket number, lands the row
//! with the counter move and the timeline append in one write, and
//! announces live; no delete exists. Lifecycle transitions and
//! dependencies arrive with their own tickets; readiness stays a
//! computed projection, so qualifying a Bug never moves its state.

use std::sync::Arc;

use kanban_domain::{
    AcceptanceCriterion, BugFacts, BugQualification, CompletionCriterion as DomainCompletion,
    ExternalReference, NumberKind, OccurrenceSnapshot, Priority as DomainPriority, Project,
    ProjectCode, ProjectId, Severity as DomainSeverity, SpecId, TaskMode as DomainMode,
    TaskSubtype as DomainSubtype, TaskTiming, Ticket, TicketBody, TicketId,
    TicketKind as DomainKind, TicketNumber, TicketState as DomainState, UserStoryRef,
    VerificationStep,
};
use kanban_dto::{
    ApiError, LiveEventName, TaskMode, TaskSubtype, TicketBugFactsRequest, TicketBugQualification,
    TicketBugQualifyRequest, TicketBugRecord, TicketCreateRequest, TicketCriterion,
    TicketExternalReference, TicketGetQuery, TicketKind, TicketListQuery, TicketListResponse,
    TicketOccurrenceSnapshot, TicketPriority, TicketRecord, TicketSeverity, TicketState,
    TicketVerificationStep, TimelineEntityKind, TimelineEntityRef, TimelineEventKind,
};
use serde_json::{Value, json};

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::events::{EventSink, emit_catalogued};
use crate::evidence::{EvidenceFilter, EvidenceStore};
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;
use crate::spec::SpecStore;
use crate::timeline::TimelineEnvelope;

/// The storage port Ticket commands call through. Implementations land
/// the row changes and the timeline envelope unchanged inside one
/// write, so a Ticket, and the Project counter its number minted,
/// never split across a crash boundary.
pub trait TicketStore: Send + Sync {
    /// Insert a fresh Ticket. `project` carries the minted Ticket
    /// number and the counter move that minted it; both land in the
    /// same write as the Ticket row. Storage assigns the Ticket's
    /// identity and asks `envelope` for the timeline row that identity
    /// belongs in.
    fn create(
        &self,
        project: &Project,
        number: TicketNumber,
        priority: DomainPriority,
        body: &TicketBody,
        envelope: &dyn Fn(TicketId) -> TimelineEnvelope,
    ) -> Result<Ticket, ApiError>;
    /// Load one Ticket, if it exists.
    fn find(&self, id: TicketId) -> Result<Option<Ticket>, ApiError>;
    /// Persist the applied Ticket — its lifecycle state, priority,
    /// kind-specific body, and Execution Profile assignment — with the
    /// timeline envelope, all in one write, guarded by the version the
    /// aggregate moved from.
    fn save(&self, ticket: &Ticket, envelope: TimelineEnvelope) -> Result<(), ApiError>;
    /// Every Ticket of one Project in id order, terminal lifecycle
    /// states included.
    fn list(&self, project: ProjectId) -> Result<Vec<Ticket>, ApiError>;
}

/// The timeline row for one Ticket change: on the Project's own
/// timeline, about the Ticket, with `action` naming the change inside
/// the closed `transition` kind.
fn transition(
    project: ProjectId,
    ticket: TicketId,
    action: &str,
    facts: Value,
) -> TimelineEnvelope {
    let mut detail = facts;
    let object = detail
        .as_object_mut()
        .expect("Ticket transition facts are a JSON object");
    object.insert("action".to_owned(), Value::from(action));
    object.insert("id".to_owned(), Value::from(ticket.value()));
    TimelineEnvelope::project(
        project.value(),
        TimelineEventKind::Transition,
        Some(TimelineEntityRef {
            kind: TimelineEntityKind::Ticket,
            id: ticket.value().to_string(),
        }),
        detail,
    )
}

/// Report a refused domain rule as the stable invalid-request code.
fn refuse(error: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

/// The stores every Ticket command reads and writes through.
#[derive(Clone)]
struct TicketContext {
    tickets: Arc<dyn TicketStore>,
    projects: Arc<dyn ProjectStore>,
    specs: Arc<dyn SpecStore>,
    evidence: Arc<dyn EvidenceStore>,
}

impl TicketContext {
    /// The Ticket a command addresses and its Project, refusing an
    /// unknown Ticket and the terminal archived-Project state.
    fn open(&self, id: u64) -> Result<(Project, Ticket), ApiError> {
        let ticket = self
            .tickets
            .find(TicketId::new(id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {id}")))?;
        let project = self.projects.find(ticket.project())?.ok_or_else(|| {
            ApiError::internal(&format!("ticket {id} belongs to no stored Project"))
        })?;
        if project.is_archived() {
            return Err(ApiError::invalid_request(
                "archived is terminal; the Project accepts no further changes",
            ));
        }
        Ok((project, ticket))
    }
}

impl Core {
    /// Register the Ticket operations against `tickets`, resolving
    /// Projects through `projects`, Spec attachments through `specs`,
    /// and Evidence Item claims through `evidence`.
    pub fn register_tickets(
        &mut self,
        tickets: Arc<dyn TicketStore>,
        projects: Arc<dyn ProjectStore>,
        specs: Arc<dyn SpecStore>,
        evidence: Arc<dyn EvidenceStore>,
    ) -> Result<(), RegistrationError> {
        let context = TicketContext {
            tickets,
            projects,
            specs,
            evidence,
        };
        self.register_command("ticket.create", Arc::new(CreateTicket(context.clone())))?;
        self.register_command("ticket.bug.qualify", Arc::new(QualifyBug(context.clone())))?;
        self.register_command(
            "ticket.bug.facts",
            Arc::new(RecordBugFacts(context.clone())),
        )?;
        self.register_query(
            "ticket.list",
            Arc::new(ListTickets {
                tickets: context.tickets.clone(),
                projects: context.projects.clone(),
            }),
        )?;
        self.register_query(
            "ticket.get",
            Arc::new(GetTicket {
                tickets: context.tickets.clone(),
                projects: context.projects.clone(),
            }),
        )?;
        Ok(())
    }
}

/// Serves `ticket.create`.
struct CreateTicket(TicketContext);

impl CommandHandler for CreateTicket {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketCreateRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        // A fresh aggregate is created at version 0.
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        events: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketCreateRequest = parse_payload(&command.payload)?;
        let mut project = self
            .0
            .projects
            .find(ProjectId::new(request.project_id))?
            .ok_or_else(|| ApiError::not_found(&format!("project {}", request.project_id)))?;
        if project.is_archived() {
            return Err(ApiError::invalid_request(
                "archived is terminal; the Project accepts no further changes",
            ));
        }
        let priority = priority_of(request.priority);
        let body = body_of(&request, &project, self.0.specs.as_ref())?;
        let number = TicketNumber::new(project.mint(NumberKind::Ticket).map_err(refuse)?)
            .expect("a minted number is positive");
        let identity = project.id();
        let kind = body.kind().wire_name().to_owned();
        let ticket = self
            .0
            .tickets
            .create(&project, number, priority, &body, &|id| {
                transition(
                    identity,
                    id,
                    "created",
                    json!({
                        "project_id": identity.value(),
                        "number": number.value(),
                        "kind": kind,
                    }),
                )
            })?;
        announce(
            events,
            LiveEventName::TicketCreated,
            &ticket,
            project.code(),
        );
        encode_record(&ticket, project.code())
    }
}

/// Serves `ticket.bug.qualify`.
struct QualifyBug(TicketContext);

impl CommandHandler for QualifyBug {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketBugQualifyRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketBugQualifyRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _events: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketBugQualifyRequest = parse_payload(&command.payload)?;
        let (project, mut ticket) = self.0.open(request.ticket_id)?;
        let qualification = qualification_of(&request.qualification, &project)?;
        let severity = qualification.severity();
        ticket.qualify(qualification).map_err(refuse)?;
        let facts = json!({
            "severity": severity.wire_name(),
            "version": ticket.version(),
        });
        self.0.tickets.save(
            &ticket,
            transition(project.id(), ticket.id(), "qualified", facts),
        )?;
        encode_record(&ticket, project.code())
    }
}

/// Serves `ticket.bug.facts`.
struct RecordBugFacts(TicketContext);

impl CommandHandler for RecordBugFacts {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<TicketBugFactsRequest>(payload)?;
        ParsedCommand::lift("ticket", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: TicketBugFactsRequest = parse_payload(&command.payload)?;
        Ok(self.0.open(request.ticket_id)?.1.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _events: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: TicketBugFactsRequest = parse_payload(&command.payload)?;
        let (project, mut ticket) = self.0.open(request.ticket_id)?;
        let references = request
            .external_references
            .iter()
            .map(|reference| ExternalReference::new(reference.uri.clone(), reference.label.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(refuse)?;
        let snapshots = request
            .occurrence_snapshots
            .iter()
            .map(|snapshot| {
                OccurrenceSnapshot::new(snapshot.observed_at.clone(), snapshot.observation.clone())
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(refuse)?;
        // A Bug names only the Evidence Items attached to it, so the
        // claim stays a reference, never a second copy of the truth.
        let attached = self.0.evidence.list(&EvidenceFilter {
            project_id: project.id().value(),
            entity_kind: Some("ticket".to_owned()),
            entity_id: Some(ticket.id().value().to_string()),
        })?;
        if let Some(unknown) = request
            .evidence_ids
            .iter()
            .copied()
            .find(|claimed| !attached.iter().any(|item| item.id().value() == *claimed))
        {
            return Err(ApiError::invalid_request(&format!(
                "evidence item {unknown} is not attached to ticket {}",
                ticket.id()
            )));
        }
        let items = request
            .evidence_ids
            .iter()
            .map(|id| kanban_domain::EvidenceId::new(*id))
            .collect();
        let facts = BugFacts::new(references, snapshots, items).map_err(refuse)?;
        let counts = json!({
            "external_references": request.external_references.len(),
            "occurrence_snapshots": request.occurrence_snapshots.len(),
            "evidence_items": request.evidence_ids.len(),
        });
        ticket.record_bug_facts(facts).map_err(refuse)?;
        self.0.tickets.save(
            &ticket,
            transition(project.id(), ticket.id(), "facts_recorded", counts),
        )?;
        encode_record(&ticket, project.code())
    }
}

/// Serves `ticket.list`.
struct ListTickets {
    tickets: Arc<dyn TicketStore>,
    projects: Arc<dyn ProjectStore>,
}

impl QueryHandler for ListTickets {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: TicketListQuery = parse_payload(payload)?;
        // An unknown Project holds no Tickets, exactly as the Spec
        // list reports it: empty, not a refusal.
        let Some(project) = self.projects.find(ProjectId::new(query.project_id))? else {
            let response = TicketListResponse {
                tickets: Vec::new(),
            };
            return serde_json::to_value(response)
                .map_err(|error| ApiError::internal(&error.to_string()));
        };
        let response = TicketListResponse {
            tickets: self
                .tickets
                .list(project.id())?
                .iter()
                .map(|ticket| record_of(ticket, project.code()))
                .collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `ticket.get`.
struct GetTicket {
    tickets: Arc<dyn TicketStore>,
    projects: Arc<dyn ProjectStore>,
}

impl QueryHandler for GetTicket {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: TicketGetQuery = parse_payload(payload)?;
        let ticket = self
            .tickets
            .find(TicketId::new(query.ticket_id))?
            .ok_or_else(|| ApiError::not_found(&format!("ticket {}", query.ticket_id)))?;
        let project = self.projects.find(ticket.project())?.ok_or_else(|| {
            ApiError::internal(&format!(
                "ticket {} belongs to no stored Project",
                query.ticket_id
            ))
        })?;
        serde_json::to_value(record_of(&ticket, project.code()))
            .map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Refuse the wire fields one kind does not carry (KAN-S4-US4): a
/// Task never sends story-linked criteria, and no other kind sends a
/// Task's bounded fields. Absence carries no fields at all, so only a
/// present field is refused.
fn refuse_cross_kind_fields(request: &TicketCreateRequest) -> Result<(), ApiError> {
    match request.kind {
        TicketKind::Task => {
            if request.criteria.is_some() {
                return Err(ApiError::invalid_request(
                    "a Task Ticket carries completion criteria, never story-linked criteria",
                ));
            }
        }
        kind => {
            let task_field_present = request.subtype.is_some()
                || request.mode.is_some()
                || request.completion.is_some()
                || request.scheduled_for.is_some()
                || request.due.is_some();
            if task_field_present {
                let named = match kind {
                    TicketKind::Implementation => "an Implementation",
                    _ => "a Bug",
                };
                return Err(ApiError::invalid_request(&format!(
                    "{named} Ticket carries no Task fields"
                )));
            }
        }
    }
    Ok(())
}

/// Decode one request's kind-specific fields into the domain's
/// validated body, resolving the Spec attachment through `specs`.
fn body_of(
    request: &TicketCreateRequest,
    project: &Project,
    specs: &dyn SpecStore,
) -> Result<TicketBody, ApiError> {
    refuse_cross_kind_fields(request)?;
    let attached = |spec_id: u64| -> Result<kanban_domain::Spec, ApiError> {
        let spec = specs
            .find(SpecId::new(spec_id))?
            .ok_or_else(|| ApiError::not_found(&format!("spec {spec_id}")))?;
        if spec.project() != project.id() {
            return Err(ApiError::invalid_request(
                "the Spec belongs to another Project",
            ));
        }
        Ok(spec)
    };
    match request.kind {
        TicketKind::Implementation => {
            let spec_id = request
                .spec_id
                .ok_or_else(|| refuse(kanban_domain::TicketError::UnattachedSpec))?;
            let spec = attached(spec_id)?;
            let criteria = criteria_of(request.criteria.as_deref().unwrap_or_default(), project)?;
            TicketBody::implementation(
                Some(spec.id()),
                spec.number(),
                request.slice.clone().unwrap_or_default(),
                criteria,
            )
            .map_err(refuse)
        }
        TicketKind::Bug => {
            let spec = request.spec_id.map(attached).transpose()?;
            TicketBody::bug(
                request.title.clone().unwrap_or_default(),
                spec.map(|spec| spec.id()),
                request.actual_behaviour.clone().unwrap_or_default(),
                request.reporter_evidence.clone().unwrap_or_default(),
            )
            .map_err(refuse)
        }
        TicketKind::Task => {
            let spec = request.spec_id.map(attached).transpose()?;
            let completion = request
                .completion
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|outcome| DomainCompletion::new(outcome.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(refuse)?;
            let timing = TaskTiming::new(request.scheduled_for.clone(), request.due.clone())
                .map_err(refuse)?;
            TicketBody::task(
                request.title.clone().unwrap_or_default(),
                spec.map(|spec| spec.id()),
                request.subtype.map(subtype_of),
                request.mode.map(mode_of),
                completion,
                timing,
            )
            .map_err(refuse)
        }
    }
}

/// Decode wire criteria into the domain's rule-valid criteria,
/// parsing every story link against the Project's code.
fn criteria_of(
    proposed: &[TicketCriterion],
    project: &Project,
) -> Result<Vec<AcceptanceCriterion>, ApiError> {
    let mut criteria = Vec::with_capacity(proposed.len());
    for criterion in proposed {
        let mut stories = Vec::with_capacity(criterion.stories.len());
        for named in &criterion.stories {
            stories.push(UserStoryRef::parse(named, project.code()).map_err(refuse)?);
        }
        criteria
            .push(AcceptanceCriterion::new(criterion.outcome.clone(), stories).map_err(refuse)?);
    }
    Ok(criteria)
}

/// The domain form of one wire priority.
fn priority_of(priority: TicketPriority) -> DomainPriority {
    match priority {
        TicketPriority::Urgent => DomainPriority::Urgent,
        TicketPriority::High => DomainPriority::High,
        TicketPriority::Normal => DomainPriority::Normal,
        TicketPriority::Low => DomainPriority::Low,
    }
}

/// The domain form of one wire severity.
fn severity_of(severity: TicketSeverity) -> DomainSeverity {
    match severity {
        TicketSeverity::Critical => DomainSeverity::Critical,
        TicketSeverity::High => DomainSeverity::High,
        TicketSeverity::Medium => DomainSeverity::Medium,
        TicketSeverity::Low => DomainSeverity::Low,
    }
}

/// The wire form of one domain severity.
fn severity_named(severity: DomainSeverity) -> TicketSeverity {
    match severity {
        DomainSeverity::Critical => TicketSeverity::Critical,
        DomainSeverity::High => TicketSeverity::High,
        DomainSeverity::Medium => TicketSeverity::Medium,
        DomainSeverity::Low => TicketSeverity::Low,
    }
}

/// Decode one wire qualification into the domain's rule-valid form,
/// parsing every story link against the Project's code.
fn qualification_of(
    record: &TicketBugQualification,
    project: &Project,
) -> Result<BugQualification, ApiError> {
    let criteria = criteria_of(&record.criteria, project)?;
    let steps = record
        .verification_steps
        .iter()
        .map(|step| VerificationStep::new(step.command.clone()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(refuse)?;
    BugQualification::new(
        record.expected_behaviour.clone(),
        record.reproduction.clone(),
        record.environment.clone(),
        severity_of(record.severity),
        record.frequency.clone(),
        record.affected_scope.clone(),
        record.risk.clone(),
        criteria,
        steps,
    )
    .map_err(refuse)
}

/// The wire form of one domain criterion, story links rendered with
/// the Project's code.
fn criterion_of(criterion: &AcceptanceCriterion, code: &ProjectCode) -> TicketCriterion {
    TicketCriterion {
        outcome: criterion.outcome().to_owned(),
        stories: criterion
            .stories()
            .iter()
            .map(|story| story.render(code))
            .collect(),
    }
}

/// The wire form of one domain Bug body.
fn bug_record_of(bug: &kanban_domain::BugTicket, code: &ProjectCode) -> TicketBugRecord {
    TicketBugRecord {
        actual_behaviour: bug.actual_behaviour().to_owned(),
        reporter_evidence: bug.reporter_evidence().to_owned(),
        qualification: bug.qualification().map(|record| TicketBugQualification {
            expected_behaviour: record.expected_behaviour().to_owned(),
            reproduction: record.reproduction().to_owned(),
            environment: record.environment().to_owned(),
            severity: severity_named(record.severity()),
            frequency: record.frequency().to_owned(),
            affected_scope: record.affected_scope().to_owned(),
            risk: record.risk().to_owned(),
            criteria: record
                .criteria()
                .iter()
                .map(|criterion| criterion_of(criterion, code))
                .collect(),
            verification_steps: record
                .verification_steps()
                .iter()
                .map(|step| TicketVerificationStep {
                    command: step.command().to_owned(),
                })
                .collect(),
        }),
        external_references: bug
            .facts()
            .external_references()
            .iter()
            .map(|reference| TicketExternalReference {
                uri: reference.uri().to_owned(),
                label: reference.label().map(str::to_owned),
            })
            .collect(),
        occurrence_snapshots: bug
            .facts()
            .occurrence_snapshots()
            .iter()
            .map(|snapshot| TicketOccurrenceSnapshot {
                observed_at: snapshot.observed_at().to_owned(),
                observation: snapshot.observation().to_owned(),
            })
            .collect(),
        evidence_ids: bug
            .facts()
            .evidence_items()
            .iter()
            .map(|item| item.value())
            .collect(),
    }
}

/// The wire form of one domain priority.
fn priority_named(priority: DomainPriority) -> TicketPriority {
    match priority {
        DomainPriority::Urgent => TicketPriority::Urgent,
        DomainPriority::High => TicketPriority::High,
        DomainPriority::Normal => TicketPriority::Normal,
        DomainPriority::Low => TicketPriority::Low,
    }
}

/// The wire form of one domain kind.
fn kind_of(kind: DomainKind) -> TicketKind {
    match kind {
        DomainKind::Implementation => TicketKind::Implementation,
        DomainKind::Bug => TicketKind::Bug,
        DomainKind::Task => TicketKind::Task,
    }
}

/// The domain form of one wire Task subtype.
fn subtype_of(subtype: TaskSubtype) -> DomainSubtype {
    match subtype {
        TaskSubtype::Operational => DomainSubtype::Operational,
        TaskSubtype::Investigative => DomainSubtype::Investigative,
        TaskSubtype::Administrative => DomainSubtype::Administrative,
        TaskSubtype::Research => DomainSubtype::Research,
        TaskSubtype::Prototype => DomainSubtype::Prototype,
        TaskSubtype::Migration => DomainSubtype::Migration,
        TaskSubtype::Manual => DomainSubtype::Manual,
    }
}

/// The wire form of one domain Task subtype.
fn subtype_named(subtype: DomainSubtype) -> TaskSubtype {
    match subtype {
        DomainSubtype::Operational => TaskSubtype::Operational,
        DomainSubtype::Investigative => TaskSubtype::Investigative,
        DomainSubtype::Administrative => TaskSubtype::Administrative,
        DomainSubtype::Research => TaskSubtype::Research,
        DomainSubtype::Prototype => TaskSubtype::Prototype,
        DomainSubtype::Migration => TaskSubtype::Migration,
        DomainSubtype::Manual => TaskSubtype::Manual,
    }
}

/// The domain form of one wire Task mode.
fn mode_of(mode: TaskMode) -> DomainMode {
    match mode {
        TaskMode::Human => DomainMode::Human,
        TaskMode::Agent => DomainMode::Agent,
    }
}

/// The wire form of one domain Task mode.
fn mode_named(mode: DomainMode) -> TaskMode {
    match mode {
        DomainMode::Human => TaskMode::Human,
        DomainMode::Agent => TaskMode::Agent,
    }
}

/// The wire form of one domain state.
fn state_of(state: DomainState) -> TicketState {
    match state {
        DomainState::Draft => TicketState::Draft,
        DomainState::Parked => TicketState::Parked,
        DomainState::Blocked => TicketState::Blocked,
        DomainState::Scheduled => TicketState::Scheduled,
        DomainState::Ready => TicketState::Ready,
        DomainState::Active => TicketState::Active,
        DomainState::InReview => TicketState::InReview,
        DomainState::Approved => TicketState::Approved,
        DomainState::Landing => TicketState::Landing,
        DomainState::Done => TicketState::Done,
        DomainState::Cancelled => TicketState::Cancelled,
        DomainState::Superseded => TicketState::Superseded,
    }
}

/// The DTO record for one Ticket. Story links render with the
/// Project's code, the full `CORE-S3-US6` form.
pub(crate) fn record_of(ticket: &Ticket, code: &ProjectCode) -> TicketRecord {
    TicketRecord {
        id: ticket.id().value(),
        project_id: ticket.project().value(),
        number: ticket.number().value(),
        kind: kind_of(ticket.kind()),
        priority: priority_named(ticket.priority()),
        state: state_of(ticket.state()),
        spec_id: ticket.spec().map(|spec| spec.value()),
        title: ticket.title().map(str::to_owned),
        slice: ticket.slice().map(str::to_owned),
        criteria: ticket
            .criteria()
            .iter()
            .map(|criterion| criterion_of(criterion, code))
            .collect(),
        bug: ticket.bug().map(|bug| bug_record_of(bug, code)),
        subtype: ticket.subtype().map(subtype_named),
        mode: ticket.task_mode().map(mode_named),
        completion: ticket
            .completion()
            .iter()
            .map(|criterion| criterion.outcome().to_owned())
            .collect(),
        scheduled_for: ticket.scheduled_for().map(str::to_owned),
        due: ticket.due().map(str::to_owned),
        profile: ticket.profile().map(|name| name.as_str().to_owned()),
        version: ticket.version(),
    }
}

/// Encode a record for a command response.
fn encode_record(ticket: &Ticket, code: &ProjectCode) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(ticket, code))
        .map_err(|error| ApiError::internal(&error.to_string()))
}

/// Publish creation on the live event stream as exactly the record
/// the command returns.
fn announce(events: &dyn EventSink, name: LiveEventName, ticket: &Ticket, code: &ProjectCode) {
    emit_catalogued(events, name, &record_of(ticket, code));
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex};

    use kanban_domain::{
        CommitIdentity, EvidenceId, EvidenceItem, EvidenceKind, EvidenceShape, ProjectId,
        RelativePath, Ticket, TicketBody, TicketId, TicketNumber,
    };
    use kanban_dto::ApiError;

    use super::TicketStore;
    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::EventSink;
    use crate::evidence::{EvidenceFilter, EvidenceStore};
    use crate::mutation::MemoryIdempotencyStore;
    use crate::plan::testing::{MemoryPlans, MemoryProjects};
    use crate::spec::testing::MemorySpecs;
    use crate::timeline::TimelineEnvelope;
    use crate::timeline::TimelineFacts;

    /// An in-memory Ticket store: rows by id, the timeline envelopes
    /// it was asked to land, and the Project rows its writes moved.
    #[derive(Default)]
    pub(crate) struct MemoryTickets {
        state: Mutex<MemoryTicketState>,
        projects: Arc<MemoryProjects>,
    }

    #[derive(Default)]
    struct MemoryTicketState {
        tickets: Vec<Ticket>,
        next_id: u64,
        timeline: Vec<TimelineEnvelope>,
    }

    impl MemoryTickets {
        /// A ticket store sharing the Project rows the harness seeded.
        pub(crate) fn sharing(projects: Arc<MemoryProjects>) -> Self {
            Self {
                projects,
                ..Self::default()
            }
        }

        /// The stored rows and timeline envelopes, for assertions.
        pub(crate) fn snapshot(&self) -> (Vec<Ticket>, Vec<TimelineEnvelope>) {
            let state = self.state.lock().expect("the memory ticket lock is sound");
            (state.tickets.clone(), state.timeline.clone())
        }
    }

    impl TicketStore for MemoryTickets {
        fn create(
            &self,
            project: &kanban_domain::Project,
            number: TicketNumber,
            priority: kanban_domain::Priority,
            body: &TicketBody,
            envelope: &dyn Fn(TicketId) -> TimelineEnvelope,
        ) -> Result<Ticket, ApiError> {
            let mut state = self.state.lock().expect("the memory ticket lock is sound");
            // The minted counter lands on the Project row in the same
            // write as the Ticket row.
            let projects = &self.projects;
            let mut project_state = projects
                .state
                .lock()
                .expect("the memory project lock is sound");
            if let Some(row) = project_state
                .projects
                .iter_mut()
                .find(|row| row.id() == project.id())
            {
                *row = project.clone();
            }
            state.next_id += 1;
            let id = TicketId::new(state.next_id);
            let ticket = Ticket::new(id, project.id(), number, priority, body.clone());
            state.tickets.push(ticket.clone());
            state.timeline.push(envelope(id));
            Ok(ticket)
        }

        fn find(&self, id: TicketId) -> Result<Option<Ticket>, ApiError> {
            let state = self.state.lock().expect("the memory ticket lock is sound");
            Ok(state.tickets.iter().find(|row| row.id() == id).cloned())
        }

        fn save(&self, ticket: &Ticket, envelope: TimelineEnvelope) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory ticket lock is sound");
            let preceding = ticket.version() - 1;
            let index = state.tickets.iter().position(|row| row.id() == ticket.id());
            match index {
                Some(index) if state.tickets[index].version() == preceding => {
                    state.tickets[index] = ticket.clone();
                    state.timeline.push(envelope);
                    Ok(())
                }
                Some(index) => Err(ApiError::stale_version(
                    preceding,
                    state.tickets[index].version(),
                )),
                None => Err(ApiError::not_found(&format!("ticket {}", ticket.id()))),
            }
        }

        fn list(&self, project: ProjectId) -> Result<Vec<Ticket>, ApiError> {
            let state = self.state.lock().expect("the memory ticket lock is sound");
            Ok(state
                .tickets
                .iter()
                .filter(|row| row.project() == project)
                .cloned()
                .collect())
        }
    }

    /// An in-memory Evidence store for the evidence claims the Ticket
    /// commands validate. Items are seeded straight onto entities;
    /// attaches record the same way the durable store would.
    #[derive(Default)]
    pub(crate) struct MemoryTicketEvidence {
        state: Mutex<MemoryEvidenceState>,
    }

    #[derive(Default)]
    struct MemoryEvidenceState {
        items: Vec<EvidenceItem>,
        next_id: u64,
    }

    impl MemoryTicketEvidence {
        /// Seed one repository evidence item onto `entity` and return
        /// its identity.
        pub(crate) fn seed_repository_item(
            &self,
            project_id: u64,
            entity_kind: &str,
            entity_id: &str,
        ) -> EvidenceId {
            let mut state = self
                .state
                .lock()
                .expect("the memory evidence lock is sound");
            state.next_id += 1;
            let item = EvidenceItem::restore(
                EvidenceId::new(state.next_id),
                EvidenceShape {
                    project_id,
                    entity_kind: entity_kind.to_owned(),
                    entity_id: entity_id.to_owned(),
                    kind: EvidenceKind::Repository,
                    content_hash: None,
                    relative_path: Some(
                        RelativePath::new("docs/evidence.md").expect("the fixture path validates"),
                    ),
                    commit_identity: Some(
                        CommitIdentity::new("deadbeef").expect("the fixture commit validates"),
                    ),
                },
            )
            .expect("the fixture item validates");
            state.items.push(item);
            EvidenceId::new(state.next_id)
        }
    }

    impl EvidenceStore for MemoryTicketEvidence {
        fn attach_managed_file(
            &self,
            project_id: u64,
            entity_kind: &str,
            entity_id: &str,
            _content_base64: &str,
            _facts: TimelineFacts,
        ) -> Result<EvidenceItem, ApiError> {
            let mut state = self
                .state
                .lock()
                .expect("the memory evidence lock is sound");
            state.next_id += 1;
            let item = EvidenceItem::restore(
                EvidenceId::new(state.next_id),
                EvidenceShape {
                    project_id,
                    entity_kind: entity_kind.to_owned(),
                    entity_id: entity_id.to_owned(),
                    kind: EvidenceKind::ManagedFile,
                    content_hash: Some(
                        kanban_domain::ContentHash::new(&"c".repeat(64))
                            .expect("the fixture digest validates"),
                    ),
                    relative_path: None,
                    commit_identity: None,
                },
            )
            .map_err(|error| ApiError::internal(&error.to_string()))?;
            state.items.push(item.clone());
            Ok(item)
        }

        fn attach_repository(
            &self,
            project_id: u64,
            entity_kind: &str,
            entity_id: &str,
            relative_path: &RelativePath,
            commit_identity: &CommitIdentity,
            _facts: TimelineFacts,
        ) -> Result<EvidenceItem, ApiError> {
            let mut state = self
                .state
                .lock()
                .expect("the memory evidence lock is sound");
            state.next_id += 1;
            let item = EvidenceItem::restore(
                EvidenceId::new(state.next_id),
                EvidenceShape {
                    project_id,
                    entity_kind: entity_kind.to_owned(),
                    entity_id: entity_id.to_owned(),
                    kind: EvidenceKind::Repository,
                    content_hash: None,
                    relative_path: Some(relative_path.clone()),
                    commit_identity: Some(commit_identity.clone()),
                },
            )
            .map_err(|error| ApiError::internal(&error.to_string()))?;
            state.items.push(item.clone());
            Ok(item)
        }

        fn list(&self, filter: &EvidenceFilter) -> Result<Vec<EvidenceItem>, ApiError> {
            let state = self
                .state
                .lock()
                .expect("the memory evidence lock is sound");
            Ok(state
                .items
                .iter()
                .filter(|item| {
                    item.project_id() == filter.project_id
                        && filter
                            .entity_kind
                            .as_deref()
                            .is_none_or(|kind| kind == item.entity_kind())
                        && filter
                            .entity_id
                            .as_deref()
                            .is_none_or(|identity| identity == item.entity_id())
                })
                .cloned()
                .collect())
        }
    }

    /// A core with the Plan, Spec, and Ticket operations wired to
    /// in-memory stores over one active Project.
    pub(crate) struct TicketHarness {
        pub(crate) tickets: Arc<MemoryTickets>,
        pub(crate) projects: Arc<MemoryProjects>,
        pub(crate) evidence: Arc<MemoryTicketEvidence>,
        pub(crate) core: Core,
    }

    /// A harness whose event sink the test chooses.
    pub(crate) fn ticket_harness_with_sink(events: Arc<dyn EventSink>) -> TicketHarness {
        let projects = Arc::new(MemoryProjects::default());
        projects.seed(crate::plan::testing::active_project(
            1,
            "CORE",
            kanban_domain::ProjectCounters::restore(0, 0, 0),
        ));
        let plans = Arc::new(MemoryPlans::sharing(projects.clone()));
        let specs = Arc::new(MemorySpecs::sharing(projects.clone()));
        let tickets = Arc::new(MemoryTickets::sharing(projects.clone()));
        let evidence = Arc::new(MemoryTicketEvidence::default());
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            events,
        );
        core.register_plans(plans.clone(), projects.clone(), specs.clone())
            .expect("the plan operations register");
        core.register_specs(specs.clone(), projects.clone(), plans.clone())
            .expect("the spec operations register");
        core.register_tickets(
            tickets.clone(),
            projects.clone(),
            specs.clone(),
            evidence.clone(),
        )
        .expect("the ticket operations register");
        TicketHarness {
            tickets,
            projects,
            evidence,
            core,
        }
    }

    /// A harness with a silent event sink.
    pub(crate) fn ticket_harness() -> TicketHarness {
        ticket_harness_with_sink(Arc::new(crate::events::NoopEventSink))
    }

    /// Author one Spec on the seeded Project, returning its identity.
    pub(crate) fn authored_spec(core: &Core, key: &str) -> u64 {
        let created = core
            .command(
                "spec.create",
                &crate::spec::testing::create(1, "Registration", key),
            )
            .expect("the Spec authors");
        created["id"].as_u64().expect("the identity is a number")
    }

    /// Quick-capture one Bug on the seeded Project, returning its
    /// storage identity.
    pub(crate) fn captured_bug(core: &Core, key: &str) -> u64 {
        let created = core
            .command(
                "ticket.create",
                &serde_json::json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": key },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "normal",
                    "title": "Landing drops the integration branch",
                    "actual_behaviour":
                        "The integration branch is dropped after a review lands.",
                    "reporter_evidence":
                        "The landing log names the drop immediately after the merge.",
                }),
            )
            .expect("the Bug quick captures");
        created["id"].as_u64().expect("the identity is a number")
    }

    /// One complete wire qualification, varied by severity.
    pub(crate) fn wire_qualification(severity: &str) -> serde_json::Value {
        serde_json::json!({
            "expected_behaviour": "The integration branch survives every landing.",
            "reproduction": "Re land a reviewed change; the branch list still names it.",
            "environment": "macOS 26, Kanban 0.1.0.",
            "severity": severity,
            "frequency": "Every landing so far.",
            "affected_scope": "All landing reviews.",
            "risk": "Duplicate landings and lost review state.",
            "criteria": [
                {
                    "outcome": "The integration branch survives a landing.",
                    "stories": ["CORE-S1-US1"],
                }
            ],
            "verification_steps": [
                { "command": "cargo test -p kanban-storage tickets" }
            ],
        })
    }

    /// The wire PRD content the fixtures author, varied by name.
    pub(crate) fn wire_spec_content(name: &str) -> serde_json::Value {
        crate::spec::testing::wire_content(name)
    }
}

#[cfg(test)]
mod ticket_create {
    use kanban_dto::ErrorCode;
    use serde_json::{Value, json};

    use super::testing::{ticket_harness, ticket_harness_with_sink};

    /// An Implementation creation request against the seeded Project,
    /// with the fields a test varies.
    #[allow(clippy::too_many_arguments)]
    fn implementation(
        spec_id: Option<u64>,
        slice: Option<&str>,
        criteria: Value,
        priority: &str,
        key: &str,
    ) -> Value {
        let mut request = json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": 1,
            "kind": "implementation",
            "priority": priority,
            "criteria": criteria,
        });
        let object = request.as_object_mut().expect("the request is an object");
        if let Some(spec_id) = spec_id {
            object.insert("spec_id".to_owned(), json!(spec_id));
        }
        if let Some(slice) = slice {
            object.insert("slice".to_owned(), json!(slice));
        }
        request
    }

    /// A Bug or Task creation request with the fields a test varies.
    /// A Bug always carries its quick-capture facts (DR-TK-08).
    /// A Task additionally carries its bounded fields: subtype, mode,
    /// and one completion criterion.
    fn titled(
        kind: &str,
        title: Option<&str>,
        spec_id: Option<u64>,
        priority: &str,
        key: &str,
    ) -> Value {
        let mut request = json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": 1,
            "kind": kind,
            "priority": priority,
        });
        let object = request.as_object_mut().expect("the request is an object");
        if let Some(title) = title {
            object.insert("title".to_owned(), json!(title));
        }
        if kind == "bug" {
            object.insert(
                "actual_behaviour".to_owned(),
                json!("The integration branch is dropped after a review lands."),
            );
            object.insert(
                "reporter_evidence".to_owned(),
                json!("The landing log names the drop immediately after the merge."),
            );
        }
        if let Some(spec_id) = spec_id {
            object.insert("spec_id".to_owned(), json!(spec_id));
        }
        if kind == "task" {
            object.insert("subtype".to_owned(), json!("administrative"));
            object.insert("mode".to_owned(), json!("human"));
            object.insert(
                "completion".to_owned(),
                json!(["The old register is archived and restorable."]),
            );
        }
        request
    }

    /// A Task creation request with the bounded fields a test varies,
    /// otherwise rule-valid.
    fn bounded(
        subtype: Option<&str>,
        mode: Option<&str>,
        completion: Value,
        scheduled_for: Option<&str>,
        due: Option<&str>,
        key: &str,
    ) -> Value {
        let mut request = json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": 1,
            "kind": "task",
            "priority": "normal",
            "title": "Archive the old register",
        });
        let object = request.as_object_mut().expect("the request is an object");
        if let Some(subtype) = subtype {
            object.insert("subtype".to_owned(), json!(subtype));
        }
        if let Some(mode) = mode {
            object.insert("mode".to_owned(), json!(mode));
        }
        object.insert("completion".to_owned(), completion);
        if let Some(scheduled_for) = scheduled_for {
            object.insert("scheduled_for".to_owned(), json!(scheduled_for));
        }
        if let Some(due) = due {
            object.insert("due".to_owned(), json!(due));
        }
        request
    }

    fn one_criterion() -> Value {
        json!([
            { "outcome": "Specs mint unique numbers.", "stories": ["CORE-S1-US1"] }
        ])
    }

    #[test]
    fn creating_an_implementation_ticket_returns_the_record_and_mints_the_number() {
        let harness = ticket_harness();
        let spec = super::testing::authored_spec(&harness.core, "key-author");

        let response = harness
            .core
            .command(
                "ticket.create",
                &implementation(
                    Some(spec),
                    Some("Spec authoring creates content versions end to end"),
                    one_criterion(),
                    "high",
                    "key-1",
                ),
            )
            .expect("the Ticket creates");

        assert_eq!(
            response,
            json!({
                "id": 1,
                "project_id": 1,
                "number": 1,
                "kind": "implementation",
                "priority": "high",
                "state": "draft",
                "spec_id": spec,
                "title": null,
                "slice": "Spec authoring creates content versions end to end",
                "criteria": [
                    { "outcome": "Specs mint unique numbers.", "stories": ["CORE-S1-US1"] }
                ],
                "bug": null,
                "subtype": null,
                "mode": null,
                "completion": [],
                "scheduled_for": null,
                "due": null,
                "profile": null,
                "version": 1,
            })
        );
        assert_eq!(
            harness.projects.rows()[0]
                .counters()
                .last(kanban_domain::NumberKind::Ticket),
            1,
            "creating consumes the Project's first Ticket number"
        );
    }

    #[test]
    fn creating_a_bug_or_task_attaches_to_zero_or_one_spec() {
        let harness = ticket_harness();
        let spec = super::testing::authored_spec(&harness.core, "key-author");

        let standing = harness
            .core
            .command(
                "ticket.create",
                &titled(
                    "bug",
                    Some("Landing drops the integration branch"),
                    None,
                    "urgent",
                    "key-bug",
                ),
            )
            .expect("a Bug may stand alone");
        assert_eq!(standing["kind"], json!("bug"));
        assert_eq!(
            standing["title"],
            json!("Landing drops the integration branch")
        );
        assert_eq!(standing["spec_id"], json!(null));
        assert_eq!(standing["criteria"], json!([]));
        assert_eq!(
            standing["bug"],
            json!({
                "actual_behaviour": "The integration branch is dropped after a review lands.",
                "reporter_evidence":
                    "The landing log names the drop immediately after the merge.",
                "external_references": [],
                "occurrence_snapshots": [],
                "evidence_ids": [],
            }),
            "quick capture lands the capture facts and nothing else (DR-TK-08)"
        );

        let attached = harness
            .core
            .command(
                "ticket.create",
                &titled(
                    "task",
                    Some("Archive the old register"),
                    Some(spec),
                    "low",
                    "key-task",
                ),
            )
            .expect("a Task may attach to one Spec");
        assert_eq!(attached["kind"], json!("task"));
        assert_eq!(attached["spec_id"], json!(spec));
        assert_eq!(attached["number"], json!(2), "the counter moves per Ticket");
    }

    #[test]
    fn creating_a_task_returns_its_bounded_record() {
        let harness = ticket_harness();

        let response = harness
            .core
            .command(
                "ticket.create",
                &bounded(
                    Some("migration"),
                    Some("agent"),
                    json!(["The register moves.", "The archive restores."]),
                    Some("2026-10-01T02:00:00+02:00"),
                    Some("2026-09-30T17:00:00Z"),
                    "key-task",
                ),
            )
            .expect("the Task creates");

        assert_eq!(
            response,
            json!({
                "id": 1,
                "project_id": 1,
                "number": 1,
                "kind": "task",
                "priority": "normal",
                "state": "draft",
                "spec_id": null,
                "title": "Archive the old register",
                "slice": null,
                "criteria": [],
                "bug": null,
                "subtype": "migration",
                "mode": "agent",
                "completion": ["The register moves.", "The archive restores."],
                "scheduled_for": "2026-10-01T00:00:00.000Z",
                "due": "2026-09-30T17:00:00.000Z",
                "profile": null,
                "version": 1,
            })
        );

        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": 1 }))
            .expect("the get serves");
        assert_eq!(read, response, "the bounded record round trips");
    }

    #[test]
    fn creating_a_task_without_subtype_mode_or_completion_is_refused() {
        let harness = ticket_harness();

        let unspecified = harness
            .core
            .command(
                "ticket.create",
                &bounded(None, Some("human"), json!(["Done."]), None, None, "key-1"),
            )
            .expect_err("a Task names one subtype");
        assert_eq!(unspecified.code, ErrorCode::InvalidRequest);
        assert_eq!(
            unspecified.message,
            "a Task Ticket names one subtype of the closed set"
        );

        let unstated = harness
            .core
            .command(
                "ticket.create",
                &bounded(
                    Some("research"),
                    None,
                    json!(["Done."]),
                    None,
                    None,
                    "key-2",
                ),
            )
            .expect_err("a Task names its mode");
        assert_eq!(unstated.code, ErrorCode::InvalidRequest);
        assert_eq!(
            unstated.message,
            "a Task Ticket names a human or agent mode"
        );

        let unbounded = harness
            .core
            .command(
                "ticket.create",
                &bounded(
                    Some("manual"),
                    Some("human"),
                    json!([]),
                    None,
                    None,
                    "key-3",
                ),
            )
            .expect_err("a Task is bounded by completion criteria");
        assert_eq!(unbounded.code, ErrorCode::InvalidRequest);
        assert_eq!(
            unbounded.message,
            "a Task Ticket carries completion criteria"
        );

        let blank = harness
            .core
            .command(
                "ticket.create",
                &bounded(
                    Some("manual"),
                    Some("human"),
                    json!(["   "]),
                    None,
                    None,
                    "key-4",
                ),
            )
            .expect_err("a completion criterion states its outcome");
        assert_eq!(
            blank.message,
            "a Ticket completion criterion cannot be blank"
        );

        let untimed = harness
            .core
            .command(
                "ticket.create",
                &bounded(
                    Some("operational"),
                    Some("agent"),
                    json!(["Done."]),
                    None,
                    Some("September"),
                    "key-5",
                ),
            )
            .expect_err("a due date names an RFC 3339 instant");
        assert_eq!(
            untimed.message,
            "a Task due date must be an RFC 3339 instant: `September`"
        );

        assert_eq!(
            harness.projects.rows()[0]
                .counters()
                .last(kanban_domain::NumberKind::Ticket),
            0,
            "a refused creation consumes no number"
        );
    }

    #[test]
    fn a_task_never_carries_story_linked_criteria() {
        let harness = ticket_harness();

        let mut request = bounded(
            Some("investigative"),
            Some("human"),
            json!(["The cause is named."]),
            None,
            None,
            "key-1",
        );
        request["criteria"] = json!([
            { "outcome": "A story claim.", "stories": ["CORE-S1-US1"] }
        ]);

        let error = harness
            .core
            .command("ticket.create", &request)
            .expect_err("a Task claims no User Story through criteria");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "a Task Ticket carries completion criteria, never story-linked criteria"
        );
    }

    #[test]
    fn task_fields_on_other_kinds_are_refused() {
        let harness = ticket_harness();
        let spec = super::testing::authored_spec(&harness.core, "key-author");

        let mut implementation = json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": "key-1" },
            "project_id": 1,
            "kind": "implementation",
            "priority": "normal",
            "spec_id": spec,
            "slice": "A slice",
            "criteria": [{ "outcome": "Done.", "stories": ["CORE-S1-US1"] }],
            "subtype": "operational",
        });
        let error = harness
            .core
            .command("ticket.create", &implementation)
            .expect_err("an Implementation carries no Task fields");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "an Implementation Ticket carries no Task fields"
        );

        implementation
            .as_object_mut()
            .expect("the request is an object")
            .remove("subtype");
        implementation["mode"] = json!("agent");
        let error = harness
            .core
            .command("ticket.create", &implementation)
            .expect_err("the mode is refused too");
        assert_eq!(
            error.message,
            "an Implementation Ticket carries no Task fields"
        );

        let mut bug = titled("bug", Some("A Bug"), None, "normal", "key-2");
        bug["completion"] = json!(["Done."]);
        let error = harness
            .core
            .command("ticket.create", &bug)
            .expect_err("a Bug carries no Task fields");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "a Bug Ticket carries no Task fields");
    }

    #[test]
    fn creating_without_the_kinds_own_fields_is_refused() {
        let harness = ticket_harness();
        let spec = super::testing::authored_spec(&harness.core, "key-author");

        let unattached = harness
            .core
            .command(
                "ticket.create",
                &implementation(None, Some("A slice"), one_criterion(), "normal", "key-1"),
            )
            .expect_err("an Implementation attaches to exactly one Spec");
        assert_eq!(unattached.code, ErrorCode::InvalidRequest);
        assert_eq!(
            unattached.message,
            "an Implementation Ticket attaches to exactly one Spec"
        );

        let untitled = harness
            .core
            .command(
                "ticket.create",
                &titled("bug", Some("  "), None, "normal", "key-2"),
            )
            .expect_err("a Bug carries a title");
        assert_eq!(untitled.code, ErrorCode::InvalidRequest);
        assert_eq!(untitled.message, "a Ticket title cannot be blank");

        let unsliced = harness
            .core
            .command(
                "ticket.create",
                &implementation(Some(spec), Some("   "), one_criterion(), "normal", "key-3"),
            )
            .expect_err("an Implementation carries a slice description");
        assert_eq!(unsliced.code, ErrorCode::InvalidRequest);
        assert_eq!(
            unsliced.message,
            "a Ticket slice description cannot be blank"
        );

        let unclaimed = harness
            .core
            .command(
                "ticket.create",
                &implementation(Some(spec), Some("A slice"), json!([]), "normal", "key-4"),
            )
            .expect_err("an Implementation carries story-linked criteria");
        assert_eq!(unclaimed.code, ErrorCode::InvalidRequest);
        assert_eq!(
            unclaimed.message,
            "an Implementation Ticket carries story-linked criteria"
        );

        assert_eq!(
            harness.projects.rows()[0]
                .counters()
                .last(kanban_domain::NumberKind::Ticket),
            0,
            "a refused creation consumes no number"
        );
    }

    #[test]
    fn criteria_claim_the_stories_of_the_spec_the_slice_delivers() {
        let harness = ticket_harness();
        let spec = super::testing::authored_spec(&harness.core, "key-author");

        let foreign = harness
            .core
            .command(
                "ticket.create",
                &implementation(
                    Some(spec),
                    Some("A slice"),
                    json!([
                        { "outcome": "Well linked, just not here.", "stories": ["CORE-S9-US1"] }
                    ]),
                    "normal",
                    "key-1",
                ),
            )
            .expect_err("another Spec's story claims nothing here");
        assert_eq!(foreign.code, ErrorCode::InvalidRequest);
        assert_eq!(
            foreign.message,
            "an Implementation Ticket claims the stories of the Spec it delivers; \
             S9-US1 names another Spec"
        );

        let malformed = harness
            .core
            .command(
                "ticket.create",
                &implementation(
                    Some(spec),
                    Some("A slice"),
                    json!([{ "outcome": "Any outcome.", "stories": ["banana"] }]),
                    "normal",
                    "key-2",
                ),
            )
            .expect_err("a story link names a User Story");
        assert_eq!(malformed.code, ErrorCode::InvalidRequest);
        assert_eq!(
            malformed.message,
            "a User Story is named like `CORE-S3-US6` or `S3-US6`"
        );
    }

    #[test]
    fn creating_for_an_unknown_project_or_spec_is_refused() {
        let harness = ticket_harness();

        let mut elsewhere = titled("bug", Some("A Bug"), None, "normal", "key-1");
        elsewhere["project_id"] = json!(9);
        let error = harness
            .core
            .command("ticket.create", &elsewhere)
            .expect_err("the unknown Project is refused");
        assert_eq!(error.code, ErrorCode::NotFound);

        let spec = super::testing::authored_spec(&harness.core, "key-author");
        let error = harness
            .core
            .command(
                "ticket.create",
                &implementation(
                    Some(spec + 9),
                    Some("A slice"),
                    one_criterion(),
                    "normal",
                    "key-3",
                ),
            )
            .expect_err("the unknown Spec is refused");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn creating_against_another_projects_spec_is_refused() {
        let harness = ticket_harness();
        harness.projects.seed(crate::plan::testing::active_project(
            2,
            "EDGE",
            kanban_domain::ProjectCounters::restore(0, 0, 0),
        ));
        let created = harness
            .core
            .command(
                "spec.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-other-spec" },
                    "project_id": 2,
                    "content": super::testing::wire_spec_content("Elsewhere"),
                }),
            )
            .expect("the other Project's Spec authors");
        let other_spec = created["id"].as_u64().expect("the identity is a number");

        let error = harness
            .core
            .command(
                "ticket.create",
                &titled("bug", Some("A Bug"), Some(other_spec), "normal", "key-1"),
            )
            .expect_err("a Ticket attaches to no other Project's Spec");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "the Spec belongs to another Project");
    }

    #[test]
    fn creating_for_an_archived_project_is_refused() {
        let harness = ticket_harness();
        let mut project = harness.projects.rows()[0].clone();
        project.archive().expect("the fixture archives");
        harness.projects.replace(project);

        let error = harness
            .core
            .command(
                "ticket.create",
                &titled("bug", Some("A Bug"), None, "normal", "key-1"),
            )
            .expect_err("an archived Project accepts no further changes");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("archived"));
    }

    #[test]
    fn creating_consumes_the_counter_without_reuse() {
        let harness = ticket_harness();
        let spec = super::testing::authored_spec(&harness.core, "key-author");

        let first = harness
            .core
            .command(
                "ticket.create",
                &implementation(Some(spec), Some("One"), one_criterion(), "normal", "key-1"),
            )
            .expect("the first Ticket creates");
        // A refused creation mints nothing, so the next number is
        // neither reused nor skipped past.
        let _ = harness
            .core
            .command(
                "ticket.create",
                &titled("bug", Some("  "), None, "normal", "key-refused"),
            )
            .expect_err("the blank title is refused");
        let second = harness
            .core
            .command(
                "ticket.create",
                &titled("task", Some("Two"), None, "normal", "key-2"),
            )
            .expect("the second Ticket creates");
        // Minting a Spec number must not move the Ticket counter.
        super::testing::authored_spec(&harness.core, "key-author-2");
        let third = harness
            .core
            .command(
                "ticket.create",
                &titled("bug", Some("Three"), None, "normal", "key-3"),
            )
            .expect("the third Ticket creates");

        assert_eq!(
            (
                first["number"].as_u64(),
                second["number"].as_u64(),
                third["number"].as_u64()
            ),
            (Some(1), Some(2), Some(3)),
            "numbers are monotonic and never reused"
        );
        assert_eq!(
            harness.projects.rows()[0].counters(),
            kanban_domain::ProjectCounters::restore(0, 2, 3),
            "each counter moves alone"
        );
    }

    #[test]
    fn creation_appends_a_timeline_row_on_the_projects_own_timeline() {
        let harness = ticket_harness();

        harness
            .core
            .command(
                "ticket.create",
                &titled(
                    "bug",
                    Some("Landing drops the integration branch"),
                    None,
                    "urgent",
                    "key-1",
                ),
            )
            .expect("the Ticket creates");

        let (tickets, timeline) = harness.tickets.snapshot();
        assert_eq!(tickets.len(), 1);
        let created = timeline.last().expect("the creation appended");
        assert_eq!(created.kind(), kanban_dto::TimelineEventKind::Transition);
        assert_eq!(
            created
                .entity()
                .map(|entity| (entity.kind, entity.id.clone())),
            Some((kanban_dto::TimelineEntityKind::Ticket, "1".to_owned()))
        );
        assert_eq!(
            created.detail(),
            &json!({
                "action": "created",
                "id": 1,
                "project_id": 1,
                "number": 1,
                "kind": "bug",
            })
        );
    }

    #[test]
    fn creation_publishes_on_the_event_stream() {
        let sink = std::sync::Arc::new(crate::plan::testing::RecordingSink::default());
        let harness = ticket_harness_with_sink(sink.clone());
        let spec = super::testing::authored_spec(&harness.core, "key-author");

        harness
            .core
            .command(
                "ticket.create",
                &implementation(
                    Some(spec),
                    Some("A slice"),
                    one_criterion(),
                    "high",
                    "key-1",
                ),
            )
            .expect("the Ticket creates");

        let events = sink.events.lock().expect("the recorder lock is sound");
        let created = events
            .iter()
            .find(|(name, _)| name == "ticket.created")
            .expect("creation announces live");
        assert_eq!(created.1["kind"], json!("implementation"));
        assert_eq!(created.1["number"], json!(1));
    }

    #[test]
    fn a_retry_replays_without_reapplying() {
        let harness = ticket_harness();
        let request = titled(
            "bug",
            Some("Landing drops the integration branch"),
            None,
            "normal",
            "key-1",
        );

        let first = harness
            .core
            .command("ticket.create", &request)
            .expect("the Ticket creates");
        let replay = harness
            .core
            .command("ticket.create", &request)
            .expect("the retry replays");

        assert_eq!(first, replay);
        let (tickets, _) = harness.tickets.snapshot();
        assert_eq!(tickets.len(), 1, "the retry must not reapply");
    }

    #[test]
    fn commands_reject_unknown_fields() {
        let harness = ticket_harness();
        let mut request = titled("bug", Some("A Bug"), None, "normal", "key-1");
        request["surprise"] = json!(true);

        let error = harness
            .core
            .command("ticket.create", &request)
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }

    #[test]
    fn no_ticket_delete_operation_is_catalogued() {
        let names: Vec<_> = crate::catalog::exposed_operations()
            .iter()
            .map(|operation| operation.name)
            .collect();
        assert!(
            !names.contains(&"ticket.delete") && !names.contains(&"ticket.remove"),
            "Tickets are superseded or cancelled, never deleted"
        );
    }
}

#[cfg(test)]
mod ticket_queries {
    use serde_json::json;

    use super::testing::ticket_harness;

    /// Three Tickets of the seeded Project: an Implementation, a Bug,
    /// and a Task, numbers one through three.
    fn three_tickets(harness: &super::testing::TicketHarness) {
        let spec = super::testing::authored_spec(&harness.core, "key-author");
        for (kind, key) in [
            ("implementation", "key-1"),
            ("bug", "key-2"),
            ("task", "key-3"),
        ] {
            let mut request = json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": key },
                "project_id": 1,
                "kind": kind,
                "priority": "normal",
            });
            if kind == "implementation" {
                request["spec_id"] = json!(spec);
                request["slice"] = json!("Spec authoring creates content versions end to end");
                request["criteria"] = json!([
                    { "outcome": "Specs mint unique numbers.", "stories": ["CORE-S1-US1"] }
                ]);
            } else {
                request["title"] = json!("Landing drops the integration branch");
                if kind == "bug" {
                    request["actual_behaviour"] =
                        json!("The integration branch is dropped after a review lands.");
                    request["reporter_evidence"] =
                        json!("The landing log names the drop immediately after the merge.");
                }
                if kind == "task" {
                    request["subtype"] = json!("administrative");
                    request["mode"] = json!("human");
                    request["completion"] = json!(["The old register is archived."]);
                }
            }
            harness
                .core
                .command("ticket.create", &request)
                .expect("the Ticket creates");
        }
    }

    #[test]
    fn listing_covers_every_ticket_of_one_project() {
        let harness = ticket_harness();
        three_tickets(&harness);

        let listed = harness
            .core
            .query("ticket.list", &json!({ "project_id": 1 }))
            .expect("the list serves");

        let numbers: Vec<_> = listed["tickets"]
            .as_array()
            .expect("the tickets are a list")
            .iter()
            .map(|ticket| ticket["number"].clone())
            .collect();
        assert_eq!(numbers, vec![json!(1), json!(2), json!(3)]);
        assert!(
            harness
                .core
                .query("ticket.list", &json!({ "project_id": 9 }))
                .expect("the list serves")["tickets"]
                .as_array()
                .expect("the tickets are a list")
                .is_empty(),
            "another Project's Tickets stay out"
        );
    }

    #[test]
    fn reading_one_ticket_returns_its_record() {
        let harness = ticket_harness();
        three_tickets(&harness);

        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": 1 }))
            .expect("the get serves");

        assert_eq!(read["id"], json!(1));
        assert_eq!(read["kind"], json!("implementation"));
        assert_eq!(
            read["slice"],
            json!("Spec authoring creates content versions end to end")
        );

        let error = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": 9 }))
            .expect_err("an unknown Ticket is refused");
        assert_eq!(error.code, kanban_dto::ErrorCode::NotFound);
    }
}

#[cfg(test)]
mod bug_capture {
    use kanban_dto::ErrorCode;
    use serde_json::{Value, json};

    use super::testing::{captured_bug, ticket_harness, wire_qualification};

    /// A qualification request body for `ticket`, varied by severity.
    fn qualify(ticket: u64, severity: &str, key: &str) -> Value {
        json!({
            "mutation": { "optimistic_version": 1, "idempotency_key": key },
            "ticket_id": ticket,
            "qualification": wire_qualification(severity),
        })
    }

    /// A facts request body for `ticket`.
    fn facts(ticket: u64, key: &str) -> Value {
        json!({
            "mutation": { "optimistic_version": 1, "idempotency_key": key },
            "ticket_id": ticket,
            "external_references": [
                {
                    "uri": "https://example.invalid/issues/12",
                    "label": "The report",
                }
            ],
            "occurrence_snapshots": [
                {
                    "observed_at": "2026-09-05T07:41:00Z",
                    "observation": "The log shows the drop.",
                }
            ],
            "evidence_ids": [],
        })
    }

    #[test]
    fn quick_capture_creates_a_bug_from_three_facts_and_nothing_else() {
        let harness = ticket_harness();

        let created = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-capture" },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "urgent",
                    "title": "Landing drops the integration branch",
                    "actual_behaviour":
                        "The integration branch is dropped after a review lands.",
                    "reporter_evidence":
                        "The landing log names the drop immediately after the merge.",
                }),
            )
            .expect("three facts are the whole of quick capture (DR-TK-08)");

        assert_eq!(created["kind"], json!("bug"));
        assert_eq!(created["state"], json!("draft"));
        assert_eq!(created["spec_id"], json!(null), "no Spec is required");
        assert_eq!(
            created["bug"]["actual_behaviour"],
            json!("The integration branch is dropped after a review lands.")
        );
        assert_eq!(
            created["bug"]["reporter_evidence"],
            json!("The landing log names the drop immediately after the merge.")
        );
        assert!(
            created["bug"].get("qualification").is_none(),
            "no qualification is required"
        );
    }

    #[test]
    fn quick_capture_refuses_a_missing_capture_fact_without_minting() {
        let harness = ticket_harness();

        for (field, message) in [
            (
                "actual_behaviour",
                "a Ticket actual behaviour cannot be blank",
            ),
            (
                "reporter_evidence",
                "a Ticket reporter evidence cannot be blank",
            ),
        ] {
            let mut request = json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "key-refused" },
                "project_id": 1,
                "kind": "bug",
                "priority": "normal",
                "title": "Landing drops the integration branch",
                "actual_behaviour": "The integration branch is dropped.",
                "reporter_evidence": "The landing log names the drop.",
            });
            request[field] = json!("   ");
            let error = harness.core.command("ticket.create", &request).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidRequest);
            assert_eq!(error.message, message);
        }

        assert_eq!(
            harness.projects.rows()[0]
                .counters()
                .last(kanban_domain::NumberKind::Ticket),
            0,
            "a refused capture consumes no number"
        );
    }

    #[test]
    fn qualifying_records_the_whole_qualification_and_keeps_the_bug_draft() {
        let harness = ticket_harness();
        let bug = captured_bug(&harness.core, "key-capture");

        let qualified = harness
            .core
            .command(
                "ticket.bug.qualify",
                &qualify(bug, "critical", "key-qualify"),
            )
            .expect("the Bug qualifies");

        assert_eq!(qualified["state"], json!("draft"));
        assert_eq!(qualified["version"], json!(2));
        assert_eq!(
            qualified["bug"]["qualification"]["severity"],
            json!("critical"),
            "qualification sets the severity (DR-LC-13)"
        );
        assert_eq!(
            qualified["bug"]["qualification"]["expected_behaviour"],
            json!("The integration branch survives every landing.")
        );
        assert_eq!(
            qualified["bug"]["qualification"]["criteria"],
            json!([{
                "outcome": "The integration branch survives a landing.",
                "stories": ["CORE-S1-US1"],
            }])
        );

        let (_, timeline) = harness.tickets.snapshot();
        let appended = timeline.last().expect("the qualification appended");
        assert_eq!(
            appended.detail(),
            &json!({
                "action": "qualified",
                "id": bug,
                "severity": "critical",
                "version": 2,
            })
        );
    }

    #[test]
    fn an_incomplete_qualification_is_refused_with_its_missing_fact() {
        let harness = ticket_harness();
        let bug = captured_bug(&harness.core, "key-capture");

        for (field, message) in [
            ("environment", "a Ticket environment cannot be blank"),
            ("risk", "a Ticket risk cannot be blank"),
        ] {
            let mut request = qualify(bug, "high", "key-refused");
            request["qualification"][field] = json!("");
            let error = harness
                .core
                .command("ticket.bug.qualify", &request)
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidRequest);
            assert_eq!(error.message, message);
        }

        let mut no_steps = qualify(bug, "high", "key-refused-steps");
        no_steps["qualification"]["verification_steps"] = json!([]);
        let error = harness
            .core
            .command("ticket.bug.qualify", &no_steps)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "a Ticket Verification Steps claim cannot be blank"
        );

        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": bug }))
            .expect("the get serves");
        assert_eq!(
            read["version"],
            json!(1),
            "every refusal left the Bug as it stood"
        );
    }

    #[test]
    fn a_severity_outside_the_closed_vocabulary_is_refused() {
        let harness = ticket_harness();
        let bug = captured_bug(&harness.core, "key-capture");

        let error = harness
            .core
            .command("ticket.bug.qualify", &qualify(bug, "urgent", "key-refused"))
            .expect_err("urgent is a priority, not a severity");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("urgent"),
            "the refusal names the unknown value: {}",
            error.message
        );
    }

    #[test]
    fn qualifying_a_non_bug_or_unknown_ticket_is_refused() {
        let harness = ticket_harness();
        let spec = super::testing::authored_spec(&harness.core, "key-author");
        let task = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-task" },
                    "project_id": 1,
                    "kind": "task",
                    "priority": "normal",
                    "title": "Archive the old register",
                    "subtype": "administrative",
                    "mode": "human",
                    "completion": ["The old register is archived."],
                }),
            )
            .expect("the Task creates");
        let _ = spec;

        let not_a_bug = harness
            .core
            .command(
                "ticket.bug.qualify",
                &qualify(
                    task["id"].as_u64().expect("the identity is a number"),
                    "low",
                    "key-1",
                ),
            )
            .expect_err("only a Bug carries qualification");
        assert_eq!(not_a_bug.code, ErrorCode::InvalidRequest);
        assert_eq!(
            not_a_bug.message,
            "only a Bug Ticket carries qualification and Bug facts"
        );

        let unknown = harness
            .core
            .command("ticket.bug.qualify", &qualify(99, "low", "key-2"))
            .expect_err("an unknown Ticket is refused");
        assert_eq!(unknown.code, ErrorCode::NotFound);
    }

    #[test]
    fn a_stale_qualification_is_refused_by_the_optimistic_version() {
        let harness = ticket_harness();
        let bug = captured_bug(&harness.core, "key-capture");

        let mut stale = qualify(bug, "high", "key-stale");
        stale["mutation"]["optimistic_version"] = json!(0);
        let error = harness
            .core
            .command("ticket.bug.qualify", &stale)
            .expect_err("the version guard refuses the stale command");

        assert_eq!(error.code, ErrorCode::StaleVersion);
        let (_, timeline) = harness.tickets.snapshot();
        assert_eq!(
            timeline.len(),
            1,
            "a stale qualification appends no timeline row"
        );
    }

    #[test]
    fn a_qualification_retry_replays_without_reapplying() {
        let harness = ticket_harness();
        let bug = captured_bug(&harness.core, "key-capture");
        let request = qualify(bug, "medium", "key-once");

        let first = harness
            .core
            .command("ticket.bug.qualify", &request)
            .expect("the Bug qualifies");
        let replay = harness
            .core
            .command("ticket.bug.qualify", &request)
            .expect("the retry replays");

        assert_eq!(first, replay);
        assert_eq!(
            replay["version"],
            json!(2),
            "the retry must not reapply the change"
        );
    }

    #[test]
    fn recording_facts_carries_references_snapshots_and_evidence_items() {
        let harness = ticket_harness();
        let bug = captured_bug(&harness.core, "key-capture");
        let evidence = harness
            .evidence
            .seed_repository_item(1, "ticket", &bug.to_string());

        let mut request = facts(bug, "key-facts");
        request["evidence_ids"] = json!([evidence.value()]);
        let recorded = harness
            .core
            .command("ticket.bug.facts", &request)
            .expect("the Bug carries its facts");

        assert_eq!(recorded["version"], json!(2));
        assert_eq!(
            recorded["bug"]["external_references"],
            json!([{
                "uri": "https://example.invalid/issues/12",
                "label": "The report",
            }]),
            "the references are vendor-neutral URIs (DR-TK-10)"
        );
        assert_eq!(
            recorded["bug"]["occurrence_snapshots"],
            json!([{
                "observed_at": "2026-09-05T07:41:00Z",
                "observation": "The log shows the drop.",
            }])
        );
        assert_eq!(recorded["bug"]["evidence_ids"], json!([evidence.value()]));

        // Replacing the collections is one whole act: the empty set
        // clears what stood.
        let mut replaced = facts(bug, "key-facts-2");
        replaced["mutation"]["optimistic_version"] = json!(2);
        replaced["external_references"] = json!([]);
        replaced["occurrence_snapshots"] = json!([]);
        let replaced = harness
            .core
            .command("ticket.bug.facts", &replaced)
            .expect("the facts replace whole");
        assert_eq!(replaced["version"], json!(3));
        assert_eq!(replaced["bug"]["external_references"], json!([]));
        assert_eq!(replaced["bug"]["occurrence_snapshots"], json!([]));
        assert_eq!(replaced["bug"]["evidence_ids"], json!([]));
    }

    #[test]
    fn naming_evidence_attached_elsewhere_is_refused() {
        let harness = ticket_harness();
        let bug = captured_bug(&harness.core, "key-capture");
        let _other = captured_bug(&harness.core, "key-other");
        let elsewhere = harness.evidence.seed_repository_item(1, "ticket", "2");

        let mut request = facts(bug, "key-refused");
        request["evidence_ids"] = json!([elsewhere.value()]);
        let error = harness
            .core
            .command("ticket.bug.facts", &request)
            .expect_err("a Bug names only its own Evidence Items");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            format!(
                "evidence item {} is not attached to ticket 1",
                elsewhere.value()
            )
        );
    }

    #[test]
    fn recording_facts_on_a_non_bug_is_refused() {
        let harness = ticket_harness();
        let task = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-task" },
                    "project_id": 1,
                    "kind": "task",
                    "priority": "normal",
                    "title": "Archive the old register",
                    "subtype": "administrative",
                    "mode": "human",
                    "completion": ["The old register is archived."],
                }),
            )
            .expect("the Task creates");

        let error = harness
            .core
            .command(
                "ticket.bug.facts",
                &facts(
                    task["id"].as_u64().expect("the identity is a number"),
                    "key-1",
                ),
            )
            .expect_err("only a Bug carries Bug facts");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn bug_commands_reject_unknown_fields_and_malformed_entries() {
        let harness = ticket_harness();
        let bug = captured_bug(&harness.core, "key-capture");

        let mut surprise = qualify(bug, "high", "key-1");
        surprise["surprise"] = json!(true);
        let error = harness
            .core
            .command("ticket.bug.qualify", &surprise)
            .expect_err("unknown fields are rejected");
        assert_eq!(error.code, ErrorCode::UnknownField);

        let mut malformed = facts(bug, "key-2");
        malformed["external_references"] = json!([{ "uri": "no-scheme-here" }]);
        let error = harness
            .core
            .command("ticket.bug.facts", &malformed)
            .expect_err("a reference names a URI");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "an External Reference names a URI with a scheme, like `https://example.invalid/1`"
        );

        let mut malformed = facts(bug, "key-3");
        malformed["occurrence_snapshots"] =
            json!([{ "observed_at": "yesterday", "observation": "Seen." }]);
        let error = harness
            .core
            .command("ticket.bug.facts", &malformed)
            .expect_err("a snapshot names an RFC 3339 moment");
        assert_eq!(
            error.message,
            "an Occurrence Snapshot names an RFC 3339 moment"
        );
    }

    #[test]
    fn the_created_bug_round_trips_through_the_queries() {
        let harness = ticket_harness();
        let bug = captured_bug(&harness.core, "key-capture");
        harness
            .core
            .command("ticket.bug.qualify", &qualify(bug, "high", "key-qualify"))
            .expect("the Bug qualifies");

        let read = harness
            .core
            .query("ticket.get", &json!({ "ticket_id": bug }))
            .expect("the get serves");
        assert_eq!(read["bug"]["qualification"]["severity"], json!("high"));

        let listed = harness
            .core
            .query("ticket.list", &json!({ "project_id": 1 }))
            .expect("the list serves");
        assert_eq!(
            listed["tickets"][0]["bug"]["qualification"]["severity"],
            json!("high")
        );
    }
}
