//! Immutable operator decisions recorded on the activity timeline
//! (CONTEXT.md, DR-AE-03). Rulings are superseded explicitly and
//! never edited.

use std::fmt;

/// The identity of one ruling. Assigned once by storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RulingId(u64);

impl RulingId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RulingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a ruling payload was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulingError {
    /// The summary held nothing but whitespace.
    BlankSummary,
}

impl fmt::Display for RulingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankSummary => write!(f, "a ruling summary cannot be blank"),
        }
    }
}

impl std::error::Error for RulingError {}

/// A validated ruling summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulingSummary(String);

impl RulingSummary {
    /// Accept any summary that holds at least one non-whitespace
    /// character.
    pub fn new(raw: &str) -> Result<Self, RulingError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(RulingError::BlankSummary);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The trimmed summary.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An optional entity reference carried on a ruling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulingEntityRef {
    pub kind: String,
    pub id: String,
}

/// One immutable ruling as stored and served to clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ruling {
    id: RulingId,
    project_id: u64,
    entity: Option<RulingEntityRef>,
    summary: RulingSummary,
    supersedes: Option<RulingId>,
    recorded_at: String,
}

impl Ruling {
    /// Restore a ruling that storage already persisted.
    pub fn restore(
        id: RulingId,
        project_id: u64,
        entity: Option<RulingEntityRef>,
        summary: RulingSummary,
        supersedes: Option<RulingId>,
        recorded_at: String,
    ) -> Self {
        Self {
            id,
            project_id,
            entity,
            summary,
            supersedes,
            recorded_at,
        }
    }

    /// Start a fresh ruling before storage assigns its identity. The
    /// caller has already resolved the Project, so its numeric
    /// identity is carried, not parsed.
    pub fn record(
        project_id: u64,
        summary: RulingSummary,
        entity: Option<RulingEntityRef>,
    ) -> RulingDraft {
        RulingDraft {
            project_id,
            entity,
            summary,
            supersedes: None,
        }
    }

    /// Validate a superseding ruling that references this record.
    pub fn supersede(&self, summary: RulingSummary) -> RulingDraft {
        RulingDraft {
            project_id: self.project_id,
            entity: self.entity.clone(),
            summary,
            supersedes: Some(self.id),
        }
    }

    /// The storage-assigned identity.
    pub fn id(&self) -> RulingId {
        self.id
    }

    /// The project the ruling belongs to.
    pub fn project_id(&self) -> u64 {
        self.project_id
    }

    /// The entity the ruling concerns, if any.
    pub fn entity(&self) -> Option<&RulingEntityRef> {
        self.entity.as_ref()
    }

    /// The operator decision text.
    pub fn summary(&self) -> &RulingSummary {
        &self.summary
    }

    /// The ruling this one supersedes, if any.
    pub fn supersedes(&self) -> Option<RulingId> {
        self.supersedes
    }

    /// When the ruling was recorded.
    pub fn recorded_at(&self) -> &str {
        &self.recorded_at
    }
}

/// A validated ruling waiting for storage to assign its identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulingDraft {
    pub project_id: u64,
    pub entity: Option<RulingEntityRef>,
    pub summary: RulingSummary,
    pub supersedes: Option<RulingId>,
}

#[cfg(test)]
mod tests {
    use super::{Ruling, RulingEntityRef, RulingError, RulingId, RulingSummary};

    #[test]
    fn recording_rejects_blank_summaries() {
        let error = RulingSummary::new("   ").expect_err("blank summaries are refused");
        assert_eq!(error, RulingError::BlankSummary);
    }

    #[test]
    fn superseding_creates_a_new_draft_referencing_the_original() {
        let original = Ruling::restore(
            RulingId::new(1),
            1,
            Some(RulingEntityRef {
                kind: "ticket".to_owned(),
                id: "kan-t12".to_owned(),
            }),
            RulingSummary::new("Hold for review").expect("the summary validates"),
            None,
            "2026-09-04T12:00:01Z".to_owned(),
        );
        let replacement = original
            .supersede(RulingSummary::new("Proceed with landing").expect("the summary validates"));

        assert_eq!(replacement.project_id, 1);
        assert_eq!(replacement.supersedes, Some(RulingId::new(1)));
        assert_eq!(replacement.summary.as_str(), "Proceed with landing");
        assert_eq!(
            replacement.entity,
            Some(RulingEntityRef {
                kind: "ticket".to_owned(),
                id: "kan-t12".to_owned(),
            })
        );
    }
}
