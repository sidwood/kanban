/// Whether an exposed surface issues a command or a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Operations the application layer currently exposes to every client.
pub fn exposed_operations() -> &'static [OperationDescriptor] {
    &[
        OperationDescriptor {
            name: "health.get",
            kind: OperationKind::Query,
            request_schema: "HealthQuery",
            response_schema: "HealthResponse",
            mcp_tool_name: "health_get",
            description: "Return current core health for boot and diagnostics.",
        },
        OperationDescriptor {
            name: "initiative.create",
            kind: OperationKind::Command,
            request_schema: "InitiativeCreateRequest",
            response_schema: "InitiativeRecord",
            mcp_tool_name: "initiative_create",
            description: "Create an Initiative folder with a validated name.",
        },
        OperationDescriptor {
            name: "initiative.rename",
            kind: OperationKind::Command,
            request_schema: "InitiativeRenameRequest",
            response_schema: "InitiativeRecord",
            mcp_tool_name: "initiative_rename",
            description: "Rename an active Initiative.",
        },
        OperationDescriptor {
            name: "initiative.archive",
            kind: OperationKind::Command,
            request_schema: "InitiativeArchiveRequest",
            response_schema: "InitiativeRecord",
            mcp_tool_name: "initiative_archive",
            description: "Archive an Initiative. Archiving is terminal and preserves every recorded fact.",
        },
        OperationDescriptor {
            name: "initiative.list",
            kind: OperationKind::Query,
            request_schema: "InitiativeListQuery",
            response_schema: "InitiativeListResponse",
            mcp_tool_name: "initiative_list",
            description: "List every Initiative, archived ones included.",
        },
    ]
}
