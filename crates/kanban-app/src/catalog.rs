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
    &[OperationDescriptor {
        name: "health.get",
        kind: OperationKind::Query,
        request_schema: "HealthQuery",
        response_schema: "HealthResponse",
        mcp_tool_name: "health_get",
        description: "Return current core health for boot and diagnostics.",
    }]
}
