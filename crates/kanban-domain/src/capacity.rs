//! Capacity: the global defaults and stricter per-Project caps that
//! constrain active runs by harness, model family, and usage pool,
//! plus a per-Project maximum active Lane count (CONTEXT.md,
//! DR-EP-06, DR-EP-07). Evaluation is pure: it reads the active
//! runs, the global defaults, the Project's caps, and the Project's
//! active Lane count as values, and answers whether one more run
//! fits. Dispatch consumes the evaluation (KAN-T42); nothing here
//! claims a slot or queues a request.

use std::fmt;

use crate::project::ProjectId;

/// Why a capacity limit was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityError {
    /// A limit of zero would refuse every run forever. The value
    /// names the dimension.
    ZeroLimit(&'static str),
    /// A Project cap sits above the global default on the same
    /// dimension: Project limits never relax global ones.
    RelaxesGlobal {
        /// The dimension the cap constrains.
        dimension: &'static str,
        /// The refused Project cap.
        cap: u64,
        /// The global default the cap tried to exceed.
        global: u64,
    },
}

impl fmt::Display for CapacityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLimit(dimension) => {
                write!(f, "a {dimension} capacity limit must be greater than zero")
            }
            Self::RelaxesGlobal {
                dimension,
                cap,
                global,
            } => write!(
                f,
                "a Project {dimension} limit of {cap} would relax the global {global}"
            ),
        }
    }
}

impl std::error::Error for CapacityError {}

/// The global capacity defaults: the maximum active runs one harness,
/// model family, or usage pool may carry across every Project
/// (DR-EP-06). Each dimension is a real quota the Operator pays for,
/// so a limit of zero is refused rather than silently grounding all
/// dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalCapacity {
    max_active_per_harness: u64,
    max_active_per_model: u64,
    max_active_per_usage_pool: u64,
}

impl GlobalCapacity {
    /// Assemble the defaults, refusing a zero limit on any dimension.
    pub fn new(
        max_active_per_harness: u64,
        max_active_per_model: u64,
        max_active_per_usage_pool: u64,
    ) -> Result<Self, CapacityError> {
        positive(max_active_per_harness, "harness")?;
        positive(max_active_per_model, "model family")?;
        positive(max_active_per_usage_pool, "usage pool")?;
        Ok(Self {
            max_active_per_harness,
            max_active_per_model,
            max_active_per_usage_pool,
        })
    }

    /// Rehydrate stored defaults exactly as they were recorded.
    pub fn restore(
        max_active_per_harness: u64,
        max_active_per_model: u64,
        max_active_per_usage_pool: u64,
    ) -> Self {
        Self {
            max_active_per_harness,
            max_active_per_model,
            max_active_per_usage_pool,
        }
    }

    /// The most active runs one harness family may carry.
    pub fn max_active_per_harness(self) -> u64 {
        self.max_active_per_harness
    }

    /// The most active runs one model family may carry.
    pub fn max_active_per_model(self) -> u64 {
        self.max_active_per_model
    }

    /// The most active runs one usage pool may carry.
    pub fn max_active_per_usage_pool(self) -> u64 {
        self.max_active_per_usage_pool
    }
}

/// The stricter caps one Project may impose (DR-EP-07): lower
/// ceilings on the same three dimensions as the global defaults,
/// plus a maximum active Lane count the globals carry no counterpart
/// for. An absent cap constrains nothing, and a set cap never
/// relaxes the global default on its dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProjectCapacity {
    max_active_per_harness: Option<u64>,
    max_active_per_model: Option<u64>,
    max_active_per_usage_pool: Option<u64>,
    max_active_lanes: Option<u64>,
}

impl ProjectCapacity {
    /// Caps for a Project that imposes nothing: every global default
    /// applies unchanged.
    pub fn unset() -> Self {
        Self::default()
    }

    /// Assemble caps against the global defaults they are judged
    /// with, refusing a zero limit and any cap above the global
    /// default on the same dimension.
    pub fn new(
        global: &GlobalCapacity,
        max_active_per_harness: Option<u64>,
        max_active_per_model: Option<u64>,
        max_active_per_usage_pool: Option<u64>,
        max_active_lanes: Option<u64>,
    ) -> Result<Self, CapacityError> {
        let max_active_per_harness = at_most(
            max_active_per_harness,
            global.max_active_per_harness(),
            "harness",
        )?;
        let max_active_per_model = at_most(
            max_active_per_model,
            global.max_active_per_model(),
            "model family",
        )?;
        let max_active_per_usage_pool = at_most(
            max_active_per_usage_pool,
            global.max_active_per_usage_pool(),
            "usage pool",
        )?;
        if let Some(lanes) = max_active_lanes {
            positive(lanes, "active Lane")?;
        }
        Ok(Self {
            max_active_per_harness,
            max_active_per_model,
            max_active_per_usage_pool,
            max_active_lanes,
        })
    }

    /// Rehydrate stored caps exactly as they were recorded. The
    /// evaluation defends the never-relax rule itself, so restored
    /// values are honoured as stored rather than re-refused here.
    pub fn restore(
        max_active_per_harness: Option<u64>,
        max_active_per_model: Option<u64>,
        max_active_per_usage_pool: Option<u64>,
        max_active_lanes: Option<u64>,
    ) -> Self {
        Self {
            max_active_per_harness,
            max_active_per_model,
            max_active_per_usage_pool,
            max_active_lanes,
        }
    }

    /// The Project's harness ceiling, when it imposes one.
    pub fn max_active_per_harness(self) -> Option<u64> {
        self.max_active_per_harness
    }

    /// The Project's model family ceiling, when it imposes one.
    pub fn max_active_per_model(self) -> Option<u64> {
        self.max_active_per_model
    }

    /// The Project's usage pool ceiling, when it imposes one.
    pub fn max_active_per_usage_pool(self) -> Option<u64> {
        self.max_active_per_usage_pool
    }

    /// The Project's maximum active Lane count, when it imposes one.
    pub fn max_active_lanes(self) -> Option<u64> {
        self.max_active_lanes
    }

    /// Whether the Project imposes no cap at all.
    pub fn is_unset(self) -> bool {
        self == Self::unset()
    }
}

/// One run the evaluation counts: the Project it belongs to and the
/// three families it draws on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveRun<'a> {
    /// The Project the run belongs to.
    pub project: ProjectId,
    /// The harness family the run executes in.
    pub harness: &'a str,
    /// The model family the run executes with.
    pub model: &'a str,
    /// The usage pool the run draws from.
    pub usage_pool: &'a str,
}

/// Everything one capacity evaluation reads: the candidate run
/// asking for room (never itself part of `active`), every active run
/// across Projects, the candidate Project's active Lane count, the
/// global defaults, and the candidate Project's caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapacityInputs<'a> {
    /// The run asking for capacity.
    pub candidate: ActiveRun<'a>,
    /// Every active run across Projects, the candidate excluded.
    pub active: &'a [ActiveRun<'a>],
    /// The candidate Project's count of active Lanes.
    pub active_lanes: u64,
    /// The global capacity defaults.
    pub defaults: GlobalCapacity,
    /// The candidate Project's caps, when it imposes any.
    pub project_caps: Option<ProjectCapacity>,
}

/// Why one more run does not fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapacityRefusal {
    /// Active runs on the harness already meet the cap. The cap names
    /// the quota that refused: a value below the global default is a
    /// Project cap.
    HarnessExhausted {
        /// The harness family at its cap.
        harness: String,
        /// The active runs already sharing the family.
        active: u64,
        /// The cap the family hit.
        cap: u64,
    },
    /// Active runs on the model family already meet the cap.
    ModelExhausted {
        /// The model family at its cap.
        model: String,
        /// The active runs already sharing the family.
        active: u64,
        /// The cap the family hit.
        cap: u64,
    },
    /// Active runs in the usage pool already meet the cap.
    UsagePoolExhausted {
        /// The usage pool at its cap.
        usage_pool: String,
        /// The active runs already drawing from the pool.
        active: u64,
        /// The cap the pool hit.
        cap: u64,
    },
    /// The Project's active Lanes already meet the maximum count.
    LaneExhausted {
        /// The Project's active Lanes.
        active: u64,
        /// The maximum active Lane count.
        cap: u64,
    },
}

impl fmt::Display for CapacityRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HarnessExhausted {
                harness,
                active,
                cap,
            } => write!(
                f,
                "{active} active runs on harness `{harness}` already meet the cap {cap}"
            ),
            Self::ModelExhausted { model, active, cap } => write!(
                f,
                "{active} active runs on model family `{model}` already meet the cap {cap}"
            ),
            Self::UsagePoolExhausted {
                usage_pool,
                active,
                cap,
            } => write!(
                f,
                "{active} active runs in usage pool `{usage_pool}` already meet the cap {cap}"
            ),
            Self::LaneExhausted { active, cap } => {
                write!(f, "{active} active Lanes already meet the maximum {cap}")
            }
        }
    }
}

impl std::error::Error for CapacityRefusal {}

/// Whether one more run fits (DR-EP-06, DR-EP-07). Pure: nothing is
/// claimed, queued, or recorded. Dimensions are judged in the fixed
/// order harness, model family, usage pool, Lanes; within a run
/// dimension the global quota — which spans every Project — refuses
/// before the Project's stricter cap, which counts only the
/// candidate Project's own runs. A Project cap never relaxes the
/// global quota: writes refuse relaxing values, and the evaluation
/// itself clamps every Project cap to the global default, so even a
/// restored value above the default cannot widen a quota.
pub fn evaluate_capacity(inputs: &CapacityInputs<'_>) -> Result<(), CapacityRefusal> {
    let candidate = &inputs.candidate;
    let global = inputs.defaults;

    // The global quotas span every Project: each counts the active
    // runs sharing the candidate's family plus the candidate itself.
    let harness_everywhere = sharing(inputs.active, |run| run.harness == candidate.harness);
    if harness_everywhere + 1 > global.max_active_per_harness() {
        return Err(CapacityRefusal::HarnessExhausted {
            harness: candidate.harness.to_owned(),
            active: harness_everywhere,
            cap: global.max_active_per_harness(),
        });
    }
    let model_everywhere = sharing(inputs.active, |run| run.model == candidate.model);
    if model_everywhere + 1 > global.max_active_per_model() {
        return Err(CapacityRefusal::ModelExhausted {
            model: candidate.model.to_owned(),
            active: model_everywhere,
            cap: global.max_active_per_model(),
        });
    }
    let pool_everywhere = sharing(inputs.active, |run| run.usage_pool == candidate.usage_pool);
    if pool_everywhere + 1 > global.max_active_per_usage_pool() {
        return Err(CapacityRefusal::UsagePoolExhausted {
            usage_pool: candidate.usage_pool.to_owned(),
            active: pool_everywhere,
            cap: global.max_active_per_usage_pool(),
        });
    }

    // The Project's own caps count only its runs. An unset cap adds
    // nothing: the global quota above already spans every Project.
    if let Some(caps) = inputs.project_caps.as_ref() {
        let project = candidate.project;
        let own = |matches: &dyn Fn(&ActiveRun<'_>) -> bool| {
            sharing(inputs.active, |run| run.project == project && matches(run))
        };
        if let Some(cap) = caps.max_active_per_harness() {
            let cap = cap.min(global.max_active_per_harness());
            let active = own(&|run| run.harness == candidate.harness);
            if active + 1 > cap {
                return Err(CapacityRefusal::HarnessExhausted {
                    harness: candidate.harness.to_owned(),
                    active,
                    cap,
                });
            }
        }
        if let Some(cap) = caps.max_active_per_model() {
            let cap = cap.min(global.max_active_per_model());
            let active = own(&|run| run.model == candidate.model);
            if active + 1 > cap {
                return Err(CapacityRefusal::ModelExhausted {
                    model: candidate.model.to_owned(),
                    active,
                    cap,
                });
            }
        }
        if let Some(cap) = caps.max_active_per_usage_pool() {
            let cap = cap.min(global.max_active_per_usage_pool());
            let active = own(&|run| run.usage_pool == candidate.usage_pool);
            if active + 1 > cap {
                return Err(CapacityRefusal::UsagePoolExhausted {
                    usage_pool: candidate.usage_pool.to_owned(),
                    active,
                    cap,
                });
            }
        }
        if let Some(cap) = caps.max_active_lanes() {
            if inputs.active_lanes + 1 > cap {
                return Err(CapacityRefusal::LaneExhausted {
                    active: inputs.active_lanes,
                    cap,
                });
            }
        }
    }
    Ok(())
}

/// Count the active runs a predicate selects.
fn sharing(active: &[ActiveRun<'_>], matches: impl Fn(&ActiveRun<'_>) -> bool) -> u64 {
    active.iter().filter(|run| matches(run)).count() as u64
}

/// Refuse a zero limit; the value names the dimension.
fn positive(limit: u64, dimension: &'static str) -> Result<(), CapacityError> {
    if limit == 0 {
        return Err(CapacityError::ZeroLimit(dimension));
    }
    Ok(())
}

/// Accept an absent cap as-is; refuse a zero or above-global cap.
fn at_most(
    cap: Option<u64>,
    global: u64,
    dimension: &'static str,
) -> Result<Option<u64>, CapacityError> {
    match cap {
        None => Ok(None),
        Some(0) => Err(CapacityError::ZeroLimit(dimension)),
        Some(cap) if cap > global => Err(CapacityError::RelaxesGlobal {
            dimension,
            cap,
            global,
        }),
        Some(cap) => Ok(Some(cap)),
    }
}

#[cfg(test)]
mod capacity_defaults {
    use super::{CapacityError, GlobalCapacity, ProjectCapacity};

    fn defaults() -> GlobalCapacity {
        GlobalCapacity::new(4, 3, 5).expect("positive limits are accepted")
    }

    #[test]
    fn a_zero_limit_is_refused_on_every_dimension() {
        assert_eq!(
            GlobalCapacity::new(0, 3, 5),
            Err(CapacityError::ZeroLimit("harness"))
        );
        assert_eq!(
            GlobalCapacity::new(4, 0, 5),
            Err(CapacityError::ZeroLimit("model family"))
        );
        assert_eq!(
            GlobalCapacity::new(4, 3, 0),
            Err(CapacityError::ZeroLimit("usage pool"))
        );
        assert_eq!(
            ProjectCapacity::new(&defaults(), Some(0), None, None, None),
            Err(CapacityError::ZeroLimit("harness"))
        );
        assert_eq!(
            ProjectCapacity::new(&defaults(), None, Some(0), None, None),
            Err(CapacityError::ZeroLimit("model family"))
        );
        assert_eq!(
            ProjectCapacity::new(&defaults(), None, None, Some(0), None),
            Err(CapacityError::ZeroLimit("usage pool"))
        );
        assert_eq!(
            ProjectCapacity::new(&defaults(), None, None, None, Some(0)),
            Err(CapacityError::ZeroLimit("active Lane"))
        );
    }

    #[test]
    fn global_defaults_carry_the_three_dimensions() {
        let stored = defaults();

        assert_eq!(stored.max_active_per_harness(), 4);
        assert_eq!(stored.max_active_per_model(), 3);
        assert_eq!(stored.max_active_per_usage_pool(), 5);

        let restored = GlobalCapacity::restore(6, 2, 7);
        assert_eq!(restored, GlobalCapacity::restore(6, 2, 7));
        assert_eq!(restored.max_active_per_harness(), 6);
        assert_eq!(restored.max_active_per_model(), 2);
        assert_eq!(restored.max_active_per_usage_pool(), 7);
    }

    #[test]
    fn a_project_may_impose_stricter_caps_and_a_lane_cap() {
        let caps = ProjectCapacity::new(&defaults(), Some(2), Some(1), Some(5), Some(3))
            .expect("caps at or below the globals are accepted");

        assert_eq!(caps.max_active_per_harness(), Some(2));
        assert_eq!(caps.max_active_per_model(), Some(1));
        assert_eq!(caps.max_active_per_usage_pool(), Some(5));
        assert_eq!(caps.max_active_lanes(), Some(3));
        assert!(!caps.is_unset());
    }

    #[test]
    fn a_project_cap_never_relaxes_a_global_one() {
        assert_eq!(
            ProjectCapacity::new(&defaults(), Some(5), None, None, None),
            Err(CapacityError::RelaxesGlobal {
                dimension: "harness",
                cap: 5,
                global: 4,
            })
        );
        assert_eq!(
            ProjectCapacity::new(&defaults(), None, Some(4), None, None),
            Err(CapacityError::RelaxesGlobal {
                dimension: "model family",
                cap: 4,
                global: 3,
            })
        );
        assert_eq!(
            ProjectCapacity::new(&defaults(), None, None, Some(6), None),
            Err(CapacityError::RelaxesGlobal {
                dimension: "usage pool",
                cap: 6,
                global: 5,
            })
        );
        // The Lane cap has no global counterpart, so any positive
        // ceiling stands.
        assert!(ProjectCapacity::new(&defaults(), None, None, None, Some(9)).is_ok());
    }

    #[test]
    fn unset_caps_and_restore_rehydrate_exactly_what_was_recorded() {
        let unset = ProjectCapacity::unset();

        assert!(unset.is_unset());
        assert_eq!(unset.max_active_per_harness(), None);
        assert_eq!(unset.max_active_lanes(), None);

        let restored = ProjectCapacity::restore(Some(2), None, Some(4), Some(8));
        assert_eq!(
            restored,
            ProjectCapacity::restore(Some(2), None, Some(4), Some(8))
        );
        assert!(!restored.is_unset());
    }
}

#[cfg(test)]
mod capacity_evaluation {
    use crate::project::ProjectId;

    use super::{
        ActiveRun, CapacityInputs, CapacityRefusal, GlobalCapacity, ProjectCapacity,
        evaluate_capacity,
    };

    const CORE: u64 = 1;
    const WAVE: u64 = 2;

    fn defaults() -> GlobalCapacity {
        GlobalCapacity::new(4, 3, 5).expect("positive limits are accepted")
    }

    /// One active run, so a test can vary the family it shares.
    fn run<'a>(project: u64, harness: &'a str, model: &'a str, pool: &'a str) -> ActiveRun<'a> {
        ActiveRun {
            project: ProjectId::new(project),
            harness,
            model,
            usage_pool: pool,
        }
    }

    fn inputs<'a>(
        candidate: ActiveRun<'a>,
        active: &'a [ActiveRun<'a>],
        caps: Option<ProjectCapacity>,
    ) -> CapacityInputs<'a> {
        CapacityInputs {
            candidate,
            active,
            active_lanes: 0,
            defaults: defaults(),
            project_caps: caps,
        }
    }

    #[test]
    fn a_run_fits_while_no_dimension_is_at_its_cap() {
        let active = [
            run(CORE, "claude-code", "opus", "operator"),
            run(CORE, "shell-agent", "sonnet", "operator"),
            run(WAVE, "claude-code", "haiku", "batch"),
        ];

        assert_eq!(
            evaluate_capacity(&inputs(
                run(CORE, "claude-code", "opus", "operator"),
                &active,
                None,
            )),
            Ok(()),
            "a fourth run inside every quota fits"
        );
    }

    #[test]
    fn the_harness_quota_spans_every_project() {
        let active = [
            run(CORE, "claude-code", "opus", "operator"),
            run(WAVE, "claude-code", "haiku", "batch"),
            run(WAVE, "claude-code", "sonnet", "operator"),
            run(WAVE, "claude-code", "haiku", "operator"),
        ];

        // The global harness cap is 4 and four runs already share the
        // harness across Projects, so a fifth is refused.
        let outcome = evaluate_capacity(&inputs(
            run(CORE, "claude-code", "opus", "operator"),
            &active,
            None,
        ));
        assert_eq!(
            outcome,
            Err(CapacityRefusal::HarnessExhausted {
                harness: "claude-code".to_owned(),
                active: 4,
                cap: 4,
            })
        );
    }

    #[test]
    fn the_model_family_quota_counts_runs_across_harnesses() {
        let active = [
            run(CORE, "claude-code", "opus", "operator"),
            run(WAVE, "shell-agent", "opus", "batch"),
            run(CORE, "codex-cli", "opus", "operator"),
        ];

        let outcome = evaluate_capacity(&inputs(
            run(CORE, "claude-code", "opus", "operator"),
            &active,
            None,
        ));
        assert_eq!(
            outcome,
            Err(CapacityRefusal::ModelExhausted {
                model: "opus".to_owned(),
                active: 3,
                cap: 3,
            })
        );
    }

    #[test]
    fn the_usage_pool_quota_counts_runs_across_families() {
        let active = [
            run(CORE, "claude-code", "opus", "operator"),
            run(WAVE, "shell-agent", "sonnet", "operator"),
            run(WAVE, "codex-cli", "haiku", "operator"),
            run(CORE, "claude-code", "sonnet", "operator"),
            run(WAVE, "shell-agent", "opus", "operator"),
        ];

        let outcome = evaluate_capacity(&inputs(
            run(CORE, "claude-code", "opus", "operator"),
            &active,
            None,
        ));
        assert_eq!(
            outcome,
            Err(CapacityRefusal::UsagePoolExhausted {
                usage_pool: "operator".to_owned(),
                active: 5,
                cap: 5,
            })
        );
    }

    #[test]
    fn distinct_families_never_share_a_quota() {
        let active = [
            run(WAVE, "shell-agent", "sonnet", "batch"),
            run(WAVE, "codex-cli", "haiku", "operator"),
        ];

        assert_eq!(
            evaluate_capacity(&inputs(
                run(CORE, "claude-code", "opus", "operator"),
                &active,
                None,
            )),
            Ok(()),
            "runs on other families never constrain this one"
        );
    }

    #[test]
    fn a_stricter_project_cap_refuses_before_the_global_one() {
        let active = [
            run(CORE, "claude-code", "opus", "operator"),
            run(CORE, "claude-code", "sonnet", "operator"),
        ];
        let caps = ProjectCapacity::new(&defaults(), Some(2), None, None, None)
            .expect("a stricter cap is accepted");

        // Two runs already share the harness inside the Project; the
        // Project cap of 2 refuses a third the global cap of 4 would
        // still allow.
        let outcome = evaluate_capacity(&inputs(
            run(CORE, "claude-code", "opus", "operator"),
            &active,
            Some(caps),
        ));
        assert_eq!(
            outcome,
            Err(CapacityRefusal::HarnessExhausted {
                harness: "claude-code".to_owned(),
                active: 2,
                cap: 2,
            })
        );
    }

    #[test]
    fn an_unset_project_cap_leaves_the_global_quota_standing() {
        let caps = ProjectCapacity::unset();

        assert_eq!(
            evaluate_capacity(&inputs(
                run(CORE, "claude-code", "opus", "operator"),
                &[],
                Some(caps),
            )),
            Ok(()),
            "an unset cap constrains nothing"
        );
    }

    #[test]
    fn the_global_and_project_quotas_count_their_own_runs() {
        // Four runs on the harness sit in another Project, so the
        // global quota of 4 is full while this Project's own count is
        // zero and its stricter cap of 2 has room.
        let active = [
            run(WAVE, "claude-code", "opus", "batch"),
            run(WAVE, "claude-code", "sonnet", "batch"),
            run(WAVE, "claude-code", "haiku", "batch"),
            run(WAVE, "claude-code", "sonnet", "batch"),
        ];
        let caps = ProjectCapacity::new(&defaults(), Some(2), None, None, None)
            .expect("a stricter cap is accepted");

        let outcome = evaluate_capacity(&inputs(
            run(CORE, "claude-code", "opus", "operator"),
            &active,
            Some(caps),
        ));
        assert_eq!(
            outcome,
            Err(CapacityRefusal::HarnessExhausted {
                harness: "claude-code".to_owned(),
                active: 4,
                cap: 4,
            }),
            "the global quota refuses; the Project cap never judges another Project's runs"
        );
    }

    #[test]
    fn the_lane_cap_counts_the_projects_active_lanes() {
        let caps = ProjectCapacity::new(&defaults(), None, None, None, Some(2))
            .expect("a lane cap is accepted");
        let candidate = run(CORE, "claude-code", "opus", "operator");

        let mut crowded = inputs(candidate, &[], Some(caps));
        crowded.active_lanes = 2;
        assert_eq!(
            evaluate_capacity(&crowded),
            Err(CapacityRefusal::LaneExhausted { active: 2, cap: 2 })
        );

        let mut with_room = inputs(candidate, &[], Some(caps));
        with_room.active_lanes = 1;
        assert_eq!(evaluate_capacity(&with_room), Ok(()));
    }

    #[test]
    fn a_restored_cap_above_the_global_one_still_constrains_there() {
        // A restored Project cap can hold anything; the evaluation
        // never lets it relax the global quota.
        let relaxed = ProjectCapacity::restore(Some(9), None, None, None);
        let active = [
            run(CORE, "claude-code", "opus", "operator"),
            run(WAVE, "claude-code", "sonnet", "operator"),
            run(WAVE, "claude-code", "haiku", "operator"),
            run(WAVE, "claude-code", "sonnet", "operator"),
        ];

        let outcome = evaluate_capacity(&inputs(
            run(CORE, "claude-code", "opus", "operator"),
            &active,
            Some(relaxed),
        ));
        assert_eq!(
            outcome,
            Err(CapacityRefusal::HarnessExhausted {
                harness: "claude-code".to_owned(),
                active: 4,
                cap: 4,
            }),
            "the global quota refuses whatever the Project cap says"
        );
    }
}
