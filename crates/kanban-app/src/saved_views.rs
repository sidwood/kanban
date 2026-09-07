//! Saved View commands and queries (DR-BP-05, DR-BP-06): the named
//! operator perspectives that own the board's presentation decisions
//! — the ten-axis filter, the expanded groups, the hidden columns,
//! the mode, the Done placement, and the sorting key. The entity and
//! its vocabularies live in `kanban-domain`; these operations only
//! read and edit what it owns, replacing the whole owned set at once
//! so writing one property never drops another. The defaults are
//! guaranteed, not assumed: one global default view and one default
//! per registered Project exist from the first read, materialised
//! then and regenerated whenever a scope loses its own, so no scope
//! is ever without its default perspective.

use std::sync::Arc;

use kanban_domain::{
    AttentionState as DomainAttention, BoardFilter as DomainFilter, BoardGroup as DomainGroup,
    DonePlacement as DomainDone, Priority as DomainPriority, ProfileName, ProjectId, SavedView,
    SavedViewError, SavedViewId, TicketKind as DomainKind, TicketState as DomainState,
    ViewMode as DomainMode, ViewName, ViewScope as DomainScope, ViewSorting as DomainSorting,
};
use kanban_dto::{
    ApiError, AttentionState, BoardFilter, BoardGroup, DonePlacement, SavedViewRecord, TicketKind,
    TicketPriority, TicketState, ViewCreateRequest, ViewListQuery, ViewListResponse, ViewMode,
    ViewRemoveRequest, ViewRemovedRecord, ViewRenameRequest, ViewScope, ViewSorting,
    ViewUpdateRequest,
};
use serde_json::Value;

use crate::dispatch::{Core, QueryHandler, RegistrationError};
use crate::mutation::{CommandHandler, ParsedCommand, parse_payload};
use crate::project::ProjectStore;
use crate::ticket::priority_of;

/// The storage port the saved view operations call through.
/// Implementations mint identities on insert and guard updates by
/// version; every collection rule — one default per scope, names
/// unique within a scope — is judged here in the application layer
/// and backed by the store's own constraints.
pub trait SavedViewStore: Send + Sync {
    /// Every stored view, every scope together.
    fn list(&self) -> Result<Vec<SavedView>, ApiError>;
    /// The view `id` names, when it stands.
    fn find(&self, id: SavedViewId) -> Result<Option<SavedView>, ApiError>;
    /// Land a new view, minting its identity; the identity the draft
    /// carries is ignored.
    fn insert(&self, draft: &SavedView) -> Result<SavedView, ApiError>;
    /// Persist a mutated view, guarding on the version it replaced.
    fn save(&self, view: &SavedView) -> Result<(), ApiError>;
    /// Remove the view `id` names.
    fn remove(&self, id: SavedViewId) -> Result<(), ApiError>;
}

/// The context every saved view operation shares: the views and the
/// Projects whose scopes defaults generate for.
#[derive(Clone)]
struct ViewContext {
    views: Arc<dyn SavedViewStore>,
    projects: Arc<dyn ProjectStore>,
}

impl Core {
    /// Register the saved view operations against `views`, resolving
    /// Project scopes through `projects`.
    pub fn register_saved_views(
        &mut self,
        views: Arc<dyn SavedViewStore>,
        projects: Arc<dyn ProjectStore>,
    ) -> Result<(), RegistrationError> {
        let context = ViewContext { views, projects };
        self.register_query("view.list", Arc::new(ListViews(context.clone())))?;
        self.register_command("view.create", Arc::new(CreateView(context.clone())))?;
        self.register_command("view.update", Arc::new(UpdateView(context.clone())))?;
        self.register_command("view.rename", Arc::new(RenameView(context.clone())))?;
        self.register_command("view.remove", Arc::new(RemoveView(context)))?;
        Ok(())
    }
}

impl ViewContext {
    /// Materialise the missing defaults (DR-BP-06): the global
    /// default and one default per registered Project. A default
    /// already standing stays exactly as the operator left it.
    fn ensure_defaults(&self) -> Result<(), ApiError> {
        let standing = self.views.list()?;
        let mut missing = vec![DomainScope::Global];
        missing.extend(
            self.projects
                .list()?
                .iter()
                .map(|project| DomainScope::Project(project.id())),
        );
        for scope in missing {
            let has_default = standing
                .iter()
                .any(|view| view.scope() == scope && view.is_default());
            if !has_default {
                self.views
                    .insert(&SavedView::generate(SavedViewId::new(0), scope))?;
            }
        }
        Ok(())
    }

    /// The stored view `id` names, or the stable not-found refusal.
    fn view(&self, id: u64) -> Result<SavedView, ApiError> {
        self.views
            .find(SavedViewId::new(id))?
            .ok_or_else(|| ApiError::not_found(&format!("view {id}")))
    }

    /// The domain scope a wire scope names, resolving its Project
    /// through the register.
    fn scope(&self, scope: &ViewScope) -> Result<DomainScope, ApiError> {
        match scope {
            ViewScope::Global => Ok(DomainScope::Global),
            ViewScope::Project(project_id) => {
                self.projects
                    .find(ProjectId::new(*project_id))?
                    .ok_or_else(|| ApiError::not_found(&format!("project {project_id}")))?;
                Ok(DomainScope::Project(ProjectId::new(*project_id)))
            }
        }
    }

    /// Refuse a name another view of the same scope already carries;
    /// the generated default's name is taken like any other's.
    fn refuse_taken_name(
        &self,
        scope: DomainScope,
        name: &ViewName,
        except: u64,
    ) -> Result<(), ApiError> {
        let taken = self.views.list()?.iter().any(|view| {
            view.id().value() != except && view.scope() == scope && view.name() == name
        });
        if taken {
            return Err(already_taken_view_name_error(name.as_str()));
        }
        Ok(())
    }

    /// The list every view of every scope reads, in its fixed order:
    /// the global scope first, then the Projects by identity, and
    /// within a scope the default first, then names.
    fn listed(&self) -> Result<Vec<SavedView>, ApiError> {
        self.ensure_defaults()?;
        let mut views = self.views.list()?;
        views.sort_by(|a, b| {
            scope_rank(&a.scope())
                .cmp(&scope_rank(&b.scope()))
                .then_with(|| b.is_default().cmp(&a.is_default()))
                .then_with(|| a.name().as_str().cmp(b.name().as_str()))
                .then_with(|| a.id().value().cmp(&b.id().value()))
        });
        Ok(views)
    }
}

/// Where a scope sits in the list: the global scope before every
/// Project, Projects by identity.
fn scope_rank(scope: &DomainScope) -> (u8, u64) {
    match scope {
        DomainScope::Global => (0, 0),
        DomainScope::Project(project) => (1, project.value()),
    }
}

/// Serves `view.list`.
struct ListViews(ViewContext);

impl QueryHandler for ListViews {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        parse_payload::<ViewListQuery>(payload)?;
        let response = ViewListResponse {
            views: self.0.listed()?.iter().map(record_of).collect(),
        };
        serde_json::to_value(response).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Serves `view.create`.
struct CreateView(ViewContext);

impl CommandHandler for CreateView {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ViewCreateRequest>(payload)?;
        ParsedCommand::lift("view", payload)
    }

    fn current_version(&self, _command: &ParsedCommand) -> Result<u64, ApiError> {
        // A fresh view is created at version 0.
        Ok(0)
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn crate::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: ViewCreateRequest = parse_payload(&command.payload)?;
        // Defaults first, so the default name is taken before the
        // new view's name is judged against it.
        self.0.ensure_defaults()?;
        let scope = self.0.scope(&request.scope)?;
        let name = ViewName::new(&request.name).map_err(refuse)?;
        self.0.refuse_taken_name(scope, &name, 0)?;
        let draft = SavedView::create(
            name,
            scope,
            domain_filter_of(&request.filter)?,
            &groups_of(&request.expanded_groups),
            &groups_of(&request.hidden_columns),
            mode_of(request.mode),
            done_of(request.done_placement),
            sorting_of(request.sorting),
        )
        .map_err(refuse)?;
        let stored = self.0.views.insert(&draft)?;
        encode(&stored)
    }
}

/// Serves `view.update`.
struct UpdateView(ViewContext);

impl CommandHandler for UpdateView {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ViewUpdateRequest>(payload)?;
        ParsedCommand::lift("view", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: ViewUpdateRequest = parse_payload(&command.payload)?;
        Ok(self.0.view(request.view_id)?.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn crate::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: ViewUpdateRequest = parse_payload(&command.payload)?;
        let mut view = self.0.view(request.view_id)?;
        view.adopt(
            domain_filter_of(&request.filter)?,
            &groups_of(&request.expanded_groups),
            &groups_of(&request.hidden_columns),
            mode_of(request.mode),
            done_of(request.done_placement),
            sorting_of(request.sorting),
        )
        .map_err(refuse)?;
        self.0.views.save(&view)?;
        encode(&view)
    }
}

/// Serves `view.rename`.
struct RenameView(ViewContext);

impl CommandHandler for RenameView {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ViewRenameRequest>(payload)?;
        ParsedCommand::lift("view", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: ViewRenameRequest = parse_payload(&command.payload)?;
        Ok(self.0.view(request.view_id)?.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn crate::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: ViewRenameRequest = parse_payload(&command.payload)?;
        // Defaults first, so renaming onto the default name is
        // refused against the default that stands.
        self.0.ensure_defaults()?;
        let mut view = self.0.view(request.view_id)?;
        let name = ViewName::new(&request.name).map_err(refuse)?;
        self.0
            .refuse_taken_name(view.scope(), &name, view.id().value())?;
        view.rename(name);
        self.0.views.save(&view)?;
        encode(&view)
    }
}

/// Serves `view.remove`.
struct RemoveView(ViewContext);

impl CommandHandler for RemoveView {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        parse_payload::<ViewRemoveRequest>(payload)?;
        ParsedCommand::lift("view", payload)
    }

    fn current_version(&self, command: &ParsedCommand) -> Result<u64, ApiError> {
        let request: ViewRemoveRequest = parse_payload(&command.payload)?;
        Ok(self.0.view(request.view_id)?.version())
    }

    fn apply(
        &self,
        command: &ParsedCommand,
        _effects: &dyn crate::CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: ViewRemoveRequest = parse_payload(&command.payload)?;
        let view = self.0.view(request.view_id)?;
        self.0.views.remove(view.id())?;
        let record = ViewRemovedRecord {
            view_id: view.id().value(),
        };
        serde_json::to_value(record).map_err(|error| ApiError::internal(&error.to_string()))
    }
}

/// Report a refused domain rule as the stable invalid-request code.
fn refuse(error: SavedViewError) -> ApiError {
    ApiError::invalid_request(&error.to_string())
}

/// The refusal a store's own uniqueness constraint answers with, so
/// the application's judgement and the constraint it backs agree.
pub fn already_taken_view_name_error(name: &str) -> ApiError {
    ApiError::invalid_request(&format!(
        "the view name `{name}` is already taken in its scope"
    ))
}

/// One record as the wire carries it.
fn encode(view: &SavedView) -> Result<Value, ApiError> {
    serde_json::to_value(record_of(view)).map_err(|error| ApiError::internal(&error.to_string()))
}

/// The wire record of one stored view.
fn record_of(view: &SavedView) -> SavedViewRecord {
    SavedViewRecord {
        id: view.id().value(),
        name: view.name().as_str().to_owned(),
        scope: scope_of(view.scope()),
        filter: filter_of(view.filter()),
        expanded_groups: view.expanded().iter().copied().map(group_of).collect(),
        hidden_columns: view.hidden().iter().copied().map(group_of).collect(),
        mode: wire_mode(view.mode()),
        done_placement: wire_done(view.done()),
        sorting: wire_sorting(view.sorting()),
        is_default: view.is_default(),
        version: view.version(),
    }
}

/// The wire scope of one domain scope.
fn scope_of(scope: DomainScope) -> ViewScope {
    match scope {
        DomainScope::Global => ViewScope::Global,
        DomainScope::Project(project) => ViewScope::Project(project.value()),
    }
}

/// The wire filter of one domain filter.
fn filter_of(filter: &DomainFilter) -> BoardFilter {
    BoardFilter {
        initiatives: filter.initiatives.iter().map(|id| id.value()).collect(),
        projects: filter.projects.iter().map(|id| id.value()).collect(),
        plans: filter.plans.iter().map(|id| id.value()).collect(),
        specs: filter.specs.iter().map(|id| id.value()).collect(),
        kinds: filter.kinds.iter().copied().map(kind_of).collect(),
        states: filter.states.iter().copied().map(state_of).collect(),
        priorities: filter
            .priorities
            .iter()
            .copied()
            .map(priority_record_of)
            .collect(),
        lanes: filter.lanes.iter().map(|id| id.value()).collect(),
        profiles: filter
            .profiles
            .iter()
            .map(|name| name.as_str().to_owned())
            .collect(),
        attention: filter.attention.iter().copied().map(attention_of).collect(),
    }
}

/// The domain filter of one wire filter; a profile name the domain
/// refuses is the invalid request it always was.
fn domain_filter_of(filter: &BoardFilter) -> Result<DomainFilter, ApiError> {
    Ok(DomainFilter {
        initiatives: filter
            .initiatives
            .iter()
            .map(|id| kanban_domain::InitiativeId::new(*id))
            .collect(),
        projects: filter
            .projects
            .iter()
            .map(|id| ProjectId::new(*id))
            .collect(),
        plans: filter
            .plans
            .iter()
            .map(|id| kanban_domain::PlanId::new(*id))
            .collect(),
        specs: filter
            .specs
            .iter()
            .map(|id| kanban_domain::SpecId::new(*id))
            .collect(),
        kinds: filter.kinds.iter().copied().map(domain_kind).collect(),
        states: filter.states.iter().copied().map(domain_state).collect(),
        priorities: filter.priorities.iter().copied().map(priority_of).collect(),
        lanes: filter
            .lanes
            .iter()
            .map(|id| kanban_domain::LaneId::new(*id))
            .collect(),
        profiles: filter
            .profiles
            .iter()
            .map(|name| ProfileName::new(name.as_str()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ApiError::invalid_request(&error.to_string()))?,
        attention: filter
            .attention
            .iter()
            .copied()
            .map(domain_attention)
            .collect(),
    })
}

/// The domain groups of one wire group list.
fn groups_of(groups: &[BoardGroup]) -> Vec<DomainGroup> {
    groups.iter().copied().map(domain_group).collect()
}

fn domain_group(group: BoardGroup) -> DomainGroup {
    match group {
        BoardGroup::Draft => DomainGroup::Draft,
        BoardGroup::Backlog => DomainGroup::Backlog,
        BoardGroup::Current => DomainGroup::Current,
        BoardGroup::Review => DomainGroup::Review,
        BoardGroup::Staged => DomainGroup::Staged,
        BoardGroup::Done => DomainGroup::Done,
    }
}

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

fn mode_of(mode: ViewMode) -> DomainMode {
    match mode {
        ViewMode::Board => DomainMode::Board,
        ViewMode::Register => DomainMode::Register,
    }
}

fn wire_mode(mode: DomainMode) -> ViewMode {
    match mode {
        DomainMode::Board => ViewMode::Board,
        DomainMode::Register => ViewMode::Register,
    }
}

fn done_of(done: DonePlacement) -> DomainDone {
    match done {
        DonePlacement::Column => DomainDone::Column,
        DonePlacement::Table => DomainDone::Table,
    }
}

fn wire_done(done: DomainDone) -> DonePlacement {
    match done {
        DomainDone::Column => DonePlacement::Column,
        DomainDone::Table => DonePlacement::Table,
    }
}

fn sorting_of(sorting: ViewSorting) -> DomainSorting {
    match sorting {
        ViewSorting::Priority => DomainSorting::Priority,
        ViewSorting::Readiness => DomainSorting::Readiness,
    }
}

fn wire_sorting(sorting: DomainSorting) -> ViewSorting {
    match sorting {
        DomainSorting::Priority => ViewSorting::Priority,
        DomainSorting::Readiness => ViewSorting::Readiness,
    }
}

fn domain_kind(kind: TicketKind) -> DomainKind {
    match kind {
        TicketKind::Implementation => DomainKind::Implementation,
        TicketKind::Bug => DomainKind::Bug,
        TicketKind::Task => DomainKind::Task,
    }
}

fn kind_of(kind: DomainKind) -> TicketKind {
    match kind {
        DomainKind::Implementation => TicketKind::Implementation,
        DomainKind::Bug => TicketKind::Bug,
        DomainKind::Task => TicketKind::Task,
    }
}

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

fn attention_of(class: DomainAttention) -> AttentionState {
    match class {
        DomainAttention::Blocker => AttentionState::Blocker,
        DomainAttention::MissingResult => AttentionState::MissingResult,
        DomainAttention::HumanDecision => AttentionState::HumanDecision,
        DomainAttention::ReviewRequest => AttentionState::ReviewRequest,
        DomainAttention::FailedSchedule => AttentionState::FailedSchedule,
        DomainAttention::InvalidApproval => AttentionState::InvalidApproval,
        DomainAttention::DisconnectedSession => AttentionState::DisconnectedSession,
        DomainAttention::StaleRun => AttentionState::StaleRun,
    }
}

fn priority_record_of(priority: DomainPriority) -> TicketPriority {
    match priority {
        DomainPriority::Urgent => TicketPriority::Urgent,
        DomainPriority::High => TicketPriority::High,
        DomainPriority::Normal => TicketPriority::Normal,
        DomainPriority::Low => TicketPriority::Low,
    }
}

#[cfg(test)]
mod saved_view_rules {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use kanban_dto::{ApiError, ErrorCode};

    use super::SavedViewStore;
    use crate::dispatch::Core;
    use crate::events::NoopEventSink;
    use crate::mutation::MemoryIdempotencyStore;
    use crate::plan::testing::{MemoryProjects, active_project};

    /// An in-memory saved view store: rows by id, identities minted
    /// on insert, updates guarded by the version they replaced.
    #[derive(Default)]
    struct MemoryViews {
        rows: Mutex<Vec<kanban_domain::SavedView>>,
    }

    impl MemoryViews {
        fn snapshot(&self) -> Vec<kanban_domain::SavedView> {
            self.rows.lock().expect("the view lock is sound").clone()
        }
    }

    impl SavedViewStore for MemoryViews {
        fn list(&self) -> Result<Vec<kanban_domain::SavedView>, ApiError> {
            Ok(self.snapshot())
        }

        fn find(
            &self,
            id: kanban_domain::SavedViewId,
        ) -> Result<Option<kanban_domain::SavedView>, ApiError> {
            Ok(self.snapshot().into_iter().find(|view| view.id() == id))
        }

        fn insert(
            &self,
            draft: &kanban_domain::SavedView,
        ) -> Result<kanban_domain::SavedView, ApiError> {
            let mut rows = self.rows.lock().expect("the view lock is sound");
            let next = rows.iter().map(|view| view.id().value()).max().unwrap_or(0) + 1;
            let stored = kanban_domain::SavedView::restore(
                kanban_domain::SavedViewId::new(next),
                draft.name().clone(),
                draft.scope(),
                draft.filter().clone(),
                draft.expanded().to_vec(),
                draft.hidden().to_vec(),
                draft.mode(),
                draft.done(),
                draft.sorting(),
                draft.is_default(),
                draft.version(),
            );
            rows.push(stored.clone());
            Ok(stored)
        }

        fn save(&self, view: &kanban_domain::SavedView) -> Result<(), ApiError> {
            let mut rows = self.rows.lock().expect("the view lock is sound");
            let Some(row) = rows.iter_mut().find(|row| row.id() == view.id()) else {
                return Err(ApiError::not_found(&format!("view {}", view.id().value())));
            };
            if row.version() != view.version() - 1 {
                return Err(ApiError::stale_version(view.version() - 1, row.version()));
            }
            *row = view.clone();
            Ok(())
        }

        fn remove(&self, id: kanban_domain::SavedViewId) -> Result<(), ApiError> {
            let mut rows = self.rows.lock().expect("the view lock is sound");
            let before = rows.len();
            rows.retain(|row| row.id() != id);
            if rows.len() == before {
                return Err(ApiError::not_found(&format!("view {}", id.value())));
            }
            Ok(())
        }
    }

    /// A core with the saved view operations wired over two active
    /// Projects: CORE (1) and EDGE (2).
    struct ViewHarness {
        views: Arc<MemoryViews>,
        projects: Arc<MemoryProjects>,
        core: Core,
    }

    fn harness() -> ViewHarness {
        let projects = Arc::new(MemoryProjects::default());
        projects.seed(active_project(
            1,
            "CORE",
            kanban_domain::ProjectCounters::zeroed(),
        ));
        projects.seed(active_project(
            2,
            "EDGE",
            kanban_domain::ProjectCounters::zeroed(),
        ));
        let views = Arc::new(MemoryViews::default());
        let mut core = Core::new(
            crate::catalog::exposed_operations(),
            Arc::new(MemoryIdempotencyStore::new()),
            Arc::new(NoopEventSink),
        );
        core.register_saved_views(views.clone(), projects.clone())
            .expect("the view operations register");
        ViewHarness {
            views,
            projects,
            core,
        }
    }

    fn mutation(version: u64, key: &str) -> serde_json::Value {
        json!({
            "optimistic_version": version,
            "idempotency_key": key,
        })
    }

    /// The listed views, as (scope, name, default) triples.
    fn listed(harness: &ViewHarness) -> Vec<(serde_json::Value, String, bool)> {
        harness
            .core
            .query("view.list", &json!({}))
            .expect("the list serves")["views"]
            .as_array()
            .expect("the views are a list")
            .iter()
            .map(|view| {
                (
                    view["scope"].clone(),
                    view["name"].as_str().expect("the name is text").to_owned(),
                    view["is_default"].as_bool().expect("the flag is boolean"),
                )
            })
            .collect()
    }

    #[test]
    fn the_first_list_generates_the_global_default_and_one_per_project() {
        let harness = harness();

        let response = harness
            .core
            .query("view.list", &json!({}))
            .expect("the list serves");

        let views = response["views"].as_array().expect("the views are a list");
        assert_eq!(
            views.len(),
            3,
            "the global default and one default per Project"
        );
        // The global scope first, then the Projects by identity; each
        // scope's default is the view the scope opens on.
        let global = &views[0];
        assert_eq!(global["scope"], json!("global"));
        assert_eq!(global["name"], json!("All work"));
        assert_eq!(global["is_default"], json!(true));
        assert_eq!(global["filter"], json!({}), "the whole board shows");
        assert_eq!(global["expanded_groups"], json!([]));
        assert_eq!(global["hidden_columns"], json!(["draft"]));
        assert_eq!(global["mode"], json!("board"));
        assert_eq!(global["done_placement"], json!("column"));
        assert_eq!(global["sorting"], json!("priority"));
        let core_default = &views[1];
        assert_eq!(core_default["scope"], json!({ "project": 1 }));
        assert_eq!(
            core_default["filter"],
            json!({ "projects": [1] }),
            "a Project default opens on its own Project"
        );
        assert_eq!(core_default["is_default"], json!(true));
        assert_eq!(views[2]["scope"], json!({ "project": 2 }));
    }

    #[test]
    fn generated_defaults_persist_across_lists() {
        let harness = harness();

        let first = harness
            .core
            .query("view.list", &json!({}))
            .expect("the first list serves");
        let second = harness
            .core
            .query("view.list", &json!({}))
            .expect("the second list serves");

        assert_eq!(
            first, second,
            "materialised defaults stand; the read never mints them twice"
        );
        assert_eq!(
            harness.views.snapshot().len(),
            3,
            "the store holds one row per default"
        );
    }

    #[test]
    fn a_registered_project_gets_its_default_on_the_next_list() {
        let harness = harness();
        harness
            .core
            .query("view.list", &json!({}))
            .expect("the first list serves");
        harness.projects.seed(active_project(
            3,
            "AUX",
            kanban_domain::ProjectCounters::zeroed(),
        ));

        let scopes = listed(&harness);

        assert_eq!(scopes.len(), 4, "the new Project's default generates");
        assert_eq!(
            scopes[3],
            (json!({ "project": 3 }), "All work".to_owned(), true),
            "the generated default carries the everyday perspective"
        );
    }

    #[test]
    fn a_created_view_owns_every_property_exactly() {
        let harness = harness();

        let created = harness
            .core
            .command(
                "view.create",
                &json!({
                    "mutation": mutation(0, "key-create"),
                    "scope": "global",
                    "name": "Review queue",
                    "filter": {
                        "kinds": ["implementation"],
                        "states": ["in_review"],
                        "priorities": ["urgent"],
                    },
                    "expanded_groups": ["backlog", "staged"],
                    "hidden_columns": ["draft", "done"],
                    "mode": "register",
                    "done_placement": "table",
                    "sorting": "readiness",
                }),
            )
            .expect("the view creates");

        assert_eq!(created["name"], json!("Review queue"));
        assert_eq!(created["is_default"], json!(false));
        assert_eq!(
            created["filter"],
            json!({
                "kinds": ["implementation"],
                "states": ["in_review"],
                "priorities": ["urgent"],
            })
        );
        assert_eq!(created["expanded_groups"], json!(["backlog", "staged"]));
        assert_eq!(created["hidden_columns"], json!(["draft", "done"]));
        assert_eq!(created["mode"], json!("register"));
        assert_eq!(created["done_placement"], json!("table"));
        assert_eq!(created["sorting"], json!("readiness"));
        assert_eq!(created["version"], json!(1));

        // The list answers with the default first, then the named
        // views by name.
        let scopes = listed(&harness);
        assert_eq!(
            scopes,
            vec![
                (json!("global"), "All work".to_owned(), true),
                (json!("global"), "Review queue".to_owned(), false),
                (json!({ "project": 1 }), "All work".to_owned(), true),
                (json!({ "project": 2 }), "All work".to_owned(), true),
            ]
        );
    }

    #[test]
    fn create_refuses_a_taken_name_within_one_scope_only() {
        let harness = harness();
        let request = |name: &str, scope: serde_json::Value, key: &str| {
            json!({
                "mutation": mutation(0, key),
                "scope": scope,
                "name": name,
                "mode": "board",
                "done_placement": "column",
                "sorting": "priority",
            })
        };

        let refused = harness
            .core
            .command(
                "view.create",
                &request("All work", json!("global"), "key-default"),
            )
            .expect_err("the generated default's name is taken");
        assert_eq!(refused.code, ErrorCode::InvalidRequest);
        assert_eq!(
            refused.message,
            "the view name `All work` is already taken in its scope"
        );

        harness
            .core
            .command(
                "view.create",
                &request("Review queue", json!("global"), "key-named"),
            )
            .expect("a fresh name lands");
        harness
            .core
            .command(
                "view.create",
                &request("Review queue", json!({ "project": 2 }), "key-other-scope"),
            )
            .expect("the same name is free in another scope");
    }

    #[test]
    fn create_refuses_blank_names_unknown_projects_and_fixed_groups() {
        let harness = harness();

        let blank = harness
            .core
            .command(
                "view.create",
                &json!({
                    "mutation": mutation(0, "key-blank"),
                    "scope": "global",
                    "name": "  ",
                    "mode": "board",
                    "done_placement": "column",
                    "sorting": "priority",
                }),
            )
            .expect_err("a blank name is refused");
        assert_eq!(blank.code, ErrorCode::InvalidRequest);
        assert_eq!(blank.message, "a saved view name cannot be blank");

        let unknown = harness
            .core
            .command(
                "view.create",
                &json!({
                    "mutation": mutation(0, "key-unknown"),
                    "scope": { "project": 9 },
                    "name": "Ghost",
                    "mode": "board",
                    "done_placement": "column",
                    "sorting": "priority",
                }),
            )
            .expect_err("an unregistered Project scope is refused");
        assert_eq!(unknown.code, ErrorCode::NotFound);
        assert_eq!(unknown.message, "project 9 was not found");

        let fixed = harness
            .core
            .command(
                "view.create",
                &json!({
                    "mutation": mutation(0, "key-fixed"),
                    "scope": "global",
                    "name": "Wide",
                    "expanded_groups": ["current"],
                    "mode": "board",
                    "done_placement": "column",
                    "sorting": "priority",
                }),
            )
            .expect_err("a group that cannot expand is refused");
        assert_eq!(fixed.code, ErrorCode::InvalidRequest);
        assert_eq!(
            fixed.message,
            "the current group cannot expand into its states"
        );
    }

    #[test]
    fn update_replaces_the_whole_owned_set_and_guards_the_version() {
        let harness = harness();
        let created = harness
            .core
            .command(
                "view.create",
                &json!({
                    "mutation": mutation(0, "key-create"),
                    "scope": "global",
                    "name": "Everyday",
                    "expanded_groups": ["backlog"],
                    "mode": "board",
                    "done_placement": "column",
                    "sorting": "priority",
                }),
            )
            .expect("the view creates");
        let id = created["id"].as_u64().expect("the identity is a number");

        let updated = harness
            .core
            .command(
                "view.update",
                &json!({
                    "mutation": mutation(1, "key-update"),
                    "view_id": id,
                    "filter": { "attention": ["stale_run"] },
                    "expanded_groups": ["staged"],
                    "hidden_columns": ["draft", "review"],
                    "mode": "register",
                    "done_placement": "table",
                    "sorting": "readiness",
                }),
            )
            .expect("the whole owned set lands");
        assert_eq!(updated["filter"], json!({ "attention": ["stale_run"] }));
        assert_eq!(updated["expanded_groups"], json!(["staged"]));
        assert_eq!(updated["hidden_columns"], json!(["draft", "review"]));
        assert_eq!(updated["mode"], json!("register"));
        assert_eq!(updated["done_placement"], json!("table"));
        assert_eq!(updated["sorting"], json!("readiness"));
        assert_eq!(updated["version"], json!(2));
        assert_eq!(updated["name"], json!("Everyday"), "the name stands");

        let stale = harness
            .core
            .command(
                "view.update",
                &json!({
                    "mutation": mutation(1, "key-stale"),
                    "view_id": id,
                    "mode": "board",
                    "done_placement": "column",
                    "sorting": "priority",
                }),
            )
            .expect_err("the replaced version is stale");
        assert_eq!(stale.code, ErrorCode::StaleVersion);
        assert_eq!(stale.current_version, Some(2));

        let missing = harness
            .core
            .command(
                "view.update",
                &json!({
                    "mutation": mutation(1, "key-missing"),
                    "view_id": 99,
                    "mode": "board",
                    "done_placement": "column",
                    "sorting": "priority",
                }),
            )
            .expect_err("an unknown view is not found");
        assert_eq!(missing.code, ErrorCode::NotFound);
    }

    #[test]
    fn rename_changes_only_the_name() {
        let harness = harness();
        let created = harness
            .core
            .command(
                "view.create",
                &json!({
                    "mutation": mutation(0, "key-create"),
                    "scope": { "project": 1 },
                    "name": "Everyday",
                    "hidden_columns": ["draft"],
                    "mode": "register",
                    "done_placement": "table",
                    "sorting": "readiness",
                }),
            )
            .expect("the view creates");
        let id = created["id"].as_u64().expect("the identity is a number");

        let renamed = harness
            .core
            .command(
                "view.rename",
                &json!({
                    "mutation": mutation(1, "key-rename"),
                    "view_id": id,
                    "name": "Deep work",
                }),
            )
            .expect("the rename lands");
        assert_eq!(renamed["name"], json!("Deep work"));
        assert_eq!(renamed["hidden_columns"], json!(["draft"]));
        assert_eq!(renamed["mode"], json!("register"), "nothing else moved");
        assert_eq!(renamed["sorting"], json!("readiness"));
        assert_eq!(renamed["version"], json!(2));

        let refused = harness
            .core
            .command(
                "view.rename",
                &json!({
                    "mutation": mutation(2, "key-taken"),
                    "view_id": id,
                    "name": "All work",
                }),
            )
            .expect_err("the default's name is taken in the scope");
        assert_eq!(refused.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn removing_a_default_regenerates_it_on_the_next_read() {
        let harness = harness();
        let first = harness
            .core
            .query("view.list", &json!({}))
            .expect("the first list serves");
        let global_default_id = first["views"][0]["id"]
            .as_u64()
            .expect("the identity is a number");

        harness
            .core
            .command(
                "view.remove",
                &json!({
                    "mutation": mutation(1, "key-remove"),
                    "view_id": global_default_id,
                }),
            )
            .expect("the default may be removed");

        let second = harness
            .core
            .query("view.list", &json!({}))
            .expect("the next list serves");
        let regenerated = &second["views"][0];
        assert_eq!(regenerated["scope"], json!("global"));
        assert_eq!(regenerated["name"], json!("All work"));
        assert_eq!(regenerated["is_default"], json!(true));
        assert_ne!(
            regenerated["id"]
                .as_u64()
                .expect("the identity is a number"),
            global_default_id,
            "the regenerated default is a fresh view, not a resurrection"
        );
        assert_eq!(
            second["views"].as_array().map(Vec::len),
            Some(3),
            "every scope keeps exactly one default"
        );
    }

    #[test]
    fn every_operation_rejects_unknown_fields() {
        let harness = harness();

        let mut request = json!({
            "mutation": mutation(0, "key-surprise"),
            "scope": "global",
            "name": "Everything",
            "mode": "board",
            "done_placement": "column",
            "sorting": "priority",
        });
        request["sort"] = json!("manual");
        let error = harness
            .core
            .command("view.create", &request)
            .expect_err("unknown fields are rejected");
        assert_eq!(error.code, ErrorCode::UnknownField);
        assert_eq!(error.message, "unknown field `sort`");

        let error = harness
            .core
            .query("view.list", &json!({ "scope": "global" }))
            .expect_err("the list query carries nothing");
        assert_eq!(error.code, ErrorCode::UnknownField);
    }
}
