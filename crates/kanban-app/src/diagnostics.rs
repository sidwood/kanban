//! The planning diagnostics query: the blocking surface of one Plan's
//! graph (KAN-S3-US7, DR-PS-18). `plan.diagnostics` reads the graph on
//! display — the working shape, or one frozen version — and reports
//! the dependency cycles, the story coverage gaps, and the invalid
//! profile references that keep the Plan's Ticket graph from becoming
//! executable. The invalid-profile list arrives through the
//! [`ProfileCatalogue`] seam; the service fills it with the stored
//! execution profile catalogue ([`StoredProfileCatalogue`], KAN-S7,
//! T38), while [`AbsentCatalogue`] keeps it empty for the cores no
//! Ticket store backs. The story claims a member Spec's coverage is
//! measured against arrive through the [`CoverageClaims`] seam for
//! the same reason: a story is covered by what the Spec's own
//! Tickets claim against the content version on display, so a Spec
//! whose stories are all claimed carries no gap and only a genuinely
//! uncovered one keeps blocking (T16).

use std::collections::BTreeSet;
use std::sync::Arc;

use kanban_domain::{
    AcceptanceCriterion, PlanId, Project, ScopeError, Spec, SpecId, SpecNumber, StoryScope,
    claims_count_for_version,
};
use kanban_dto::{
    ApiError, PlanCoverageGap, PlanCycle, PlanDiagnosticsQuery, PlanDiagnosticsResponse,
    PlanInvalidProfile,
};
use serde_json::Value;

use crate::coverage::claimed_criteria;
use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::mutation::parse_payload;
use crate::plan::PlanStore;
use crate::profile::ProfileStore;
use crate::project::ProjectStore;
use crate::spec::SpecStore;
use crate::ticket::TicketStore;

/// The execution profile catalogue behind the invalid-profile
/// diagnostics (DR-PS-18): it reads the profile references a Plan's
/// graph carries and reports every one that resolves to no catalogue
/// entry. The catalogue itself lands with KAN-S7 (T38); this seam is
/// the interface its entries feed, so the aggregate already counts
/// them as blocking.
pub trait ProfileCatalogue: Send + Sync {
    /// Every profile reference the diagnosed graph — the Project's
    /// members in the order the diagnostics resolved, the working
    /// shape or one frozen version — carries that resolves to no
    /// catalogue entry, as written where it was referenced.
    fn invalid_references(
        &self,
        project: &Project,
        order: &[SpecNumber],
    ) -> Result<Vec<String>, ApiError>;
}

/// The empty catalogue: the cores no Ticket store backs install it, so
/// no profile is referenced anywhere they can read and nothing is
/// invalid. The service replaces it through
/// [`Core::register_plan_diagnostics`] with [`StoredProfileCatalogue`].
pub struct AbsentCatalogue;

impl ProfileCatalogue for AbsentCatalogue {
    fn invalid_references(
        &self,
        _project: &Project,
        _order: &[SpecNumber],
    ) -> Result<Vec<String>, ApiError> {
        Ok(Vec::new())
    }
}

/// The stored execution profile catalogue behind the invalid-profile
/// diagnostics (KAN-S7, T38): the profile references the Tickets
/// attached to the diagnosed member Specs carry, resolved against the
/// stored profile rows. Assignments keep the names they referenced
/// through every catalogue change (DR-EP-05), so the references read
/// live while the membership follows the order the diagnostics
/// resolved — and a reference resolves only against an entry still
/// offered for assignment: a name no entry carries, or one a retired
/// entry keeps out of the assignable catalogue, is invalid
/// (DR-EP-03).
pub struct StoredProfileCatalogue {
    profiles: Arc<dyn ProfileStore>,
    tickets: Arc<dyn TicketStore>,
    specs: Arc<dyn SpecStore>,
}

impl StoredProfileCatalogue {
    /// Read the references through the stores the service wires.
    pub fn new(
        profiles: Arc<dyn ProfileStore>,
        tickets: Arc<dyn TicketStore>,
        specs: Arc<dyn SpecStore>,
    ) -> Self {
        Self {
            profiles,
            tickets,
            specs,
        }
    }

    /// The Specs of the diagnosed graph, as stored identities.
    fn members(&self, project: &Project, order: &[SpecNumber]) -> Result<Vec<SpecId>, ApiError> {
        let mut members = Vec::with_capacity(order.len());
        for number in order {
            let held = self
                .specs
                .find_by_number(project.id(), *number)?
                .ok_or_else(|| {
                    ApiError::internal(&format!(
                        "member Spec {} of plan is not stored",
                        number.value()
                    ))
                })?;
            members.push(held.id());
        }
        Ok(members)
    }
}

impl ProfileCatalogue for StoredProfileCatalogue {
    fn invalid_references(
        &self,
        project: &Project,
        order: &[SpecNumber],
    ) -> Result<Vec<String>, ApiError> {
        let catalogue = kanban_domain::ProfileCatalogue::restore(self.profiles.list()?);
        let members = self.members(project, order)?;
        // One entry per invalid reference, in name order: the list is
        // the set of broken names the operator must repair, however
        // many Tickets carry them.
        let mut invalid: BTreeSet<String> = BTreeSet::new();
        for ticket in self.tickets.list(project.id())? {
            let Some(spec) = ticket.spec() else {
                continue;
            };
            if !members.contains(&spec) {
                continue;
            }
            let Some(name) = ticket.profile() else {
                continue;
            };
            if !catalogue.assignable(name) {
                invalid.insert(name.as_str().to_owned());
            }
        }
        Ok(invalid.into_iter().collect())
    }
}

/// The story claims behind the coverage-gap diagnostics (DR-PS-18,
/// DR-PS-14): every story-linked criterion the Tickets attached to
/// one member Spec claim against the content version the diagnostics
/// read. A Spec whose scope is fully claimed has no gap; the stories
/// nothing claims are what blocks.
pub trait CoverageClaims: Send + Sync {
    /// The criteria counting toward one member Spec's coverage of the
    /// content version `version`, which is the Spec's operative read.
    fn claimed(&self, spec: &Spec, version: u64) -> Result<Vec<AcceptanceCriterion>, ApiError>;
}

/// The empty claim set: the cores no Ticket store backs install it, so
/// nothing they can read claims a story and every story in scope is a
/// gap. The service replaces it through
/// [`Core::register_plan_diagnostics`] with [`StoredCoverageClaims`].
pub struct AbsentClaims;

impl CoverageClaims for AbsentClaims {
    fn claimed(&self, _spec: &Spec, _version: u64) -> Result<Vec<AcceptanceCriterion>, ApiError> {
        Ok(Vec::new())
    }
}

/// The stored claims behind the coverage-gap diagnostics: the criteria
/// the Spec's own attached Tickets claim, scoped to the content
/// version on display by the same rule the coverage matrix applies
/// (`claims_count_for_version`, Sid ruling 5). The version the
/// diagnostics read is always the Spec's operative one — its approved
/// version, else its working content — so a pinned Ticket counts while
/// it stays open and the active unpinned population a new graph
/// completes over counts beside it. A claim pinned to another version
/// is that version's, never this one's.
pub struct StoredCoverageClaims {
    tickets: Arc<dyn TicketStore>,
}

impl StoredCoverageClaims {
    /// Read the claims through the Ticket store the service wires.
    pub fn new(tickets: Arc<dyn TicketStore>) -> Self {
        Self { tickets }
    }
}

impl CoverageClaims for StoredCoverageClaims {
    fn claimed(&self, spec: &Spec, version: u64) -> Result<Vec<AcceptanceCriterion>, ApiError> {
        Ok(self
            .tickets
            .list(spec.project())?
            .into_iter()
            .filter(|ticket| ticket.spec() == Some(spec.id()))
            .filter(|ticket| claims_count_for_version(ticket, version, true))
            .flat_map(|ticket| {
                claimed_criteria(&ticket)
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect())
    }
}

/// Serves `plan.diagnostics`.
pub(crate) struct DiagnosePlan {
    plans: Arc<dyn PlanStore>,
    projects: Arc<dyn ProjectStore>,
    specs: Arc<dyn SpecStore>,
    profiles: Arc<dyn ProfileCatalogue>,
    claims: Arc<dyn CoverageClaims>,
}

impl QueryHandler for DiagnosePlan {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: PlanDiagnosticsQuery = parse_payload(payload)?;
        let plan = self
            .plans
            .find(PlanId::new(query.plan_id))?
            .ok_or_else(|| ApiError::not_found(&format!("plan {}", query.plan_id)))?;
        let project = self.projects.find(plan.project())?.ok_or_else(|| {
            ApiError::internal(&format!(
                "plan {} belongs to no stored Project",
                query.plan_id
            ))
        })?;
        let (order, cycles) = match query.version {
            None => (plan.order(), plan.cycles()),
            Some(number) => {
                let frozen = plan
                    .versions()
                    .iter()
                    .find(|version| version.number() == number)
                    .ok_or_else(|| ApiError::not_found(&format!("version {number}")))?;
                (frozen.order(), frozen.cycles())
            }
        };
        let cycles: Vec<PlanCycle> = cycles
            .iter()
            .map(|cycle| PlanCycle {
                spec_numbers: cycle.specs().iter().map(|spec| spec.value()).collect(),
            })
            .collect();
        let coverage_gaps = self.coverage_gaps(&project, order)?;
        let invalid_profiles: Vec<PlanInvalidProfile> = self
            .profiles
            .invalid_references(&project, order)?
            .into_iter()
            .map(|reference| PlanInvalidProfile { reference })
            .collect();
        let blocking = !cycles.is_empty()
            || coverage_gaps
                .iter()
                .any(|gap| gap.claims_no_stories || !gap.uncovered.is_empty())
            || !invalid_profiles.is_empty();
        let response = PlanDiagnosticsResponse {
            cycles,
            coverage_gaps,
            invalid_profiles,
            blocking,
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

impl DiagnosePlan {
    /// One gap entry per member Spec that has one: the stories its
    /// scope claims that no criterion of its own attached Tickets
    /// claims against the version on display, or the fact that its
    /// version claims no story at all, which no Ticket graph could
    /// ever cover. The scope and the claims both read the
    /// still-approved version when one is operative and the working
    /// content otherwise, so the diagnostics measure one version
    /// against itself (T16).
    fn coverage_gaps(
        &self,
        project: &Project,
        order: &[SpecNumber],
    ) -> Result<Vec<PlanCoverageGap>, ApiError> {
        let mut gaps = Vec::new();
        for spec in order {
            let held = self
                .specs
                .find_by_number(project.id(), *spec)?
                .ok_or_else(|| {
                    ApiError::internal(&format!(
                        "member Spec {} of plan is not stored",
                        spec.value()
                    ))
                })?;
            let operative = held
                .approved_version()
                .or_else(|| held.current_version())
                .ok_or_else(|| {
                    ApiError::internal(&format!("Spec {} holds no content version", held.number()))
                })?;
            match StoryScope::extract(
                project.code(),
                held.number(),
                operative.content().user_stories(),
            ) {
                Ok(scope) => {
                    let claimed = self.claims.claimed(&held, operative.number())?;
                    let uncovered = scope.uncovered(&claimed);
                    if uncovered.is_empty() {
                        continue;
                    }
                    gaps.push(PlanCoverageGap {
                        spec_number: spec.value(),
                        uncovered: uncovered
                            .iter()
                            .map(|story| story.render(project.code()))
                            .collect(),
                        claims_no_stories: false,
                    });
                }
                Err(ScopeError::NoStories) => gaps.push(PlanCoverageGap {
                    spec_number: spec.value(),
                    uncovered: Vec::new(),
                    claims_no_stories: true,
                }),
            }
        }
        Ok(gaps)
    }
}

impl Core {
    /// Register the planning diagnostics query against the stores the
    /// Plan commands use, the profile catalogue that resolves profile
    /// references, and the claims that close coverage gaps.
    pub fn register_plan_diagnostics(
        &mut self,
        plans: Arc<dyn PlanStore>,
        projects: Arc<dyn ProjectStore>,
        specs: Arc<dyn SpecStore>,
        profiles: Arc<dyn ProfileCatalogue>,
        claims: Arc<dyn CoverageClaims>,
    ) -> Result<(), RegistrationError> {
        self.register_query(
            "plan.diagnostics",
            Arc::new(DiagnosePlan {
                plans,
                projects,
                specs,
                profiles,
                claims,
            }),
        )
    }
}

#[cfg(test)]
mod planning_diagnostics {
    use std::sync::Arc;

    use kanban_dto::{ApiError, ErrorCode};
    use serde_json::{Value, json};

    use crate::diagnostics::ProfileCatalogue;
    use crate::spec::testing::{spec_harness, spec_harness_with_catalogue};

    /// The PRD wire content with a story section naming `stories`,
    /// varied by name.
    fn content(name: &str, user_stories: &str) -> Value {
        json!({
            "name": name,
            "short_description": "Versioned Plan graphs of Specs",
            "problem_statement": "Planning must survive change without losing truth.",
            "solution": "Enforced story coverage.",
            "user_stories": user_stories,
            "implementation_decisions": "The gate is consumed by graph approval.",
            "testing_decisions": "Application tests prove the gate refuses gaps.",
            "out_of_scope": "The Ticket graph proposal.",
            "further_notes": "None",
        })
    }

    /// Author one Spec on the seeded CORE Project with the story
    /// section given, returning its minted number.
    fn spec_with_stories(core: &crate::dispatch::Core, user_stories: &str, key: &str) -> u64 {
        let created = core
            .command(
                "spec.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": key },
                    "project_id": 1,
                    "content": content("Registration", user_stories),
                }),
            )
            .expect("the Spec authors");
        created["number"]
            .as_u64()
            .expect("the minted number is a number")
    }

    /// A draft Plan holding the Specs given, with the dependency edges
    /// given, returning its identity and aggregate version.
    fn plan_over(
        core: &crate::dispatch::Core,
        specs: &[u64],
        edges: &[(u64, u64)],
        key: &str,
    ) -> (u64, u64) {
        let created = core
            .command(
                "plan.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": format!("{key}-create") },
                    "project_id": 1,
                }),
            )
            .expect("the Plan creates");
        let id = created["id"].as_u64().expect("the identity is a number");
        let mut version = created["version"]
            .as_u64()
            .expect("the version is a number");
        for spec in specs {
            let response = core
                .command(
                    "plan.spec.add",
                    &json!({
                        "mutation": {
                            "optimistic_version": version,
                            "idempotency_key": format!("{key}-add-{spec}"),
                        },
                        "plan_id": id,
                        "spec_number": spec,
                    }),
                )
                .expect("the Spec joins");
            version = response["version"]
                .as_u64()
                .expect("the version is a number");
        }
        for (from, to) in edges {
            let response = core
                .command(
                    "plan.edge.add",
                    &json!({
                        "mutation": {
                            "optimistic_version": version,
                            "idempotency_key": format!("{key}-edge-{from}-{to}"),
                        },
                        "plan_id": id,
                        "from_spec": from,
                        "to_spec": to,
                    }),
                )
                .expect("the edge lands");
            version = response["version"]
                .as_u64()
                .expect("the version is a number");
        }
        (id, version)
    }

    /// The stored identity of the Project's Spec numbered `number`.
    fn spec_id(core: &crate::dispatch::Core, number: u64) -> u64 {
        core.query("spec.list", &json!({ "project_id": 1 }))
            .expect("the list serves")["specs"]
            .as_array()
            .expect("the listing is an array")
            .iter()
            .find(|spec| spec["number"] == json!(number))
            .expect("the Spec is stored")["id"]
            .as_u64()
            .expect("the identity is a number")
    }

    /// One Implementation Ticket attached to the Spec given, claiming
    /// the stories named by one criterion each.
    fn implementation_claiming(
        core: &crate::dispatch::Core,
        spec: u64,
        stories: &[&str],
        key: &str,
    ) -> u64 {
        let criteria: Vec<Value> = stories
            .iter()
            .map(|story| json!({ "outcome": format!("{story} is delivered."), "stories": [story] }))
            .collect();
        let created = core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": key },
                    "project_id": 1,
                    "kind": "implementation",
                    "priority": "normal",
                    "spec_id": spec,
                    "slice": "Deliver the claimed stories end to end.",
                    "criteria": criteria,
                }),
            )
            .expect("the Implementation creates");
        created["id"].as_u64().expect("the identity is a number")
    }

    /// The diagnostics of one graph: the working shape when `version`
    /// is None, the frozen version when it is not.
    fn diagnose(core: &crate::dispatch::Core, plan_id: u64, version: Option<u64>) -> Value {
        core.query(
            "plan.diagnostics",
            &json!({ "plan_id": plan_id, "version": version }),
        )
        .expect("the diagnostics serve")
    }

    /// Two stories Spec 1 claims, for the ring and scope tests.
    const STORIES_ONE: &str = "\
- CORE-S1-US1: As an operator, I want linked criteria.
- CORE-S1-US2: As an operator, I want covered stories.
";

    /// The story Spec 2 claims.
    const STORIES_TWO: &str = "\
- CORE-S2-US1: As an operator, I want a gate before execution.
";

    /// The single story a lone Spec 1 claims.
    const STORIES_SOLE: &str = "\
- CORE-S1-US1: As an operator, I want a gate before execution.
";

    #[test]
    fn cycles_and_coverage_gaps_report_as_blocking() {
        let harness = spec_harness();
        let first = spec_with_stories(&harness.core, STORIES_ONE, "key-spec-one");
        let second = spec_with_stories(&harness.core, STORIES_TWO, "key-spec-two");
        let (plan, _) = plan_over(
            &harness.core,
            &[first, second],
            &[(first, second), (second, first)],
            "key-plan",
        );

        assert_eq!(
            diagnose(&harness.core, plan, None),
            json!({
                "cycles": [{ "spec_numbers": [1, 2] }],
                "coverage_gaps": [
                    {
                        "spec_number": 1,
                        "uncovered": ["CORE-S1-US1", "CORE-S1-US2"],
                        "claims_no_stories": false,
                    },
                    {
                        "spec_number": 2,
                        "uncovered": ["CORE-S2-US1"],
                        "claims_no_stories": false,
                    },
                ],
                "invalid_profiles": [],
                "blocking": true,
            }),
            "the ring and every unclaimed story block, and the \
             invalid-profile interface rides empty until the catalogue"
        );
    }

    #[test]
    fn a_member_claiming_no_story_reports_an_impossible_scope() {
        let harness = spec_harness();
        let only = spec_with_stories(
            &harness.core,
            "As an operator, I want prose alone.",
            "key-spec-prose",
        );
        let (plan, _) = plan_over(&harness.core, &[only], &[], "key-plan");

        assert_eq!(
            diagnose(&harness.core, plan, None),
            json!({
                "cycles": [],
                "coverage_gaps": [
                    {
                        "spec_number": 1,
                        "uncovered": [],
                        "claims_no_stories": true,
                    },
                ],
                "invalid_profiles": [],
                "blocking": true,
            }),
            "an empty scope admits no coverage, so the member blocks"
        );
    }

    #[test]
    fn a_fed_invalid_profile_list_blocks_on_its_own() {
        // The catalogue stand-in: every Plan's graph carries one
        // reference that resolves to no entry.
        struct Dangling;

        impl ProfileCatalogue for Dangling {
            fn invalid_references(
                &self,
                _project: &kanban_domain::Project,
                _order: &[kanban_domain::SpecNumber],
            ) -> Result<Vec<String>, ApiError> {
                Ok(vec!["ghost-profile".to_owned()])
            }
        }

        let harness = spec_harness_with_catalogue(Arc::new(Dangling));
        let (plan, _) = plan_over(&harness.core, &[], &[], "key-plan");

        assert_eq!(
            diagnose(&harness.core, plan, None),
            json!({
                "cycles": [],
                "coverage_gaps": [],
                "invalid_profiles": [{ "reference": "ghost-profile" }],
                "blocking": true,
            }),
            "a memberless graph holds no cycle and no gap, so the fed \
             invalid-profile list alone blocks"
        );
    }

    #[test]
    fn the_working_shape_and_frozen_versions_report_their_own_graphs() {
        let harness = spec_harness();
        let first = spec_with_stories(&harness.core, STORIES_ONE, "key-spec-one");
        let second = spec_with_stories(&harness.core, STORIES_TWO, "key-spec-two");
        let (plan, mut version) = plan_over(
            &harness.core,
            &[first, second],
            &[(first, second), (second, first)],
            "key-plan",
        );

        let activated = harness
            .core
            .command(
                "plan.activate",
                &json!({
                    "mutation": {
                        "optimistic_version": version,
                        "idempotency_key": "key-activate",
                    },
                    "plan_id": plan,
                }),
            )
            .expect("the ring freezes");
        version = activated["version"]
            .as_u64()
            .expect("the version is a number");
        let replanned = harness
            .core
            .command(
                "plan.replan",
                &json!({
                    "mutation": {
                        "optimistic_version": version,
                        "idempotency_key": "key-replan",
                    },
                    "plan_id": plan,
                }),
            )
            .expect("the draft reopens");
        version = replanned["version"]
            .as_u64()
            .expect("the version is a number");
        harness
            .core
            .command(
                "plan.edge.remove",
                &json!({
                    "mutation": {
                        "optimistic_version": version,
                        "idempotency_key": "key-break",
                    },
                    "plan_id": plan,
                    "from_spec": second,
                    "to_spec": first,
                }),
            )
            .expect("the ring breaks");

        let working = diagnose(&harness.core, plan, None);
        assert_eq!(
            working["cycles"],
            json!([]),
            "the working graph reports its own acyclic shape"
        );
        assert_eq!(working["blocking"], json!(true), "the gaps still block");

        let frozen = diagnose(&harness.core, plan, Some(1));
        assert_eq!(
            frozen["cycles"],
            json!([{ "spec_numbers": [1, 2] }]),
            "the frozen version keeps the ring it froze"
        );
    }

    #[test]
    fn the_scope_reads_the_operative_version() {
        let harness = spec_harness();
        let spec = spec_with_stories(&harness.core, STORIES_ONE, "key-spec-one");
        let spec_id = harness
            .core
            .query("spec.list", &json!({ "project_id": 1 }))
            .expect("the list serves")["specs"][0]["id"]
            .clone();
        let (plan, version) = plan_over(&harness.core, &[spec], &[], "key-plan");
        let _ = version;

        harness
            .core
            .command(
                "spec.version.approve",
                &json!({
                    "mutation": {
                        "optimistic_version": 1,
                        "idempotency_key": "key-approve",
                    },
                    "spec_id": spec_id,
                }),
            )
            .expect("version one approves");
        harness
            .core
            .command(
                "spec.content.update",
                &json!({
                    "mutation": {
                        "optimistic_version": 2,
                        "idempotency_key": "key-update",
                    },
                    "spec_id": spec_id,
                    "content": content(
                        "Registration",
                        "- CORE-S1-US1: kept.\n- CORE-S1-US2: kept.\n- CORE-S1-US3: added.\n",
                    ),
                }),
            )
            .expect("the material change mints a new draft");

        let diagnostics = diagnose(&harness.core, plan, None);
        assert_eq!(
            diagnostics["coverage_gaps"],
            json!([{
                "spec_number": 1,
                "uncovered": ["CORE-S1-US1", "CORE-S1-US2"],
                "claims_no_stories": false,
            }]),
            "the still-approved scope governs, not the working draft's \
             extra story"
        );
    }

    #[test]
    fn an_unknown_plan_or_version_is_not_found() {
        let harness = spec_harness();
        let spec = spec_with_stories(&harness.core, STORIES_ONE, "key-spec-one");
        let (plan, _) = plan_over(&harness.core, &[spec], &[], "key-plan");

        let error = harness
            .core
            .query(
                "plan.diagnostics",
                &json!({ "plan_id": 9, "version": null }),
            )
            .expect_err("the unknown Plan is refused");
        assert_eq!(error.code, ErrorCode::NotFound);

        let error = harness
            .core
            .query(
                "plan.diagnostics",
                &json!({ "plan_id": plan, "version": 9 }),
            )
            .expect_err("the unknown version is refused");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn a_member_whose_stories_are_all_claimed_reports_no_coverage_gap() {
        let harness = spec_harness();
        let only = spec_with_stories(&harness.core, STORIES_ONE, "key-spec-one");
        let spec = spec_id(&harness.core, only);
        implementation_claiming(
            &harness.core,
            spec,
            &["CORE-S1-US1", "CORE-S1-US2"],
            "key-ticket",
        );
        let (plan, _) = plan_over(&harness.core, &[only], &[], "key-plan");

        assert_eq!(
            diagnose(&harness.core, plan, None),
            json!({
                "cycles": [],
                "coverage_gaps": [],
                "invalid_profiles": [],
                "blocking": false,
            }),
            "every story the member claims is claimed by a criterion of \
             its own attached Ticket, so the member carries no gap and \
             the graph does not block"
        );
    }

    #[test]
    fn a_partly_claimed_member_reports_only_its_unclaimed_stories() {
        let harness = spec_harness();
        let covered = spec_with_stories(&harness.core, STORIES_ONE, "key-spec-one");
        let bare = spec_with_stories(&harness.core, STORIES_TWO, "key-spec-two");
        implementation_claiming(
            &harness.core,
            spec_id(&harness.core, covered),
            &["CORE-S1-US1"],
            "key-ticket",
        );
        let (plan, _) = plan_over(&harness.core, &[covered, bare], &[], "key-plan");

        assert_eq!(
            diagnose(&harness.core, plan, None)["coverage_gaps"],
            json!([
                {
                    "spec_number": 1,
                    "uncovered": ["CORE-S1-US2"],
                    "claims_no_stories": false,
                },
                {
                    "spec_number": 2,
                    "uncovered": ["CORE-S2-US1"],
                    "claims_no_stories": false,
                },
            ]),
            "the claimed story leaves the gap; the unclaimed one and the \
             member nothing is attached to stay blocking"
        );
    }

    #[test]
    fn an_unqualified_bug_claims_nothing_and_its_qualification_closes_the_gap() {
        let harness = spec_harness();
        let only = spec_with_stories(&harness.core, STORIES_SOLE, "key-spec-sole");
        let spec = spec_id(&harness.core, only);
        let bug = harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-bug" },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "normal",
                    "spec_id": spec,
                    "title": "The gate admits an uncovered graph",
                    "actual_behaviour": "The graph approves with a story nothing claims.",
                    "reporter_evidence": "The approval log shows the uncovered story.",
                }),
            )
            .expect("the Bug creates")["id"]
            .as_u64()
            .expect("the identity is a number");
        let (plan, _) = plan_over(&harness.core, &[only], &[], "key-plan");

        assert_eq!(
            diagnose(&harness.core, plan, None)["coverage_gaps"],
            json!([{
                "spec_number": 1,
                "uncovered": ["CORE-S1-US1"],
                "claims_no_stories": false,
            }]),
            "a Bug claims through its qualification alone (DR-TK-09), so \
             an unqualified one leaves the story a gap"
        );

        harness
            .core
            .command(
                "ticket.bug.qualify",
                &json!({
                    "mutation": { "optimistic_version": 1, "idempotency_key": "key-qualify" },
                    "ticket_id": bug,
                    "qualification": {
                        "affected_scope": "The graph approval gate.",
                        "environment": "The local control plane.",
                        "expected_behaviour": "The gate refuses an uncovered graph.",
                        "frequency": "Every approval of an uncovered graph.",
                        "reproduction": "Approve a graph leaving CORE-S1-US1 unclaimed.",
                        "risk": "An unowned story reaches execution.",
                        "severity": "high",
                        "criteria": [{
                            "outcome": "The gate refuses the uncovered graph.",
                            "stories": ["CORE-S1-US1"],
                        }],
                        "verification_steps": [{ "command": "cargo test -p kanban-app" }],
                    },
                }),
            )
            .expect("the qualification lands");

        assert_eq!(
            diagnose(&harness.core, plan, None),
            json!({
                "cycles": [],
                "coverage_gaps": [],
                "invalid_profiles": [],
                "blocking": false,
            }),
            "the qualification's criterion claims the story, so the \
             member carries no gap"
        );
    }

    #[test]
    fn the_query_rejects_unknown_fields() {
        let harness = spec_harness();
        let mut request = json!({ "plan_id": 1, "version": null });
        request["surprise"] = json!(true);

        let error = harness
            .core
            .query("plan.diagnostics", &request)
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }
}
