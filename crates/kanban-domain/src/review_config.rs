//! The staged review configuration one Ticket assignment carries
//! (CONTEXT.md, KAN-S10-US1): ordered stages of parallel review slots,
//! each slot required or optional and occupied by a human or by an
//! Execution Profile named by reference (DR-EP-09). Separation
//! validates at configuration time against the Ticket's implementer
//! assignment: a model family never reviews its own work (DR-EP-13),
//! at least one reviewer uses a different harness family from the
//! implementer (DR-EP-14), and human slots are exempt from both
//! checks — neither compared against the implementer nor counted as
//! satisfying the harness difference, because a human uses no harness
//! family at all (DR-EP-15). Stage resolution, same-tip verdicts, and
//! bounce collection are KAN-T48's; this module owns the shape and
//! the separation rules alone.

use std::fmt;

use crate::profile::{ProfileCatalogue, ProfileName};

/// The closed slot requirement vocabulary (DR-EP-09): every required
/// slot in a stage must finish before the stage resolves, while an
/// optional slot never blocks it. Resolution is KAN-T48's; the
/// configuration carries the requirement alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlotRequirement {
    /// The stage cannot resolve without this slot finishing.
    Required,
    /// The slot never blocks the stage it sits in.
    Optional,
}

impl SlotRequirement {
    /// Every requirement, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Required, Self::Optional];

    /// The stored and wire name of this requirement.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Optional => "optional",
        }
    }

    /// The requirement a stored row names, or `None` outside the
    /// vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|requirement| requirement.wire_name() == stored)
    }
}

/// One slot's occupant: a human, or a catalogue profile named by
/// reference. The reference keeps its name through every later
/// catalogue change, exactly as a Ticket's implementer assignment
/// does (DR-EP-05).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewSlotAssignment {
    /// A human reviews; exempt from harness and model separation
    /// (DR-EP-15).
    Human,
    /// An agent reviews under the named Execution Profile.
    Profile(ProfileName),
}

/// One parallel review slot (DR-EP-09): its occupant assignment and
/// whether the slot is required or optional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSlot {
    assignment: ReviewSlotAssignment,
    requirement: SlotRequirement,
}

impl ReviewSlot {
    /// A human-occupied slot.
    pub fn human(requirement: SlotRequirement) -> Self {
        Self {
            assignment: ReviewSlotAssignment::Human,
            requirement,
        }
    }

    /// A profile-occupied slot, assigned through the catalogue by
    /// name.
    pub fn profile(name: ProfileName, requirement: SlotRequirement) -> Self {
        Self {
            assignment: ReviewSlotAssignment::Profile(name),
            requirement,
        }
    }

    /// The slot's occupant assignment.
    pub fn assignment(&self) -> &ReviewSlotAssignment {
        &self.assignment
    }

    /// Whether a human occupies this slot.
    pub fn is_human(&self) -> bool {
        matches!(self.assignment, ReviewSlotAssignment::Human)
    }

    /// The profile an agent slot names, if this slot carries one.
    pub fn profile_name(&self) -> Option<&ProfileName> {
        match &self.assignment {
            ReviewSlotAssignment::Profile(name) => Some(name),
            ReviewSlotAssignment::Human => None,
        }
    }

    /// Whether the stage cannot resolve without this slot finishing.
    pub fn requirement(&self) -> SlotRequirement {
        self.requirement
    }
}

/// One ordered stage of the review configuration (DR-EP-09): parallel
/// slots that resolve together. The stage holds no requirement of its
/// own — a stage with no required slot is legal, and what it means
/// for the stage to resolve is KAN-T48's rule, never this shape's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewStage {
    slots: Vec<ReviewSlot>,
}

impl ReviewStage {
    /// A stage of `slots`, in the order given. The configuration a
    /// stage lands in refuses an empty one; the stage itself holds
    /// only the shape.
    pub fn new(slots: Vec<ReviewSlot>) -> Self {
        Self { slots }
    }

    /// Rehydrate a stored stage exactly as it was recorded.
    pub fn restore(slots: Vec<ReviewSlot>) -> Self {
        Self { slots }
    }

    /// The parallel slots, in the order they were configured.
    pub fn slots(&self) -> &[ReviewSlot] {
        &self.slots
    }
}

/// Why a review configuration was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewConfigError {
    /// A configuration with no stage reviews nothing.
    NoStages,
    /// A stage with no slot reviews nothing. The value names the
    /// stage's one-based position.
    EmptyStage { stage: usize },
    /// A slot names a profile the catalogue holds no assignable entry
    /// for.
    UnknownProfile { name: String },
    /// The implementer's named profile resolves to no catalogue
    /// entry, so separation has nothing to separate from.
    UnknownImplementer { name: String },
    /// A reviewer profile shares the implementer's model family
    /// (DR-EP-13): a model family never reviews its own work.
    SameModelFamily { profile: String, model: String },
    /// No reviewer uses a different harness family from the
    /// implementer (DR-EP-14). Human slots never satisfy this: a
    /// human uses no harness family at all (DR-EP-15).
    NoHarnessSeparation { harness: String },
}

impl fmt::Display for ReviewConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoStages => write!(f, "a review configuration carries at least one stage"),
            Self::EmptyStage { stage } => {
                write!(f, "stage {stage} carries at least one review slot")
            }
            Self::UnknownProfile { name } => {
                write!(f, "the profile name `{name}` is not in the catalogue")
            }
            Self::UnknownImplementer { name } => {
                write!(
                    f,
                    "the implementer profile `{name}` is not in the catalogue"
                )
            }
            Self::SameModelFamily { profile, model } => write!(
                f,
                "the reviewer profile `{profile}` shares the implementer's \
                 `{model}` model family; a model family never reviews its own work"
            ),
            Self::NoHarnessSeparation { harness } => write!(
                f,
                "at least one reviewer must use a different harness family \
                 than the implementer's `{harness}`"
            ),
        }
    }
}

impl std::error::Error for ReviewConfigError {}

/// The staged review configuration one Ticket assignment carries
/// (DR-EP-09, KAN-S10-US1): an ordered list of stages, each stage
/// holding parallel slots. The version counts applied changes: the
/// first configuration lands at 1 and every later legal replacement
/// bumps it, so a stored version is all a caller needs for optimistic
/// checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewConfiguration {
    stages: Vec<ReviewStage>,
    version: u64,
}

impl ReviewConfiguration {
    /// Assemble a first configuration, refusing an empty stage list
    /// and any stage holding no slot. The version lands at 1.
    pub fn new(stages: Vec<ReviewStage>) -> Result<Self, ReviewConfigError> {
        let version = 1;
        check_structure(&stages)?;
        Ok(Self { stages, version })
    }

    /// Rehydrate a stored configuration exactly as it was recorded.
    pub fn restore(stages: Vec<ReviewStage>, version: u64) -> Self {
        Self { stages, version }
    }

    /// The ordered stages, in the order they were configured.
    pub fn stages(&self) -> &[ReviewStage] {
        &self.stages
    }

    /// The number of applied changes, for optimistic version checks.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Replace the whole stage list, refusing the shapes `new` refuses
    /// and leaving the standing configuration untouched when it does.
    /// The applied change bumps the version.
    pub fn replace(&mut self, stages: Vec<ReviewStage>) -> Result<(), ReviewConfigError> {
        check_structure(&stages)?;
        self.stages = stages;
        self.version += 1;
        Ok(())
    }

    /// Validate separation at configuration time (DR-EP-13 to
    /// DR-EP-15) against the implementer assignment `implementer`
    /// names and the catalogue it resolves in. Every agent slot must
    /// name an assignable entry whose model family differs from the
    /// implementer's, and at least one agent slot must name an entry
    /// whose harness family differs. Human slots join neither check:
    /// they are exempt from separation, and a human satisfies no
    /// harness requirement. The implementer's own entry may be
    /// retired — the assignment keeps its name, and the stored
    /// definition still separates (DR-EP-05).
    pub fn validate_separation(
        &self,
        implementer: &ProfileName,
        catalogue: &ProfileCatalogue,
    ) -> Result<(), ReviewConfigError> {
        let implementer_entry = catalogue.resolve(implementer).ok_or_else(|| {
            ReviewConfigError::UnknownImplementer {
                name: implementer.as_str().to_owned(),
            }
        })?;
        let mut harness_separated = false;
        for stage in &self.stages {
            for slot in stage.slots() {
                // A human slot joins neither check (DR-EP-15): it
                // carries no profile to compare and satisfies no
                // harness requirement.
                let Some(name) = slot.profile_name() else {
                    continue;
                };
                if !catalogue.assignable(name) {
                    return Err(ReviewConfigError::UnknownProfile {
                        name: name.as_str().to_owned(),
                    });
                }
                let entry = catalogue
                    .resolve(name)
                    .expect("an assignable name resolves to an entry");
                if entry.model() == implementer_entry.model() {
                    return Err(ReviewConfigError::SameModelFamily {
                        profile: name.as_str().to_owned(),
                        model: entry.model().to_owned(),
                    });
                }
                if entry.harness() != implementer_entry.harness() {
                    harness_separated = true;
                }
            }
        }
        if !harness_separated {
            return Err(ReviewConfigError::NoHarnessSeparation {
                harness: implementer_entry.harness().to_owned(),
            });
        }
        Ok(())
    }
}

/// Refuse the shapes no configuration may hold: an empty stage list,
/// and any stage holding no slot.
fn check_structure(stages: &[ReviewStage]) -> Result<(), ReviewConfigError> {
    if stages.is_empty() {
        return Err(ReviewConfigError::NoStages);
    }
    if let Some(position) = stages.iter().position(|stage| stage.slots().is_empty()) {
        return Err(ReviewConfigError::EmptyStage {
            stage: position + 1,
        });
    }
    Ok(())
}

#[cfg(test)]
mod review_stages {
    use super::{
        ProfileName, ReviewConfigError, ReviewConfiguration, ReviewSlot, ReviewStage,
        SlotRequirement,
    };
    use crate::profile::{ProfileCatalogue, ProfileDefinition};

    fn named(raw: &str) -> ProfileName {
        ProfileName::new(raw).expect("a non-blank name is accepted")
    }

    /// One catalogue entry, defined under `name`.
    fn entry(name: &str, harness: &str, model: &str) -> ProfileCatalogue {
        let mut catalogue = ProfileCatalogue::new();
        catalogue
            .define(
                named(name),
                ProfileDefinition::new(harness, model, "high", "operator", None)
                    .expect("the fixture definition validates"),
            )
            .expect("the fixture entry lands");
        catalogue
    }

    /// The fixture catalogue: an implementer on claude-code/opus, a
    /// reviewer sharing only the model, one sharing only the harness,
    /// one differing in both, and a retired entry.
    fn catalogue() -> ProfileCatalogue {
        let mut catalogue = entry("implementer", "claude-code", "opus");
        for (name, harness, model) in [
            ("same-model", "shell-agent", "opus"),
            ("same-harness", "claude-code", "sonnet"),
            ("outsider", "gemini-cli", "flash"),
            ("retired", "codex-cli", "gpt"),
        ] {
            catalogue
                .define(
                    named(name),
                    ProfileDefinition::new(harness, model, "high", "operator", None)
                        .expect("the fixture definition validates"),
                )
                .expect("the fixture entry lands");
        }
        catalogue
            .retire(&named("retired"))
            .expect("the fixture retire lands");
        catalogue
    }

    fn slot(name: &str, requirement: SlotRequirement) -> ReviewSlot {
        ReviewSlot::profile(named(name), requirement)
    }

    fn implementer() -> ProfileName {
        named("implementer")
    }

    /// A two-stage configuration whose reviewers separate: stage one
    /// holds a required outsider and an optional same-harness slot in
    /// parallel, stage two holds a required human.
    fn separated() -> Vec<ReviewStage> {
        vec![
            ReviewStage::new(vec![
                slot("outsider", SlotRequirement::Required),
                slot("same-harness", SlotRequirement::Optional),
            ]),
            ReviewStage::new(vec![ReviewSlot::human(SlotRequirement::Required)]),
        ]
    }

    #[test]
    fn slots_compose_ordered_stages_of_parallel_slots() {
        let configuration =
            ReviewConfiguration::new(separated()).expect("a separated configuration assembles");

        assert_eq!(configuration.stages().len(), 2);
        let first = &configuration.stages()[0];
        assert_eq!(
            first.slots().len(),
            2,
            "the first stage holds parallel slots"
        );
        assert_eq!(
            first.slots()[0].profile_name().map(|name| name.as_str()),
            Some("outsider")
        );
        assert_eq!(first.slots()[0].requirement(), SlotRequirement::Required);
        assert_eq!(first.slots()[1].requirement(), SlotRequirement::Optional);
        let second = &configuration.stages()[1];
        assert!(second.slots()[0].is_human());
        assert_eq!(second.slots()[0].profile_name(), None);
        assert_eq!(
            second.slots()[0].assignment(),
            &super::ReviewSlotAssignment::Human
        );
        assert_eq!(
            configuration.version(),
            1,
            "the first configuration lands at 1"
        );
    }

    #[test]
    fn the_requirement_vocabulary_round_trips() {
        assert_eq!(SlotRequirement::ALL.len(), 2);
        for requirement in SlotRequirement::ALL {
            assert_eq!(
                SlotRequirement::parse(requirement.wire_name()),
                Some(*requirement),
                "`{}` must survive the round trip",
                requirement.wire_name()
            );
        }
        assert_eq!(SlotRequirement::parse("ghost"), None);
    }

    #[test]
    fn a_configuration_with_no_stage_is_refused() {
        assert_eq!(
            ReviewConfiguration::new(Vec::new()).unwrap_err(),
            ReviewConfigError::NoStages
        );
        assert_eq!(
            ReviewConfigError::NoStages.to_string(),
            "a review configuration carries at least one stage"
        );
    }

    #[test]
    fn a_stage_with_no_slot_is_refused_with_its_position() {
        let refused = ReviewConfiguration::new(vec![
            ReviewStage::new(vec![slot("outsider", SlotRequirement::Required)]),
            ReviewStage::new(Vec::new()),
            ReviewStage::new(vec![ReviewSlot::human(SlotRequirement::Required)]),
        ])
        .unwrap_err();

        assert_eq!(refused, ReviewConfigError::EmptyStage { stage: 2 });
        assert_eq!(
            ReviewConfigError::EmptyStage { stage: 2 }.to_string(),
            "stage 2 carries at least one review slot"
        );
    }

    #[test]
    fn replacing_revalidates_the_structure_and_bumps_the_version() {
        let mut configuration =
            ReviewConfiguration::new(separated()).expect("the configuration assembles");

        configuration
            .replace(vec![ReviewStage::new(vec![slot(
                "outsider",
                SlotRequirement::Required,
            )])])
            .expect("the replacement validates");
        assert_eq!(configuration.stages().len(), 1);
        assert_eq!(configuration.version(), 2, "the replacement is one change");

        let refused = configuration
            .replace(vec![ReviewStage::new(Vec::new())])
            .unwrap_err();
        assert_eq!(refused, ReviewConfigError::EmptyStage { stage: 1 });
        assert_eq!(
            configuration.stages().len(),
            1,
            "the refusal changed nothing"
        );
        assert_eq!(configuration.version(), 2, "the refusal changed nothing");
    }

    #[test]
    fn restore_rehydrates_every_recorded_fact() {
        let configuration = ReviewConfiguration::restore(separated(), 7);

        assert_eq!(configuration.stages().len(), 2);
        assert_eq!(configuration.version(), 7);
    }

    #[test]
    fn separation_accepts_a_reviewer_on_a_different_harness_and_model() {
        let configuration =
            ReviewConfiguration::new(separated()).expect("the configuration assembles");

        assert_eq!(
            configuration.validate_separation(&implementer(), &catalogue()),
            Ok(())
        );
    }

    #[test]
    fn separation_refuses_a_reviewer_sharing_the_implementers_model_family() {
        let configuration = ReviewConfiguration::new(vec![ReviewStage::new(vec![slot(
            "same-model",
            SlotRequirement::Required,
        )])])
        .expect("the configuration assembles");

        let refused = configuration
            .validate_separation(&implementer(), &catalogue())
            .unwrap_err();

        assert_eq!(
            refused,
            ReviewConfigError::SameModelFamily {
                profile: "same-model".to_owned(),
                model: "opus".to_owned(),
            }
        );
        assert_eq!(
            refused.to_string(),
            "the reviewer profile `same-model` shares the implementer's `opus` model family; \
             a model family never reviews its own work"
        );
        assert_eq!(
            configuration.version(),
            1,
            "a refused validation is not a change"
        );
    }

    #[test]
    fn separation_refuses_every_reviewer_on_the_implementers_harness_family() {
        let configuration = ReviewConfiguration::new(vec![ReviewStage::new(vec![
            slot("same-harness", SlotRequirement::Required),
            ReviewSlot::human(SlotRequirement::Required),
        ])])
        .expect("the configuration assembles");

        let refused = configuration
            .validate_separation(&implementer(), &catalogue())
            .unwrap_err();

        assert_eq!(
            refused,
            ReviewConfigError::NoHarnessSeparation {
                harness: "claude-code".to_owned(),
            }
        );
        assert_eq!(
            refused.to_string(),
            "at least one reviewer must use a different harness family \
             than the implementer's `claude-code`"
        );
    }

    #[test]
    fn separation_refuses_a_configuration_of_human_slots_alone() {
        let configuration =
            ReviewConfiguration::new(vec![ReviewStage::new(vec![ReviewSlot::human(
                SlotRequirement::Required,
            )])])
            .expect("the configuration assembles");

        assert_eq!(
            configuration
                .validate_separation(&implementer(), &catalogue())
                .unwrap_err(),
            ReviewConfigError::NoHarnessSeparation {
                harness: "claude-code".to_owned(),
            },
            "a human uses no harness family, so no slot separates (DR-EP-14, DR-EP-15)"
        );
    }

    #[test]
    fn human_slots_are_exempt_from_harness_and_model_separation() {
        // The human slot sits beside reviewers that separate, and the
        // exemption holds both ways: the human is never compared
        // against the implementer, and removing the separating agent
        // while keeping the human refuses — exemption is not
        // satisfaction (DR-EP-15).
        let beside = ReviewConfiguration::new(vec![ReviewStage::new(vec![
            ReviewSlot::human(SlotRequirement::Required),
            slot("outsider", SlotRequirement::Optional),
        ])])
        .expect("the configuration assembles");
        assert_eq!(
            beside.validate_separation(&implementer(), &catalogue()),
            Ok(())
        );

        let alone = ReviewConfiguration::new(vec![ReviewStage::new(vec![
            ReviewSlot::human(SlotRequirement::Required),
            slot("same-harness", SlotRequirement::Required),
        ])])
        .expect("the configuration assembles");
        assert_eq!(
            alone
                .validate_separation(&implementer(), &catalogue())
                .unwrap_err(),
            ReviewConfigError::NoHarnessSeparation {
                harness: "claude-code".to_owned(),
            }
        );
    }

    #[test]
    fn separation_refuses_unknown_and_retired_reviewer_profiles() {
        let unknown = ReviewConfiguration::new(vec![ReviewStage::new(vec![slot(
            "ghost",
            SlotRequirement::Required,
        )])])
        .expect("the configuration assembles");
        assert_eq!(
            unknown
                .validate_separation(&implementer(), &catalogue())
                .unwrap_err(),
            ReviewConfigError::UnknownProfile {
                name: "ghost".to_owned()
            }
        );

        let retired = ReviewConfiguration::new(vec![ReviewStage::new(vec![slot(
            "retired",
            SlotRequirement::Required,
        )])])
        .expect("the configuration assembles");
        assert_eq!(
            retired
                .validate_separation(&implementer(), &catalogue())
                .unwrap_err(),
            ReviewConfigError::UnknownProfile {
                name: "retired".to_owned()
            },
            "a retired entry is out of the assignable catalogue"
        );
    }

    #[test]
    fn a_retired_implementer_entry_still_separates() {
        let mut retired = catalogue();
        retired
            .retire(&implementer())
            .expect("the fixture retire lands");

        let configuration = ReviewConfiguration::new(vec![ReviewStage::new(vec![slot(
            "outsider",
            SlotRequirement::Required,
        )])])
        .expect("the configuration assembles");

        assert_eq!(
            configuration.validate_separation(&implementer(), &retired),
            Ok(()),
            "the assignment keeps its name, and the stored definition still separates"
        );
    }

    #[test]
    fn an_implementer_no_entry_resolves_to_is_refused() {
        let configuration = ReviewConfiguration::new(vec![ReviewStage::new(vec![slot(
            "outsider",
            SlotRequirement::Required,
        )])])
        .expect("the configuration assembles");

        let refused = configuration
            .validate_separation(&named("ghost"), &catalogue())
            .unwrap_err();

        assert_eq!(
            refused,
            ReviewConfigError::UnknownImplementer {
                name: "ghost".to_owned()
            }
        );
        assert_eq!(
            refused.to_string(),
            "the implementer profile `ghost` is not in the catalogue"
        );
    }

    #[test]
    fn an_empty_catalogue_refuses_every_configuration() {
        let configuration = ReviewConfiguration::new(vec![ReviewStage::new(vec![slot(
            "outsider",
            SlotRequirement::Required,
        )])])
        .expect("the configuration assembles");

        assert_eq!(
            configuration
                .validate_separation(&implementer(), &ProfileCatalogue::new())
                .unwrap_err(),
            ReviewConfigError::UnknownImplementer {
                name: "implementer".to_owned()
            }
        );
    }
}
