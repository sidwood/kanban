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
    MissingReview,
    IncompleteReview,
    RejectedReview,
    ExpiredReview,
    ReviewWrongTip,
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
            Self::MissingReview => write!(
                f,
                "a criterion is satisfied only through a completed required-stage review at the bound tip"
            ),
            Self::IncompleteReview => {
                write!(f, "an incomplete review cannot satisfy a criterion")
            }
            Self::RejectedReview => write!(f, "a rejected review cannot satisfy a criterion"),
            Self::ExpiredReview => write!(f, "an expired review cannot satisfy a criterion"),
            Self::ReviewWrongTip => {
                write!(f, "a criterion is satisfied only at the reviewed code tip")
            }
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

/// The latest review execution for a Ticket, as satisfaction sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewExecutionState {
    Absent,
    Incomplete,
    Rejected,
    Expired,
    Approved,
}

/// Satisfaction is derived from a completed required-stage review at
/// the exact bound tip, never from a per-criterion evidence flag alone.
pub fn require_completed_required_stage_review(
    state: ReviewExecutionState,
    review_tip: Option<&str>,
    requested_tip: &str,
) -> Result<(), TipBindingError> {
    match state {
        ReviewExecutionState::Absent => Err(TipBindingError::MissingReview),
        ReviewExecutionState::Incomplete => Err(TipBindingError::IncompleteReview),
        ReviewExecutionState::Rejected => Err(TipBindingError::RejectedReview),
        ReviewExecutionState::Expired => Err(TipBindingError::ExpiredReview),
        ReviewExecutionState::Approved => match review_tip {
            Some(tip) if tip == requested_tip => Ok(()),
            _ => Err(TipBindingError::ReviewWrongTip),
        },
    }
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

/// Identities the current Workspace content presents to tip binding.
/// Dirty or unreadable content is untrusted and matches nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedContent {
    identities: Vec<String>,
    trusted: bool,
}

impl ReviewedContent {
    /// Clean, readable content presenting these commit and/or tree hashes.
    pub fn clean(identities: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            identities: identities.into_iter().map(Into::into).collect(),
            trusted: true,
        }
    }

    /// Dirty or unreadable content. Outstanding approvals must void
    /// even when a historical commit hash is still sitting on HEAD.
    pub fn untrusted(identities: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            identities: identities.into_iter().map(Into::into).collect(),
            trusted: false,
        }
    }

    /// Whether `tip` is one of the identities this content presents.
    pub fn matches(&self, tip: &str) -> bool {
        self.trusted && self.identities.iter().any(|identity| identity == tip)
    }
}

pub fn invalidate_on_content_change(binding: &mut CriterionBinding, observed: &ReviewedContent) {
    if !observed.matches(&binding.tip) {
        binding.void = true;
        binding.satisfied = false;
    }
}

/// Void a binding because the criterion it satisfied was replaced.
/// Count is not identity: a same-index replacement cannot keep the
/// earned review or satisfaction.
pub fn invalidate_on_criterion_replacement(binding: &mut CriterionBinding) {
    binding.void = true;
    binding.satisfied = false;
}

/// How many current landing criteria have a non-void satisfied binding
/// at `source_tip`. A matching binding count is not enough; each
/// current index must still be earned.
pub fn satisfied_landing_criteria(
    criteria: &[crate::AcceptanceCriterion],
    bindings: &[CriterionBinding],
    source_tip: &str,
) -> usize {
    criteria
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            bindings.iter().any(|binding| {
                binding.criterion_index() as usize == *index
                    && binding.satisfied()
                    && binding.tip() == source_tip
            })
        })
        .count()
}

/// A historical approval that content change already voided cannot
/// satisfy a criterion again.
pub fn refuse_spent_approval(spent: bool) -> Result<(), TipBindingError> {
    if spent {
        Err(TipBindingError::AlreadyVoid)
    } else {
        Ok(())
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
        CriterionKind, EvidenceReview, ReviewExecutionState, ReviewedContent, TipBindingError,
        attach_criterion_evidence, complete_task_criterion, complete_task_criterion_kind,
        invalidate_on_content_change, invalidate_on_criterion_replacement, refuse_spent_approval,
        require_completed_required_stage_review, review_criterion_evidence,
        satisfied_landing_criteria, satisfy_at_approved_tip,
    };

    fn approved_at(tip: &str) -> super::CriterionBinding {
        let mut binding = attach_criterion_evidence(CriterionKind::Acceptance, 0, 7, tip)
            .expect("implementers attach evidence to a criterion");
        review_criterion_evidence(&mut binding, EvidenceReview::Validated)
            .expect("reviewers validate attached evidence");
        satisfy_at_approved_tip(&mut binding, tip).unwrap();
        binding
    }

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

        invalidate_on_content_change(&mut binding, &ReviewedContent::clean(["b".repeat(40)]));

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
    fn tip_binding_requires_a_completed_required_stage_review() {
        let tip = "a".repeat(40);
        assert_eq!(
            require_completed_required_stage_review(ReviewExecutionState::Absent, None, &tip),
            Err(TipBindingError::MissingReview)
        );
        assert_eq!(
            require_completed_required_stage_review(
                ReviewExecutionState::Incomplete,
                Some(&tip),
                &tip
            ),
            Err(TipBindingError::IncompleteReview)
        );
        assert_eq!(
            require_completed_required_stage_review(
                ReviewExecutionState::Rejected,
                Some(&tip),
                &tip
            ),
            Err(TipBindingError::RejectedReview)
        );
        assert_eq!(
            require_completed_required_stage_review(
                ReviewExecutionState::Expired,
                Some(&tip),
                &tip
            ),
            Err(TipBindingError::ExpiredReview)
        );
        assert_eq!(
            require_completed_required_stage_review(
                ReviewExecutionState::Approved,
                Some(&"b".repeat(40)),
                &tip
            ),
            Err(TipBindingError::ReviewWrongTip)
        );
        assert!(
            require_completed_required_stage_review(
                ReviewExecutionState::Approved,
                Some(&tip),
                &tip
            )
            .is_ok(),
            "a completed required-stage review at the bound tip may satisfy"
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

    #[test]
    fn invalidation_keeps_a_commit_bound_approval_on_clean_unchanged_head() {
        let tip = "a".repeat(40);
        let mut binding = approved_at(&tip);

        invalidate_on_content_change(&mut binding, &ReviewedContent::clean([tip.clone()]));

        assert!(
            !binding.void(),
            "clean unchanged commit-bound content keeps the approval"
        );
        assert!(binding.satisfied());
    }

    #[test]
    fn invalidation_keeps_a_tree_bound_approval_on_the_owning_clean_commit() {
        let tree = "a".repeat(40);
        let commit = "b".repeat(40);
        let mut binding = approved_at(&tree);

        invalidate_on_content_change(
            &mut binding,
            &ReviewedContent::clean([commit, tree.clone()]),
        );

        assert!(
            !binding.void(),
            "the tree hash of the owning commit is a like-for-like identity"
        );
        assert!(binding.satisfied());
    }

    #[test]
    fn invalidation_voids_dirty_content_at_the_same_commit() {
        let tip = "a".repeat(40);
        let mut binding = approved_at(&tip);

        invalidate_on_content_change(&mut binding, &ReviewedContent::untrusted([tip]));

        assert!(
            binding.void(),
            "dirty content at the same HEAD voids the outstanding approval"
        );
        assert!(!binding.satisfied());
    }

    #[test]
    fn invalidation_voids_unreadable_content() {
        let mut binding = approved_at(&"a".repeat(40));

        invalidate_on_content_change(
            &mut binding,
            &ReviewedContent::untrusted(Vec::<String>::new()),
        );

        assert!(
            binding.void(),
            "unreadable content conservatively voids outstanding approvals"
        );
        assert!(!binding.satisfied());
    }

    #[test]
    fn criterion_replacement_voids_earned_satisfaction() {
        let tip = "a".repeat(40);
        let mut binding = approved_at(&tip);

        invalidate_on_criterion_replacement(&mut binding);

        assert!(binding.void());
        assert!(!binding.satisfied());
        assert_eq!(
            satisfy_at_approved_tip(&mut binding, &tip),
            Err(TipBindingError::AlreadyVoid),
            "the old binding cannot satisfy a replacement criterion"
        );
    }

    #[test]
    fn landing_satisfaction_does_not_treat_matching_counts_as_identity() {
        let tip = "a".repeat(40);
        let mut binding = approved_at(&tip);
        let criteria = [crate::AcceptanceCriterion::new(
            "The source tip is reviewed before the merge.",
            vec![
                crate::UserStoryRef::new(
                    crate::SpecNumber::new(1).expect("the Spec number is valid"),
                    7,
                )
                .expect("the story ordinal is valid"),
            ],
        )
        .expect("the replacement criterion is valid")];

        assert_eq!(
            satisfied_landing_criteria(&criteria, &[binding.clone()], &tip),
            1
        );
        invalidate_on_criterion_replacement(&mut binding);
        assert_eq!(
            satisfied_landing_criteria(&criteria, &[binding], &tip),
            0,
            "a voided same-index binding does not satisfy the current criterion"
        );
    }

    #[test]
    fn a_spent_historical_approval_cannot_satisfy_again() {
        assert_eq!(
            refuse_spent_approval(true),
            Err(TipBindingError::AlreadyVoid)
        );
        assert!(
            refuse_spent_approval(false).is_ok(),
            "a live approval may still satisfy"
        );
    }
}
