//! Ordered stage resolution. Telemetry is never a review verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewStageResolution {
    Waiting,
    Approved,
    Rejected,
}

pub struct ReviewVote<'a> {
    pub required: bool,
    pub verdict: Option<(&'a str, bool)>,
}

/// Optional slots never hold a stage open. A completed veto still matters
/// when the required slots resolve together, and all votes bind one tip.
pub fn resolve_review_stage(tip: &str, slots: &[ReviewVote<'_>]) -> ReviewStageResolution {
    if slots
        .iter()
        .any(|slot| slot.required && slot.verdict.is_none())
    {
        return ReviewStageResolution::Waiting;
    }
    if slots.iter().any(|slot| {
        slot.verdict
            .is_some_and(|(reviewed, approved)| !approved || reviewed != tip)
    }) {
        return ReviewStageResolution::Rejected;
    }
    ReviewStageResolution::Approved
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewSequenceResolution {
    Waiting(usize),
    Approved,
    Rejected,
}

pub fn resolve_review_sequence(stages: &[ReviewStageResolution]) -> ReviewSequenceResolution {
    for (index, stage) in stages.iter().enumerate() {
        match stage {
            ReviewStageResolution::Waiting => return ReviewSequenceResolution::Waiting(index),
            ReviewStageResolution::Rejected => return ReviewSequenceResolution::Rejected,
            ReviewStageResolution::Approved => {}
        }
    }
    ReviewSequenceResolution::Approved
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stage_resolution_waits_for_every_required_slot() {
        let votes = [
            ReviewVote {
                required: true,
                verdict: Some(("tip", false)),
            },
            ReviewVote {
                required: true,
                verdict: None,
            },
        ];
        assert_eq!(
            resolve_review_stage("tip", &votes),
            ReviewStageResolution::Waiting
        );
    }
    #[test]
    fn stage_resolution_does_not_wait_for_optional_slots() {
        let votes = [
            ReviewVote {
                required: true,
                verdict: Some(("tip", true)),
            },
            ReviewVote {
                required: false,
                verdict: None,
            },
        ];
        assert_eq!(
            resolve_review_stage("tip", &votes),
            ReviewStageResolution::Approved
        );
    }
    #[test]
    fn same_tip_is_required_even_when_every_vote_approves() {
        let votes = [
            ReviewVote {
                required: true,
                verdict: Some(("first", true)),
            },
            ReviewVote {
                required: true,
                verdict: Some(("second", true)),
            },
        ];
        assert_eq!(
            resolve_review_stage("first", &votes),
            ReviewStageResolution::Rejected
        );
    }
    #[test]
    fn bounce_resolves_only_after_parallel_votes_are_complete() {
        let votes = [
            ReviewVote {
                required: true,
                verdict: Some(("tip", false)),
            },
            ReviewVote {
                required: true,
                verdict: Some(("tip", true)),
            },
        ];
        assert_eq!(
            resolve_review_stage("tip", &votes),
            ReviewStageResolution::Rejected
        );
    }
    #[test]
    fn stage_resolution_advances_only_through_approved_predecessors() {
        assert_eq!(
            resolve_review_sequence(&[
                ReviewStageResolution::Approved,
                ReviewStageResolution::Waiting
            ]),
            ReviewSequenceResolution::Waiting(1)
        );
        assert_eq!(
            resolve_review_sequence(&[
                ReviewStageResolution::Approved,
                ReviewStageResolution::Rejected
            ]),
            ReviewSequenceResolution::Rejected
        );
        assert_eq!(
            resolve_review_sequence(&[
                ReviewStageResolution::Approved,
                ReviewStageResolution::Approved
            ]),
            ReviewSequenceResolution::Approved
        );
    }
}
