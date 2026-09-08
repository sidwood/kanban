//! Verdicts and criterion satisfaction bind to one reviewed code tip.
//! Content changes void outstanding approvals; history is preserved
//! elsewhere.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriterionKind {
    Acceptance,
    Task,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceReview {
    Pending,
    Validated,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipBindingError {
    WrongTip,
    UnvalidatedEvidence,
    RejectedEvidence,
    AlreadyVoid,
    NotATask,
}

impl std::fmt::Display for TipBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongTip => write!(f, "a criterion is satisfied only at the bound code tip"),
            Self::UnvalidatedEvidence => {
                write!(f, "reviewers must validate evidence before satisfaction")
            }
            Self::RejectedEvidence => write!(f, "rejected evidence cannot satisfy a criterion"),
            Self::AlreadyVoid => {
                write!(f, "a content change voided the outstanding approval")
            }
            Self::NotATask => write!(f, "only humans complete Task criteria directly"),
        }
    }
}
impl std::error::Error for TipBindingError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriterionBinding {
    kind: CriterionKind,
    criterion_index: u64,
    evidence_id: u64,
    tip: String,
    review: EvidenceReview,
    satisfied: bool,
    void: bool,
}

impl CriterionBinding {
    pub fn kind(&self) -> CriterionKind {
        self.kind
    }

    pub fn criterion_index(&self) -> u64 {
        self.criterion_index
    }

    pub fn evidence_id(&self) -> u64 {
        self.evidence_id
    }

    pub fn tip(&self) -> &str {
        &self.tip
    }

    pub fn review(&self) -> EvidenceReview {
        self.review
    }

    pub fn satisfied(&self) -> bool {
        self.satisfied && !self.void
    }

    pub fn void(&self) -> bool {
        self.void
    }

    pub fn restore(
        kind: CriterionKind,
        criterion_index: u64,
        evidence_id: u64,
        tip: impl Into<String>,
        review: EvidenceReview,
        satisfied: bool,
        void: bool,
    ) -> Self {
        Self {
            kind,
            criterion_index,
            evidence_id,
            tip: tip.into(),
            review,
            satisfied,
            void,
        }
    }
}

pub fn attach_criterion_evidence(
    kind: CriterionKind,
    criterion_index: u64,
    evidence_id: u64,
    tip: impl Into<String>,
) -> Result<CriterionBinding, TipBindingError> {
    Ok(CriterionBinding {
        kind,
        criterion_index,
        evidence_id,
        tip: tip.into(),
        review: EvidenceReview::Pending,
        satisfied: false,
        void: false,
    })
}

pub fn review_criterion_evidence(
    binding: &mut CriterionBinding,
    review: EvidenceReview,
) -> Result<(), TipBindingError> {
    if binding.void {
        return Err(TipBindingError::AlreadyVoid);
    }
    binding.review = review;
    if review != EvidenceReview::Validated {
        binding.satisfied = false;
    }
    Ok(())
}

pub fn satisfy_at_approved_tip(
    binding: &mut CriterionBinding,
    tip: &str,
) -> Result<bool, TipBindingError> {
    if binding.void {
        return Err(TipBindingError::AlreadyVoid);
    }
    if binding.review == EvidenceReview::Rejected {
        return Err(TipBindingError::RejectedEvidence);
    }
    if binding.review != EvidenceReview::Validated {
        return Err(TipBindingError::UnvalidatedEvidence);
    }
    if tip != binding.tip {
        return Err(TipBindingError::WrongTip);
    }
    binding.satisfied = true;
    Ok(true)
}

pub fn invalidate_on_content_change(binding: &mut CriterionBinding, observed_tip: &str) {
    if observed_tip != binding.tip {
        binding.void = true;
        binding.satisfied = false;
    }
}

pub fn complete_task_criterion(criterion_index: u64) -> Result<CriterionBinding, TipBindingError> {
    complete_task_criterion_kind(CriterionKind::Task, criterion_index)
}

pub fn complete_task_criterion_kind(
    kind: CriterionKind,
    criterion_index: u64,
) -> Result<CriterionBinding, TipBindingError> {
    if kind != CriterionKind::Task {
        return Err(TipBindingError::NotATask);
    }
    Ok(CriterionBinding {
        kind,
        criterion_index,
        evidence_id: 0,
        tip: String::new(),
        review: EvidenceReview::Validated,
        satisfied: true,
        void: false,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CriterionKind, EvidenceReview, TipBindingError, attach_criterion_evidence,
        complete_task_criterion, complete_task_criterion_kind, invalidate_on_content_change,
        review_criterion_evidence, satisfy_at_approved_tip,
    };

    #[test]
    fn tip_binding_satisfies_a_criterion_only_at_the_approved_tip() {
        let mut binding =
            attach_criterion_evidence(CriterionKind::Acceptance, 0, 7, "a".repeat(40))
                .expect("implementers attach evidence to a criterion");
        review_criterion_evidence(&mut binding, EvidenceReview::Validated)
            .expect("reviewers validate attached evidence");

        assert!(
            satisfy_at_approved_tip(&mut binding, &"a".repeat(40)).unwrap(),
            "approval of the exact code tip satisfies the criterion"
        );
        assert!(
            satisfy_at_approved_tip(&mut binding, &"b".repeat(40)).is_err(),
            "a different tip cannot satisfy the bound criterion"
        );
    }

    #[test]
    fn invalidation_voids_outstanding_approvals_when_the_reviewed_tree_changes() {
        let mut binding =
            attach_criterion_evidence(CriterionKind::Acceptance, 0, 7, "a".repeat(40))
                .expect("implementers attach evidence to a criterion");
        review_criterion_evidence(&mut binding, EvidenceReview::Validated)
            .expect("reviewers validate attached evidence");
        satisfy_at_approved_tip(&mut binding, &"a".repeat(40)).unwrap();

        invalidate_on_content_change(&mut binding, &"b".repeat(40));

        assert!(
            binding.void(),
            "content change voids the outstanding approval"
        );
        assert!(
            !binding.satisfied(),
            "a voided approval no longer satisfies"
        );
        assert_eq!(
            satisfy_at_approved_tip(&mut binding, &"a".repeat(40)),
            Err(TipBindingError::AlreadyVoid)
        );
    }

    #[test]
    fn humans_may_complete_task_criteria_directly() {
        let binding = complete_task_criterion(0).expect("humans complete Task criteria");
        assert_eq!(binding.kind(), CriterionKind::Task);
        assert!(
            binding.satisfied(),
            "direct completion satisfies the Task criterion"
        );
        assert!(
            complete_task_criterion_kind(CriterionKind::Acceptance, 0).is_err(),
            "Acceptance Criteria are not completed by humans"
        );
    }
}
