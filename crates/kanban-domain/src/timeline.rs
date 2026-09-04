//! Closed timeline vocabularies and validation helpers.

use std::str::FromStr;

/// Every event kind the Plan records on the activity timeline.
pub const EVENT_KINDS: &[&str] = &[
    "transition",
    "run",
    "telemetry",
    "review",
    "finding",
    "evidence",
    "comment",
    "deferral",
    "ruling",
];

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
];

/// Whether `kind` is a known timeline event kind.
pub fn is_event_kind(kind: &str) -> bool {
    EVENT_KINDS.contains(&kind)
}

/// Whether `kind` is a known timeline entity kind.
pub fn is_entity_kind(kind: &str) -> bool {
    ENTITY_KINDS.contains(&kind)
}

/// A timeline event kind parsed from its wire representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineEventKind(&'static str);

impl TimelineEventKind {
    /// Parse a wire kind, rejecting unknown values.
    pub fn parse(kind: &str) -> Option<Self> {
        EVENT_KINDS
            .iter()
            .copied()
            .find(|candidate| *candidate == kind)
            .map(Self)
    }

    /// The wire representation.
    pub fn as_str(&self) -> &str {
        self.0
    }
}

impl FromStr for TimelineEventKind {
    type Err = ();

    fn from_str(kind: &str) -> Result<Self, Self::Err> {
        Self::parse(kind).ok_or(())
    }
}

/// A timeline entity kind parsed from its wire representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineEntityKind(&'static str);

impl TimelineEntityKind {
    /// Parse a wire kind, rejecting unknown values.
    pub fn parse(kind: &str) -> Option<Self> {
        ENTITY_KINDS
            .iter()
            .copied()
            .find(|candidate| *candidate == kind)
            .map(Self)
    }

    /// The wire representation.
    pub fn as_str(&self) -> &str {
        self.0
    }
}

impl FromStr for TimelineEntityKind {
    type Err = ();

    fn from_str(kind: &str) -> Result<Self, Self::Err> {
        Self::parse(kind).ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::{EVENT_KINDS, is_entity_kind, is_event_kind};

    #[test]
    fn event_kinds_cover_every_required_category() {
        for kind in [
            "transition",
            "run",
            "telemetry",
            "review",
            "finding",
            "evidence",
            "comment",
            "deferral",
            "ruling",
        ] {
            assert!(is_event_kind(kind), "missing required event kind `{kind}`");
        }
        assert_eq!(EVENT_KINDS.len(), 9);
    }

    #[test]
    fn entity_kinds_reject_unknown_values() {
        assert!(is_entity_kind("ticket"));
        assert!(!is_entity_kind("ghost"));
    }
}
