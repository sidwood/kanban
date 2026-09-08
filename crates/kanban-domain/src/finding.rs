//! Finding requirements are domain rules, not reviewer opinion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingError {
    BlankDetail(&'static str),
    BlockingFinding,
    RejectionWithoutBlocker,
    InvalidIdentity,
}
impl std::fmt::Display for FindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlankDetail(field) => write!(f, "a finding requires a non-empty {field}"),
            Self::BlockingFinding => write!(
                f,
                "in-scope P0 to P2 findings must be resolved before approval"
            ),
            Self::InvalidIdentity => write!(
                f,
                "a finding identity must be canonical and name a review, slot and index"
            ),
            Self::RejectionWithoutBlocker => write!(
                f,
                "a blocking rejection requires an in-scope P0 to P2 finding"
            ),
        }
    }
}
impl std::error::Error for FindingError {}

pub fn validate_finding_details(
    summary: &str,
    evidence: &str,
    location: &str,
    proposed_resolution: &str,
) -> Result<(), FindingError> {
    for (field, value) in [
        ("summary", summary),
        ("evidence", evidence),
        ("location", location),
        ("proposed resolution", proposed_resolution),
    ] {
        if value.trim().is_empty() {
            return Err(FindingError::BlankDetail(field));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingSeverity {
    P0,
    P1,
    P2,
    P3,
}

pub fn finding_blocks(severity: FindingSeverity, in_scope: bool) -> bool {
    in_scope && severity != FindingSeverity::P3
}

pub fn validate_finding_verdict(
    approve: bool,
    blocking: bool,
    counts_for_resolution: bool,
) -> Result<(), FindingError> {
    if !counts_for_resolution {
        return Ok(());
    }
    if approve && blocking {
        return Err(FindingError::BlockingFinding);
    }
    if !approve && !blocking {
        return Err(FindingError::RejectionWithoutBlocker);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finding_rules_require_every_narrative_field() {
        assert_eq!(
            validate_finding_details(" ", "proof", "file", "fix"),
            Err(FindingError::BlankDetail("summary"))
        );
        assert!(validate_finding_details("summary", "proof", "file", "fix").is_ok());
    }
    #[test]
    fn blocking_requires_scope_and_p0_to_p2_severity() {
        for severity in [
            FindingSeverity::P0,
            FindingSeverity::P1,
            FindingSeverity::P2,
        ] {
            assert!(finding_blocks(severity, true));
            assert!(!finding_blocks(severity, false));
        }
        assert!(!finding_blocks(FindingSeverity::P3, true));
        assert!(!finding_blocks(FindingSeverity::P3, false));
    }
    #[test]
    fn finding_rules_preserve_verdict_intent_instead_of_silently_rewriting_it() {
        assert_eq!(
            validate_finding_verdict(true, true, true),
            Err(FindingError::BlockingFinding)
        );
        assert_eq!(
            validate_finding_verdict(false, false, true),
            Err(FindingError::RejectionWithoutBlocker)
        );
        assert!(validate_finding_verdict(false, false, false).is_ok());
    }
}

/// A stable reference into an immutable slot verdict, never a mutable list position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindingIdentity {
    review: u64,
    slot: u64,
    index: u64,
}
impl FindingIdentity {
    pub fn new(review: u64, slot: u64, index: u64) -> Result<Self, FindingError> {
        if review == 0
            || slot == 0
            || review > i64::MAX as u64
            || slot > i64::MAX as u64
            || index > i64::MAX as u64
        {
            return Err(FindingError::InvalidIdentity);
        }
        Ok(Self {
            review,
            slot,
            index,
        })
    }
    pub fn parse(raw: &str) -> Result<Self, FindingError> {
        let parts: Vec<_> = raw.split(':').collect();
        let ["review", review, "slot", slot, "finding", index] = parts.as_slice() else {
            return Err(FindingError::InvalidIdentity);
        };
        let id = Self::new(
            review.parse().map_err(|_| FindingError::InvalidIdentity)?,
            slot.parse().map_err(|_| FindingError::InvalidIdentity)?,
            index.parse().map_err(|_| FindingError::InvalidIdentity)?,
        )?;
        if id.to_string() != raw {
            return Err(FindingError::InvalidIdentity);
        }
        Ok(id)
    }
    pub fn review(self) -> u64 {
        self.review
    }
    pub fn slot(self) -> u64 {
        self.slot
    }
    pub fn index(self) -> u64 {
        self.index
    }
}
impl std::fmt::Display for FindingIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "review:{}:slot:{}:finding:{}",
            self.review, self.slot, self.index
        )
    }
}
