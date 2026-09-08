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
    Ok(())
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        LandingKind, LandingRefusal, LandingRequest, land_lane, land_seed, land_standalone_bug,
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
        };
        assert_eq!(
            land_standalone_bug(&request).unwrap_err(),
            LandingRefusal::SpecAttached
        );
    }
}
