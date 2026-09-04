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
        OperationDescriptor {
            name: "timeline.query",
            kind: OperationKind::Query,
            request_schema: "TimelineQuery",
            response_schema: "TimelineQueryResponse",
            mcp_tool_name: "timeline_query",
            description: "Query the per-Project append-only activity timeline.",
        },
        OperationDescriptor {
            name: "comment.create",
            kind: OperationKind::Command,
            request_schema: "CommentCreateRequest",
            response_schema: "CommentRecord",
            mcp_tool_name: "comment_create",
            description: "Create a Comment with its first revision on a timeline-visible entity.",
        },
        OperationDescriptor {
            name: "comment.edit",
            kind: OperationKind::Command,
            request_schema: "CommentEditRequest",
            response_schema: "CommentRecord",
            mcp_tool_name: "comment_edit",
            description: "Edit a Comment by appending a new immutable revision.",
        },
        OperationDescriptor {
            name: "comment.revisions",
            kind: OperationKind::Query,
            request_schema: "CommentRevisionsQuery",
            response_schema: "CommentRevisionsResponse",
            mcp_tool_name: "comment_revisions",
            description: "Query the full revision history for one Comment.",
        },
        OperationDescriptor {
            name: "ruling.record",
            kind: OperationKind::Command,
            request_schema: "RulingRecordRequest",
            response_schema: "RulingRecord",
            mcp_tool_name: "ruling_record",
            description: "Record an immutable operator ruling on the timeline.",
        },
        OperationDescriptor {
            name: "ruling.supersede",
            kind: OperationKind::Command,
            request_schema: "RulingSupersedeRequest",
            response_schema: "RulingRecord",
            mcp_tool_name: "ruling_supersede",
            description: "Supersede a ruling with a new immutable record.",
        },
        OperationDescriptor {
            name: "ruling.list",
            kind: OperationKind::Query,
            request_schema: "RulingListQuery",
            response_schema: "RulingListResponse",
            mcp_tool_name: "ruling_list",
            description: "List every ruling for a project, superseded originals included.",
        },
        OperationDescriptor {
            name: "deferral.record",
            kind: OperationKind::Command,
            request_schema: "DeferralRecordRequest",
            response_schema: "DeferralRecord",
            mcp_tool_name: "deferral_record",
            description: "Record an immutable finding deferral on the timeline.",
        },
        OperationDescriptor {
            name: "deferral.supersede",
            kind: OperationKind::Command,
            request_schema: "DeferralSupersedeRequest",
            response_schema: "DeferralRecord",
            mcp_tool_name: "deferral_supersede",
            description: "Supersede a deferral with a new immutable record.",
        },
        OperationDescriptor {
            name: "deferral.list",
            kind: OperationKind::Query,
            request_schema: "DeferralListQuery",
            response_schema: "DeferralListResponse",
            mcp_tool_name: "deferral_list",
            description: "List every deferral for a project, superseded originals included.",
        },
        OperationDescriptor {
            name: "evidence.attach",
            kind: OperationKind::Command,
            request_schema: "EvidenceAttachRequest",
            response_schema: "EvidenceRecord",
            mcp_tool_name: "evidence_attach",
            description: "Attach managed-file or repository evidence to an entity.",
        },
        OperationDescriptor {
            name: "evidence.list",
            kind: OperationKind::Command,
            request_schema: "EvidenceListRequest",
            response_schema: "EvidenceListResponse",
            mcp_tool_name: "evidence_list",
            description: "List evidence for a Project and append a timeline event.",
        },
    ]
}
