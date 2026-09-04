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
    ]
}

/// Look up one catalogued live event by wire name.
pub fn event_descriptor(name: &str) -> &'static EventDescriptor {
    exposed_events()
        .iter()
        .find(|event| event.name == name)
        .unwrap_or_else(|| panic!("catalogue must list `{name}`"))
}
