//! The global search query (DR-BP-17): one read-only projection of
//! every Initiative, Project, Plan, Spec, and Ticket through the
//! domain's matching and ordering rules. The query gathers each
//! entity beside the facts its identifier and text resolve, hands
//! the domain rule the operator's text, and returns the hits already
//! in the deterministic order — a client renders the list, it never
//! recomputes it.

use std::sync::Arc;

use kanban_domain::{
    NumberKind, SearchCandidate, SearchHitKind as DomainHitKind, search as domain_search,
};
use kanban_dto::{
    ApiError, SearchGlobalHit, SearchGlobalQuery, SearchGlobalResponse, SearchHitKind,
};
use serde_json::Value;

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::initiative::InitiativeStore;
use crate::mutation::parse_payload;
use crate::plan::PlanStore;
use crate::project::ProjectStore;
use crate::spec::SpecStore;
use crate::ticket::TicketStore;

/// The stores global search reads. Every one is read-only here:
/// search is a projection, and no path through it mutates workflow.
#[derive(Clone)]
struct SearchContext {
    initiatives: Arc<dyn InitiativeStore>,
    projects: Arc<dyn ProjectStore>,
    plans: Arc<dyn PlanStore>,
    specs: Arc<dyn SpecStore>,
    tickets: Arc<dyn TicketStore>,
}

impl Core {
    /// Register the search operations against every store the global
    /// search projection reads.
    pub fn register_search(
        &mut self,
        initiatives: Arc<dyn InitiativeStore>,
        projects: Arc<dyn ProjectStore>,
        plans: Arc<dyn PlanStore>,
        specs: Arc<dyn SpecStore>,
        tickets: Arc<dyn TicketStore>,
    ) -> Result<(), RegistrationError> {
        let context = SearchContext {
            initiatives,
            projects,
            plans,
            specs,
            tickets,
        };
        self.register_query("search.global", Arc::new(GlobalSearch(context)))?;
        Ok(())
    }
}

/// Serves `search.global`.
struct GlobalSearch(SearchContext);

impl QueryHandler for GlobalSearch {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: SearchGlobalQuery = parse_payload(payload)?;
        let response = self.project(&query.q)?;
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

impl GlobalSearch {
    /// The filtered projection: every searchable entity, matched by
    /// the domain rule and returned in the domain's deterministic
    /// order.
    fn project(&self, query: &str) -> Result<SearchGlobalResponse, ApiError> {
        let mut candidates = Vec::new();
        for initiative in self.0.initiatives.list()? {
            let name = initiative.name();
            candidates.push(SearchCandidate {
                kind: DomainHitKind::Initiative,
                id: initiative.id().value(),
                identifier: name.to_owned(),
                label: name.to_owned(),
                project_id: None,
                texts: vec![],
            });
        }
        for project in self.0.projects.list()? {
            let code = project.code().as_str();
            let name = project.registration().name();
            candidates.push(SearchCandidate {
                kind: DomainHitKind::Project,
                id: project.id().value(),
                identifier: code.to_owned(),
                label: name.to_owned(),
                project_id: Some(project.id().value()),
                texts: vec![name.to_owned()],
            });
            let project_id = project.id();
            for plan in self.0.plans.list(project_id)? {
                let identifier = NumberKind::Plan.render(project.code(), plan.number());
                candidates.push(SearchCandidate {
                    kind: DomainHitKind::Plan,
                    id: plan.id().value(),
                    identifier,
                    label: format!("Plan {}", plan.number()),
                    project_id: Some(project_id.value()),
                    texts: vec![],
                });
            }
            for spec in self.0.specs.list(project_id)? {
                let identifier = NumberKind::Spec.render(project.code(), spec.number().value());
                let name = spec.name();
                candidates.push(SearchCandidate {
                    kind: DomainHitKind::Spec,
                    id: spec.id().value(),
                    identifier,
                    label: name.to_owned(),
                    project_id: Some(project_id.value()),
                    texts: vec![name.to_owned()],
                });
            }
            for ticket in self.0.tickets.list(project_id)? {
                let identifier = NumberKind::Ticket.render(project.code(), ticket.number().value());
                let label = ticket
                    .slice()
                    .or_else(|| ticket.title())
                    .map(str::to_owned)
                    .unwrap_or_else(|| "Untitled Ticket".to_owned());
                let texts = [ticket.slice(), ticket.title()]
                    .into_iter()
                    .flatten()
                    .map(str::to_owned)
                    .collect();
                candidates.push(SearchCandidate {
                    kind: DomainHitKind::Ticket,
                    id: ticket.id().value(),
                    identifier,
                    label,
                    project_id: Some(project_id.value()),
                    texts,
                });
            }
        }
        let hits = domain_search(query, &candidates)
            .into_iter()
            .map(hit_of)
            .collect();
        Ok(SearchGlobalResponse { hits })
    }
}

/// The wire form of one domain hit.
fn hit_of(hit: kanban_domain::SearchHit) -> SearchGlobalHit {
    SearchGlobalHit {
        kind: kind_of(hit.kind),
        id: hit.id,
        identifier: hit.identifier,
        label: hit.label,
        project_id: hit.project_id,
    }
}

/// The wire form of one domain kind.
fn kind_of(kind: DomainHitKind) -> SearchHitKind {
    match kind {
        DomainHitKind::Initiative => SearchHitKind::Initiative,
        DomainHitKind::Project => SearchHitKind::Project,
        DomainHitKind::Plan => SearchHitKind::Plan,
        DomainHitKind::Spec => SearchHitKind::Spec,
        DomainHitKind::Ticket => SearchHitKind::Ticket,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kanban_domain::{
        Initiative, InitiativeId, InitiativeName, Project, ProjectCounters, ProjectId,
        ProjectRegistration, ProjectState,
    };
    use serde_json::json;

    use crate::catalog::exposed_operations;
    use crate::dispatch::Core;
    use crate::events::NoopEventSink;
    use crate::initiative::InitiativeStore;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::plan::testing::MemoryPlans;
    use crate::plan::testing::MemoryProjects;
    use crate::spec::testing::MemorySpecs;
    use crate::ticket::testing::MemoryTickets;
    use crate::timeline::TimelineEnvelope;

    /// An in-memory Initiative store: rows by id, seeded directly by
    /// the harness.
    #[derive(Default)]
    struct BoardInitiatives {
        rows: std::sync::Mutex<Vec<Initiative>>,
    }

    impl BoardInitiatives {
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
        ) -> Result<Initiative, kanban_dto::ApiError> {
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

        fn find(&self, id: InitiativeId) -> Result<Option<Initiative>, kanban_dto::ApiError> {
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
        ) -> Result<(), kanban_dto::ApiError> {
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

        fn list(&self) -> Result<Vec<Initiative>, kanban_dto::ApiError> {
            Ok(self
                .rows
                .lock()
                .expect("the memory initiative lock is sound")
                .clone())
        }
    }

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

    struct SearchHarness {
        core: Core,
    }

    fn search_harness() -> SearchHarness {
        let projects = Arc::new(MemoryProjects::default());
        projects.seed(project(1, "CORE", "Control plane", Some(1)));
        let initiatives = Arc::new(BoardInitiatives::default());
        initiatives.seed(1, "Personal tooling");
        let plans = Arc::new(MemoryPlans::sharing(projects.clone()));
        let specs = Arc::new(MemorySpecs::sharing(projects.clone()));
        specs.seed_authored(ProjectId::new(1), &[1]);
        let tickets = Arc::new(MemoryTickets::sharing(projects.clone()));
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
        core.register_search(initiatives, projects, plans, specs.clone(), tickets.clone())
            .expect("the search operations register");
        SearchHarness { core }
    }

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
                object.insert("slice".to_owned(), json!(title));
                object.insert(
                    "criteria".to_owned(),
                    json!([{ "outcome": "Done.", "stories": ["CORE-S1-US1"] }]),
                );
            }
            "bug" => {
                object.insert("title".to_owned(), json!(title));
                object.insert("actual_behaviour".to_owned(), json!("Broken."));
                object.insert("reporter_evidence".to_owned(), json!("Seen."));
            }
            _ => {
                object.insert("title".to_owned(), json!(title));
                object.insert("subtype".to_owned(), json!("operational"));
                object.insert("mode".to_owned(), json!("human"));
                object.insert("completion".to_owned(), json!(["Done."]));
            }
        }
        request
    }

    #[test]
    fn search_global_finds_entities_by_identifier_and_text() {
        let harness = search_harness();
        harness
            .core
            .command(
                "plan.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "plan-1" },
                    "project_id": 1,
                }),
            )
            .expect("the plan creates");
        harness
            .core
            .command(
                "ticket.create",
                &create(1, "task", "normal", None, "Archive the register", "t-1"),
            )
            .expect("the ticket creates");
        harness
            .core
            .command(
                "ticket.create",
                &create(
                    1,
                    "implementation",
                    "normal",
                    Some(1),
                    "Board presentation",
                    "t-2",
                ),
            )
            .expect("the implementation creates");

        let by_initiative = harness
            .core
            .query("search.global", &json!({ "q": "personal" }))
            .expect("the initiative matches");
        assert_eq!(by_initiative["hits"][0]["kind"], "initiative");

        let by_project = harness
            .core
            .query("search.global", &json!({ "q": "control" }))
            .expect("the project matches");
        assert!(
            by_project["hits"]
                .as_array()
                .expect("hits are an array")
                .iter()
                .any(|hit| hit["kind"] == "project")
        );

        let by_plan = harness
            .core
            .query("search.global", &json!({ "q": "CORE-P1" }))
            .expect("the plan matches");
        assert_eq!(by_plan["hits"][0]["kind"], "plan");

        let by_spec = harness
            .core
            .query("search.global", &json!({ "q": "fixture 1" }))
            .expect("the spec matches");
        assert_eq!(by_spec["hits"][0]["kind"], "spec");

        let by_ticket_number = harness
            .core
            .query("search.global", &json!({ "q": "core-t1" }))
            .expect("the ticket number matches");
        assert_eq!(by_ticket_number["hits"][0]["kind"], "ticket");

        let by_ticket_text = harness
            .core
            .query("search.global", &json!({ "q": "archive" }))
            .expect("the ticket title matches");
        assert_eq!(by_ticket_text["hits"][0]["label"], "Archive the register");

        let by_slice = harness
            .core
            .query("search.global", &json!({ "q": "presentation" }))
            .expect("the implementation slice matches");
        assert_eq!(by_slice["hits"][0]["label"], "Board presentation");
    }

    #[test]
    fn search_global_is_registered_as_a_query() {
        let operation = exposed_operations()
            .iter()
            .find(|entry| entry.name == "search.global")
            .expect("search.global is exposed");
        assert_eq!(operation.kind, crate::catalog::OperationKind::Query);
    }
}
