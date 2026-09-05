/// Whether an exposed surface issues a command or a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OperationKind {
    Command,
    Query,
}

/// One named application operation that may appear in generated clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationDescriptor {
    pub name: &'static str,
    pub kind: OperationKind,
    pub request_schema: &'static str,
    pub response_schema: &'static str,
    pub mcp_tool_name: &'static str,
    pub description: &'static str,
}

macro_rules! define_exposed_catalogue {
    (
        $(
            $name:literal => {
                kind: $kind:ident,
                request: $request:literal,
                response: $response:literal,
                mcp: $mcp:literal,
                description: $description:literal,
            }
        ),* $(,)?
    ) => {
        /// Operations the application layer currently exposes to every client.
        pub fn exposed_operations() -> &'static [OperationDescriptor] {
            &[
                $(
                    OperationDescriptor {
                        name: $name,
                        kind: OperationKind::$kind,
                        request_schema: $request,
                        response_schema: $response,
                        mcp_tool_name: $mcp,
                        description: $description,
                    },
                )*
            ]
        }

        /// The Tauri and MCP names derived from the same catalogue entries.
        pub const EXPOSED_MCP_TOOL_NAMES: &[&str] = &[$( $mcp ),*];

        /// How many operations the catalogue carries; shell drift checks
        /// compare against this at compile time.
        pub const EXPOSED_OPERATION_COUNT: usize = [$( $name ),*].len();
    };
}

define_exposed_catalogue! {
    "health.get" => {
        kind: Query,
        request: "HealthQuery",
        response: "HealthResponse",
        mcp: "health_get",
        description: "Return current core health for boot and diagnostics.",
    },
    "initiative.create" => {
        kind: Command,
        request: "InitiativeCreateRequest",
        response: "InitiativeRecord",
        mcp: "initiative_create",
        description: "Create an Initiative folder with a validated name.",
    },
    "initiative.rename" => {
        kind: Command,
        request: "InitiativeRenameRequest",
        response: "InitiativeRecord",
        mcp: "initiative_rename",
        description: "Rename an active Initiative.",
    },
    "initiative.archive" => {
        kind: Command,
        request: "InitiativeArchiveRequest",
        response: "InitiativeRecord",
        mcp: "initiative_archive",
        description: "Archive an Initiative. Archiving is terminal and preserves every recorded fact.",
    },
    "initiative.list" => {
        kind: Query,
        request: "InitiativeListQuery",
        response: "InitiativeListResponse",
        mcp: "initiative_list",
        description: "List every Initiative, archived ones included.",
    },
    "project.register" => {
        kind: Command,
        request: "ProjectRegisterRequest",
        response: "ProjectRecord",
        mcp: "project_register",
        description: "Register a Project with one Git repository, Seed Workspace, default branch, and exclusive Herdr session.",
    },
    "project.archive" => {
        kind: Command,
        request: "ProjectArchiveRequest",
        response: "ProjectRecord",
        mcp: "project_archive",
        description: "Archive a Project. Archiving is terminal and preserves every recorded fact.",
    },
    "project.list" => {
        kind: Query,
        request: "ProjectListQuery",
        response: "ProjectListResponse",
        mcp: "project_list",
        description: "List every Project, archived ones included.",
    },
    "plan.create" => {
        kind: Command,
        request: "PlanCreateRequest",
        response: "PlanRecord",
        mcp: "plan_create",
        description: "Create a draft Plan, minting its number from the Project counter.",
    },
    "plan.spec.add" => {
        kind: Command,
        request: "PlanSpecAddRequest",
        response: "PlanRecord",
        mcp: "plan_spec_add",
        description: "Add a Spec to a draft Plan's membership and display order.",
    },
    "plan.spec.remove" => {
        kind: Command,
        request: "PlanSpecRemoveRequest",
        response: "PlanRecord",
        mcp: "plan_spec_remove",
        description: "Remove a Spec from a draft Plan; a Spec carrying edges is refused.",
    },
    "plan.spec.move" => {
        kind: Command,
        request: "PlanSpecMoveRequest",
        response: "PlanRecord",
        mcp: "plan_spec_move",
        description: "Move a Spec within a draft Plan's display order.",
    },
    "plan.edge.add" => {
        kind: Command,
        request: "PlanEdgeAddRequest",
        response: "PlanRecord",
        mcp: "plan_edge_add",
        description: "Add a dependency edge inside one draft Plan; an edge leaving the Plan is refused.",
    },
    "plan.edge.remove" => {
        kind: Command,
        request: "PlanEdgeRemoveRequest",
        response: "PlanRecord",
        mcp: "plan_edge_remove",
        description: "Remove a dependency edge from a draft Plan.",
    },
    "plan.activate" => {
        kind: Command,
        request: "PlanActivateRequest",
        response: "PlanRecord",
        mcp: "plan_activate",
        description: "Freeze a draft Plan's membership, order, and graph into an immutable version.",
    },
    "plan.replan" => {
        kind: Command,
        request: "PlanReplanRequest",
        response: "PlanRecord",
        mcp: "plan_replan",
        description: "Reopen an active Plan's draft and reserve its auditable replacement version.",
    },
    "plan.complete" => {
        kind: Command,
        request: "PlanCompleteRequest",
        response: "PlanRecord",
        mcp: "plan_complete",
        description: "Complete an active Plan. Complete is terminal and off the active surface.",
    },
    "plan.cancel" => {
        kind: Command,
        request: "PlanCancelRequest",
        response: "PlanRecord",
        mcp: "plan_cancel",
        description: "Cancel a draft or active Plan. Cancelled is terminal and off the active surface.",
    },
    "plan.archive" => {
        kind: Command,
        request: "PlanArchiveRequest",
        response: "PlanRecord",
        mcp: "plan_archive",
        description: "Archive a Plan from any open state. Archiving is terminal and preserves every recorded fact.",
    },
    "plan.list" => {
        kind: Query,
        request: "PlanListQuery",
        response: "PlanListResponse",
        mcp: "plan_list",
        description: "List every Plan of a Project, terminal states included.",
    },
    "plan.get" => {
        kind: Query,
        request: "PlanGetQuery",
        response: "PlanGetResponse",
        mcp: "plan_get",
        description: "Read one Plan with every frozen version beside it.",
    },
    "timeline.query" => {
        kind: Query,
        request: "TimelineQuery",
        response: "TimelineQueryResponse",
        mcp: "timeline_query",
        description: "Query the per-Project append-only activity timeline.",
    },
    "comment.create" => {
        kind: Command,
        request: "CommentCreateRequest",
        response: "CommentRecord",
        mcp: "comment_create",
        description: "Create a Comment with its first revision on a timeline-visible entity.",
    },
    "comment.edit" => {
        kind: Command,
        request: "CommentEditRequest",
        response: "CommentRecord",
        mcp: "comment_edit",
        description: "Edit a Comment by appending a new immutable revision.",
    },
    "comment.revisions" => {
        kind: Query,
        request: "CommentRevisionsQuery",
        response: "CommentRevisionsResponse",
        mcp: "comment_revisions",
        description: "Query the full revision history for one Comment.",
    },
    "ruling.record" => {
        kind: Command,
        request: "RulingRecordRequest",
        response: "RulingRecord",
        mcp: "ruling_record",
        description: "Record an immutable operator ruling on the timeline.",
    },
    "ruling.supersede" => {
        kind: Command,
        request: "RulingSupersedeRequest",
        response: "RulingRecord",
        mcp: "ruling_supersede",
        description: "Supersede a ruling with a new immutable record.",
    },
    "ruling.list" => {
        kind: Query,
        request: "RulingListQuery",
        response: "RulingListResponse",
        mcp: "ruling_list",
        description: "List every ruling for a project, superseded originals included.",
    },
    "deferral.record" => {
        kind: Command,
        request: "DeferralRecordRequest",
        response: "DeferralRecord",
        mcp: "deferral_record",
        description: "Record an immutable finding deferral on the timeline.",
    },
    "deferral.supersede" => {
        kind: Command,
        request: "DeferralSupersedeRequest",
        response: "DeferralRecord",
        mcp: "deferral_supersede",
        description: "Supersede a deferral with a new immutable record.",
    },
    "deferral.list" => {
        kind: Query,
        request: "DeferralListQuery",
        response: "DeferralListResponse",
        mcp: "deferral_list",
        description: "List every deferral for a project, superseded originals included.",
    },
    "evidence.attach" => {
        kind: Command,
        request: "EvidenceAttachRequest",
        response: "EvidenceRecord",
        mcp: "evidence_attach",
        description: "Attach managed-file or repository evidence to an entity.",
    },
    "evidence.list" => {
        kind: Command,
        request: "EvidenceListRequest",
        response: "EvidenceListResponse",
        mcp: "evidence_list",
        description: "List evidence for a Project and append a timeline event.",
    },
    "herdr.settings.get" => {
        kind: Query,
        request: "HerdrSettingsGetQuery",
        response: "HerdrSettingsGetResponse",
        mcp: "herdr_settings_get",
        description: "Return one Project's Herdr settings and connection diagnostics.",
    },
    "herdr.settings.update" => {
        kind: Command,
        request: "HerdrSettingsUpdateRequest",
        response: "HerdrProjectSettings",
        mcp: "herdr_settings_update",
        description: "Update one Project's Herdr reconciliation, fallback polling, and deadlines.",
    },
    "herdr.defaults.get" => {
        kind: Query,
        request: "HerdrDefaultsGetQuery",
        response: "HerdrDefaultsGetResponse",
        mcp: "herdr_defaults_get",
        description: "Return global Herdr observation defaults.",
    },
    "herdr.defaults.update" => {
        kind: Command,
        request: "HerdrDefaultsUpdateRequest",
        response: "HerdrGlobalDefaults",
        mcp: "herdr_defaults_update",
        description: "Update global Herdr reconciliation and deadline defaults.",
    },
    "workspace.register" => {
        kind: Command,
        request: "WorkspaceRegisterRequest",
        response: "WorkspaceRecord",
        mcp: "workspace_register",
        description: "Register a Workspace path for a Project.",
    },
    "workspace.observe" => {
        kind: Command,
        request: "WorkspaceObserveRequest",
        response: "WorkspaceRecord",
        mcp: "workspace_observe",
        description: "Observe git state for a Workspace without mutating the clone.",
    },
    "workspace.list" => {
        kind: Query,
        request: "WorkspaceListQuery",
        response: "WorkspaceListResponse",
        mcp: "workspace_list",
        description: "List every Workspace for one Project with health and observation.",
    },
}

/// Compare registered core handlers with the exposed catalogue.
pub fn assert_registered_matches_exposed_catalogue(registered: &[(&'static str, OperationKind)]) {
    use std::collections::BTreeMap;

    let registered: BTreeMap<_, _> = registered.iter().copied().collect();
    let catalogue: BTreeMap<_, _> = exposed_operations()
        .iter()
        .map(|operation| (operation.name, operation.kind))
        .collect();

    assert_eq!(
        registered, catalogue,
        "registered operations must equal the exposed catalogue exactly"
    );
}
