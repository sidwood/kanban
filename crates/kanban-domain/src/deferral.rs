//! Immutable records that a finding was deliberately not acted on
//! (CONTEXT.md, DR-AE-03). Deferrals are superseded explicitly and
//! never edited.

use std::fmt;

/// The identity of one deferral. Assigned once by storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeferralId(u64);

impl DeferralId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for DeferralId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a deferral payload was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferralError {
    /// The project identity was blank.
    BlankProject,
    /// The finding identity was blank.
    BlankFinding,
    /// The reason held nothing but whitespace.
    BlankReason,
}

impl fmt::Display for DeferralError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankProject => write!(f, "a deferral must name its project"),
            Self::BlankFinding => write!(f, "a deferral must name its finding"),
            Self::BlankReason => write!(f, "a deferral reason cannot be blank"),
        }
    }
}

impl std::error::Error for DeferralError {}

/// A validated deferral reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferralReason(String);

impl DeferralReason {
    /// Accept any reason that holds at least one non-whitespace
    /// character.
    pub fn new(raw: &str) -> Result<Self, DeferralError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DeferralError::BlankReason);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The trimmed reason.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One immutable deferral as stored and served to clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deferral {
    id: DeferralId,
    project_id: String,
    finding_id: String,
    reason: DeferralReason,
    supersedes: Option<DeferralId>,
    recorded_at: String,
}

impl Deferral {
    /// Restore a deferral that storage already persisted.
    pub fn restore(
        id: DeferralId,
        project_id: String,
        finding_id: String,
        reason: DeferralReason,
        supersedes: Option<DeferralId>,
        recorded_at: String,
    ) -> Self {
        Self {
            id,
            project_id,
            finding_id,
            reason,
            supersedes,
            recorded_at,
        }
    }

    /// Validate a fresh deferral before storage assigns its identity.
    pub fn record(
        project_id: &str,
        finding_id: &str,
        reason: DeferralReason,
    ) -> Result<DeferralDraft, DeferralError> {
        Ok(DeferralDraft {
            project_id: validate_project(project_id)?,
            finding_id: validate_finding(finding_id)?,
            reason,
            supersedes: None,
        })
    }

    /// Validate a superseding deferral that references this record.
    pub fn supersede(&self, reason: DeferralReason) -> DeferralDraft {
        DeferralDraft {
            project_id: self.project_id.clone(),
            finding_id: self.finding_id.clone(),
            reason,
            supersedes: Some(self.id),
        }
    }

    /// The storage-assigned identity.
    pub fn id(&self) -> DeferralId {
        self.id
    }

    /// The project the deferral belongs to.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// The finding that was deferred.
    pub fn finding_id(&self) -> &str {
        &self.finding_id
    }

    /// Why the finding was deferred.
    pub fn reason(&self) -> &DeferralReason {
        &self.reason
    }

    /// The deferral this one supersedes, if any.
    pub fn supersedes(&self) -> Option<DeferralId> {
        self.supersedes
    }

    /// When the deferral was recorded.
    pub fn recorded_at(&self) -> &str {
        &self.recorded_at
    }
}

/// A validated deferral waiting for storage to assign its identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferralDraft {
    pub project_id: String,
    pub finding_id: String,
    pub reason: DeferralReason,
    pub supersedes: Option<DeferralId>,
}

fn validate_project(project_id: &str) -> Result<String, DeferralError> {
    let trimmed = project_id.trim();
    if trimmed.is_empty() {
        return Err(DeferralError::BlankProject);
    }
    Ok(trimmed.to_owned())
}

fn validate_finding(finding_id: &str) -> Result<String, DeferralError> {
    let trimmed = finding_id.trim();
    if trimmed.is_empty() {
        return Err(DeferralError::BlankFinding);
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{Deferral, DeferralError, DeferralId, DeferralReason};

    #[test]
    fn recording_rejects_blank_reasons() {
        let error = DeferralReason::new("   ").expect_err("blank reasons are refused");
        assert_eq!(error, DeferralError::BlankReason);
    }

    #[test]
    fn recording_rejects_blank_findings() {
        let reason = DeferralReason::new("Out of scope for this plan").expect("reason validates");
        let error = Deferral::record("kan", "  ", reason).expect_err("blank findings are refused");
        assert_eq!(error, DeferralError::BlankFinding);
    }

    #[test]
    fn superseding_creates_a_new_draft_referencing_the_original() {
        let original = Deferral::restore(
            DeferralId::new(1),
            "kan".to_owned(),
            "finding-1".to_owned(),
            DeferralReason::new("Cosmetic only").expect("reason validates"),
            None,
            "2026-09-04T12:00:01Z".to_owned(),
        );
        let replacement = original.supersede(
            DeferralReason::new("Accepted risk for this release").expect("reason validates"),
        );

        assert_eq!(replacement.project_id, "kan");
        assert_eq!(replacement.finding_id, "finding-1");
        assert_eq!(replacement.supersedes, Some(DeferralId::new(1)));
        assert_eq!(
            replacement.reason.as_str(),
            "Accepted risk for this release"
        );
    }
}
