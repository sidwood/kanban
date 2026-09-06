//! The global board query (DR-BP-01): one read-only projection of
//! every Project's work through the domain's filter and ordering
//! rules. The query gathers each Ticket beside the facts its Project,
//! Spec, Lane, and Execution Profile assignment resolve, hands the
//! domain rule the ten filter axes as one intersection, and returns
//! the cards already placed in their fixed groups and already in the
//! deterministic order — a client renders the projection, it never
//! recomputes it. The attention axis is complete on the wire today
//! while no Ticket yet raises a class: the projection that feeds it
//! lands with the attention inbox, and until then the axis selects
//! nothing.

use std::collections::HashMap;
use std::sync::Arc;

use kanban_domain::{
    AttentionState as DomainAttention, BoardCard, BoardFilter as DomainFilter,
    BoardGroup as DomainGroup, InitiativeId, LaneId, NumberKind, PlanId, ProfileName, ProjectId,
    SpecId, TicketKind as DomainKind, TicketState as DomainState, admits, board_group_for,
    compare_cards,
};
use kanban_dto::{
    ApiError, AttentionState, BoardFilter, BoardFilterOption, BoardFilterOptions, BoardGlobalCard,
    BoardGlobalQuery, BoardGlobalResponse, BoardGroup, TicketKind, TicketState,
};
use serde_json::Value;

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::initiative::InitiativeStore;
use crate::lane::LaneStore;
use crate::mutation::parse_payload;
use crate::plan::PlanStore;
use crate::profile::ProfileStore;
use crate::project::ProjectStore;
use crate::spec::SpecStore;
use crate::ticket::{TicketStore, priority_of, record_of};

/// The stores the global board reads. Every one is read-only here:
/// the board is a projection, and no path through it mutates
/// workflow.
#[derive(Clone)]
struct BoardContext {
    initiatives: Arc<dyn InitiativeStore>,
    projects: Arc<dyn ProjectStore>,
    plans: Arc<dyn PlanStore>,
    specs: Arc<dyn SpecStore>,
    tickets: Arc<dyn TicketStore>,
    lanes: Arc<dyn LaneStore>,
    profiles: Arc<dyn ProfileStore>,
}

impl Core {
    /// Register the board operations against every store the global
    /// projection reads.
    #[allow(clippy::too_many_arguments)]
    pub fn register_board(
        &mut self,
        initiatives: Arc<dyn InitiativeStore>,
        projects: Arc<dyn ProjectStore>,
        plans: Arc<dyn PlanStore>,
        specs: Arc<dyn SpecStore>,
        tickets: Arc<dyn TicketStore>,
        lanes: Arc<dyn LaneStore>,
        profiles: Arc<dyn ProfileStore>,
    ) -> Result<(), RegistrationError> {
        let context = BoardContext {
            initiatives,
            projects,
            plans,
            specs,
            tickets,
            lanes,
            profiles,
        };
        self.register_query("board.global", Arc::new(GlobalBoard(context)))?;
        Ok(())
    }
}

/// Serves `board.global`.
struct GlobalBoard(BoardContext);

impl QueryHandler for GlobalBoard {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: BoardGlobalQuery = parse_payload(payload)?;
        let response = self.project(&query.filter)?;
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

impl GlobalBoard {
    /// The filtered projection: every Project's Tickets, filtered by
    /// the domain rule, placed in their groups, and returned in the
    /// domain's deterministic order, beside the values each reference
    /// axis offers.
    fn project(&self, filter: &BoardFilter) -> Result<BoardGlobalResponse, ApiError> {
        let domain_filter = domain_filter_of(filter)?;
        let projects = self.0.projects.list()?;
        // One read per Project, gathered first so the cards borrow
        // facts that outlive every loop: the Project's Tickets, the
        // Spec facts a Ticket's attachment resolves to, and the Lane
        // holding each Ticket. The option lists gather beside them.
        let mut facts = Vec::with_capacity(projects.len());
        let mut plan_options = Vec::new();
        let mut spec_options = Vec::new();
        let mut lane_options = Vec::new();
        for project in &projects {
            let code = project.code();
            let tickets = self.0.tickets.list(project.id())?;
            let specs = self.0.specs.list(project.id())?;
            let lanes = self.0.lanes.list_for_project(project.id())?;
            plan_options.extend(self.0.plans.list(project.id())?.into_iter().map(|plan| {
                (
                    plan.id().value(),
                    NumberKind::Plan.render(code, plan.number()),
                )
            }));
            spec_options.extend(specs.iter().map(|spec| {
                (
                    spec.id().value(),
                    spec.name().to_owned(),
                    NumberKind::Spec.render(code, spec.number().value()),
                )
            }));
            lane_options.extend(lanes.iter().map(|lane| {
                (
                    lane.id().value(),
                    format!("{} lane {}", code, lane.id().value()),
                )
            }));
            // The Spec facts a Ticket's attachment resolves to: the
            // Plan it belongs to and the number its card wears.
            let plan_of_spec: HashMap<SpecId, PlanId> = specs
                .iter()
                .filter_map(|spec| spec.plan().map(|plan| (spec.id(), plan)))
                .collect();
            let number_of_spec: HashMap<SpecId, u64> = specs
                .iter()
                .map(|spec| (spec.id(), spec.number().value()))
                .collect();
            // The Lane holding each Ticket, by the occupant it names.
            let lane_of_ticket: HashMap<u64, LaneId> = lanes
                .iter()
                .filter_map(|lane| lane.ticket_id().map(|ticket| (ticket.value(), lane.id())))
                .collect();
            facts.push(ProjectFacts {
                initiative: project.registration().initiative(),
                code: project.code().clone(),
                tickets,
                plan_of_spec,
                number_of_spec,
                lane_of_ticket,
            });
        }
        // The attention classes raised on a Ticket; no feed exists
        // yet, so the axis selects nothing until the attention
        // projection lands and starts filling this in.
        const SILENT: &[DomainAttention] = &[];
        let mut cards: Vec<(BoardCard<'_>, BoardGlobalCard)> = Vec::new();
        for fact in &facts {
            for ticket in &fact.tickets {
                let card = BoardCard {
                    ticket,
                    initiative: fact.initiative,
                    plan: ticket
                        .spec()
                        .and_then(|spec| fact.plan_of_spec.get(&spec).copied()),
                    lane: fact.lane_of_ticket.get(&ticket.id().value()).copied(),
                    attention: SILENT,
                };
                if !admits(&domain_filter, &card) {
                    continue;
                }
                // The terminal states reach no group and appear on no
                // board (DR-LC-02), whatever the filter names.
                let Some(group) = board_group_for(ticket.state()) else {
                    continue;
                };
                cards.push((
                    card,
                    BoardGlobalCard {
                        ticket: record_of(ticket, &fact.code),
                        project_code: fact.code.to_string(),
                        spec_number: ticket
                            .spec()
                            .and_then(|spec| fact.number_of_spec.get(&spec).copied()),
                        lane_id: fact
                            .lane_of_ticket
                            .get(&ticket.id().value())
                            .map(|lane| lane.value()),
                        group: group_of(group),
                    },
                ));
            }
        }
        cards.sort_by(|a, b| compare_cards(&a.0, &b.0));
        let options = BoardFilterOptions {
            initiatives: ordered(
                self.0
                    .initiatives
                    .list()?
                    .into_iter()
                    .map(|initiative| BoardFilterOption {
                        id: initiative.id().value(),
                        label: initiative.name().to_owned(),
                    })
                    .collect(),
            ),
            projects: ordered(
                projects
                    .iter()
                    .map(|project| BoardFilterOption {
                        id: project.id().value(),
                        label: format!("{} — {}", project.code(), project.registration().name()),
                    })
                    .collect(),
            ),
            plans: ordered(
                plan_options
                    .into_iter()
                    .map(|(id, label)| BoardFilterOption { id, label })
                    .collect(),
            ),
            specs: ordered(
                spec_options
                    .into_iter()
                    .map(|(id, name, label)| BoardFilterOption {
                        id,
                        label: format!("{label} · {name}"),
                    })
                    .collect(),
            ),
            lanes: ordered(
                lane_options
                    .into_iter()
                    .map(|(id, label)| BoardFilterOption { id, label })
                    .collect(),
            ),
            profiles: {
                let mut names: Vec<String> = self
                    .0
                    .profiles
                    .list()?
                    .into_iter()
                    .map(|profile| profile.name().as_str().to_owned())
                    .collect();
                names.sort();
                names
            },
            attention: AttentionState::ALL.to_vec(),
        };
        Ok(BoardGlobalResponse {
            cards: cards.into_iter().map(|(_, card)| card).collect(),
            options,
        })
    }
}

/// One Project's gathered facts, read once so the projection borrows
/// a stable home.
struct ProjectFacts {
    /// The Initiative the Project sits under, if any.
    initiative: Option<InitiativeId>,
    /// The Project's code, the prefix every minted number wears.
    code: kanban_domain::ProjectCode,
    /// Every Ticket of the Project, terminal states included.
    tickets: Vec<kanban_domain::Ticket>,
    /// The Plan each planned Spec belongs to.
    plan_of_spec: HashMap<SpecId, PlanId>,
    /// The minted number each Spec carries.
    number_of_spec: HashMap<SpecId, u64>,
    /// The Lane holding each held Ticket.
    lane_of_ticket: HashMap<u64, LaneId>,
}

/// The domain form of the wire filter; a profile name the domain
/// refuses is the invalid request it always was.
fn domain_filter_of(filter: &BoardFilter) -> Result<DomainFilter, ApiError> {
    Ok(DomainFilter {
        initiatives: filter
            .initiatives
            .iter()
            .map(|id| InitiativeId::new(*id))
            .collect(),
        projects: filter
            .projects
            .iter()
            .map(|id| ProjectId::new(*id))
            .collect(),
        plans: filter.plans.iter().map(|id| PlanId::new(*id)).collect(),
        specs: filter.specs.iter().map(|id| SpecId::new(*id)).collect(),
        kinds: filter.kinds.iter().map(|kind| domain_kind(*kind)).collect(),
        states: filter
            .states
            .iter()
            .map(|state| domain_state(*state))
            .collect(),
        priorities: filter
            .priorities
            .iter()
            .map(|priority| priority_of(*priority))
            .collect(),
        lanes: filter.lanes.iter().map(|id| LaneId::new(*id)).collect(),
        profiles: filter
            .profiles
            .iter()
            .map(|name| ProfileName::new(name.as_str()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?,
        attention: filter
            .attention
            .iter()
            .map(|class| domain_attention(*class))
            .collect(),
    })
}

/// The domain form of one wire kind.
fn domain_kind(kind: TicketKind) -> DomainKind {
    match kind {
        TicketKind::Implementation => DomainKind::Implementation,
        TicketKind::Bug => DomainKind::Bug,
        TicketKind::Task => DomainKind::Task,
    }
}

/// The domain form of one wire state.
fn domain_state(state: TicketState) -> DomainState {
    match state {
        TicketState::Draft => DomainState::Draft,
        TicketState::Parked => DomainState::Parked,
        TicketState::Blocked => DomainState::Blocked,
        TicketState::Scheduled => DomainState::Scheduled,
        TicketState::Ready => DomainState::Ready,
        TicketState::Active => DomainState::Active,
        TicketState::InReview => DomainState::InReview,
        TicketState::Approved => DomainState::Approved,
        TicketState::Landing => DomainState::Landing,
        TicketState::Done => DomainState::Done,
        TicketState::Cancelled => DomainState::Cancelled,
        TicketState::Superseded => DomainState::Superseded,
    }
}

/// The domain form of one wire attention class.
fn domain_attention(class: AttentionState) -> DomainAttention {
    match class {
        AttentionState::Blocker => DomainAttention::Blocker,
        AttentionState::MissingResult => DomainAttention::MissingResult,
        AttentionState::HumanDecision => DomainAttention::HumanDecision,
        AttentionState::ReviewRequest => DomainAttention::ReviewRequest,
        AttentionState::FailedSchedule => DomainAttention::FailedSchedule,
        AttentionState::InvalidApproval => DomainAttention::InvalidApproval,
        AttentionState::DisconnectedSession => DomainAttention::DisconnectedSession,
        AttentionState::StaleRun => DomainAttention::StaleRun,
    }
}

/// The wire form of one domain group.
fn group_of(group: DomainGroup) -> BoardGroup {
    match group {
        DomainGroup::Draft => BoardGroup::Draft,
        DomainGroup::Backlog => BoardGroup::Backlog,
        DomainGroup::Current => BoardGroup::Current,
        DomainGroup::Review => BoardGroup::Review,
        DomainGroup::Staged => BoardGroup::Staged,
        DomainGroup::Done => BoardGroup::Done,
    }
}

/// One option, ordered by what the operator reads and settled by the
/// identity behind it, so the same state always offers the same list.
fn ordered(mut options: Vec<BoardFilterOption>) -> Vec<BoardFilterOption> {
    options.sort_by(|a, b| (&a.label, a.id).cmp(&(&b.label, b.id)));
    options
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use kanban_domain::{
        Initiative, InitiativeId, InitiativeName, Project, ProjectCounters, ProjectId,
        ProjectRegistration, ProjectState, Ticket, TicketId, TicketState,
    };
    use kanban_dto::{ApiError, ErrorCode};
    use serde_json::json;

    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::NoopEventSink;
    use crate::initiative::InitiativeStore;
    use crate::lane::testing::MemoryLaneStore;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::plan::testing::MemoryPlans;
    use crate::plan::testing::MemoryProjects;
    use crate::profile::testing::MemoryProfiles;
    use crate::spec::testing::MemorySpecs;
    use crate::ticket::TicketStore;
    use crate::ticket::testing::MemoryTickets;
    use crate::timeline::TimelineEnvelope;
    use crate::workspace::testing::MemoryWorkspaceStore;

    /// An in-memory Initiative store: rows by id, seeded directly by
    /// the harness.
    #[derive(Default)]
    struct BoardInitiatives {
        rows: Mutex<Vec<Initiative>>,
    }

    impl BoardInitiatives {
        /// Seed one Initiative as-is.
        fn seed(&self, id: u64, name: &str) {
            self.rows
                .lock()
                .expect("the memory initiative lock is sound")
                .push(Initiative::new(
                    InitiativeId::new(id),
                    InitiativeName::new(name).expect("the fixture name validates"),
                ));
        }
    }

    impl InitiativeStore for BoardInitiatives {
        fn create(
            &self,
            name: &InitiativeName,
            envelope: &dyn Fn(InitiativeId) -> TimelineEnvelope,
        ) -> Result<Initiative, ApiError> {
            let mut rows = self
                .rows
                .lock()
                .expect("the memory initiative lock is sound");
            let next = rows.len() as u64 + 1;
            let id = InitiativeId::new(next);
            let initiative = Initiative::new(id, name.clone());
            rows.push(initiative.clone());
            drop(rows);
            envelope(id);
            Ok(initiative)
        }

        fn find(&self, id: InitiativeId) -> Result<Option<Initiative>, ApiError> {
            Ok(self
                .rows
                .lock()
                .expect("the memory initiative lock is sound")
                .iter()
                .find(|row| row.id() == id)
                .cloned())
        }

        fn save(
            &self,
            initiative: &Initiative,
            envelope: TimelineEnvelope,
        ) -> Result<(), ApiError> {
            let mut rows = self
                .rows
                .lock()
                .expect("the memory initiative lock is sound");
            if let Some(row) = rows.iter_mut().find(|row| row.id() == initiative.id()) {
                *row = initiative.clone();
            }
            drop(rows);
            let _ = envelope;
            Ok(())
        }

        fn list(&self) -> Result<Vec<Initiative>, ApiError> {
            Ok(self
                .rows
                .lock()
                .expect("the memory initiative lock is sound")
                .clone())
        }
    }

    /// One active Project, optionally under one Initiative.
    fn project(id: u64, code: &str, name: &str, initiative: Option<u64>) -> Project {
        let registration = ProjectRegistration::new(
            code,
            name,
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            None,
            initiative.map(InitiativeId::new),
        )
        .expect("the fixture registration validates");
        Project::restore(
            ProjectId::new(id),
            registration,
            ProjectState::Active,
            ProjectCounters::zeroed(),
            1,
        )
    }

    /// A core with the planning, Ticket, Lane, Profile, Initiative,
    /// and board operations wired to in-memory stores over two
    /// Projects: CORE (1) under Initiative 1, EDGE (2) under none.
    struct BoardHarness {
        tickets: Arc<MemoryTickets>,
        core: Core,
    }

    fn board_harness() -> BoardHarness {
        let projects = Arc::new(MemoryProjects::default());
        projects.seed(project(1, "CORE", "Control plane", Some(1)));
        projects.seed(project(2, "EDGE", "Edge tooling", None));
        let initiatives = Arc::new(BoardInitiatives::default());
        initiatives.seed(1, "Personal tooling");
        initiatives.seed(2, "Archive");
        let plans = Arc::new(MemoryPlans::sharing(projects.clone()));
        let specs = Arc::new(MemorySpecs::sharing(projects.clone()));
        let tickets = Arc::new(MemoryTickets::sharing(projects.clone()));
        let workspaces = Arc::new(MemoryWorkspaceStore::default());
        let lanes = Arc::new(MemoryLaneStore::sharing(workspaces.clone()));
        let profiles = Arc::new(MemoryProfiles::default());
        let mut core = Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            Arc::new(NoopEventSink),
        );
        core.register_initiatives(initiatives.clone())
            .expect("the initiative operations register");
        core.register_plans(plans.clone(), projects.clone(), specs.clone())
            .expect("the plan operations register");
        core.register_specs(specs.clone(), projects.clone(), plans.clone())
            .expect("the spec operations register");
        core.register_tickets(
            tickets.clone(),
            projects.clone(),
            specs.clone(),
            Arc::new(crate::ticket::testing::MemoryTicketEvidence::default()),
        )
        .expect("the ticket operations register");
        core.register_workspaces(
            workspaces.clone(),
            projects.clone(),
            Arc::new(crate::workspace::testing::ScriptedObserver::default()),
        )
        .expect("the workspace operations register");
        core.register_lanes(lanes.clone(), projects.clone(), workspaces, tickets.clone())
            .expect("the lane operations register");
        core.register_profiles(profiles.clone(), tickets.clone(), projects.clone())
            .expect("the profile operations register");
        core.register_board(
            initiatives,
            projects.clone(),
            plans,
            specs,
            tickets.clone(),
            lanes,
            profiles,
        )
        .expect("the board operations register");
        BoardHarness { tickets, core }
    }

    /// One ticket.create request with the fields a test varies.
    fn create(
        project: u64,
        kind: &str,
        priority: &str,
        spec: Option<u64>,
        title: &str,
        key: &str,
    ) -> serde_json::Value {
        let mut request = json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": project,
            "kind": kind,
            "priority": priority,
        });
        let object = request.as_object_mut().expect("the request is an object");
        if let Some(spec) = spec {
            object.insert("spec_id".to_owned(), json!(spec));
        }
        match kind {
            "implementation" => {
                object.insert(
                    "slice".to_owned(),
                    json!("Spec authoring creates content versions end to end"),
                );
                object.insert(
                    "criteria".to_owned(),
                    json!([{ "outcome": "Specs mint unique numbers.", "stories": ["CORE-S1-US1"] }]),
                );
            }
            "bug" => {
                object.insert("title".to_owned(), json!(title));
                object.insert(
                    "actual_behaviour".to_owned(),
                    json!("The integration branch is dropped after a review lands."),
                );
                object.insert(
                    "reporter_evidence".to_owned(),
                    json!("The landing log names the drop immediately after the merge."),
                );
            }
            _ => {
                object.insert("title".to_owned(), json!(title));
                object.insert("subtype".to_owned(), json!("operational"));
                object.insert("mode".to_owned(), json!("human"));
                object.insert(
                    "completion".to_owned(),
                    json!(["The old register is archived."]),
                );
            }
        }
        request
    }

    /// The identity one creation minted.
    fn minted(created: &serde_json::Value) -> u64 {
        created["id"].as_u64().expect("the identity is a number")
    }

    /// The standing fixture: CORE owns an Implementation (attached to
    /// planned Spec 1), an urgent Task, and a normal Bug; EDGE owns
    /// one low Bug. The Task sits ready in Lane 1; the Implementation
    /// is assigned profile `standard` and sits in review. Returns the
    /// four Ticket identities.
    fn seeded(harness: &BoardHarness) -> (u64, u64, u64, u64) {
        let core = &harness.core;
        let spec = minted(
            &core
                .command(
                    "spec.create",
                    &json!({
                        "mutation": { "optimistic_version": 0, "idempotency_key": "key-spec" },
                        "project_id": 1,
                        "content": crate::spec::testing::wire_content("Serve the board"),
                    }),
                )
                .expect("the Spec authors"),
        );
        let plan = minted(
            &core
                .command(
                    "plan.create",
                    &json!({
                        "mutation": { "optimistic_version": 0, "idempotency_key": "key-plan" },
                        "project_id": 1,
                    }),
                )
                .expect("the Plan creates"),
        );
        core.command(
            "plan.spec.add",
            &json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "key-plan-add" },
                "plan_id": plan,
                "spec_number": 1,
            }),
        )
        .expect("the Spec joins the Plan's membership");
        core.command(
            "spec.plan.join",
            &json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "key-join" },
                "spec_id": spec,
                "plan_id": plan,
            }),
        )
        .expect("the Spec joins the Plan");
        core.command(
            "profile.define",
            &json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "key-profile" },
                "name": "standard",
                "harness": "claude-code",
                "model": "opus",
                "effort": "high",
                "usage_pool": "operator",
            }),
        )
        .expect("the profile defines");
        let implementation = minted(
            &core
                .command(
                    "ticket.create",
                    &create(
                        1,
                        "implementation",
                        "high",
                        Some(spec),
                        "Implementation",
                        "key-1",
                    ),
                )
                .expect("the Implementation creates"),
        );
        let task = minted(
            &core
                .command(
                    "ticket.create",
                    &create(1, "task", "urgent", None, "Archive the register", "key-2"),
                )
                .expect("the Task creates"),
        );
        let bug = minted(
            &core
                .command(
                    "ticket.create",
                    &create(1, "bug", "normal", None, "Landing drops branches", "key-3"),
                )
                .expect("the Bug creates"),
        );
        let edge_bug = minted(
            &core
                .command(
                    "ticket.create",
                    &create(2, "bug", "low", None, "Edge bug", "key-4"),
                )
                .expect("the EDGE Bug creates"),
        );
        let lane = minted(
            &core
                .command(
                    "lane.create",
                    &json!({
                        "mutation": { "optimistic_version": 0, "idempotency_key": "key-lane" },
                        "project_id": 1,
                    }),
                )
                .expect("the Lane creates"),
        );
        core.command(
            "lane.ticket.assign",
            &json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "key-lane-hold" },
                "lane_id": lane,
                "ticket_id": task,
            }),
        )
        .expect("the Lane holds the Task");
        core.command(
            "ticket.assign",
            &json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "key-assign" },
                "ticket_id": implementation,
                "profile": "standard",
            }),
        )
        .expect("the Implementation carries its profile");
        // The Task sits ready and the Implementation in review, so
        // the projection spans groups and readiness positions.
        restate(harness, task, TicketState::Ready);
        restate(harness, implementation, TicketState::InReview);
        (implementation, task, bug, edge_bug)
    }

    /// Replace one stored row's state, standing in for the lifecycle
    /// moves that produced it.
    fn restate(harness: &BoardHarness, id: u64, state: TicketState) {
        let standing = harness
            .tickets
            .find(TicketId::new(id))
            .expect("the find serves")
            .expect("the Ticket stands");
        harness
            .tickets
            .replace_pinned(Ticket::restore(
                standing.id(),
                standing.project(),
                standing.number(),
                standing.priority(),
                state,
                standing.body().clone(),
                standing.predecessor(),
                standing.profile().cloned(),
                standing.pinned_version(),
                standing.version() + 1,
            ))
            .expect("the row moves");
    }

    /// The cards of one response, as (project code, number) pairs.
    fn named(response: &serde_json::Value) -> Vec<(String, u64)> {
        response["cards"]
            .as_array()
            .expect("the cards are a list")
            .iter()
            .map(|card| {
                (
                    card["project_code"]
                        .as_str()
                        .expect("the code is text")
                        .to_owned(),
                    card["ticket"]["number"]
                        .as_u64()
                        .expect("the number is a number"),
                )
            })
            .collect()
    }

    /// The groups of one response's cards, in order.
    fn groups(response: &serde_json::Value) -> Vec<&str> {
        response["cards"]
            .as_array()
            .expect("the cards are a list")
            .iter()
            .map(|card| card["group"].as_str().expect("the group is text"))
            .collect()
    }

    #[test]
    fn an_empty_filter_projects_every_projects_work_in_order() {
        let harness = board_harness();
        let (implementation, task, bug, edge_bug) = seeded(&harness);

        let response = harness
            .core
            .query("board.global", &json!({ "filter": {} }))
            .expect("the projection serves");

        // Urgent ready Task first, then the high-priority in-review
        // Implementation, then the normal draft Bug, then EDGE's low
        // draft Bug: priority, then readiness, then Project, then
        // number (DR-LC-11).
        assert_eq!(
            named(&response),
            vec![
                ("CORE".to_owned(), 2),
                ("CORE".to_owned(), 1),
                ("CORE".to_owned(), 3),
                ("EDGE".to_owned(), 1),
            ],
            "the projection returns the deterministic global order"
        );
        assert_eq!(
            groups(&response),
            ["backlog", "review", "draft", "draft"],
            "each card carries the group its state projects onto"
        );
        let task_card = &response["cards"][0];
        assert_eq!(task_card["ticket"]["id"], json!(task));
        assert_eq!(task_card["lane_id"], json!(1), "the Lane holding the Task");
        let implementation_card = &response["cards"][1];
        assert_eq!(implementation_card["ticket"]["id"], json!(implementation));
        assert_eq!(
            implementation_card["spec_number"],
            json!(1),
            "the Spec the Implementation attaches to"
        );
        assert_eq!(implementation_card["ticket"]["profile"], json!("standard"));
        assert_eq!(response["cards"][2]["ticket"]["id"], json!(bug));
        assert_eq!(response["cards"][3]["ticket"]["id"], json!(edge_bug));
    }

    #[test]
    fn terminal_states_reach_no_group_on_any_filter() {
        let harness = board_harness();
        let (_, task, _, _) = seeded(&harness);
        restate(&harness, task, TicketState::Cancelled);

        let response = harness
            .core
            .query("board.global", &json!({ "filter": {} }))
            .expect("the projection serves");

        assert!(
            !named(&response).contains(&("CORE".to_owned(), 2)),
            "cancelled work never appears on the board (DR-LC-02)"
        );
        // Even a filter naming the terminal state selects nothing:
        // the state reaches no group.
        let selected = harness
            .core
            .query(
                "board.global",
                &json!({ "filter": { "states": ["cancelled"] } }),
            )
            .expect("the projection serves");
        assert_eq!(named(&selected), Vec::<(String, u64)>::new());
    }

    #[test]
    fn the_reference_axes_narrow_the_board_to_their_own_work() {
        let harness = board_harness();
        let (implementation, task, _bug, edge_bug) = seeded(&harness);

        let by_initiative = harness
            .core
            .query("board.global", &json!({ "filter": { "initiatives": [1] } }))
            .expect("the projection serves");
        assert!(
            !named(&by_initiative).contains(&("EDGE".to_owned(), 1)),
            "EDGE sits under no Initiative"
        );
        assert_eq!(named(&by_initiative).len(), 3);

        let by_project = harness
            .core
            .query("board.global", &json!({ "filter": { "projects": [2] } }))
            .expect("the projection serves");
        assert_eq!(named(&by_project), vec![("EDGE".to_owned(), 1)]);

        let by_plan = harness
            .core
            .query("board.global", &json!({ "filter": { "plans": [1] } }))
            .expect("the projection serves");
        assert_eq!(
            named(&by_plan),
            vec![("CORE".to_owned(), 1)],
            "only the Ticket whose Spec belongs to the Plan shows"
        );

        let by_spec = harness
            .core
            .query("board.global", &json!({ "filter": { "specs": [1] } }))
            .expect("the projection serves");
        assert_eq!(named(&by_spec), vec![("CORE".to_owned(), 1)]);

        let by_lane = harness
            .core
            .query("board.global", &json!({ "filter": { "lanes": [1] } }))
            .expect("the projection serves");
        assert_eq!(
            named(&by_lane),
            vec![("CORE".to_owned(), 2)],
            "only the Ticket the Lane holds shows"
        );
        assert_eq!(by_lane["cards"][0]["ticket"]["id"], json!(task));

        let by_profile = harness
            .core
            .query(
                "board.global",
                &json!({ "filter": { "profiles": ["standard"] } }),
            )
            .expect("the projection serves");
        assert_eq!(
            named(&by_profile),
            vec![("CORE".to_owned(), 1)],
            "only the Ticket carrying the assignment shows"
        );
        assert_eq!(
            by_profile["cards"][0]["ticket"]["id"],
            json!(implementation)
        );
        let _ = edge_bug;
    }

    #[test]
    fn the_vocabulary_axes_narrow_the_board_to_their_own_work() {
        let harness = board_harness();
        seeded(&harness);

        let by_kind = harness
            .core
            .query("board.global", &json!({ "filter": { "kinds": ["task"] } }))
            .expect("the projection serves");
        assert_eq!(named(&by_kind), vec![("CORE".to_owned(), 2)]);

        let by_state = harness
            .core
            .query(
                "board.global",
                &json!({ "filter": { "states": ["ready"] } }),
            )
            .expect("the projection serves");
        assert_eq!(named(&by_state), vec![("CORE".to_owned(), 2)]);

        let by_priority = harness
            .core
            .query(
                "board.global",
                &json!({ "filter": { "priorities": ["low"] } }),
            )
            .expect("the projection serves");
        assert_eq!(named(&by_priority), vec![("EDGE".to_owned(), 1)]);
    }

    #[test]
    fn the_attention_axis_selects_nothing_until_its_feed_lands() {
        let harness = board_harness();
        seeded(&harness);

        let by_attention = harness
            .core
            .query(
                "board.global",
                &json!({ "filter": { "attention": ["stale_run"] } }),
            )
            .expect("the projection serves");

        assert_eq!(
            named(&by_attention),
            Vec::<(String, u64)>::new(),
            "no Ticket raises a class until the attention projection lands"
        );
        let attention_options: Vec<&str> = by_attention["options"]["attention"]
            .as_array()
            .expect("the attention options are a list")
            .iter()
            .map(|class| class.as_str().expect("the class is text"))
            .collect();
        assert_eq!(
            attention_options,
            [
                "blocker",
                "missing_result",
                "human_decision",
                "review_request",
                "failed_schedule",
                "invalid_approval",
                "disconnected_session",
                "stale_run",
            ],
            "the closed vocabulary is offered whole"
        );
    }

    #[test]
    fn filters_compose_as_one_intersection() {
        let harness = board_harness();
        seeded(&harness);

        let response = harness
            .core
            .query(
                "board.global",
                &json!({
                    "filter": {
                        "kinds": ["task", "bug"],
                        "priorities": ["urgent", "low"],
                        "projects": [1, 2],
                    }
                }),
            )
            .expect("the projection serves");

        // The urgent Task of CORE passes every axis; the low Bug of
        // EDGE passes too; CORE's normal Bug fails the priority axis
        // and its Implementation fails the kind axis.
        assert_eq!(
            named(&response),
            vec![("CORE".to_owned(), 2), ("EDGE".to_owned(), 1)]
        );
    }

    #[test]
    fn the_projection_is_stable_across_reloads() {
        let harness = board_harness();
        seeded(&harness);

        let first = harness
            .core
            .query("board.global", &json!({}))
            .expect("the first projection serves");
        let second = harness
            .core
            .query("board.global", &json!({}))
            .expect("the second projection serves");

        assert_eq!(first, second, "the same state projects identically");
    }

    #[test]
    fn the_response_carries_the_filter_options() {
        let harness = board_harness();
        seeded(&harness);

        let response = harness
            .core
            .query("board.global", &json!({}))
            .expect("the projection serves");
        let options = &response["options"];

        let labels = |axis: &str| -> Vec<String> {
            options[axis]
                .as_array()
                .expect("the axis options are a list")
                .iter()
                .map(|option| {
                    option["label"]
                        .as_str()
                        .expect("the label is text")
                        .to_owned()
                })
                .collect()
        };
        assert_eq!(
            labels("initiatives"),
            ["Archive".to_owned(), "Personal tooling".to_owned()],
            "Initiatives offer by name, ordered by what the operator reads"
        );
        assert_eq!(
            labels("projects"),
            [
                "CORE — Control plane".to_owned(),
                "EDGE — Edge tooling".to_owned(),
            ]
        );
        assert_eq!(labels("plans"), ["CORE-P1".to_owned()]);
        assert_eq!(labels("specs"), ["CORE-S1 · Serve the board".to_owned()]);
        assert_eq!(labels("lanes"), ["CORE lane 1".to_owned()]);
        assert_eq!(
            options["profiles"],
            json!(["standard".to_owned()]),
            "a profile offers its name, its whole identity"
        );
    }

    #[test]
    fn the_query_rejects_unknown_fields() {
        let harness = board_harness();

        let error = harness
            .core
            .query("board.global", &json!({ "filter": {}, "sort": "manual" }))
            .expect_err("the query carries its filter and nothing else");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `sort`");
    }
}
