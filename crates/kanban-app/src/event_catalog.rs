/// One named live event that may appear in generated clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventDescriptor {
    pub name: &'static str,
    pub payload_schema: &'static str,
    pub description: &'static str,
}

/// Live events the application layer currently publishes to every
/// subscriber.
pub fn exposed_events() -> &'static [EventDescriptor] {
    &[
        EventDescriptor {
            name: "initiative.created",
            payload_schema: "InitiativeRecord",
            description: "An Initiative was created.",
        },
        EventDescriptor {
            name: "initiative.renamed",
            payload_schema: "InitiativeRecord",
            description: "An Initiative was renamed.",
        },
        EventDescriptor {
            name: "initiative.archived",
            payload_schema: "InitiativeRecord",
            description: "An Initiative was archived.",
        },
        EventDescriptor {
            name: "project.registered",
            payload_schema: "ProjectRecord",
            description: "A Project was registered.",
        },
        EventDescriptor {
            name: "project.archived",
            payload_schema: "ProjectRecord",
            description: "A Project was archived.",
        },
        EventDescriptor {
            name: "plan.created",
            payload_schema: "PlanRecord",
            description: "A Plan was created.",
        },
        EventDescriptor {
            name: "plan.activated",
            payload_schema: "PlanRecord",
            description: "A Plan was activated, freezing a version.",
        },
        EventDescriptor {
            name: "plan.replanned",
            payload_schema: "PlanRecord",
            description: "A Plan was replanned, reserving its replacement version.",
        },
        EventDescriptor {
            name: "plan.completed",
            payload_schema: "PlanRecord",
            description: "A Plan was completed.",
        },
        EventDescriptor {
            name: "plan.cancelled",
            payload_schema: "PlanRecord",
            description: "A Plan was cancelled.",
        },
        EventDescriptor {
            name: "plan.archived",
            payload_schema: "PlanRecord",
            description: "A Plan was archived.",
        },
        EventDescriptor {
            name: "spec.created",
            payload_schema: "SpecRecord",
            description: "A Spec was authored.",
        },
        EventDescriptor {
            name: "spec.planned",
            payload_schema: "SpecRecord",
            description: "A Spec joined a Plan, planning its execution.",
        },
        EventDescriptor {
            name: "spec.version.approved",
            payload_schema: "SpecRecord",
            description: "A Spec content version was approved.",
        },
        EventDescriptor {
            name: "spec.version.superseded",
            payload_schema: "SpecRecord",
            description: "A Spec content version was superseded.",
        },
        EventDescriptor {
            name: "spec.execution.moved",
            payload_schema: "SpecRecord",
            description: "A Spec's execution moved along its state set.",
        },
        EventDescriptor {
            name: "comment.created",
            payload_schema: "CommentRecord",
            description: "A Comment was created.",
        },
        EventDescriptor {
            name: "comment.edited",
            payload_schema: "CommentRecord",
            description: "A Comment was edited.",
        },
        EventDescriptor {
            name: "ruling.recorded",
            payload_schema: "RulingIdentity",
            description: "A ruling was recorded.",
        },
        EventDescriptor {
            name: "ruling.superseded",
            payload_schema: "RulingIdentity",
            description: "A ruling was superseded.",
        },
        EventDescriptor {
            name: "deferral.recorded",
            payload_schema: "DeferralIdentity",
            description: "A deferral was recorded.",
        },
        EventDescriptor {
            name: "deferral.superseded",
            payload_schema: "DeferralIdentity",
            description: "A deferral was superseded.",
        },
        EventDescriptor {
            name: "evidence.attached",
            payload_schema: "EvidenceRecord",
            description: "Evidence was attached to a subject entity.",
        },
        EventDescriptor {
            name: "evidence.listed",
            payload_schema: "EvidenceListSummary",
            description: "Evidence was listed for a Project.",
        },
        EventDescriptor {
            name: "workspace.registered",
            payload_schema: "WorkspaceRecord",
            description: "A Workspace was registered.",
        },
        EventDescriptor {
            name: "workspace.observed",
            payload_schema: "WorkspaceRecord",
            description: "A Workspace was observed and its health updated.",
        },
        EventDescriptor {
            name: "workspace.retired",
            payload_schema: "WorkspaceRecord",
            description: "A Workspace was retired. The record is preserved, never deleted.",
        },
    ]
}

/// Look up one catalogued live event by wire name.
pub fn event_descriptor(name: &str) -> &'static EventDescriptor {
    exposed_events()
        .iter()
        .find(|event| event.name == name)
        .unwrap_or_else(|| panic!("catalogue must list `{name}`"))
}
