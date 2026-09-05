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
    SpecCoverageCheckQuery, SpecCoverageCheckResponse,
};
use serde_json::Value;

use crate::dispatch::{Core, QueryHandler};
use crate::mutation::parse_payload;
use crate::project::ProjectStore;
use crate::spec::SpecStore;

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
