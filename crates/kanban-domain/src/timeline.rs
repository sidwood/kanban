//! The entity vocabulary domain rules validate against.
//!
//! Event kinds are wire vocabulary and live with the payload
//! definitions; this list exists because domain rules — a Comment's
//! target, for one — refuse an entity kind outside it.

/// Entity kinds that may be referenced from timeline events.
pub const ENTITY_KINDS: &[&str] = &[
    "initiative",
    "project",
    "plan",
    "spec",
    "ticket",
    "run",
    "review",
    "finding",
    "evidence",
    "comment",
    "workspace",
    "lane",
    "profile",
];

/// Whether `kind` is a known timeline entity kind.
pub fn is_entity_kind(kind: &str) -> bool {
    ENTITY_KINDS.contains(&kind)
}

#[cfg(test)]
mod tests {
    use super::is_entity_kind;

    #[test]
    fn entity_kinds_reject_unknown_values() {
        assert!(is_entity_kind("ticket"));
        assert!(!is_entity_kind("ghost"));
    }
}
