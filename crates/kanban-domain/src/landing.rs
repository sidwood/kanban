//! Landing topology: Ticket Lanes land into a Spec integration
//! branch; a final integration review lands through the Seed; a
//! standalone Bug may land through the Seed when no active Spec is
//! attached.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingKind {
    Lane,
    Seed,
    StandaloneBug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingRefusal {
    WrongIntegrationBranch,
    SeedRequired,
    IntegrationReviewRequired,
    SpecAttached,
    UnguardedPath,
    TicketReviewRequired,
    CriteriaUnsatisfied,
    WalkthroughUnproven,
}

impl std::fmt::Display for LandingRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongIntegrationBranch => {
                write!(f, "ticket lanes land into the Spec integration branch")
            }
            Self::SeedRequired => write!(f, "this landing must go through the Seed Workspace"),
            Self::IntegrationReviewRequired => {
                write!(
                    f,
                    "a final Spec integration review must approve the combined result"
                )
            }
            Self::SpecAttached => {
                write!(
                    f,
                    "standalone Bugs land through the Seed only when no active Spec is attached"
                )
            }
            Self::UnguardedPath => write!(f, "landing refuses paths outside this topology"),
            Self::TicketReviewRequired => {
                write!(f, "landing requires a Ticket review of the source tip")
            }
            Self::CriteriaUnsatisfied => {
                write!(
                    f,
                    "landing requires every criterion to be satisfied at the source tip"
                )
            }
            Self::WalkthroughUnproven => write!(
                f,
                "landing requires a shell-minted walkthrough with captured artefacts at the source tip"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandingRequest {
    pub kind: LandingKind,
    pub from_branch: String,
    pub into_branch: String,
    pub integration_branch: String,
    pub through_seed: bool,
    pub spec_active: bool,
    pub integration_review_approved: bool,
    /// Git tip of the source Workspace (the Ticket Lane or Bug branch).
    pub source_tip: String,
    /// Whether a completed Ticket review approved that exact source tip.
    pub ticket_review_approved: bool,
    /// Tip the Ticket review bound, when one exists.
    pub ticket_reviewed_tip: Option<String>,
    /// How many Ticket criteria must be satisfied at the source tip.
    pub criterion_count: usize,
    /// How many of those criteria are satisfied at the source tip.
    pub criteria_satisfied_at_source: usize,
    /// Whether a shell-minted walkthrough run proved this Ticket at
    /// the source tip. File, git, and MCP claims do not set this.
    pub walkthrough_proven: bool,
}

pub fn land_lane(request: &LandingRequest) -> Result<(), LandingRefusal> {
    if request.kind != LandingKind::Lane {
        return Err(LandingRefusal::UnguardedPath);
    }
    if request.through_seed {
        return Err(LandingRefusal::UnguardedPath);
    }
    if request.into_branch != request.integration_branch {
        return Err(LandingRefusal::WrongIntegrationBranch);
    }
    require_reviewed_source(request)?;
    require_walkthrough_proof(request)
}

pub fn land_seed(request: &LandingRequest) -> Result<(), LandingRefusal> {
    if request.kind != LandingKind::Seed {
        return Err(LandingRefusal::UnguardedPath);
    }
    if !request.through_seed {
        return Err(LandingRefusal::SeedRequired);
    }
    if request.from_branch != request.integration_branch {
        return Err(LandingRefusal::UnguardedPath);
    }
    if !request.integration_review_approved {
        return Err(LandingRefusal::IntegrationReviewRequired);
    }
    Ok(())
}

pub fn land_standalone_bug(request: &LandingRequest) -> Result<(), LandingRefusal> {
    if request.kind != LandingKind::StandaloneBug {
        return Err(LandingRefusal::UnguardedPath);
    }
    if request.spec_active {
        return Err(LandingRefusal::SpecAttached);
    }
    if !request.through_seed {
        return Err(LandingRefusal::SeedRequired);
    }
    require_reviewed_source(request)?;
    require_walkthrough_proof(request)
}

/// Operator policy for reconciling a landing whose Git effect outlived
/// its durable outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingRecoveryPolicy {
    /// Persist the landing record for a Git merge that already succeeded.
    Complete,
    /// Drop the reservation when Git did not complete a merge.
    Release,
}

impl LandingRecoveryPolicy {
    /// The named ruling recorded for this policy.
    pub fn ruling_name(self) -> &'static str {
        match self {
            Self::Complete => "git_succeeded_before_durable_completion",
            Self::Release => "landing_reservation_release",
        }
    }
}

/// Git facts the recovery rule judges, with no I/O of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandingGitObservation {
    pub head: String,
    pub merge_in_progress: bool,
    pub parents: Vec<String>,
}

/// What the destination Workspace actually holds after the failed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LandingObservedOutcome {
    Merged { landed_tip: String },
    Conflicted,
    Unchanged,
    Diverged,
}

/// The durable effect the chosen policy is allowed to apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LandingRecoveryEffect {
    Complete { landed_tip: String },
    Release { abort_merge: bool },
}

/// Why recovery refused the operator's policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingRecoveryRefusal {
    PolicyDoesNotMatchOutcome,
}

impl std::fmt::Display for LandingRecoveryRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PolicyDoesNotMatchOutcome => write!(
                f,
                "the chosen landing recovery policy does not match the observed Git outcome"
            ),
        }
    }
}

/// Classify the destination Workspace against the pending landing draft.
pub fn observe_landing_git(
    from_tip: &str,
    into_tip: &str,
    observation: &LandingGitObservation,
) -> LandingObservedOutcome {
    if observation.merge_in_progress {
        return LandingObservedOutcome::Conflicted;
    }
    if observation.head == into_tip {
        return LandingObservedOutcome::Unchanged;
    }
    if observation.parents.len() == 2
        && observation.parents[0] == into_tip
        && observation.parents[1] == from_tip
    {
        return LandingObservedOutcome::Merged {
            landed_tip: observation.head.clone(),
        };
    }
    LandingObservedOutcome::Diverged
}

/// Apply the operator-chosen policy only when it matches the Git outcome.
pub fn reconcile_landing(
    outcome: &LandingObservedOutcome,
    policy: LandingRecoveryPolicy,
) -> Result<LandingRecoveryEffect, LandingRecoveryRefusal> {
    match (outcome, policy) {
        (LandingObservedOutcome::Merged { landed_tip }, LandingRecoveryPolicy::Complete) => {
            Ok(LandingRecoveryEffect::Complete {
                landed_tip: landed_tip.clone(),
            })
        }
        (LandingObservedOutcome::Conflicted, LandingRecoveryPolicy::Release) => {
            Ok(LandingRecoveryEffect::Release { abort_merge: true })
        }
        (LandingObservedOutcome::Unchanged, LandingRecoveryPolicy::Release) => {
            Ok(LandingRecoveryEffect::Release { abort_merge: false })
        }
        _ => Err(LandingRecoveryRefusal::PolicyDoesNotMatchOutcome),
    }
}

fn require_reviewed_source(request: &LandingRequest) -> Result<(), LandingRefusal> {
    if request.source_tip.is_empty()
        || !request.ticket_review_approved
        || request.ticket_reviewed_tip.as_deref() != Some(request.source_tip.as_str())
    {
        return Err(LandingRefusal::TicketReviewRequired);
    }
    if request.criteria_satisfied_at_source != request.criterion_count {
        return Err(LandingRefusal::CriteriaUnsatisfied);
    }
    Ok(())
}

fn require_walkthrough_proof(request: &LandingRequest) -> Result<(), LandingRefusal> {
    if request.walkthrough_proven {
        Ok(())
    } else {
        Err(LandingRefusal::WalkthroughUnproven)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LandingGitObservation, LandingKind, LandingObservedOutcome, LandingRecoveryEffect,
        LandingRecoveryPolicy, LandingRecoveryRefusal, LandingRefusal, LandingRequest, land_lane,
        land_seed, land_standalone_bug, observe_landing_git, reconcile_landing,
    };

    fn lane() -> LandingRequest {
        LandingRequest {
            kind: LandingKind::Lane,
            from_branch: "kan-t1".to_owned(),
            into_branch: "kan-s1".to_owned(),
            integration_branch: "kan-s1".to_owned(),
            through_seed: false,
            spec_active: true,
            integration_review_approved: true,
            source_tip: "a".repeat(40),
            ticket_review_approved: true,
            ticket_reviewed_tip: Some("a".repeat(40)),
            criterion_count: 1,
            criteria_satisfied_at_source: 1,
            walkthrough_proven: true,
        }
    }

    #[test]
    fn integration_landing_refuses_a_lane_that_does_not_target_the_spec_branch() {
        let mut request = lane();
        request.into_branch = "feature/other".to_owned();
        assert_eq!(
            land_lane(&request).unwrap_err(),
            LandingRefusal::WrongIntegrationBranch
        );
    }

    #[test]
    fn integration_landing_accepts_a_lane_that_targets_the_spec_branch() {
        land_lane(&lane()).expect("the lane lands into the Spec branch");
    }

    #[test]
    fn integration_landing_requires_an_approved_review_before_the_seed() {
        let request = LandingRequest {
            kind: LandingKind::Seed,
            from_branch: "kan-s1".to_owned(),
            into_branch: "main".to_owned(),
            integration_branch: "kan-s1".to_owned(),
            through_seed: true,
            spec_active: true,
            integration_review_approved: false,
            source_tip: String::new(),
            ticket_review_approved: false,
            ticket_reviewed_tip: None,
            criterion_count: 0,
            criteria_satisfied_at_source: 0,
            walkthrough_proven: true,
        };
        assert_eq!(
            land_seed(&request).unwrap_err(),
            LandingRefusal::IntegrationReviewRequired
        );
    }

    #[test]
    fn standalone_bug_landing_refuses_an_active_spec() {
        let request = LandingRequest {
            kind: LandingKind::StandaloneBug,
            from_branch: "kan-t1".to_owned(),
            into_branch: "main".to_owned(),
            integration_branch: String::new(),
            through_seed: true,
            spec_active: true,
            integration_review_approved: false,
            source_tip: "a".repeat(40),
            ticket_review_approved: true,
            ticket_reviewed_tip: Some("a".repeat(40)),
            criterion_count: 0,
            criteria_satisfied_at_source: 0,
            walkthrough_proven: true,
        };
        assert_eq!(
            land_standalone_bug(&request).unwrap_err(),
            LandingRefusal::SpecAttached
        );
    }

    #[test]
    fn ordinary_landing_requires_ticket_review_at_the_source_tip() {
        let mut request = lane();
        request.ticket_review_approved = false;
        assert_eq!(
            land_lane(&request).unwrap_err(),
            LandingRefusal::TicketReviewRequired
        );

        let mut bug = LandingRequest {
            kind: LandingKind::StandaloneBug,
            from_branch: "kan-t2".to_owned(),
            into_branch: "main".to_owned(),
            integration_branch: String::new(),
            through_seed: true,
            spec_active: false,
            integration_review_approved: false,
            source_tip: "a".repeat(40),
            ticket_review_approved: true,
            ticket_reviewed_tip: Some("b".repeat(40)),
            criterion_count: 0,
            criteria_satisfied_at_source: 0,
            walkthrough_proven: true,
        };
        assert_eq!(
            land_standalone_bug(&bug).unwrap_err(),
            LandingRefusal::TicketReviewRequired
        );
        bug.ticket_reviewed_tip = Some(bug.source_tip.clone());
        bug.criterion_count = 1;
        bug.criteria_satisfied_at_source = 1;
        land_standalone_bug(&bug).expect("a reviewed standalone Bug may land");
    }

    #[test]
    fn ordinary_landing_requires_satisfied_criteria_at_the_source_tip() {
        let mut request = lane();
        request.criteria_satisfied_at_source = 0;
        assert_eq!(
            land_lane(&request).unwrap_err(),
            LandingRefusal::CriteriaUnsatisfied
        );
    }

    #[test]
    fn standalone_bug_landing_requires_satisfied_qualification_criteria() {
        let mut request = LandingRequest {
            kind: LandingKind::StandaloneBug,
            from_branch: "kan-t2".to_owned(),
            into_branch: "main".to_owned(),
            integration_branch: String::new(),
            through_seed: true,
            spec_active: false,
            integration_review_approved: false,
            source_tip: "a".repeat(40),
            ticket_review_approved: true,
            ticket_reviewed_tip: Some("a".repeat(40)),
            criterion_count: 1,
            criteria_satisfied_at_source: 0,
            walkthrough_proven: true,
        };
        assert_eq!(
            land_standalone_bug(&request).unwrap_err(),
            LandingRefusal::CriteriaUnsatisfied
        );
        request.criteria_satisfied_at_source = 1;
        land_standalone_bug(&request).expect("satisfied qualification criteria may land");
    }

    #[test]
    fn landing_refuses_a_ticket_without_shell_minted_walkthrough_proof() {
        let mut request = lane();
        request.walkthrough_proven = false;
        assert_eq!(
            land_lane(&request).unwrap_err(),
            LandingRefusal::WalkthroughUnproven
        );
        request.walkthrough_proven = true;
        land_lane(&request).expect("a proven walkthrough may land the lane");
    }

    fn observation(head: &str, merge_in_progress: bool, parents: &[&str]) -> LandingGitObservation {
        LandingGitObservation {
            head: head.to_owned(),
            merge_in_progress,
            parents: parents.iter().map(|parent| (*parent).to_owned()).collect(),
        }
    }

    #[test]
    fn landing_recovery_completes_a_no_ff_merge_that_outlived_its_outcome() {
        let from = "a".repeat(40);
        let into = "b".repeat(40);
        let landed = "c".repeat(40);
        let outcome =
            observe_landing_git(&from, &into, &observation(&landed, false, &[&into, &from]));
        assert_eq!(
            outcome,
            LandingObservedOutcome::Merged {
                landed_tip: landed.clone()
            }
        );
        assert_eq!(
            reconcile_landing(&outcome, LandingRecoveryPolicy::Complete).unwrap(),
            LandingRecoveryEffect::Complete {
                landed_tip: landed.clone()
            }
        );
        assert_eq!(
            LandingRecoveryPolicy::Complete.ruling_name(),
            "git_succeeded_before_durable_completion"
        );
        assert_eq!(
            reconcile_landing(&outcome, LandingRecoveryPolicy::Release).unwrap_err(),
            LandingRecoveryRefusal::PolicyDoesNotMatchOutcome
        );
    }

    #[test]
    fn landing_recovery_releases_a_conflicted_or_aborted_merge() {
        let from = "a".repeat(40);
        let into = "b".repeat(40);
        let conflicted = observe_landing_git(&from, &into, &observation(&into, true, &[&into]));
        assert_eq!(conflicted, LandingObservedOutcome::Conflicted);
        assert_eq!(
            reconcile_landing(&conflicted, LandingRecoveryPolicy::Release).unwrap(),
            LandingRecoveryEffect::Release { abort_merge: true }
        );
        let aborted = observe_landing_git(&from, &into, &observation(&into, false, &[&into]));
        assert_eq!(aborted, LandingObservedOutcome::Unchanged);
        assert_eq!(
            reconcile_landing(&aborted, LandingRecoveryPolicy::Release).unwrap(),
            LandingRecoveryEffect::Release { abort_merge: false }
        );
        assert_eq!(
            LandingRecoveryPolicy::Release.ruling_name(),
            "landing_reservation_release"
        );
        assert_eq!(
            reconcile_landing(&conflicted, LandingRecoveryPolicy::Complete).unwrap_err(),
            LandingRecoveryRefusal::PolicyDoesNotMatchOutcome
        );
    }
}
