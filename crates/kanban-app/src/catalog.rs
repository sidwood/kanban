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
    "diagnostics.export" => {
        kind: Query,
        request: "DiagnosticsExportQuery",
        response: "DiagnosticsExportResponse",
        mcp: "diagnostics_export",
        description: "Export one redacted diagnostic bundle of logs, health, and configuration under managed application data.",
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
    "plan.diagnostics" => {
        kind: Query,
        request: "PlanDiagnosticsQuery",
        response: "PlanDiagnosticsResponse",
        mcp: "plan_diagnostics",
        description: "Report one Plan graph's blocking diagnostics: dependency cycles, story coverage gaps, and invalid profile references.",
    },
    "spec.create" => {
        kind: Command,
        request: "SpecCreateRequest",
        response: "SpecRecord",
        mcp: "spec_create",
        description: "Author a Spec with its opening PRD content, minting its number from the Project counter.",
    },
    "spec.content.update" => {
        kind: Command,
        request: "SpecContentUpdateRequest",
        response: "SpecRecord",
        mcp: "spec_content_update",
        description: "Update a Spec's working content; a material change past approval mints a new draft version.",
    },
    "spec.version.approve" => {
        kind: Command,
        request: "SpecVersionApproveRequest",
        response: "SpecRecord",
        mcp: "spec_version_approve",
        description: "Approve the working draft into immutable operative content.",
    },
    "spec.version.supersede" => {
        kind: Command,
        request: "SpecVersionSupersedeRequest",
        response: "SpecRecord",
        mcp: "spec_version_supersede",
        description: "Supersede one content version explicitly. Superseded versions stay queryable for pinned Tickets.",
    },
    "spec.plan.join" => {
        kind: Command,
        request: "SpecPlanJoinRequest",
        response: "SpecRecord",
        mcp: "spec_plan_join",
        description: "Join an unplanned Spec to the Plan holding its number, planning its execution.",
    },
    "spec.execution.move" => {
        kind: Command,
        request: "SpecExecutionMoveRequest",
        response: "SpecRecord",
        mcp: "spec_execution_move",
        description: "Move Spec execution along its closed state set, independently of content.",
    },
    "spec.list" => {
        kind: Query,
        request: "SpecListQuery",
        response: "SpecListResponse",
        mcp: "spec_list",
        description: "List every Spec of a Project, terminal execution states included.",
    },
    "spec.get" => {
        kind: Query,
        request: "SpecGetQuery",
        response: "SpecGetResponse",
        mcp: "spec_get",
        description: "Read one Spec with every content version beside it.",
    },
    "spec.version.get" => {
        kind: Query,
        request: "SpecVersionGetQuery",
        response: "SpecVersionRecord",
        mcp: "spec_version_get",
        description: "Read one content version, superseded versions included, as Ticket pins resolve.",
    },
    "spec.coverage.check" => {
        kind: Query,
        request: "SpecCoverageCheckQuery",
        response: "SpecCoverageCheckResponse",
        mcp: "spec_coverage_check",
        description: "Check one Spec version's story coverage against proposed criteria; the executable gate refuses uncovered stories.",
    },
    "ticket.create" => {
        kind: Command,
        request: "TicketCreateRequest",
        response: "TicketRecord",
        mcp: "ticket_create",
        description: "Create a Ticket under its kind's schema, minting its number from the Project counter.",
    },
    "ticket.bug.qualify" => {
        kind: Command,
        request: "TicketBugQualifyRequest",
        response: "TicketRecord",
        mcp: "ticket_bug_qualify",
        description: "Qualify one Bug with its complete qualification, severity included; the Bug stays draft and readiness stays computed.",
    },
    "ticket.bug.facts" => {
        kind: Command,
        request: "TicketBugFactsRequest",
        response: "TicketRecord",
        mcp: "ticket_bug_facts",
        description: "Record the vendor-neutral External References, Occurrence Snapshots, and Evidence Items one Bug carries.",
    },
    "ticket.list" => {
        kind: Query,
        request: "TicketListQuery",
        response: "TicketListResponse",
        mcp: "ticket_list",
        description: "List every Ticket of a Project, terminal lifecycle states included.",
    },
    "ticket.get" => {
        kind: Query,
        request: "TicketGetQuery",
        response: "TicketRecord",
        mcp: "ticket_get",
        description: "Read one Ticket with the record of its kind's schema.",
    },
    "ticket.dependency.add" => {
        kind: Command,
        request: "TicketDependencyAddRequest",
        response: "TicketDependenciesResponse",
        mcp: "ticket_dependency_add",
        description: "Register a Ticket dependency. Dependencies may cross Specs and registered Projects; cycles are refused.",
    },
    "ticket.dependency.remove" => {
        kind: Command,
        request: "TicketDependencyRemoveRequest",
        response: "TicketDependenciesResponse",
        mcp: "ticket_dependency_remove",
        description: "Remove one registered Ticket dependency.",
    },
    "ticket.blocker.add" => {
        kind: Command,
        request: "TicketBlockerAddRequest",
        response: "TicketDependenciesResponse",
        mcp: "ticket_blocker_add",
        description: "Record one explicit external blocker naming the unregistered work a Ticket waits on.",
    },
    "ticket.blocker.remove" => {
        kind: Command,
        request: "TicketBlockerRemoveRequest",
        response: "TicketDependenciesResponse",
        mcp: "ticket_blocker_remove",
        description: "Remove one explicit external blocker; removal is the operator action that clears it.",
    },
    "ticket.dependencies" => {
        kind: Query,
        request: "TicketDependenciesQuery",
        response: "TicketDependenciesResponse",
        mcp: "ticket_dependencies",
        description: "Read one Ticket's registered dependencies and external blockers.",
    },
    "ticket.readiness" => {
        kind: Query,
        request: "TicketReadinessQuery",
        response: "TicketReadinessResponse",
        mcp: "ticket_readiness",
        description: "Compute one Ticket's readiness from its dependencies and external blockers. The projection never mutates state.",
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
        kind: Query,
        request: "EvidenceListQuery",
        response: "EvidenceListResponse",
        mcp: "evidence_list",
        description: "List evidence for a Project without mutating the timeline.",
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
    "workspace.retire" => {
        kind: Command,
        request: "WorkspaceRetireRequest",
        response: "WorkspaceRecord",
        mcp: "workspace_retire",
        description: "Retire a Workspace. Retirement is the explicit operator action; the record is preserved, never deleted.",
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
