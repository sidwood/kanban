//! Export commands and queries: render Plans, Specs, and Tickets as
//! deterministic Markdown — stable ids, ordering, and formatting —
//! for a configured directory within the Seed, and report drift
//! between those exports and the current planning state on demand
//! (KAN-S6-US5). Exports never commit, push, or stage anything:
//! rendering is pure, and the only effect a render performs is the
//! atomic file replacement its port performs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kanban_domain::{
    DependencyEdge, NumberKind, Plan, PlanId, PlanState, Project, ProjectCode, Spec, SpecContent,
    SpecId, SpecNumber, Ticket, TicketKind,
};
use kanban_dto::{
    ApiError, ExportDriftEntry, ExportDriftQuery, ExportDriftResponse, ExportDriftStatus,
    ExportRenderRequest, ExportRenderResponse,
};
use serde_json::Value;

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::mutation::{CommandEffects, CommandHandler, ParsedCommand, parse_payload};
use crate::plan::PlanStore;
use crate::project::ProjectStore;
use crate::spec::SpecStore;
use crate::ticket::TicketStore;

/// One rendered export file: the path it belongs at, relative to the
/// configured directory, and the complete deterministic Markdown
/// document. Rendering is a pure function of the planning state, so
/// identical state renders byte-identical artifacts every time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportArtifact {
    /// The artifact's path relative to the export directory, with
    /// forward separators, for example `plans/CORE-P1.md`.
    pub relative_path: String,
    /// The complete Markdown document.
    pub markdown: String,
}

/// Render one Project's Plans, Specs, and Tickets as deterministic
/// Markdown artifacts: one file per artifact under `plans/`, `specs/`,
/// and `tickets/`, named by the rendered identity (`CORE-P1`), each
/// kind ordered by its minted number.
pub fn render_project_export(
    project: &Project,
    plans: &[Plan],
    specs: &[Spec],
    tickets: &[Ticket],
) -> Vec<ExportArtifact> {
    let code = project.code();
    let mut plans = plans.to_vec();
    plans.sort_by_key(|plan| plan.number());
    let mut specs = specs.to_vec();
    specs.sort_by_key(|spec| spec.number().value());
    let mut tickets = tickets.to_vec();
    tickets.sort_by_key(|ticket| ticket.number().value());

    let plan_labels: BTreeMap<PlanId, String> = plans
        .iter()
        .map(|plan| (plan.id(), NumberKind::Plan.render(code, plan.number())))
        .collect();
    let spec_labels: BTreeMap<SpecId, String> = specs
        .iter()
        .map(|spec| {
            (
                spec.id(),
                NumberKind::Spec.render(code, spec.number().value()),
            )
        })
        .collect();

    plans
        .iter()
        .map(|plan| ExportArtifact {
            relative_path: format!("plans/{}.md", plan_labels[&plan.id()]),
            markdown: render_plan(code, plan),
        })
        .chain(specs.iter().map(|spec| ExportArtifact {
            relative_path: format!("specs/{}.md", spec_labels[&spec.id()]),
            markdown: render_spec(code, &plan_labels, spec),
        }))
        .chain(tickets.iter().map(|ticket| ExportArtifact {
            relative_path: format!(
                "tickets/{}.md",
                NumberKind::Ticket.render(code, ticket.number().value())
            ),
            markdown: render_ticket(code, &spec_labels, ticket),
        }))
        .collect()
}

/// One Plan's document: lifecycle, display order, dependencies, and
/// every frozen version.
fn render_plan(code: &ProjectCode, plan: &Plan) -> String {
    let mut document = String::new();
    document.push_str(&format!(
        "# {} — Plan\n\n",
        NumberKind::Plan.render(code, plan.number())
    ));
    document.push_str(&format!("- State: {}\n", plan_state_name(plan)));
    document.push_str(&format!("- Aggregate version: {}\n", plan.version()));
    document.push_str("\n## Display order\n\n");
    push_numbered(&mut document, &ordered_specs(code, plan.order()));
    document.push_str("\n## Dependencies\n\n");
    push_bulleted(
        &mut document,
        &sorted_edges(code, plan.edges())
            .into_iter()
            .map(|(from, to)| format!("{from} → {to}"))
            .collect::<Vec<_>>(),
    );
    document.push_str("\n## Frozen versions\n\n");
    if plan.versions().is_empty() {
        document.push_str("(none)\n");
    } else {
        for version in plan.versions() {
            document.push_str(&format!("### Version {}\n\n", version.number()));
            let order: Vec<_> = ordered_specs(code, version.order());
            document.push_str(&format!("- Order: {}\n", joined_or_none(&order)));
            let edges: Vec<_> = sorted_edges(code, version.edges())
                .into_iter()
                .map(|(from, to)| format!("{from} → {to}"))
                .collect::<Vec<_>>();
            document.push_str(&format!("- Edges: {}\n", joined_or_none(&edges)));
        }
    }
    document
}

/// One Spec's document: execution, Plan binding, and every content
/// version with its nine PRD sections.
fn render_spec(code: &ProjectCode, plan_labels: &BTreeMap<PlanId, String>, spec: &Spec) -> String {
    let mut document = String::new();
    document.push_str(&format!(
        "# {} — {}\n\n",
        NumberKind::Spec.render(code, spec.number().value()),
        spec.name()
    ));
    document.push_str(&format!("- Execution: {}\n", spec.execution().wire_name()));
    if let Some(binding) = spec.plan() {
        let label = plan_labels
            .get(&binding)
            .cloned()
            .unwrap_or_else(|| format!("plan {}", binding.value()));
        document.push_str(&format!("- Plan: {label}\n"));
    }
    document.push_str(&format!("- Aggregate version: {}\n", spec.version()));
    for version in spec.versions() {
        document.push_str(&format!(
            "\n## Version {} — {}\n",
            version.number(),
            version.state().wire_name()
        ));
        for (heading, body) in spec_sections(version.content()) {
            document.push_str(&format!("\n### {heading}\n\n{body}\n"));
        }
    }
    document
}

/// One Ticket's document: the kind's schema under the kind's own
/// headings.
fn render_ticket(
    code: &ProjectCode,
    spec_labels: &BTreeMap<SpecId, String>,
    ticket: &Ticket,
) -> String {
    let mut document = String::new();
    document.push_str(&format!(
        "# {} — {}\n\n",
        NumberKind::Ticket.render(code, ticket.number().value()),
        kind_name(ticket.kind())
    ));
    document.push_str(&format!("- Priority: {}\n", ticket.priority().wire_name()));
    document.push_str(&format!("- State: {}\n", ticket.state().wire_name()));
    if let Some(spec) = ticket.spec() {
        let label = spec_labels
            .get(&spec)
            .cloned()
            .unwrap_or_else(|| format!("spec {}", spec.value()));
        document.push_str(&format!("- Spec: {label}\n"));
    }
    document.push_str(&format!("- Aggregate version: {}\n", ticket.version()));
    if let Some(slice) = ticket.slice() {
        document.push_str(&format!("\n## Slice\n\n{slice}\n"));
    }
    if let Some(title) = ticket.title() {
        document.push_str(&format!("\n## Title\n\n{title}\n"));
    }
    if !ticket.criteria().is_empty() {
        document.push_str("\n## Acceptance criteria\n\n");
        for (index, criterion) in ticket.criteria().iter().enumerate() {
            let stories: Vec<_> = criterion
                .stories()
                .iter()
                .map(|story| story.render(code))
                .collect();
            document.push_str(&format!(
                "{}. {} [{}]\n",
                index + 1,
                criterion.outcome(),
                stories.join(", ")
            ));
        }
    }
    document
}

/// The nine PRD sections with their display headings, in editorial
/// order; an empty section renders a visible `(none)` placeholder so
/// the document shape stays stable.
fn spec_sections(content: &SpecContent) -> Vec<(&'static str, String)> {
    let headings = [
        "Name",
        "Short description",
        "Problem statement",
        "Solution",
        "User stories",
        "Implementation decisions",
        "Testing decisions",
        "Out of scope",
        "Further notes",
    ];
    let bodies = [
        content.name(),
        content.short_description(),
        content.problem_statement(),
        content.solution(),
        content.user_stories(),
        content.implementation_decisions(),
        content.testing_decisions(),
        content.out_of_scope(),
        content.further_notes(),
    ];
    headings
        .into_iter()
        .zip(bodies)
        .map(|(heading, body)| {
            let text = if body.trim().is_empty() {
                "(none)"
            } else {
                body
            };
            (heading, text.to_owned())
        })
        .collect()
}

/// A Spec display order as rendered identities.
fn ordered_specs(code: &ProjectCode, order: &[SpecNumber]) -> Vec<String> {
    order
        .iter()
        .map(|number| NumberKind::Spec.render(code, number.value()))
        .collect()
}

/// Dependency edges as rendered identity pairs, ordered by endpoint so
/// the document never depends on insertion history.
fn sorted_edges(code: &ProjectCode, edges: &[DependencyEdge]) -> Vec<(String, String)> {
    let mut rendered: Vec<_> = edges
        .iter()
        .map(|edge| {
            (
                NumberKind::Spec.render(code, edge.from().value()),
                NumberKind::Spec.render(code, edge.to().value()),
            )
        })
        .collect();
    rendered.sort();
    rendered
}

fn push_numbered(document: &mut String, items: &[String]) {
    if items.is_empty() {
        document.push_str("(none)\n");
        return;
    }
    for (index, item) in items.iter().enumerate() {
        document.push_str(&format!("{}. {}\n", index + 1, item));
    }
}

fn push_bulleted(document: &mut String, items: &[String]) {
    if items.is_empty() {
        document.push_str("(none)\n");
        return;
    }
    for item in items {
        document.push_str(&format!("- {item}\n"));
    }
}

fn joined_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_owned()
    } else {
        items.join(", ")
    }
}

fn plan_state_name(plan: &Plan) -> &'static str {
    match plan.state() {
        PlanState::Draft => "draft",
        PlanState::Active => "active",
        PlanState::Complete => "complete",
        PlanState::Cancelled => "cancelled",
        PlanState::Archived => "archived",
    }
}

fn kind_name(kind: TicketKind) -> &'static str {
    match kind {
        TicketKind::Implementation => "Implementation",
        TicketKind::Bug => "Bug",
        TicketKind::Task => "Task",
    }
}

/// The file port export operations write and read through. The
/// service wires the local-filesystem adapter; tests steer an
/// in-memory one. `replace` is the only mutating call the export
/// surface ever performs, and the port holds no git capability at
/// all, so an export can never commit, push, or stage anything
/// (KAN-T35-AC3, DR-LW-13).
pub trait ExportFiles: Send + Sync {
    /// Replace the file at `path` with `contents` atomically: a
    /// reader sees either the previous bytes or the complete new
    /// ones, never a partial write. Parent directories are created.
    fn replace(&self, path: &Path, contents: &[u8]) -> Result<(), ApiError>;

    /// The bytes currently at `path`; `None` when no file exists.
    fn current(&self, path: &Path) -> Result<Option<Vec<u8>>, ApiError>;

    /// Every regular file under `directory`, sorted by path; an
    /// empty list when the directory does not exist.
    fn walk(&self, directory: &Path) -> Result<Vec<PathBuf>, ApiError>;
}

/// The stores every export operation reads through, and the file
/// port it writes and compares through.
#[derive(Clone)]
struct ExportContext {
    plans: Arc<dyn PlanStore>,
    specs: Arc<dyn SpecStore>,
    tickets: Arc<dyn TicketStore>,
    projects: Arc<dyn ProjectStore>,
    files: Arc<dyn ExportFiles>,
}

impl ExportContext {
    /// The Project an export addresses, its planning state, and the
    /// rendered artifacts of that state.
    fn render(&self, project_id: u64) -> Result<(Project, Vec<ExportArtifact>), ApiError> {
        let project = self
            .projects
            .find(kanban_domain::ProjectId::new(project_id))?
            .ok_or_else(|| ApiError::not_found(&format!("project {project_id}")))?;
        let plans = self.plans.list(project.id())?;
        let specs = self.specs.list(project.id())?;
        let tickets = self.tickets.list(project.id())?;
        let artifacts = render_project_export(&project, &plans, &specs, &tickets);
        Ok((project, artifacts))
    }
}

/// The configured export directory within the Seed: a relative path
/// without parent traversal, joined onto the Project's Seed
/// Workspace, so an export can never write outside the Seed
/// (DR-LW-12).
fn export_root(project: &Project, directory: &str) -> Result<PathBuf, ApiError> {
    let trimmed = directory.trim();
    let refused = || {
        ApiError::invalid_request(
            "the export directory must be relative to the Seed Workspace and must not \
             contain parent segments",
        )
    };
    if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.contains('\\') {
        return Err(refused());
    }
    if trimmed
        .split('/')
        .any(|segment| segment == ".." || segment == ".")
    {
        return Err(refused());
    }
    Ok(Path::new(project.registration().seed_workspace()).join(trimmed))
}

impl Core {
    /// Register the export operations against the planning stores,
    /// resolving Projects through `projects` and writing files
    /// through `files`.
    pub fn register_exports(
        &mut self,
        plans: Arc<dyn PlanStore>,
        specs: Arc<dyn SpecStore>,
        tickets: Arc<dyn TicketStore>,
        projects: Arc<dyn ProjectStore>,
        files: Arc<dyn ExportFiles>,
    ) -> Result<(), RegistrationError> {
        let context = ExportContext {
            plans,
            specs,
            tickets,
            projects,
            files,
        };
        self.register_command("export.render", Arc::new(RenderExport(context.clone())))?;
        self.register_query("export.drift", Arc::new(ExportDrift { context }))?;
        Ok(())
    }
}

/// Serves `export.render`.
struct RenderExport(ExportContext);

impl CommandHandler for RenderExport {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ExportRenderRequest>(payload)?;
        ParsedCommand::lift("export", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        // A render changes no aggregate; the file replacement is its
        // whole effect.
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: ExportRenderRequest = parse_payload(&command.payload)?;
        let (project, artifacts) = self.0.render(request.project_id)?;
        let root = export_root(&project, &request.directory)?;
        for artifact in &artifacts {
            self.0.files.replace(
                &root.join(&artifact.relative_path),
                artifact.markdown.as_bytes(),
            )?;
        }
        let response = ExportRenderResponse {
            project_id: request.project_id,
            directory: request.directory.trim().to_owned(),
            files: artifacts
                .iter()
                .map(|artifact| artifact.relative_path.clone())
                .collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `export.drift`.
struct ExportDrift {
    context: ExportContext,
}

impl QueryHandler for ExportDrift {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        let query: ExportDriftQuery = parse_payload(payload)?;
        let (project, artifacts) = self.context.render(query.project_id)?;
        let root = export_root(&project, &query.directory)?;
        let expected: BTreeMap<&str, &ExportArtifact> = artifacts
            .iter()
            .map(|artifact| (artifact.relative_path.as_str(), artifact))
            .collect();

        let mut entries: Vec<ExportDriftEntry> = Vec::new();
        for (path, artifact) in &expected {
            let status = match self.context.files.current(&root.join(*path))? {
                None => ExportDriftStatus::Missing,
                Some(bytes) if bytes != artifact.markdown.as_bytes() => ExportDriftStatus::Differs,
                Some(_) => continue,
            };
            entries.push(ExportDriftEntry {
                path: (*path).to_owned(),
                status,
            });
        }

        for present in self.context.files.walk(&root)? {
            let relative = present
                .strip_prefix(&root)
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| present.to_string_lossy().into_owned());
            if !expected.contains_key(relative.as_str()) {
                entries.push(ExportDriftEntry {
                    path: relative,
                    status: ExportDriftStatus::Unmatched,
                });
            }
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));

        let response = ExportDriftResponse {
            project_id: query.project_id,
            directory: query.directory.trim().to_owned(),
            in_drift: !entries.is_empty(),
            entries,
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

#[cfg(test)]
mod testing {
    use kanban_domain::{
        AcceptanceCriterion, Plan, PlanId, Priority, Project, ProjectCounters, ProjectId,
        ProjectRegistration, ProjectState, Spec, SpecContent, SpecContentState, SpecExecutionState,
        SpecId, SpecNumber, SpecVersion, Ticket, TicketBody, TicketId, TicketNumber, TicketState,
        UserStoryRef,
    };

    use super::{ExportArtifact, render_project_export};

    fn spec_number(value: u64) -> SpecNumber {
        SpecNumber::new(value).expect("the fixture number is positive")
    }

    fn story(spec: u64, ordinal: u64) -> UserStoryRef {
        UserStoryRef::new(spec_number(spec), ordinal).expect("the fixture ordinal is positive")
    }

    fn criterion(outcome: &str, stories: Vec<UserStoryRef>) -> AcceptanceCriterion {
        AcceptanceCriterion::new(outcome, stories).expect("the fixture criterion links")
    }

    fn content(name: &str) -> SpecContent {
        SpecContent::new(
            name,
            "Lanes, workspaces, and Git",
            "Agents work in real working copies.",
            "Registered, observed Workspaces with health.",
            "KAN-S6-US5: As an operator, I want deterministic exports.",
            "Exports render deterministically and write atomically.",
            "Export tests prove byte-stable rendering.",
            "Automatic Workspace cleanup.",
            "The fleet's branch-clone tooling is the only sanctioned clone mechanism.",
        )
        .expect("the fixture content validates")
    }

    pub(super) fn project() -> Project {
        let registration = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            None,
        )
        .expect("the fixture registration validates");
        Project::restore(
            ProjectId::new(1),
            registration,
            ProjectState::Active,
            ProjectCounters::restore(2, 6, 18),
            9,
        )
    }

    /// Plan 1: membership 1, 3, 2 with edges 1 → 2 and 3 → 2, frozen
    /// once at activation. Plan 2 exists only to prove per-number
    /// ordering, so it stays an empty draft minted later.
    pub(super) fn plans() -> Vec<Plan> {
        let mut first = Plan::new(PlanId::new(1), ProjectId::new(1), 1);
        for number in [1, 3, 2] {
            first
                .add_spec(spec_number(number))
                .expect("the fixture membership lands");
        }
        first
            .add_edge(spec_number(1), spec_number(2))
            .expect("the fixture edge lands");
        first
            .add_edge(spec_number(3), spec_number(2))
            .expect("the fixture edge lands");
        first.activate().expect("the fixture freezes");
        let second = Plan::new(PlanId::new(2), ProjectId::new(1), 2);
        vec![second, first]
    }

    /// Spec 6 with an approved version 1 and a draft version 2 minted
    /// after it, executing actively inside Plan 1. Spec 1 exists only
    /// to prove per-number ordering.
    pub(super) fn specs() -> Vec<Spec> {
        let mut sixth = Spec::new(
            SpecId::new(6),
            ProjectId::new(1),
            spec_number(6),
            content("Lanes, workspaces, and Git"),
        )
        .expect("the fixture content validates");
        sixth.approve_version().expect("the first version approves");
        sixth
            .update_content(content("Lanes, workspaces, and exports"))
            .expect("the material change mints");
        sixth
            .assign_to_plan(PlanId::new(1))
            .expect("the fixture joins its Plan");
        sixth
            .transition_execution(SpecExecutionState::Ready)
            .expect("planned work becomes ready");
        sixth
            .transition_execution(SpecExecutionState::Active)
            .expect("ready work activates");
        let first = Spec::restore(
            SpecId::new(1),
            ProjectId::new(1),
            spec_number(1),
            vec![SpecVersion::new(
                1,
                SpecContentState::Approved,
                content("Registration"),
            )],
            SpecExecutionState::Complete,
            Some(PlanId::new(1)),
            4,
        );
        vec![sixth, first]
    }

    /// Ticket 17 delivers Spec 6; Ticket 18 is an attached Bug; Ticket
    /// 19 stands alone as a Task.
    pub(super) fn tickets() -> Vec<Ticket> {
        let implementation = TicketBody::implementation(
            Some(SpecId::new(6)),
            spec_number(6),
            "Exports render planning state end to end",
            vec![
                criterion(
                    "Identical planning state renders identical export bytes.",
                    vec![story(6, 5)],
                ),
                criterion(
                    "Drift between exports and state is reported on demand.",
                    vec![story(6, 5), story(6, 6)],
                ),
            ],
        )
        .expect("the fixture body validates");
        let bug = TicketBody::bug("Drift report misses a deleted file", Some(SpecId::new(6)))
            .expect("the fixture body validates");
        let task =
            TicketBody::task("Rotate the fleet board", None).expect("the fixture body validates");
        vec![
            Ticket::restore(
                TicketId::new(19),
                ProjectId::new(1),
                TicketNumber::new(19).expect("the fixture number is positive"),
                Priority::Low,
                TicketState::Parked,
                task,
                3,
            ),
            Ticket::restore(
                TicketId::new(18),
                ProjectId::new(1),
                TicketNumber::new(18).expect("the fixture number is positive"),
                Priority::Urgent,
                TicketState::Draft,
                bug,
                1,
            ),
            Ticket::restore(
                TicketId::new(17),
                ProjectId::new(1),
                TicketNumber::new(17).expect("the fixture number is positive"),
                Priority::Normal,
                TicketState::Active,
                implementation,
                5,
            ),
        ]
    }

    /// The full fixture state, rendered ready.
    pub(super) fn rendered() -> Vec<ExportArtifact> {
        render_project_export(&project(), &plans(), &specs(), &tickets())
    }
}

#[cfg(test)]
mod exports_render {
    use super::render_project_export;
    use super::testing::{project, rendered};

    #[test]
    fn rendering_twice_from_identical_state_is_byte_stable() {
        let first = rendered();
        let second = rendered();

        assert!(!first.is_empty(), "the fixture state renders artifacts");
        assert_eq!(
            first, second,
            "identical state must render byte-identical artifacts"
        );
        let joined = |artifacts: &Vec<super::ExportArtifact>| -> String {
            artifacts
                .iter()
                .map(|artifact| artifact.markdown.clone())
                .collect()
        };
        assert_eq!(
            joined(&first),
            joined(&second),
            "the rendered bytes themselves must be identical"
        );
    }

    #[test]
    fn artifacts_use_stable_ids_and_per_kind_ordering() {
        let rendered = rendered();

        let paths: Vec<_> = rendered
            .iter()
            .map(|artifact| artifact.relative_path.as_str())
            .collect();

        assert_eq!(
            paths,
            vec![
                "plans/CORE-P1.md",
                "plans/CORE-P2.md",
                "specs/CORE-S1.md",
                "specs/CORE-S6.md",
                "tickets/CORE-T17.md",
                "tickets/CORE-T18.md",
                "tickets/CORE-T19.md",
            ],
            "each kind orders by its minted number whatever the storage ids"
        );
    }

    #[test]
    fn plan_documents_render_state_order_and_frozen_versions() {
        let rendered = rendered();

        let plan = &rendered[0];
        assert!(plan.markdown.starts_with("# CORE-P1 — Plan\n"));
        for line in [
            "- State: active",
            "## Display order",
            "1. CORE-S1",
            "2. CORE-S3",
            "3. CORE-S2",
            "## Dependencies",
            "- CORE-S1 → CORE-S2",
            "- CORE-S3 → CORE-S2",
            "## Frozen versions",
            "### Version 1",
        ] {
            assert!(
                plan.markdown.contains(line),
                "the plan document carries `{line}`"
            );
        }
        let draft = &rendered[1];
        assert!(draft.markdown.contains("- State: draft"));
        assert!(
            draft.markdown.contains("## Frozen versions\n\n(none)\n"),
            "a Plan with nothing frozen says so"
        );
    }

    #[test]
    fn spec_documents_render_every_version_and_section() {
        let rendered = rendered();

        let spec = &rendered[3];
        assert!(
            spec.markdown
                .starts_with("# CORE-S6 — Lanes, workspaces, and exports\n")
        );
        for line in [
            "- Execution: active",
            "- Plan: CORE-P1",
            "## Version 1 — approved",
            "## Version 2 — draft",
            "### Name",
            "### Short description",
            "### Problem statement",
            "### Solution",
            "### User stories",
            "### Implementation decisions",
            "### Testing decisions",
            "### Out of scope",
            "### Further notes",
        ] {
            assert!(
                spec.markdown.contains(line),
                "the spec document carries `{line}`"
            );
        }
        assert!(
            spec.markdown
                .contains("KAN-S6-US5: As an operator, I want deterministic exports."),
            "the PRD text renders verbatim"
        );
        let complete = &rendered[2];
        assert!(complete.markdown.contains("- Execution: complete"));
    }

    #[test]
    fn ticket_documents_render_the_kind_schema() {
        let rendered = rendered();

        let implementation = &rendered[4];
        assert!(
            implementation
                .markdown
                .starts_with("# CORE-T17 — Implementation\n")
        );
        for line in [
            "- Priority: normal",
            "- State: active",
            "- Spec: CORE-S6",
            "## Slice",
            "Exports render planning state end to end",
            "## Acceptance criteria",
            "1. Identical planning state renders identical export bytes. [CORE-S6-US5]",
            "2. Drift between exports and state is reported on demand. [CORE-S6-US5, CORE-S6-US6]",
        ] {
            assert!(
                implementation.markdown.contains(line),
                "the implementation document carries `{line}`"
            );
        }

        let bug = &rendered[5];
        assert!(bug.markdown.starts_with("# CORE-T18 — Bug\n"));
        assert!(bug.markdown.contains("- Spec: CORE-S6"));
        assert!(
            bug.markdown
                .contains("## Title\n\nDrift report misses a deleted file")
        );

        let task = &rendered[6];
        assert!(task.markdown.starts_with("# CORE-T19 — Task\n"));
        assert!(
            task.markdown.contains("- Priority: low"),
            "the task document carries its priority"
        );
        assert!(
            !task.markdown.contains("- Spec:"),
            "a standalone Task carries no Spec line"
        );
    }

    #[test]
    fn an_empty_project_renders_no_artifacts() {
        let rendered = render_project_export(&project(), &[], &[], &[]);

        assert!(
            rendered.is_empty(),
            "a Project with nothing planned exports nothing"
        );
    }
}

#[cfg(test)]
mod export_testing {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use kanban_dto::ApiError;

    use super::ExportFiles;

    /// An in-memory file port: one byte map plus the writes it was
    /// asked to perform, in order.
    #[derive(Default)]
    pub(super) struct MemoryFiles {
        state: Mutex<MemoryFileState>,
    }

    #[derive(Default)]
    struct MemoryFileState {
        files: BTreeMap<PathBuf, Vec<u8>>,
        writes: Vec<PathBuf>,
    }

    impl MemoryFiles {
        /// The paths the port was asked to replace, in order.
        pub(super) fn writes(&self) -> Vec<PathBuf> {
            self.state
                .lock()
                .expect("the memory files lock is sound")
                .writes
                .clone()
        }

        /// The stored bytes at `path`.
        pub(super) fn bytes(&self, path: &Path) -> Option<Vec<u8>> {
            self.state
                .lock()
                .expect("the memory files lock is sound")
                .files
                .get(path)
                .cloned()
        }

        /// Overwrite the stored bytes at `path`, standing in for a
        /// hand edit between exports.
        pub(super) fn overwrite(&self, path: &Path, contents: &[u8]) {
            self.state
                .lock()
                .expect("the memory files lock is sound")
                .files
                .insert(path.to_owned(), contents.to_vec());
        }

        /// Remove the file at `path`, standing in for a deletion
        /// between exports.
        pub(super) fn remove(&self, path: &Path) {
            self.state
                .lock()
                .expect("the memory files lock is sound")
                .files
                .remove(path);
        }
    }

    impl ExportFiles for MemoryFiles {
        fn replace(&self, path: &Path, contents: &[u8]) -> Result<(), ApiError> {
            let mut state = self.state.lock().expect("the memory files lock is sound");
            state.files.insert(path.to_owned(), contents.to_vec());
            state.writes.push(path.to_owned());
            Ok(())
        }

        fn current(&self, path: &Path) -> Result<Option<Vec<u8>>, ApiError> {
            Ok(self
                .state
                .lock()
                .expect("the memory files lock is sound")
                .files
                .get(path)
                .cloned())
        }

        fn walk(&self, directory: &Path) -> Result<Vec<PathBuf>, ApiError> {
            Ok(self
                .state
                .lock()
                .expect("the memory files lock is sound")
                .files
                .keys()
                .filter(|path| path.starts_with(directory))
                .cloned()
                .collect())
        }
    }

    /// A core with the Plan, Spec, Ticket, and export operations
    /// wired to in-memory stores over one active Project holding a
    /// frozen Plan, an approved Spec, an attached Implementation
    /// Ticket, and a standalone Task.
    pub(super) struct ExportHarness {
        pub(super) core: crate::dispatch::Core,
        pub(super) files: Arc<MemoryFiles>,
    }

    /// The export directory every harness render uses.
    pub(super) const DIRECTORY: &str = "docs/planning";

    /// The Seed Workspace path the harness Project anchors to.
    pub(super) fn seed_root() -> PathBuf {
        PathBuf::from("/workspaces/kanban.seed").join(DIRECTORY)
    }

    pub(super) fn harness() -> ExportHarness {
        use crate::catalog::exposed_operations;
        use crate::events::NoopEventSink;
        use crate::mutation::MemoryIdempotencyStore;
        use crate::plan::testing::{MemoryPlans, MemoryProjects};
        use crate::spec::testing::MemorySpecs;

        let projects = Arc::new(MemoryProjects::default());
        projects.seed(crate::plan::testing::active_project(
            1,
            "CORE",
            kanban_domain::ProjectCounters::zeroed(),
        ));
        let plans = Arc::new(MemoryPlans::sharing(projects.clone()));
        let specs = Arc::new(MemorySpecs::sharing(projects.clone()));
        let tickets = Arc::new(crate::ticket::testing::MemoryTickets::sharing(
            projects.clone(),
        ));
        let files = Arc::new(MemoryFiles::default());
        let mut core = crate::dispatch::Core::new(
            exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            Arc::new(NoopEventSink),
        );
        core.register_plans(plans.clone(), projects.clone(), specs.clone())
            .expect("the plan operations register");
        core.register_specs(specs.clone(), projects.clone(), plans.clone())
            .expect("the spec operations register");
        core.register_tickets(tickets.clone(), projects.clone(), specs.clone())
            .expect("the ticket operations register");
        core.register_exports(
            plans.clone(),
            specs.clone(),
            tickets.clone(),
            projects.clone(),
            files.clone(),
        )
        .expect("the export operations register");
        ExportHarness { core, files }
    }

    /// Author the fixture planning state through the served core: one
    /// approved Spec inside one frozen Plan, one Implementation Ticket
    /// delivering it, and one standalone Task.
    pub(super) fn author_planning_state(harness: &ExportHarness) {
        let spec = harness
            .core
            .command(
                "spec.create",
                &serde_json::json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "spec-1" },
                    "project_id": 1,
                    "content": {
                        "name": "Lanes, workspaces, and Git",
                        "short_description": "Registered, observed Workspaces.",
                        "problem_statement": "Agents work in real working copies.",
                        "solution": "Guarded clone commands and deterministic exports.",
                        "user_stories": "KAN-S6-US5",
                        "implementation_decisions": "Exports write atomically.",
                        "testing_decisions": "Export tests prove byte stability.",
                        "out_of_scope": "Automatic Workspace cleanup.",
                        "further_notes": "None",
                    },
                }),
            )
            .expect("the Spec authors");
        let spec_id = spec["id"].as_u64().expect("the identity is a number");

        let plan = harness
            .core
            .command(
                "plan.create",
                &serde_json::json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "plan-1" },
                    "project_id": 1,
                }),
            )
            .expect("the Plan creates");
        let plan_id = plan["id"].as_u64().expect("the identity is a number");
        let mut version = plan["version"].as_u64().expect("the version is a number");
        let response = harness
            .core
            .command(
                "plan.spec.add",
                &serde_json::json!({
                    "mutation": { "optimistic_version": version, "idempotency_key": "plan-add-1" },
                    "plan_id": plan_id,
                    "spec_number": 1,
                }),
            )
            .expect("the Spec joins the Plan");
        version = response["version"]
            .as_u64()
            .expect("the version is a number");
        harness
            .core
            .command(
                "plan.activate",
                &serde_json::json!({
                    "mutation": { "optimistic_version": version, "idempotency_key": "plan-activate" },
                    "plan_id": plan_id,
                }),
            )
            .expect("the Plan freezes");

        let joined = harness
            .core
            .command(
                "spec.plan.join",
                &serde_json::json!({
                    "mutation": {
                        "optimistic_version": spec["version"].as_u64().expect("the version is a number"),
                        "idempotency_key": "spec-join",
                    },
                    "spec_id": spec_id,
                    "plan_id": plan_id,
                }),
            )
            .expect("the Spec joins its Plan");
        let version = joined["version"].as_u64().expect("the version is a number");
        harness
            .core
            .command(
                "spec.version.approve",
                &serde_json::json!({
                    "mutation": { "optimistic_version": version, "idempotency_key": "spec-approve" },
                    "spec_id": spec_id,
                }),
            )
            .expect("the content approves");

        harness
            .core
            .command(
                "ticket.create",
                &serde_json::json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "ticket-1" },
                    "project_id": 1,
                    "kind": "implementation",
                    "priority": "normal",
                    "spec_id": spec_id,
                    "slice": "Exports render planning state end to end",
                    "criteria": [
                        {
                            "outcome": "Identical state renders identical bytes.",
                            "stories": ["CORE-S1-US5"],
                        },
                    ],
                }),
            )
            .expect("the Implementation Ticket creates");
        harness
            .core
            .command(
                "ticket.create",
                &serde_json::json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "ticket-2" },
                    "project_id": 1,
                    "kind": "task",
                    "priority": "low",
                    "title": "Rotate the fleet board",
                }),
            )
            .expect("the Task Ticket creates");
    }

    /// A render request against the harness Project.
    pub(super) fn render(key: &str) -> serde_json::Value {
        serde_json::json!({
            "mutation": { "optimistic_version": 0, "idempotency_key": key },
            "project_id": 1,
            "directory": DIRECTORY,
        })
    }

    /// A drift query against the harness Project.
    pub(super) fn drift() -> serde_json::Value {
        serde_json::json!({ "project_id": 1, "directory": DIRECTORY })
    }
}

#[cfg(test)]
mod exports_command {
    use serde_json::json;

    use super::export_testing::{ExportHarness, author_planning_state, harness, render, seed_root};
    use kanban_dto::ErrorCode;

    fn authored() -> ExportHarness {
        let harness = harness();
        author_planning_state(&harness);
        harness
    }

    #[test]
    fn rendering_writes_the_configured_directory_within_the_seed() {
        let harness = authored();

        let response = harness
            .core
            .command("export.render", &render("render-1"))
            .expect("the render applies");

        assert_eq!(response["project_id"], json!(1));
        assert_eq!(response["directory"], json!("docs/planning"));
        assert_eq!(
            response["files"],
            json!([
                "plans/CORE-P1.md",
                "specs/CORE-S1.md",
                "tickets/CORE-T1.md",
                "tickets/CORE-T2.md",
            ])
        );
        let writes = harness.files.writes();
        assert_eq!(writes.len(), 4, "every artifact lands exactly once");
        for path in writes {
            assert!(
                path.starts_with(seed_root()),
                "the write `{}` lands inside the configured Seed directory",
                path.display()
            );
        }
        let plan = harness
            .files
            .bytes(&seed_root().join("plans/CORE-P1.md"))
            .expect("the plan document is written");
        let text = String::from_utf8(plan).expect("the document is UTF-8");
        assert!(text.starts_with("# CORE-P1 — Plan\n"));
    }

    #[test]
    fn rendering_replaces_the_previous_export_rather_than_growing_it() {
        let harness = authored();
        harness
            .core
            .command("export.render", &render("render-1"))
            .expect("the first render applies");
        let ticket = seed_root().join("tickets/CORE-T2.md");
        harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "ticket-3" },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "urgent",
                    "title": "A late Bug",
                }),
            )
            .expect("the Bug creates");

        let response = harness
            .core
            .command("export.render", &render("render-2"))
            .expect("the second render applies");

        assert_eq!(
            response["files"]
                .as_array()
                .expect("the files are a list")
                .len(),
            5,
            "the new Ticket joins the export"
        );
        assert!(
            harness.files.bytes(&ticket).is_some(),
            "the standing export stays"
        );
        let writes = harness.files.writes();
        assert_eq!(
            writes.len(),
            9,
            "the port replaced files; it never removed or duplicated"
        );
    }

    #[test]
    fn rendering_refuses_a_directory_that_leaves_the_seed() {
        let harness = authored();

        for directory in [
            json!("../escape"),
            json!("/absolute/docs"),
            json!("docs/../escape"),
            json!("docs/./planning"),
            json!("  "),
            json!("docs\\planning"),
        ] {
            let error = harness
                .core
                .command(
                    "export.render",
                    &json!({
                        "mutation": { "optimistic_version": 0, "idempotency_key": "escape" },
                        "project_id": 1,
                        "directory": directory,
                    }),
                )
                .expect_err("a directory leaving the Seed is refused");

            assert_eq!(
                error.code,
                ErrorCode::InvalidRequest,
                "`{directory}` must be refused"
            );
        }
        assert!(
            harness.files.writes().is_empty(),
            "a refused render writes nothing"
        );
    }

    #[test]
    fn rendering_an_unknown_project_is_not_found() {
        let harness = harness();

        let error = harness
            .core
            .command(
                "export.render",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "unknown" },
                    "project_id": 9,
                    "directory": "docs/planning",
                }),
            )
            .expect_err("an unknown Project is refused");

        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn rendering_rejects_unknown_fields() {
        let harness = harness();

        let error = harness
            .core
            .command(
                "export.render",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "surprise" },
                    "project_id": 1,
                    "directory": "docs/planning",
                    "commit": true,
                }),
            )
            .expect_err("unknown fields are rejected");

        assert_eq!(error.code, ErrorCode::UnknownField);
    }

    #[test]
    fn the_export_surface_offers_no_commit_push_or_stage_operation() {
        let destructive = ["export.commit", "export.push", "export.stage", "export.git"];
        for operation in crate::catalog::exposed_operations() {
            assert!(
                !destructive.contains(&operation.name),
                "`{}` must not exist: exports never touch git",
                operation.name
            );
        }
    }
}

#[cfg(test)]
mod export_drift {
    use serde_json::json;

    use super::export_testing::{author_planning_state, drift, harness, render, seed_root};

    fn rendered_and_clean(harness: &super::export_testing::ExportHarness) {
        harness
            .core
            .command("export.render", &render("render-1"))
            .expect("the fixture render applies");
        let report = harness
            .core
            .query("export.drift", &drift())
            .expect("the fixture drift query serves");
        assert_eq!(
            report["in_drift"],
            json!(false),
            "the fixture export starts clean: {report}"
        );
        assert_eq!(report["entries"], json!([]));
    }

    #[test]
    fn a_fresh_export_reports_no_drift() {
        let harness = harness();
        author_planning_state(&harness);

        rendered_and_clean(&harness);

        let again = harness
            .core
            .query("export.drift", &drift())
            .expect("the second drift query serves");

        assert_eq!(
            again["in_drift"],
            json!(false),
            "repeated checks stay clean"
        );
    }

    #[test]
    fn a_hand_edited_file_reports_differs() {
        let harness = harness();
        author_planning_state(&harness);
        rendered_and_clean(&harness);

        let plan = seed_root().join("plans/CORE-P1.md");
        harness.files.overwrite(&plan, b"# hand edit\n");

        let report = harness
            .core
            .query("export.drift", &drift())
            .expect("the drift query serves");

        assert_eq!(report["in_drift"], json!(true));
        assert_eq!(
            report["entries"],
            json!([{ "path": "plans/CORE-P1.md", "status": "differs" }])
        );
    }

    #[test]
    fn a_deleted_file_reports_missing() {
        let harness = harness();
        author_planning_state(&harness);
        rendered_and_clean(&harness);

        harness.files.remove(&seed_root().join("specs/CORE-S1.md"));

        let report = harness
            .core
            .query("export.drift", &drift())
            .expect("the drift query serves");

        assert_eq!(
            report["entries"],
            json!([{ "path": "specs/CORE-S1.md", "status": "missing" }])
        );
    }

    #[test]
    fn an_unexpected_file_reports_unmatched() {
        let harness = harness();
        author_planning_state(&harness);
        rendered_and_clean(&harness);

        harness
            .files
            .overwrite(&seed_root().join("plans/CORE-P9.md"), b"# a stale plan\n");

        let report = harness
            .core
            .query("export.drift", &drift())
            .expect("the drift query serves");

        assert_eq!(
            report["entries"],
            json!([{ "path": "plans/CORE-P9.md", "status": "unmatched" }])
        );
    }

    #[test]
    fn state_moving_on_leaves_drift_until_rerendered() {
        let harness = harness();
        author_planning_state(&harness);
        rendered_and_clean(&harness);

        harness
            .core
            .command(
                "ticket.create",
                &json!({
                    "mutation": { "optimistic_version": 0, "idempotency_key": "ticket-3" },
                    "project_id": 1,
                    "kind": "bug",
                    "priority": "urgent",
                    "title": "A late Bug",
                }),
            )
            .expect("the Bug creates");

        let drifted = harness
            .core
            .query("export.drift", &drift())
            .expect("the drift query serves");

        assert_eq!(drifted["in_drift"], json!(true));
        assert_eq!(
            drifted["entries"],
            json!([{ "path": "tickets/CORE-T3.md", "status": "missing" }]),
            "the new Ticket has no export yet"
        );

        harness
            .core
            .command("export.render", &render("render-2"))
            .expect("the rerender applies");

        let clean = harness
            .core
            .query("export.drift", &drift())
            .expect("the drift query serves");

        assert_eq!(clean["in_drift"], json!(false), "a rerender clears drift");
    }

    #[test]
    fn drift_refuses_a_directory_that_leaves_the_seed() {
        let harness = harness();
        author_planning_state(&harness);

        let error = harness
            .core
            .query(
                "export.drift",
                &json!({ "project_id": 1, "directory": "../escape" }),
            )
            .expect_err("a directory leaving the Seed is refused");

        assert_eq!(error.code, kanban_dto::ErrorCode::InvalidRequest);
    }

    #[test]
    fn drift_for_an_unknown_project_is_not_found() {
        let harness = harness();

        let error = harness
            .core
            .query(
                "export.drift",
                &json!({ "project_id": 9, "directory": "docs/planning" }),
            )
            .expect_err("an unknown Project is refused");

        assert_eq!(error.code, kanban_dto::ErrorCode::NotFound);
    }
}
