//! The Execution Profile catalogue: named entries defining harness,
//! model, effort, usage pool, and fallback policy (CONTEXT.md). The
//! schema is closed — a profile carries exactly those five decisions —
//! names are unique and immutable per entry, and references name
//! entries rather than inlining values, so a catalogue change never
//! rewrites what a past assignment named (DR-EP-01, DR-EP-02,
//! DR-EP-05).

use std::fmt;

/// Why a profile rule was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    /// A text field holds nothing but whitespace. The value names the
    /// field.
    Blank(&'static str),
    /// A profile never falls back to itself.
    SelfFallback,
    /// The name is already defined; names are unique and immutable per
    /// entry, retired entries included.
    DuplicateName { name: String },
    /// The name resolves to no catalogue entry.
    UnknownName { name: String },
    /// The profile is already retired.
    AlreadyRetired,
    /// Retired is terminal: the entry accepts no further changes.
    RetiredIsTerminal,
    /// Another entry names this one as its fallback, so retiring it
    /// would leave a reference nothing resolves to.
    FallbackHeld { name: String },
    /// The fallback chain returns to its starting entry.
    FallbackCycle,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blank(field) => write!(f, "a profile {field} cannot be blank"),
            Self::SelfFallback => write!(f, "a profile never falls back to itself"),
            Self::DuplicateName { name } => {
                write!(f, "the profile name `{name}` is already defined")
            }
            Self::UnknownName { name } => {
                write!(f, "the profile name `{name}` is not in the catalogue")
            }
            Self::AlreadyRetired => write!(f, "the profile is already retired"),
            Self::RetiredIsTerminal => {
                write!(
                    f,
                    "retired is terminal; the profile accepts no further changes"
                )
            }
            Self::FallbackHeld { name } => {
                write!(
                    f,
                    "the profile `{name}` is still the fallback of another profile"
                )
            }
            Self::FallbackCycle => write!(f, "a fallback chain may not loop"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// A validated, trimmed profile name. The name is the entry's
/// identity: assignments and fallbacks name entries by it, and it is
/// never renamed after the entry is defined.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProfileName(String);

impl ProfileName {
    /// Accept any name that holds at least one non-whitespace
    /// character; surrounding whitespace is not part of the name.
    pub fn new(raw: &str) -> Result<Self, ProfileError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ProfileError::Blank("name"));
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The trimmed name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The lifecycle vocabulary of one catalogue entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileState {
    /// Defined and assignable.
    Active,
    /// Terminal: off the assignable catalogue, every recorded fact
    /// preserved.
    Retired,
}

/// The closed definition one profile carries (DR-EP-02): the harness
/// family, the model family, the effort, the usage pool, and the
/// fallback policy as the profile another entry names — never
/// inlined values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileDefinition {
    harness: String,
    model: String,
    effort: String,
    usage_pool: String,
    fallback: Option<ProfileName>,
}

impl ProfileDefinition {
    /// Assemble a definition, refusing a blank harness, model,
    /// effort, or usage pool. The fallback names another entry by
    /// reference, or nothing.
    pub fn new(
        harness: impl Into<String>,
        model: impl Into<String>,
        effort: impl Into<String>,
        usage_pool: impl Into<String>,
        fallback: Option<ProfileName>,
    ) -> Result<Self, ProfileError> {
        let harness = harness.into();
        if harness.trim().is_empty() {
            return Err(ProfileError::Blank("harness"));
        }
        let model = model.into();
        if model.trim().is_empty() {
            return Err(ProfileError::Blank("model"));
        }
        let effort = effort.into();
        if effort.trim().is_empty() {
            return Err(ProfileError::Blank("effort"));
        }
        let usage_pool = usage_pool.into();
        if usage_pool.trim().is_empty() {
            return Err(ProfileError::Blank("usage pool"));
        }
        Ok(Self {
            harness,
            model,
            effort,
            usage_pool,
            fallback,
        })
    }

    /// The fallback policy, as the profile it names.
    pub fn fallback(&self) -> Option<&ProfileName> {
        self.fallback.as_ref()
    }
}

/// One named catalogue entry. The version counts applied changes:
/// definition lands at 1 and every later legal change bumps it, so a
/// stored version is all a caller needs for optimistic checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionProfile {
    name: ProfileName,
    definition: ProfileDefinition,
    state: ProfileState,
    version: u64,
}

impl ExecutionProfile {
    /// Define a fresh entry: active, at version 1. The entry's own
    /// rules refuse a fallback naming itself; the catalogue adds the
    /// rules that need the other entries.
    pub fn define(name: ProfileName, definition: ProfileDefinition) -> Result<Self, ProfileError> {
        if definition.fallback.as_ref() == Some(&name) {
            return Err(ProfileError::SelfFallback);
        }
        Ok(Self {
            name,
            definition,
            state: ProfileState::Active,
            version: 1,
        })
    }

    /// Rehydrate a stored entry exactly as it was recorded.
    pub fn restore(
        name: ProfileName,
        definition: ProfileDefinition,
        state: ProfileState,
        version: u64,
    ) -> Self {
        Self {
            name,
            definition,
            state,
            version,
        }
    }

    /// The entry's immutable identity.
    pub fn name(&self) -> &ProfileName {
        &self.name
    }

    /// The harness family.
    pub fn harness(&self) -> &str {
        &self.definition.harness
    }

    /// The model family.
    pub fn model(&self) -> &str {
        &self.definition.model
    }

    /// The effort.
    pub fn effort(&self) -> &str {
        &self.definition.effort
    }

    /// The usage pool.
    pub fn usage_pool(&self) -> &str {
        &self.definition.usage_pool
    }

    /// The fallback policy, as the profile it names.
    pub fn fallback(&self) -> Option<&ProfileName> {
        self.definition.fallback()
    }

    /// The lifecycle state.
    pub fn state(&self) -> ProfileState {
        self.state
    }

    /// Whether the entry is retired.
    pub fn is_retired(&self) -> bool {
        self.state == ProfileState::Retired
    }

    /// The number of applied changes, for optimistic version checks.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Replace the definition under the same name. Retired is
    /// terminal.
    pub fn redefine(&mut self, definition: ProfileDefinition) -> Result<(), ProfileError> {
        if self.state == ProfileState::Retired {
            return Err(ProfileError::RetiredIsTerminal);
        }
        if definition.fallback.as_ref() == Some(&self.name) {
            return Err(ProfileError::SelfFallback);
        }
        self.definition = definition;
        self.version += 1;
        Ok(())
    }

    /// Retire an active entry. Retired is terminal, so a second
    /// retire is refused rather than absorbed.
    pub fn retire(&mut self) -> Result<(), ProfileError> {
        if self.state == ProfileState::Retired {
            return Err(ProfileError::AlreadyRetired);
        }
        self.state = ProfileState::Retired;
        self.version += 1;
        Ok(())
    }
}

/// The catalogue the collection rules live in: names are unique
/// across every entry, retired ones included, and every fallback
/// resolves to an active entry without looping. Assignments ask the
/// catalogue whether a name is assignable; a retired entry keeps the
/// names past assignments already carry (DR-EP-05).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileCatalogue {
    entries: Vec<ExecutionProfile>,
}

impl ProfileCatalogue {
    /// An empty catalogue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Rehydrate a stored catalogue exactly as it was recorded.
    pub fn restore(entries: Vec<ExecutionProfile>) -> Self {
        Self { entries }
    }

    /// Every entry, retired ones included, in definition order.
    pub fn entries(&self) -> &[ExecutionProfile] {
        &self.entries
    }

    /// The entry `name` resolves to, retired ones included.
    pub fn resolve(&self, name: &ProfileName) -> Option<&ExecutionProfile> {
        self.entries.iter().find(|entry| entry.name() == name)
    }

    /// Whether `name` resolves to an active entry an assignment may
    /// reference.
    pub fn assignable(&self, name: &ProfileName) -> bool {
        self.resolve(name).is_some_and(|entry| !entry.is_retired())
    }

    /// Define a new entry, refusing a name any entry — retired ones
    /// included — already carries, a self-fallback, and a fallback
    /// that resolves to no active entry.
    pub fn define(
        &mut self,
        name: ProfileName,
        definition: ProfileDefinition,
    ) -> Result<&ExecutionProfile, ProfileError> {
        if self.resolve(&name).is_some() {
            return Err(ProfileError::DuplicateName {
                name: name.as_str().to_owned(),
            });
        }
        let entry = ExecutionProfile::define(name.clone(), definition)?;
        self.check_fallback(&entry)?;
        self.entries.push(entry);
        Ok(self.entries.last().expect("the entry just landed"))
    }

    /// Replace the definition of one active entry under its own
    /// name.
    pub fn redefine(
        &mut self,
        name: &ProfileName,
        definition: ProfileDefinition,
    ) -> Result<&ExecutionProfile, ProfileError> {
        let index = self.position(name)?;
        let entry = &mut self.entries[index];
        entry.redefine(definition)?;
        let entry = &self.entries[index];
        self.check_fallback(entry)?;
        self.check_cycles(entry.name())?;
        Ok(entry)
    }

    /// Retire one active entry, refusing while another entry still
    /// names it as a fallback: a reference must always resolve.
    pub fn retire(&mut self, name: &ProfileName) -> Result<&ExecutionProfile, ProfileError> {
        let index = self.position(name)?;
        if self.entries[index].is_retired() {
            return Err(ProfileError::AlreadyRetired);
        }
        if self
            .entries
            .iter()
            .enumerate()
            .any(|(other, entry)| other != index && entry.fallback() == Some(name))
        {
            return Err(ProfileError::FallbackHeld {
                name: name.as_str().to_owned(),
            });
        }
        let entry = &mut self.entries[index];
        entry.retire()?;
        Ok(&self.entries[index])
    }

    /// The position of the entry `name` resolves to.
    fn position(&self, name: &ProfileName) -> Result<usize, ProfileError> {
        self.entries
            .iter()
            .position(|entry| entry.name() == name)
            .ok_or_else(|| ProfileError::UnknownName {
                name: name.as_str().to_owned(),
            })
    }

    /// A fallback must resolve to an active entry other than its own.
    fn check_fallback(&self, entry: &ExecutionProfile) -> Result<(), ProfileError> {
        let Some(fallback) = entry.fallback() else {
            return Ok(());
        };
        if fallback == entry.name() {
            return Err(ProfileError::SelfFallback);
        }
        match self.resolve(fallback) {
            Some(target) if !target.is_retired() => Ok(()),
            _ => Err(ProfileError::UnknownName {
                name: fallback.as_str().to_owned(),
            }),
        }
    }

    /// Walk the fallback chain from `start`; a chain that returns to
    /// its starting entry can never resolve an effective profile.
    fn check_cycles(&self, start: &ProfileName) -> Result<(), ProfileError> {
        let mut cursor = start.clone();
        while let Some(next) = self
            .resolve(&cursor)
            .and_then(ExecutionProfile::fallback)
            .cloned()
        {
            if &next == start {
                return Err(ProfileError::FallbackCycle);
            }
            cursor = next;
        }
        Ok(())
    }
}

#[cfg(test)]
mod profile_schema {
    use super::{
        ExecutionProfile, ProfileCatalogue, ProfileDefinition, ProfileError, ProfileName,
        ProfileState,
    };

    fn named(raw: &str) -> ProfileName {
        ProfileName::new(raw).expect("a non-blank name is accepted")
    }

    fn definition(fallback: Option<&str>) -> ProfileDefinition {
        ProfileDefinition::new(
            "claude-code",
            "opus",
            "high",
            "operator",
            fallback.map(named),
        )
        .expect("a complete definition is accepted")
    }

    #[test]
    fn a_blank_name_is_refused_and_a_name_is_stored_trimmed() {
        for blank in ["", " ", " \t "] {
            assert_eq!(ProfileName::new(blank), Err(ProfileError::Blank("name")));
        }
        assert_eq!(named("  standard  ").as_str(), "standard");
    }

    #[test]
    fn the_schema_is_closed_over_its_five_decisions() {
        let profile = ExecutionProfile::define(named("standard"), definition(None))
            .expect("a complete profile is defined");

        assert_eq!(profile.name().as_str(), "standard");
        assert_eq!(profile.harness(), "claude-code");
        assert_eq!(profile.model(), "opus");
        assert_eq!(profile.effort(), "high");
        assert_eq!(profile.usage_pool(), "operator");
        assert_eq!(profile.fallback(), None);
        assert_eq!(profile.state(), ProfileState::Active);
        assert_eq!(profile.version(), 1);
    }

    #[test]
    fn every_blank_decision_is_refused() {
        for blank in ["", " "] {
            for field in ["harness", "model", "effort", "usage pool"] {
                let error = ProfileDefinition::new(
                    if field == "harness" {
                        blank
                    } else {
                        "claude-code"
                    },
                    if field == "model" { blank } else { "opus" },
                    if field == "effort" { blank } else { "high" },
                    if field == "usage pool" {
                        blank
                    } else {
                        "operator"
                    },
                    None,
                )
                .expect_err("a blank decision is refused");
                assert_eq!(error, ProfileError::Blank(field), "{field} must be named");
            }
        }
    }

    #[test]
    fn defining_against_the_catalogue_refuses_a_duplicate_name() {
        let mut catalogue = ProfileCatalogue::new();
        catalogue
            .define(named("standard"), definition(None))
            .expect("the first define lands");

        let error = catalogue
            .define(named("standard"), definition(None))
            .expect_err("names are unique");

        assert_eq!(
            error,
            ProfileError::DuplicateName {
                name: "standard".to_owned()
            }
        );
    }

    #[test]
    fn a_retired_name_is_never_reusable() {
        let mut catalogue = ProfileCatalogue::new();
        catalogue
            .define(named("standard"), definition(None))
            .expect("the define lands");
        catalogue
            .retire(&named("standard"))
            .expect("the retire lands");

        let error = catalogue
            .define(named("standard"), definition(None))
            .expect_err("history is never rewritten by redefinition");

        assert_eq!(
            error,
            ProfileError::DuplicateName {
                name: "standard".to_owned()
            }
        );
    }

    #[test]
    fn a_fallback_must_name_another_active_entry() {
        let mut catalogue = ProfileCatalogue::new();
        catalogue
            .define(named("standard"), definition(None))
            .expect("the primary lands");

        let unknown = catalogue
            .define(named("nightly"), definition(Some("ghost")))
            .expect_err("an unknown fallback is refused");
        assert_eq!(
            unknown,
            ProfileError::UnknownName {
                name: "ghost".to_owned()
            }
        );

        let self_fallback = catalogue
            .define(named("nightly"), definition(Some("nightly")))
            .expect_err("a profile never falls back to itself");
        assert_eq!(self_fallback, ProfileError::SelfFallback);

        catalogue
            .define(named("nightly"), definition(Some("standard")))
            .expect("a named fallback lands");
    }

    #[test]
    fn a_fallback_never_forms_a_cycle() {
        let mut catalogue = ProfileCatalogue::new();
        catalogue
            .define(named("standard"), definition(None))
            .expect("the primary lands");
        catalogue
            .define(named("nightly"), definition(Some("standard")))
            .expect("the secondary lands");

        let error = catalogue
            .redefine(&named("standard"), definition(Some("nightly")))
            .expect_err("a fallback chain may not loop");

        assert_eq!(error, ProfileError::FallbackCycle);
    }

    #[test]
    fn redefining_replaces_the_definition_under_the_same_name() {
        let mut catalogue = ProfileCatalogue::new();
        catalogue
            .define(named("standard"), definition(None))
            .expect("the define lands");

        let redefined = catalogue
            .redefine(
                &named("standard"),
                ProfileDefinition::new("shell-agent", "sonnet", "medium", "operator", None)
                    .expect("the replacement validates"),
            )
            .expect("the redefine lands");

        assert_eq!(redefined.name().as_str(), "standard");
        assert_eq!(redefined.harness(), "shell-agent");
        assert_eq!(redefined.model(), "sonnet");
        assert_eq!(redefined.version(), 2);
    }

    #[test]
    fn retiring_is_terminal_and_preserves_the_entry() {
        let mut catalogue = ProfileCatalogue::new();
        catalogue
            .define(named("standard"), definition(None))
            .expect("the define lands");

        let retired = catalogue
            .retire(&named("standard"))
            .expect("the retire lands");

        assert_eq!(retired.state(), ProfileState::Retired);
        assert_eq!(retired.version(), 2);
        assert_eq!(
            catalogue.retire(&named("standard")),
            Err(ProfileError::AlreadyRetired)
        );
        assert_eq!(
            catalogue.redefine(&named("standard"), definition(None)),
            Err(ProfileError::RetiredIsTerminal)
        );
        assert_eq!(
            catalogue.entries()[0].name().as_str(),
            "standard",
            "the retired entry stays in the catalogue"
        );
    }

    #[test]
    fn an_unknown_name_is_refused_for_every_mutation() {
        let mut catalogue = ProfileCatalogue::new();

        assert_eq!(
            catalogue.redefine(&named("ghost"), definition(None)),
            Err(ProfileError::UnknownName {
                name: "ghost".to_owned()
            })
        );
        assert_eq!(
            catalogue.retire(&named("ghost")),
            Err(ProfileError::UnknownName {
                name: "ghost".to_owned()
            })
        );
    }

    #[test]
    fn retiring_a_fallback_target_other_entries_still_name_is_refused() {
        let mut catalogue = ProfileCatalogue::new();
        catalogue
            .define(named("standard"), definition(None))
            .expect("the primary lands");
        catalogue
            .define(named("nightly"), definition(Some("standard")))
            .expect("the secondary names the primary");

        let error = catalogue
            .retire(&named("standard"))
            .expect_err("a named fallback target may not retire");

        assert_eq!(
            error,
            ProfileError::FallbackHeld {
                name: "standard".to_owned()
            }
        );
    }

    #[test]
    fn a_retired_fallback_target_may_not_be_named_again() {
        let mut catalogue = ProfileCatalogue::new();
        catalogue
            .define(named("standard"), definition(None))
            .expect("the primary lands");
        catalogue
            .define(named("spare"), definition(None))
            .expect("the spare lands");
        catalogue
            .retire(&named("spare"))
            .expect("the spare retires");

        assert_eq!(
            catalogue.redefine(&named("standard"), definition(Some("spare"))),
            Err(ProfileError::UnknownName {
                name: "spare".to_owned()
            }),
            "a retired entry is out of the assignable catalogue"
        );
    }

    #[test]
    fn assignments_resolve_against_active_entries_only() {
        let mut catalogue = ProfileCatalogue::new();
        catalogue
            .define(named("standard"), definition(None))
            .expect("the define lands");
        catalogue
            .define(named("spare"), definition(None))
            .expect("the spare lands");
        catalogue
            .retire(&named("spare"))
            .expect("the spare retires");

        assert!(catalogue.assignable(&named("standard")));
        assert!(
            !catalogue.assignable(&named("spare")),
            "a retired entry is not assignable"
        );
        assert!(!catalogue.assignable(&named("ghost")));
        assert_eq!(
            catalogue
                .resolve(&named("standard"))
                .map(|entry| entry.name().as_str()),
            Some("standard")
        );
    }

    #[test]
    fn restore_rehydrates_every_recorded_fact() {
        let profile = ExecutionProfile::restore(
            named("standard"),
            definition(Some("nightly")),
            ProfileState::Retired,
            7,
        );

        assert_eq!(profile.name().as_str(), "standard");
        assert_eq!(
            profile.fallback().map(|name| name.as_str()),
            Some("nightly")
        );
        assert_eq!(profile.state(), ProfileState::Retired);
        assert_eq!(profile.version(), 7);
    }
}
