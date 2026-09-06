//! The card chip vocabulary the board renders: the closed set of chip
//! kinds, the chip set each Ticket kind carries, and the version that
//! pins both in the application schema (DR-BP-07 to DR-BP-14,
//! DR-BP-16). Adding a chip kind is a schema change: the enum grows
//! here, the generated contracts change with it, and the card surface
//! renders from the vocabulary rather than patching itself.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ticket::TicketKind;

/// The version of the chip vocabulary the application schema pins.
/// Every change to the vocabulary — a new chip kind, a kind gaining or
/// losing a chip — bumps this version (DR-BP-16).
pub const CHIP_VOCABULARY_VERSION: u32 = 1;

/// One chip kind a card can carry. The set is closed: a card renders
/// only chips this enum names, so a new chip kind cannot reach the
/// board without a schema change (DR-BP-07, DR-BP-16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChipKind {
    /// The Ticket's priority (DR-LC-12). Every card carries it.
    Priority,
    /// The kind-scoped progress: Acceptance Criteria progress for
    /// Implementations and Bugs, completion progress for Tasks. Every
    /// card carries it.
    Progress,
    /// The Spec the Ticket attaches to; optional for a Bug.
    Spec,
    /// The effective implementer of an Implementation, named by its
    /// Execution Profile.
    Implementer,
    /// The reviewers of an Implementation; more than two collapse to
    /// `+N` on the card (DR-BP-14).
    Reviewers,
    /// The Lane holding the Ticket's active execution (DR-LW-01).
    Lane,
    /// What holds the Ticket back: registered dependencies and
    /// explicit external blockers.
    Blockers,
    /// The Bug's qualified severity (DR-LC-13).
    Severity,
    /// The Bug's qualified frequency.
    Frequency,
    /// Where the Bug report came from: its reporter evidence.
    Origin,
    /// The execution profiles a Bug runs under: planned before
    /// dispatch, effective with a fallback indicator during execution
    /// (DR-BP-12, DR-BP-13).
    Profiles,
    /// The Task's subtype of the closed set (DR-TK-08).
    Subtype,
    /// The Task's human-or-agent mode (DR-TK-09).
    Mode,
    /// The Task's schedule or due date.
    Schedule,
    /// Who executes a Task: the operator, or the named profile.
    Executor,
}

impl ChipKind {
    /// Every chip kind, in vocabulary order.
    pub const ALL: &'static [Self] = &[
        Self::Priority,
        Self::Progress,
        Self::Spec,
        Self::Implementer,
        Self::Reviewers,
        Self::Lane,
        Self::Blockers,
        Self::Severity,
        Self::Frequency,
        Self::Origin,
        Self::Profiles,
        Self::Subtype,
        Self::Mode,
        Self::Schedule,
        Self::Executor,
    ];

    /// The wire name, matching this chip's serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Priority => "priority",
            Self::Progress => "progress",
            Self::Spec => "spec",
            Self::Implementer => "implementer",
            Self::Reviewers => "reviewers",
            Self::Lane => "lane",
            Self::Blockers => "blockers",
            Self::Severity => "severity",
            Self::Frequency => "frequency",
            Self::Origin => "origin",
            Self::Profiles => "profiles",
            Self::Subtype => "subtype",
            Self::Mode => "mode",
            Self::Schedule => "schedule",
            Self::Executor => "executor",
        }
    }

    /// Parses a wire name; unknown names find nothing, because the
    /// vocabulary is closed.
    pub fn parse(wire: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.as_str() == wire)
    }
}

/// The chip set one kind of card carries, in the order the card
/// renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChipSet {
    /// The Ticket kind this chip set belongs to.
    pub kind: TicketKind,
    /// Every chip a card of this kind carries, in render order.
    pub chips: Vec<ChipKind>,
}

/// The closed, versioned chip vocabulary the application schema pins
/// (DR-BP-16). The generated contracts carry it to every client, and
/// the board renders from it rather than from a list of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChipVocabulary {
    /// The vocabulary version; changing the vocabulary changes it.
    pub version: u32,
    /// One chip set per Ticket kind, in Ticket-kind order.
    pub sets: Vec<ChipSet>,
}

impl ChipVocabulary {
    /// The current vocabulary, as the generated contracts carry it.
    pub fn current() -> Self {
        Self {
            version: CHIP_VOCABULARY_VERSION,
            sets: TicketKind::ALL
                .iter()
                .map(|&kind| ChipSet {
                    kind,
                    chips: chips_for(kind),
                })
                .collect(),
        }
    }

    /// The chips one kind of card carries, in render order.
    pub fn chips_for(&self, kind: TicketKind) -> &[ChipKind] {
        self.sets
            .iter()
            .find(|set| set.kind == kind)
            .map(|set| set.chips.as_slice())
            .unwrap_or(&[])
    }
}

/// The chip set each kind of card carries: every card shows priority
/// and its kind-scoped progress (DR-BP-08); Implementations add the
/// Spec, effective implementer, reviewers, Lane, and blockers
/// (DR-BP-09); Bugs add the optional Spec, severity, frequency,
/// origin, profiles, and blockers (DR-BP-10); Tasks add the subtype,
/// mode, schedule or due date, executor, and blockers (DR-BP-11).
fn chips_for(kind: TicketKind) -> Vec<ChipKind> {
    match kind {
        TicketKind::Implementation => vec![
            ChipKind::Priority,
            ChipKind::Progress,
            ChipKind::Spec,
            ChipKind::Implementer,
            ChipKind::Reviewers,
            ChipKind::Lane,
            ChipKind::Blockers,
        ],
        TicketKind::Bug => vec![
            ChipKind::Priority,
            ChipKind::Progress,
            ChipKind::Spec,
            ChipKind::Severity,
            ChipKind::Frequency,
            ChipKind::Origin,
            ChipKind::Profiles,
            ChipKind::Blockers,
        ],
        TicketKind::Task => vec![
            ChipKind::Priority,
            ChipKind::Progress,
            ChipKind::Subtype,
            ChipKind::Mode,
            ChipKind::Schedule,
            ChipKind::Executor,
            ChipKind::Blockers,
        ],
    }
}

#[cfg(test)]
mod tests {
    use schemars::schema_for;
    use serde_json::json;

    use super::{CHIP_VOCABULARY_VERSION, ChipKind, ChipSet, ChipVocabulary};
    use crate::schema_definitions;
    use crate::ticket::TicketKind;

    #[test]
    fn chip_schema_pins_one_closed_set_per_kind() {
        let vocabulary = ChipVocabulary::current();

        assert_eq!(
            vocabulary
                .sets
                .iter()
                .map(|set| set.kind)
                .collect::<Vec<_>>(),
            TicketKind::ALL.to_vec(),
            "one chip set per Ticket kind, in kind order"
        );
        for set in &vocabulary.sets {
            assert!(
                !set.chips.is_empty(),
                "{} carries at least priority and progress",
                set.kind.as_str()
            );
            let mut seen: Vec<&super::ChipKind> = Vec::new();
            for chip in &set.chips {
                assert!(
                    !seen.contains(&chip),
                    "{} repeats {chip:?}",
                    set.kind.as_str()
                );
                seen.push(chip);
            }
        }
    }

    #[test]
    fn chip_schema_carries_the_register_chip_sets() {
        let vocabulary = ChipVocabulary::current();

        // Every card: priority and progress (DR-BP-08).
        for kind in TicketKind::ALL {
            let chips = vocabulary.chips_for(*kind);
            assert!(
                chips.contains(&ChipKind::Priority),
                "{kind:?} shows priority"
            );
            assert!(
                chips.contains(&ChipKind::Progress),
                "{kind:?} shows progress"
            );
            assert!(
                chips.contains(&ChipKind::Blockers),
                "{kind:?} shows blockers"
            );
        }

        // Implementation: Spec, effective implementer, reviewers,
        // Lane, blockers (DR-BP-09).
        assert_eq!(
            vocabulary.chips_for(TicketKind::Implementation),
            &[
                ChipKind::Priority,
                ChipKind::Progress,
                ChipKind::Spec,
                ChipKind::Implementer,
                ChipKind::Reviewers,
                ChipKind::Lane,
                ChipKind::Blockers,
            ]
        );

        // Bug: optional Spec, severity, frequency, origin, effective
        // profiles, blockers (DR-BP-10).
        assert_eq!(
            vocabulary.chips_for(TicketKind::Bug),
            &[
                ChipKind::Priority,
                ChipKind::Progress,
                ChipKind::Spec,
                ChipKind::Severity,
                ChipKind::Frequency,
                ChipKind::Origin,
                ChipKind::Profiles,
                ChipKind::Blockers,
            ]
        );

        // Task: subtype, mode, schedule or due date, executor,
        // blockers (DR-BP-11).
        assert_eq!(
            vocabulary.chips_for(TicketKind::Task),
            &[
                ChipKind::Priority,
                ChipKind::Progress,
                ChipKind::Subtype,
                ChipKind::Mode,
                ChipKind::Schedule,
                ChipKind::Executor,
                ChipKind::Blockers,
            ]
        );
    }

    #[test]
    fn chip_schema_leaves_no_kind_unplaced() {
        let vocabulary = ChipVocabulary::current();
        let placed: Vec<_> = vocabulary
            .sets
            .iter()
            .flat_map(|set| set.chips.iter().copied())
            .collect();

        for chip in ChipKind::ALL {
            assert!(
                placed.contains(chip),
                "{} belongs to no card; the vocabulary carries no unused kinds",
                chip.as_str()
            );
        }
    }

    #[test]
    fn chip_schema_records_its_version() {
        let vocabulary = ChipVocabulary::current();

        assert_eq!(vocabulary.version, CHIP_VOCABULARY_VERSION);
        assert!(vocabulary.version > 0, "version zero pins nothing");
    }

    #[test]
    fn chip_schema_rejects_an_unknown_chip_kind() {
        let payload = json!({
            "kind": "implementation",
            "chips": ["priority", "triage"],
        });

        let error = serde_json::from_value::<ChipSet>(payload)
            .expect_err("a chip outside the enum is refused");

        assert!(
            error.to_string().contains("unknown variant"),
            "adding a chip kind must be a schema change, not a wire patch: {error}"
        );
    }

    #[test]
    fn chip_schema_round_trips_the_current_vocabulary() {
        let vocabulary = ChipVocabulary::current();
        let encoded = serde_json::to_value(&vocabulary).expect("the vocabulary encodes");
        let decoded: ChipVocabulary =
            serde_json::from_value(encoded).expect("the vocabulary decodes");

        assert_eq!(decoded, vocabulary);
    }

    #[test]
    fn chip_schema_derives_the_closed_enum() {
        let schema = schema_for!(ChipKind);
        let json = serde_json::to_value(schema).expect("the chip enum schematises");
        // A documented enum schematises as one branch per variant; an
        // undocumented one lists its variants whole. Either shape must
        // carry exactly the closed set.
        let values: Vec<serde_json::Value> = if let Some(branches) = json.get("oneOf") {
            branches
                .as_array()
                .expect("the chip enum branches are a list")
                .iter()
                .map(|branch| {
                    branch
                        .get("enum")
                        .and_then(|enum_values| enum_values.as_array())
                        .expect("each chip branch names its variant")[0]
                        .clone()
                })
                .collect()
        } else {
            json.get("enum")
                .and_then(|enum_values| enum_values.as_array())
                .expect("the chip enum lists its variants")
                .clone()
        };
        let wire_names: Vec<_> = ChipKind::ALL
            .iter()
            .map(|chip| json!(chip.as_str()))
            .collect();

        assert_eq!(
            values, wire_names,
            "the schema enum is exactly the closed set"
        );
    }

    #[test]
    fn chip_schema_is_in_the_schema_registry() {
        let names: Vec<_> = schema_definitions()
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        for expected in ["ChipKind", "ChipSet", "ChipVocabulary"] {
            assert!(names.contains(&expected), "{expected} must be registered");
        }
    }
}
