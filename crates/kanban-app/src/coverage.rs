//! The story coverage query: the executable gate's evaluation surface
//! (KAN-S3-US6). `spec.coverage.check` reads the story scope one
//! pinned Spec version claims and checks proposed story-linked
//! criteria against it — the same rules graph approval enforces
//! (DR-PS-13 to DR-PS-15, T23) — reporting the coverage gaps and rule
//! refusals the planning surfaces render (T16) before any graph
//! becomes executable (DR-PS-14).

use std::sync::Arc;

use kanban_domain::{
    AcceptanceCriterion, CriterionError, ProjectCode, SpecId, StoryRefError, StoryScope,
    UserStoryRef,
};
use kanban_dto::{
    ApiError, CoverageCriterionProposal, CriterionRefusal, RefusedCriterion,
    SpecCoverageCheckQuery, SpecCoverageCheckResponse, SpecCoverageClaim, SpecCoverageMatrixQuery,
    SpecCoverageMatrixResponse, SpecCoverageMatrixRow,
};
use serde_json::Value;

use crate::dispatch::{Core, QueryHandler};
use crate::mutation::parse_payload;
use crate::project::ProjectStore;
use crate::spec::SpecStore;
use crate::ticket::TicketStore;

/// Serves `spec.coverage.check`.
pub(crate) struct CheckCoverage {
    specs: Arc<dyn SpecStore>,
    projects: Arc<dyn ProjectStore>,
}

impl QueryHandler for CheckCoverage {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: SpecCoverageCheckQuery = parse_payload(payload)?;
        let spec = self
            .specs
            .find(SpecId::new(query.spec_id))?
            .ok_or_else(|| ApiError::not_found(&format!("spec {}", query.spec_id)))?;
        let project = self.projects.find(spec.project())?.ok_or_else(|| {
            ApiError::internal(&format!(
                "spec {} belongs to no stored Project",
                query.spec_id
            ))
        })?;
        let pinned = spec
            .pinned_version(query.version)
            .ok_or_else(|| ApiError::not_found(&format!("version {}", query.version)))?;
        let scope = StoryScope::extract(
            project.code(),
            spec.number(),
            pinned.content().user_stories(),
        )
        .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let mut criteria = Vec::with_capacity(query.criteria.len());
        let mut refused = Vec::new();
        for proposal in &query.criteria {
            match criterion_of(proposal, project.code()) {
                Ok(criterion) => criteria.push(criterion),
                Err(reason) => refused.push(RefusedCriterion {
                    outcome: proposal.outcome.clone(),
                    reason,
                }),
            }
        }
        let uncovered = scope.uncovered(&criteria);
        let response = SpecCoverageCheckResponse {
            scope: scope
                .stories()
                .iter()
                .map(|story| story.render(project.code()))
                .collect(),
            uncovered: uncovered
                .iter()
                .map(|story| story.render(project.code()))
                .collect(),
            executable: refused.is_empty() && uncovered.is_empty(),
            refused,
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Decode one proposal into a rule-valid criterion, or the refusal
/// naming the rule it broke.
fn criterion_of(
    proposal: &CoverageCriterionProposal,
    code: &ProjectCode,
) -> Result<AcceptanceCriterion, CriterionRefusal> {
    let mut stories = Vec::with_capacity(proposal.stories.len());
    for named in &proposal.stories {
        stories.push(UserStoryRef::parse(named, code).map_err(story_refusal)?);
    }
    AcceptanceCriterion::new(proposal.outcome.clone(), stories).map_err(criterion_refusal)
}

/// The wire refusal a refused story link reports.
fn story_refusal(error: StoryRefError) -> CriterionRefusal {
    match error {
        StoryRefError::ForeignProject => CriterionRefusal::ForeignStory,
        StoryRefError::Malformed | StoryRefError::Zero => CriterionRefusal::MalformedStory,
    }
}

/// The wire refusal a refused criterion reports.
fn criterion_refusal(error: CriterionError) -> CriterionRefusal {
    match error {
        CriterionError::NoOutcome => CriterionRefusal::NoOutcome,
        CriterionError::Unlinked => CriterionRefusal::Unlinked,
        CriterionError::TechnicalCommand => CriterionRefusal::TechnicalCommand,
    }
}

impl Core {
    /// Register the story coverage query, resolving the Spec and its
    /// Project through the same stores the Spec commands use.
    pub fn register_coverage_check(
        &mut self,
        specs: Arc<dyn SpecStore>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), crate::dispatch::RegistrationError> {
        self.register_query(
            "spec.coverage.check",
            Arc::new(CheckCoverage { specs, projects }),
        )
    }

    /// Register the coverage matrix query — the story-to-criterion-
    /// to-Ticket view of one Spec version (DR-PS-18) — resolving the
    /// Spec and its Project through the Spec stores and the claims
    /// through the Ticket store.
    pub fn register_coverage_matrix(
        &mut self,
        tickets: Arc<dyn TicketStore>,
        specs: Arc<dyn SpecStore>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), crate::dispatch::RegistrationError> {
        self.register_query(
            "spec.coverage.matrix",
            Arc::new(CoverageMatrix {
                tickets,
                specs,
                projects,
            }),
        )
    }
}

/// Serves `spec.coverage.matrix`: one Spec version's claims, story by
/// story, from every Ticket attached to the Spec. The version read is
/// the query's when it names one, else the approved one when
/// operative, else the working content.
struct CoverageMatrix {
    tickets: Arc<dyn TicketStore>,
    specs: Arc<dyn SpecStore>,
    projects: Arc<dyn ProjectStore>,
}

impl QueryHandler for CoverageMatrix {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: SpecCoverageMatrixQuery = parse_payload(payload)?;
        let spec = self
            .specs
            .find(SpecId::new(query.spec_id))?
            .ok_or_else(|| ApiError::not_found(&format!("spec {}", query.spec_id)))?;
        let project = self.projects.find(spec.project())?.ok_or_else(|| {
            ApiError::internal(&format!(
                "spec {} belongs to no stored Project",
                query.spec_id
            ))
        })?;
        let version = match query.version {
            Some(number) => spec
                .pinned_version(number)
                .ok_or_else(|| ApiError::not_found(&format!("version {number}")))?,
            None => spec
                .approved_version()
                .or_else(|| spec.current_version())
                .ok_or_else(|| ApiError::not_found(&format!("spec {}", query.spec_id)))?,
        };
        let scope = StoryScope::extract(
            project.code(),
            spec.number(),
            version.content().user_stories(),
        )
        .map_err(|error| ApiError::invalid_request(&error.to_string()))?;
        let attached: Vec<_> = self
            .tickets
            .list(project.id())?
            .into_iter()
            .filter(|ticket| ticket.spec() == Some(spec.id()))
            .collect();
        let stories = scope
            .stories()
            .iter()
            .map(|story| SpecCoverageMatrixRow {
                story: story.render(project.code()),
                claims: attached
                    .iter()
                    .flat_map(|ticket| {
                        claims_of(ticket)
                            .iter()
                            .filter(|criterion| criterion.stories().contains(story))
                            .map(|criterion| SpecCoverageClaim {
                                ticket_id: ticket.id().value(),
                                ticket_number: ticket.number().value(),
                                outcome: criterion.outcome().to_owned(),
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect(),
            })
            .collect();
        let response = SpecCoverageMatrixResponse {
            spec_id: spec.id().value(),
            version: version.number(),
            stories,
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Every story-linked criterion one Ticket claims: an Implementation
/// claims through its criteria, a qualified Bug through its
/// qualification's criteria (DR-TK-09), and a Task claims nothing
/// (DR-TK-07).
fn claims_of(ticket: &kanban_domain::Ticket) -> Vec<&AcceptanceCriterion> {
    match ticket.bug() {
        Some(bug) => bug
            .qualification()
            .map(|record| record.criteria().iter().collect())
            .unwrap_or_default(),
        None => ticket.criteria().iter().collect(),
    }
}

#[cfg(test)]
mod executable_gate {
    use kanban_dto::ErrorCode;
    use serde_json::{Value, json};

    use crate::spec::testing::spec_harness;

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
    /// section given, returning its identity.
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
        created["id"].as_u64().expect("the identity is a number")
    }

    /// A check query against one Spec version with the proposals
    /// given as (outcome, stories) pairs.
    fn check(spec_id: u64, version: u64, criteria: &[(&str, &[&str])]) -> Value {
        let criteria: Vec<Value> = criteria
            .iter()
            .map(|(outcome, stories)| {
                json!({
                    "outcome": outcome,
                    "stories": stories,
                })
            })
            .collect();
        json!({
            "spec_id": spec_id,
            "version": version,
            "criteria": criteria,
        })
    }

    /// The story section the gate tests vary claims from.
    const STORIES: &str = "\
- CORE-S1-US1: As an operator, I want linked criteria.
- CORE-S1-US2: As an operator, I want covered stories.
- CORE-S1-US3: As an operator, I want a gate before execution.
";

    #[test]
    fn the_gate_refuses_while_any_story_stays_uncovered() {
        let harness = spec_harness();
        let spec = spec_with_stories(&harness.core, STORIES, "key-author");

        let response = harness
            .core
            .query(
                "spec.coverage.check",
                &check(spec, 1, &[("Criteria link to stories.", &["CORE-S1-US1"])]),
            )
            .expect("the check serves");

        assert_eq!(
            response,
            json!({
                "scope": ["CORE-S1-US1", "CORE-S1-US2", "CORE-S1-US3"],
                "uncovered": ["CORE-S1-US2", "CORE-S1-US3"],
                "refused": [],
                "executable": false,
            }),
            "the gaps are listed in scope order and the gate refuses (DR-PS-14)"
        );
    }

    #[test]
    fn the_gate_passes_when_the_pinned_version_is_fully_claimed() {
        let harness = spec_harness();
        let spec = spec_with_stories(&harness.core, STORIES, "key-author");
        // Approving freezes version one; the check pins it like a
        // Ticket graph would (DR-PS-11).
        let approved = harness
            .core
            .command(
                "spec.version.approve",
                &json!({
                    "mutation": { "optimistic_version": 1, "idempotency_key": "key-approve" },
                    "spec_id": spec,
                }),
            )
            .expect("the draft approves");

        let _ = approved;
        let response = harness
            .core
            .query(
                "spec.coverage.check",
                &check(
                    spec,
                    1,
                    &[
                        ("Criteria link to stories.", &["CORE-S1-US1"]),
                        (
                            "Every story is claimed by some criterion.",
                            &["CORE-S1-US2", "CORE-S1-US3"],
                        ),
                    ],
                ),
            )
            .expect("the check serves");

        assert_eq!(
            response,
            json!({
                "scope": ["CORE-S1-US1", "CORE-S1-US2", "CORE-S1-US3"],
                "uncovered": [],
                "refused": [],
                "executable": true,
            }),
            "claims accumulate across criteria (DR-PS-14)"
        );
    }

    #[test]
    fn unlinked_proposals_are_refused() {
        let harness = spec_harness();
        let spec = spec_with_stories(&harness.core, STORIES, "key-author");

        let response = harness
            .core
            .query(
                "spec.coverage.check",
                &check(
                    spec,
                    1,
                    &[
                        ("Criteria link to stories.", &["CORE-S1-US1"]),
                        (
                            "Every story is claimed by some criterion.",
                            &["CORE-S1-US2", "CORE-S1-US3"],
                        ),
                        ("An unlinked outcome ships nothing owned.", &[]),
                    ],
                ),
            )
            .expect("the check serves");

        assert_eq!(
            response["refused"],
            json!([
                {
                    "outcome": "An unlinked outcome ships nothing owned.",
                    "reason": "unlinked",
                }
            ]),
            "an unlinked criterion never exists (DR-PS-13)"
        );
        assert_eq!(response["executable"], json!(false));
    }

    #[test]
    fn technical_commands_are_refused_as_criteria() {
        let harness = spec_harness();
        let spec = spec_with_stories(&harness.core, STORIES, "key-author");

        let response = harness
            .core
            .query(
                "spec.coverage.check",
                &check(
                    spec,
                    1,
                    &[
                        ("cargo test -p kanban-domain coverage", &["CORE-S1-US1"]),
                        (
                            "The suite `cargo test -p x` passes on the approval tip.",
                            &["CORE-S1-US1"],
                        ),
                    ],
                ),
            )
            .expect("the check serves");

        assert_eq!(
            response["refused"],
            json!([
                {
                    "outcome": "cargo test -p kanban-domain coverage",
                    "reason": "technical_command",
                }
            ]),
            "commands are Verification Steps, never criteria (DR-PS-15)"
        );
        assert_eq!(response["executable"], json!(false));
    }

    #[test]
    fn malformed_and_foreign_story_links_are_refused() {
        let harness = spec_harness();
        let spec = spec_with_stories(&harness.core, STORIES, "key-author");

        let response = harness
            .core
            .query(
                "spec.coverage.check",
                &check(
                    spec,
                    1,
                    &[
                        ("A malformed link names no story.", &["banana"]),
                        (
                            "A foreign link names another Project's story.",
                            &["EDGE-S1-US1"],
                        ),
                    ],
                ),
            )
            .expect("the check serves");

        assert_eq!(
            response["refused"],
            json!([
                {
                    "outcome": "A malformed link names no story.",
                    "reason": "malformed_story",
                },
                {
                    "outcome": "A foreign link names another Project's story.",
                    "reason": "foreign_story",
                }
            ]),
            "every link names a User Story of this Project"
        );
        assert_eq!(response["executable"], json!(false));
    }

    #[test]
    fn an_unknown_spec_or_version_is_not_found() {
        let harness = spec_harness();
        let spec = spec_with_stories(&harness.core, STORIES, "key-author");

        let error = harness
            .core
            .query("spec.coverage.check", &check(9, 1, &[]))
            .expect_err("the unknown Spec is refused");
        assert_eq!(error.code, ErrorCode::NotFound);

        let error = harness
            .core
            .query("spec.coverage.check", &check(spec, 9, &[]))
            .expect_err("the unknown version is refused");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn a_version_claiming_no_stories_admits_no_coverage() {
        let harness = spec_harness();
        let spec = spec_with_stories(
            &harness.core,
            "As an operator, I want prose alone.",
            "key-author",
        );

        let error = harness
            .core
            .query(
                "spec.coverage.check",
                &check(spec, 1, &[("Any outcome.", &["CORE-S1-US1"])]),
            )
            .expect_err("an empty scope can never prove coverage");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "the Spec version claims no User Stories to cover"
        );
    }

    #[test]
    fn the_check_rejects_unknown_fields() {
        let harness = spec_harness();
        let mut request = check(1, 1, &[]);
        request["surprise"] = json!(true);

        let error = harness
            .core
            .query("spec.coverage.check", &request)
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `surprise`");
    }
}

#[cfg(test)]
mod coverage_matrix {
    use serde_json::{Value, json};

    use crate::ticket::testing::ticket_harness;

    /// The PRD wire content with a story section naming `stories`.
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

    /// The story section the matrix tests read.
    const STORIES: &str = "- CORE-S1-US1: As an operator, I want linked criteria.
- CORE-S1-US2: As an operator, I want covered stories.
- CORE-S1-US3: As an operator, I want a gate before execution.
";

    /// A core whose Spec and Ticket operations share one in-memory
    /// world, so the matrix reads Tickets the same Spec commands
    /// wrote. The ticket harness already wires Plans, Specs, and
    /// Tickets over shared stores, matrix included.
    fn shared() -> crate::dispatch::Core {
        ticket_harness().core
    }

    /// Author the Spec with the fixture story section, returning its
    /// identity.
    fn spec_with_stories(core: &crate::dispatch::Core) -> u64 {
        let created = core
            .command(
                "spec.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "key-author" },
                    "project_id": 1,
                    "content": content("Registration", STORIES),
                }),
            )
            .expect("the Spec authors");
        created["id"].as_u64().expect("the identity is a number")
    }

    /// One Implementation Ticket attached to the Spec claiming
    /// `stories`, returning its identity.
    fn implementation(
        core: &crate::dispatch::Core,
        spec: u64,
        slice: &str,
        stories: Value,
        key: &str,
    ) -> u64 {
        let created = core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": key },
                    "project_id": 1,
                    "kind": "implementation",
                    "priority": "normal",
                    "spec_id": spec,
                    "slice": slice,
                    "criteria": stories,
                }),
            )
            .expect("the Ticket creates");
        created["id"].as_u64().expect("the identity is a number")
    }

    #[test]
    fn the_matrix_lists_every_claim_and_gap_story_by_story() {
        let core = shared();
        let spec = spec_with_stories(&core);
        let first = implementation(
            &core,
            spec,
            "Criteria link to stories",
            json!([{ "outcome": "Criteria link to stories.", "stories": ["CORE-S1-US1"] }]),
            "key-ticket-1",
        );
        let second = implementation(
            &core,
            spec,
            "Every story is claimed",
            json!([
                { "outcome": "Every story is claimed by some criterion.", "stories": ["CORE-S1-US2", "CORE-S1-US3"] },
            ]),
            "key-ticket-2",
        );

        let response = core
            .query("spec.coverage.matrix", &json!({ "spec_id": spec }))
            .expect("the matrix serves");

        assert_eq!(
            response,
            json!({
                "spec_id": spec,
                "version": 1,
                "stories": [
                    {
                        "story": "CORE-S1-US1",
                        "claims": [{
                            "ticket_id": first,
                            "ticket_number": 1,
                            "outcome": "Criteria link to stories.",
                        }],
                    },
                    {
                        "story": "CORE-S1-US2",
                        "claims": [{
                            "ticket_id": second,
                            "ticket_number": 2,
                            "outcome": "Every story is claimed by some criterion.",
                        }],
                    },
                    {
                        "story": "CORE-S1-US3",
                        "claims": [{
                            "ticket_id": second,
                            "ticket_number": 2,
                            "outcome": "Every story is claimed by some criterion.",
                        }],
                    },
                ],
            }),
            "the matrix renders story, criterion, and Ticket together (DR-PS-18)"
        );
    }

    #[test]
    fn an_uncovered_story_carries_an_empty_claim_list() {
        let core = shared();
        let spec = spec_with_stories(&core);
        implementation(
            &core,
            spec,
            "Criteria link to stories",
            json!([{ "outcome": "Criteria link to stories.", "stories": ["CORE-S1-US1"] }]),
            "key-ticket-1",
        );

        let response = core
            .query("spec.coverage.matrix", &json!({ "spec_id": spec }))
            .expect("the matrix serves");

        assert_eq!(
            response["stories"].as_array().map(|rows| {
                rows.iter()
                    .map(|row| (row["story"].clone(), row["claims"].as_array().map(Vec::len)))
                    .collect::<Vec<_>>()
            }),
            Some(vec![
                (json!("CORE-S1-US1"), Some(1)),
                (json!("CORE-S1-US2"), Some(0)),
                (json!("CORE-S1-US3"), Some(0)),
            ]),
            "the empty claim list is the row's coverage gap"
        );
    }

    #[test]
    fn the_version_read_defaults_to_the_approved_one() {
        let core = shared();
        let spec = spec_with_stories(&core);
        core.command(
            "spec.version.approve",
            &json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "key-approve" },
                "spec_id": spec,
            }),
        )
        .expect("version one approves");
        core.command(
            "spec.content.update",
            &json!({
                "mutation": { "optimistic_version": 2, "idempotency_key": "key-update" },
                "spec_id": spec,
                "content": content("Registration", "- CORE-S1-US9: As an operator, I want a later draft.\n"),
            }),
        )
        .expect("the material change mints a draft");

        let approved = core
            .query("spec.coverage.matrix", &json!({ "spec_id": spec }))
            .expect("the matrix serves");
        assert_eq!(
            approved["version"],
            json!(1),
            "the approved version is operative"
        );

        let draft = core
            .query(
                "spec.coverage.matrix",
                &json!({ "spec_id": spec, "version": 2 }),
            )
            .expect("the matrix serves");
        assert_eq!(draft["version"], json!(2));
        assert_eq!(
            draft["stories"].as_array().map(Vec::len),
            Some(1),
            "an explicit version names the scope it wants"
        );
    }

    #[test]
    fn an_unknown_spec_or_version_is_not_found_and_unknown_fields_refused() {
        let core = shared();
        let spec = spec_with_stories(&core);

        let error = core
            .query("spec.coverage.matrix", &json!({ "spec_id": 9 }))
            .expect_err("the unknown Spec is refused");
        assert_eq!(error.code, kanban_dto::ErrorCode::NotFound);

        let error = core
            .query(
                "spec.coverage.matrix",
                &json!({ "spec_id": spec, "version": 9 }),
            )
            .expect_err("the unknown version is refused");
        assert_eq!(error.code, kanban_dto::ErrorCode::NotFound);

        let error = core
            .query(
                "spec.coverage.matrix",
                &json!({ "spec_id": spec, "surprise": true }),
            )
            .expect_err("unknown fields are rejected");
        assert_eq!(error.code, kanban_dto::ErrorCode::UnknownField);
    }
}
