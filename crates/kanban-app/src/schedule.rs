//! The Schedule storage port and the activation pass the core
//! service's scheduler drives (KAN-T53, DR-SA-01 to DR-SA-06). The
//! port lands one Schedule with the Ticket it holds in a single
//! write, answers which one-time activations have come due on live
//! Projects, and spends a fired activation atomically under the
//! archived guard. The pass applies the domain's activation rule to
//! each due Schedule and announces the Tickets it made ready. The
//! scheduling command itself stays with the lifecycle
//! (`ticket.schedule`), which now carries the Schedule's facts when
//! the operator sets one.

use std::sync::Arc;

use kanban_domain::{
    Project, Readiness, ReadinessInputs, Schedule, ScheduleId, Ticket, TicketDependencyGraph,
    compute_readiness,
};
use kanban_dto::ApiError;

use crate::dependency::DependencyStore;
use crate::events::{EventSink, emit_catalogued};
use crate::ticket::{TicketStore, record_of};
use crate::timeline::TimelineEnvelope;

/// One due one-time activation: the waiting Schedule whose moment
/// arrived, the Ticket it holds, and that Ticket's Project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueActivation {
    /// The waiting Schedule, rehydrated with its stored identity.
    pub schedule: Schedule,
    /// The Ticket the Schedule holds.
    pub ticket: Ticket,
    /// The Project the Ticket belongs to.
    pub project: Project,
}

impl DueActivation {
    /// The stored identity of the due Schedule. Storage rehydrates
    /// every due Schedule with its identity, so a due activation
    /// always names one.
    pub fn id(&self) -> ScheduleId {
        self.schedule
            .id()
            .expect("storage rehydrates a due Schedule with its identity")
    }
}

/// The storage port the scheduling surface and the activation pass
/// call through. Implementations land the Schedule row, the Ticket
/// row it holds, and the timeline envelope unchanged inside one
/// write, so a Schedule, its Ticket, and its audit row never split
/// across a crash boundary.
pub trait ScheduleStore: Send + Sync {
    /// Land one waiting one-time Schedule with the Ticket it holds:
    /// the Schedule row, the Ticket row — already moved to scheduled
    /// by the domain, guarded by the version the aggregate moved from
    /// — and the timeline envelope, all in one write. Storage assigns
    /// the Schedule's identity and asks `envelope` for the timeline
    /// row that identity belongs in.
    fn attach(
        &self,
        ticket: &Ticket,
        schedule: &Schedule,
        envelope: &dyn Fn(ScheduleId) -> TimelineEnvelope,
    ) -> Result<Schedule, ApiError>;

    /// Every waiting one-time Schedule whose activation has come due
    /// at `now` — a stored-shape instant — with the Ticket it holds
    /// and that Ticket's Project, in activation order. Archived is
    /// terminal, so a Schedule an archived Project holds never
    /// answers; it waits outside every scan.
    fn due(&self, now: &str) -> Result<Vec<DueActivation>, ApiError>;

    /// Spend one due activation: mark the Schedule fired, move the
    /// Ticket row when `moved` carries the activated Ticket, and
    /// append the timeline envelope, all in one write guarded by the
    /// Schedule's waiting state, the Ticket's version, and the
    /// Project staying live. `Ok(false)` names an activation another
    /// writer already spent, or one whose Project archived since the
    /// scan.
    fn fire(
        &self,
        due: &DueActivation,
        moved: Option<&Ticket>,
        fired_at: &str,
        envelope: TimelineEnvelope,
    ) -> Result<bool, ApiError>;
}

/// What one activation pass did: how many one-time activations fired
/// and how many due Schedules it left waiting for a later tick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActivationReport {
    /// Activations that made their Ticket ready or spent an
    /// already-circulating one.
    pub fired: usize,
    /// Due activations the rules still hold back — a readiness gate,
    /// or a Ticket no activation may move — which stay waiting for a
    /// later tick to retry.
    pub skipped: usize,
}

/// The activation pass the core service's scheduler owns (DR-SA-03,
/// DR-SA-06): read every due one-time activation fresh, apply the
/// domain's activation rule, spend the Schedule whatever the rule did
/// — a held-back Ticket refuses and waits — and announce each Ticket
/// the pass made ready. Nothing here owns a clock; the scheduler
/// hands `now` in as a stored-shape instant.
pub struct ActivationPass {
    tickets: Arc<dyn TicketStore>,
    dependencies: Arc<dyn DependencyStore>,
    schedules: Arc<dyn ScheduleStore>,
}

impl ActivationPass {
    /// Wire the pass over the stores the serving core shares.
    pub fn new(
        tickets: Arc<dyn TicketStore>,
        dependencies: Arc<dyn DependencyStore>,
        schedules: Arc<dyn ScheduleStore>,
    ) -> Self {
        Self {
            tickets,
            dependencies,
            schedules,
        }
    }

    /// Fire every one-time activation due at `now`, announcing each
    /// Ticket the pass made ready on `events`. One refused activation
    /// never stops the pass: it stays waiting and the report counts
    /// it skipped, so the operator's later tick — or the restart pass
    /// — retries it (DR-SA-06).
    pub fn fire_due(
        &self,
        now: &str,
        events: &dyn EventSink,
    ) -> Result<ActivationReport, ApiError> {
        let mut report = ActivationReport::default();
        for due in self.schedules.due(now)? {
            let mut ticket = due.ticket.clone();
            let readiness = self.readiness_of(&ticket)?;
            match due.schedule.activate(&mut ticket, &readiness) {
                Ok(kanban_domain::Activation::BecameReady) => {
                    let from = due.ticket.state().wire_name().to_owned();
                    let envelope = activated_envelope(&due, &from, ticket.state().wire_name());
                    if self.schedules.fire(&due, Some(&ticket), now, envelope)? {
                        emit_catalogued(
                            events,
                            kanban_dto::LiveEventName::TicketStateChanged,
                            &record_of(&ticket, due.project.code()),
                        );
                        report.fired += 1;
                    }
                }
                Ok(kanban_domain::Activation::AlreadyCirculating) => {
                    let state = ticket.state().wire_name().to_owned();
                    let envelope = activated_envelope(&due, &state, &state);
                    if self.schedules.fire(&due, None, now, envelope)? {
                        report.fired += 1;
                    }
                }
                Err(_) => report.skipped += 1,
            }
        }
        Ok(report)
    }

    /// The readiness an activation answers: the projection KAN-T20
    /// computes from the Ticket's dependencies and external blockers,
    /// read fresh, never widened.
    fn readiness_of(&self, ticket: &Ticket) -> Result<Readiness, ApiError> {
        let mut states = Vec::new();
        for edge in TicketDependencyGraph::restore(self.dependencies.list_dependencies()?)
            .required_by(ticket.id())
        {
            let blocking = self.tickets.find(edge.from())?.ok_or_else(|| {
                ApiError::internal(&format!(
                    "dependency {} names no stored Ticket",
                    edge.from().value()
                ))
            })?;
            states.push(kanban_domain::DependencyState {
                dependency: edge,
                state: blocking.state(),
            });
        }
        let blockers = self.dependencies.blockers_of(ticket.id())?;
        Ok(compute_readiness(ReadinessInputs {
            dependencies: &states,
            blockers: &blockers,
        }))
    }
}

/// The timeline row one spent activation appends: on the Project's
/// own timeline, about the Ticket, naming both states and the
/// Schedule that moved between them.
fn activated_envelope(due: &DueActivation, from: &str, to: &str) -> TimelineEnvelope {
    TimelineEnvelope::project(
        due.project.id().value(),
        kanban_dto::TimelineEventKind::Transition,
        Some(kanban_dto::TimelineEntityRef {
            kind: kanban_dto::TimelineEntityKind::Ticket,
            id: due.ticket.id().value().to_string(),
        }),
        serde_json::json!({
            "action": "activated",
            "id": due.ticket.id().value(),
            "from": from,
            "to": to,
            "schedule": due.id().value(),
            "activation": due.schedule.next_activation(),
        }),
    )
}

#[cfg(test)]
mod activation_pass {
    use std::sync::Arc;

    use serde_json::json;

    use super::{ActivationPass, ScheduleStore};
    use crate::lifecycle::testing::LifecycleRows;
    use crate::plan::testing::{MemoryProjects, RecordingSink, active_project};
    use crate::project::ProjectStore;
    use crate::ticket::TicketStore;
    use kanban_domain::{
        NumberKind, Priority, ProjectCounters, ProjectId, Schedule, ScheduleState, TaskMode,
        TaskSubtype, TaskTiming, Ticket, TicketBody, TicketNumber, TicketState,
    };

    /// The moment every due fixture activates at, and the later
    /// moment the pass reads.
    const ACTIVATION: &str = "2026-09-10T09:00:00Z";
    const NOW: &str = "2026-09-11T00:00:00Z";

    /// One harness over the shared in-memory rows: Tickets, edges,
    /// Projects, and Schedules all live in one store, the way the
    /// serving core shares its stores.
    struct PassHarness {
        rows: Arc<LifecycleRows>,
        projects: Arc<MemoryProjects>,
        pass: ActivationPass,
    }

    fn harness() -> PassHarness {
        let projects = Arc::new(MemoryProjects::default());
        projects.seed(active_project(1, "CORE", ProjectCounters::restore(0, 0, 0)));
        let rows = Arc::new(LifecycleRows::sharing(projects.clone()));
        let pass = ActivationPass::new(rows.clone(), rows.clone(), rows.clone());
        PassHarness {
            rows,
            projects,
            pass,
        }
    }

    /// One Task body, the bounded kind a Schedule may hold.
    fn task_body() -> TicketBody {
        TicketBody::task(
            "Archive the old register",
            None,
            Some(TaskSubtype::Operational),
            Some(TaskMode::Human),
            vec![
                kanban_domain::CompletionCriterion::new("The register is archived.")
                    .expect("the fixture outcome binds"),
            ],
            TaskTiming::none(),
        )
        .expect("the fixture body validates")
    }

    /// Create one Task Ticket under the Project `project` through the
    /// Ticket port and return the stored aggregate.
    fn created_under(harness: &PassHarness, project: ProjectId) -> Ticket {
        let mut aggregate = harness
            .projects
            .find(project)
            .expect("the reload serves")
            .expect("the Project exists");
        let number = TicketNumber::new(aggregate.mint(NumberKind::Ticket).expect("active mints"))
            .expect("a minted number is positive");
        let scope = aggregate.id().value();
        harness
            .rows
            .create(&aggregate, number, Priority::Normal, &task_body(), &|id| {
                envelope(
                    scope,
                    id.value(),
                    "created",
                    json!({ "from": "none", "to": "draft" }),
                )
            })
            .expect("the fixture Ticket lands")
    }

    /// Create one Task Ticket under the seeded live Project.
    fn created(harness: &PassHarness) -> Ticket {
        created_under(harness, ProjectId::new(1))
    }

    /// Schedule `ticket` under the Project `project` through the
    /// port, the way the command will: the moved aggregate and the
    /// Schedule land together.
    fn scheduled_under(harness: &PassHarness, project: ProjectId, ticket: &Ticket) {
        let moved = Ticket::restore(
            ticket.id(),
            ticket.project(),
            ticket.number(),
            ticket.priority(),
            TicketState::Scheduled,
            ticket.body().clone(),
            ticket.profile().cloned(),
            ticket.version() + 1,
        );
        ScheduleStore::attach(
            &*harness.rows,
            &moved,
            &Schedule::one_time(ticket.id(), ACTIVATION, "UTC", "standard")
                .expect("the fixture schedule validates"),
            &|id| {
                envelope(
                    project.value(),
                    ticket.id().value(),
                    "scheduled",
                    json!({ "from": "draft", "to": "scheduled", "schedule": id.value() }),
                )
            },
        )
        .expect("the fixture schedule lands");
    }

    /// Schedule `ticket` under the seeded live Project.
    fn scheduled(harness: &PassHarness, ticket: &Ticket) {
        scheduled_under(harness, ProjectId::new(1), ticket);
    }

    /// The timeline envelope one Ticket change lands, scoped to the
    /// Project `project`.
    fn envelope(
        project: u64,
        ticket: u64,
        action: &str,
        facts: serde_json::Value,
    ) -> crate::timeline::TimelineEnvelope {
        let mut detail = facts;
        let object = detail.as_object_mut().expect("the facts are an object");
        object.insert("action".to_owned(), serde_json::Value::from(action));
        object.insert("id".to_owned(), serde_json::Value::from(ticket));
        crate::timeline::TimelineEnvelope::project(
            project,
            kanban_dto::TimelineEventKind::Transition,
            Some(kanban_dto::TimelineEntityRef {
                kind: kanban_dto::TimelineEntityKind::Ticket,
                id: ticket.to_string(),
            }),
            detail,
        )
    }

    #[test]
    fn the_pass_fires_a_due_activation_and_announces_the_ready_ticket() {
        let harness = harness();
        let task = created(&harness);
        scheduled(&harness, &task);
        let sink = RecordingSink::default();

        let report = harness.pass.fire_due(NOW, &sink).expect("the pass serves");

        assert_eq!(report.fired, 1);
        assert_eq!(report.skipped, 0);
        let (tickets, _timeline) = harness.rows.snapshot();
        assert_eq!(tickets[0].state(), TicketState::Ready, "DR-SA-03");
        assert_eq!(tickets[0].version(), 3);
        assert_eq!(
            harness.rows.schedules()[0].state(),
            ScheduleState::Fired,
            "the Schedule spent"
        );
        let events = sink.events.lock().expect("the recorder lock is sound");
        assert!(
            events
                .iter()
                .any(|(name, payload)| name == "ticket.state.changed"
                    && payload["id"] == json!(task.id().value())
                    && payload["state"] == json!("ready")),
            "the made-ready Ticket announces live, got {events:?}"
        );
        assert!(
            harness.rows.due(NOW).expect("the rescan serves").is_empty(),
            "the spent Schedule is due never again"
        );
    }

    #[test]
    fn an_archived_projects_schedule_never_fires_while_a_live_one_does() {
        let harness = harness();
        // A second Project holds one due Schedule, then archives.
        harness
            .projects
            .seed(active_project(2, "OLD", ProjectCounters::restore(0, 0, 0)));
        let retired_task = created_under(&harness, ProjectId::new(2));
        scheduled_under(&harness, ProjectId::new(2), &retired_task);
        let mut retired = harness
            .projects
            .find(ProjectId::new(2))
            .expect("the reload serves")
            .expect("the Project exists");
        retired.archive().expect("the fixture Project archives");
        harness.projects.replace(retired);
        // A live Project's due Schedule stands beside it in the same
        // tick.
        let live = created(&harness);
        scheduled(&harness, &live);
        let sink = RecordingSink::default();

        let report = harness.pass.fire_due(NOW, &sink).expect("the pass serves");

        assert_eq!(
            report.fired, 1,
            "only the live Project's Schedule fired; the exclusion aborted no tick"
        );
        assert_eq!(report.skipped, 0);
        let (tickets, timeline) = harness.rows.snapshot();
        let archived_row = tickets
            .iter()
            .find(|row| row.id() == retired_task.id())
            .expect("the archived Ticket stands");
        assert_eq!(
            archived_row.state(),
            TicketState::Scheduled,
            "archival prevents the transition"
        );
        assert_eq!(archived_row.version(), 2, "the pass moved nothing");
        let live_row = tickets
            .iter()
            .find(|row| row.id() == live.id())
            .expect("the live Ticket stands");
        assert_eq!(live_row.state(), TicketState::Ready, "the live one fired");
        assert_eq!(live_row.version(), 3);
        assert_eq!(
            harness
                .rows
                .schedules()
                .iter()
                .find(|schedule| schedule.ticket() == retired_task.id())
                .expect("the archived Schedule stands")
                .state(),
            ScheduleState::Waiting,
            "the archived Project's Schedule spends nothing"
        );
        assert!(
            !timeline.iter().any(|row| {
                row.detail()["id"] == json!(retired_task.id().value())
                    && row.detail()["action"] == json!("activated")
            }),
            "archival prevents the activation timeline entry"
        );
        assert!(
            timeline.iter().any(|row| {
                row.detail()["id"] == json!(live.id().value())
                    && row.detail()["action"] == json!("activated")
            }),
            "the live activation still appends"
        );
        let events = sink.events.lock().expect("the recorder lock is sound");
        assert_eq!(events.len(), 1, "only the live Ticket announces");
        assert!(
            events
                .iter()
                .any(|(name, payload)| name == "ticket.state.changed"
                    && payload["id"] == json!(live.id().value())
                    && payload["state"] == json!("ready")),
            "the made-ready Ticket announces live, got {events:?}"
        );
        assert!(
            harness.rows.due(NOW).expect("the rescan serves").is_empty(),
            "the archived Project's Schedule stays outside every scan"
        );
    }

    #[test]
    fn a_held_back_activation_is_skipped_and_stays_waiting() {
        let harness = harness();
        let waiting = created(&harness);
        scheduled(&harness, &waiting);
        // An unlanded dependency holds the waiting Ticket back.
        let blocker = created(&harness);
        harness
            .rows
            .seed_edge(blocker.id().value(), waiting.id().value());

        let report = harness
            .pass
            .fire_due(NOW, &RecordingSink::default())
            .expect("the pass serves");

        assert_eq!(report.fired, 0);
        assert_eq!(report.skipped, 1, "the refusal is reported, not raised");
        let (tickets, _) = harness.rows.snapshot();
        assert_eq!(tickets[0].state(), TicketState::Scheduled);
        assert_eq!(tickets[0].version(), 2, "the refusal moved nothing");
        assert_eq!(harness.rows.schedules()[0].state(), ScheduleState::Waiting);
        assert_eq!(
            harness.rows.due(NOW).expect("the rescan serves").len(),
            1,
            "a later tick — or the restart pass — retries the activation"
        );
    }

    #[test]
    fn an_already_circulating_ticket_spends_its_schedule_quietly() {
        let harness = harness();
        let task = created(&harness);
        scheduled(&harness, &task);
        harness
            .rows
            .force_state(task.id().value(), TicketState::Ready);
        let sink = RecordingSink::default();

        let report = harness.pass.fire_due(NOW, &sink).expect("the pass serves");

        assert_eq!(report.fired, 1);
        assert_eq!(report.skipped, 0);
        assert_eq!(
            harness.rows.schedules()[0].state(),
            ScheduleState::Fired,
            "the Schedule still spends"
        );
        let (tickets, timeline) = harness.rows.snapshot();
        // force_state counts its own change; the pass adds none.
        assert_eq!(tickets[0].version(), 3, "the pass moved nothing");
        assert_eq!(tickets[0].state(), TicketState::Ready);
        let events = sink.events.lock().expect("the recorder lock is sound");
        assert!(
            events.is_empty(),
            "a Ticket that never moved announces nothing"
        );
        let appended = timeline.last().expect("the activation appended");
        assert_eq!(
            appended.detail()["action"],
            json!("activated"),
            "the audit row still records the spent activation"
        );
    }

    #[test]
    fn an_empty_scan_reports_nothing() {
        let harness = harness();
        created(&harness);

        let report = harness
            .pass
            .fire_due(NOW, &RecordingSink::default())
            .expect("the pass serves an empty scan");

        assert_eq!(report.fired, 0);
        assert_eq!(report.skipped, 0);
    }
}
