//! The Initiative entity: a lightweight, non-nested folder that
//! groups Projects (CONTEXT.md). An Initiative has no parent, so
//! nesting cannot be expressed; archive is its terminal state and
//! nothing is ever deleted.

use std::fmt;

/// The identity of one Initiative. Assigned once by storage and
/// immutable afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InitiativeId(u64);

impl InitiativeId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for InitiativeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The closed lifecycle vocabulary for an Initiative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitiativeState {
    /// Created and mutable.
    Active,
    /// Terminal: every recorded fact is preserved and no further
    /// change is legal.
    Archived,
}

/// Why a name was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    /// The name holds nothing but whitespace.
    Blank,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blank => write!(f, "an Initiative name cannot be blank"),
        }
    }
}

impl std::error::Error for NameError {}

/// A validated, trimmed Initiative name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitiativeName(String);

impl InitiativeName {
    /// Accept any name that holds at least one non-whitespace
    /// character; surrounding whitespace is not part of the name.
    pub fn new(raw: &str) -> Result<Self, NameError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(NameError::Blank);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The trimmed name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitiativeError {
    /// The Initiative is already archived.
    AlreadyArchived,
    /// Archived is terminal: the Initiative accepts no further
    /// changes.
    ArchivedIsTerminal,
}

impl fmt::Display for InitiativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyArchived => write!(f, "the Initiative is already archived"),
            Self::ArchivedIsTerminal => {
                write!(
                    f,
                    "archived is terminal; the Initiative accepts no further changes"
                )
            }
        }
    }
}

impl std::error::Error for InitiativeError {}

/// One Initiative aggregate. The version counts applied changes:
/// creation lands at 1 and every legal transition bumps it, so a
/// stored version is all a caller needs for optimistic checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Initiative {
    id: InitiativeId,
    name: InitiativeName,
    state: InitiativeState,
    version: u64,
}

impl Initiative {
    /// A fresh Initiative: active, at version 1.
    pub fn new(id: InitiativeId, name: InitiativeName) -> Self {
        Self {
            id,
            name,
            state: InitiativeState::Active,
            version: 1,
        }
    }

    /// Rehydrate a stored Initiative exactly as it was recorded.
    pub fn restore(
        id: InitiativeId,
        name: InitiativeName,
        state: InitiativeState,
        version: u64,
    ) -> Self {
        Self {
            id,
            name,
            state,
            version,
        }
    }

    /// The immutable identity.
    pub fn id(&self) -> InitiativeId {
        self.id
    }

    /// The current name.
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// The lifecycle state.
    pub fn state(&self) -> InitiativeState {
        self.state
    }

    /// Whether the Initiative is archived.
    pub fn is_archived(&self) -> bool {
        self.state == InitiativeState::Archived
    }

    /// The number of applied changes, for optimistic version checks.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Rename an active Initiative. Archived is terminal.
    pub fn rename(&mut self, name: InitiativeName) -> Result<(), InitiativeError> {
        if self.state == InitiativeState::Archived {
            return Err(InitiativeError::ArchivedIsTerminal);
        }
        self.name = name;
        self.version += 1;
        Ok(())
    }

    /// Archive an active Initiative. Archived is terminal, so a
    /// second archive is refused rather than absorbed.
    pub fn archive(&mut self) -> Result<(), InitiativeError> {
        if self.state == InitiativeState::Archived {
            return Err(InitiativeError::AlreadyArchived);
        }
        self.state = InitiativeState::Archived;
        self.version += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Initiative, InitiativeError, InitiativeId, InitiativeName, InitiativeState, NameError,
    };

    fn named(raw: &str) -> InitiativeName {
        InitiativeName::new(raw).expect("a non-blank name is accepted")
    }

    #[test]
    fn a_blank_name_is_refused() {
        for blank in ["", " ", " \t "] {
            assert_eq!(InitiativeName::new(blank), Err(NameError::Blank));
        }
    }

    #[test]
    fn a_name_is_stored_trimmed() {
        assert_eq!(named("  Reliability  ").as_str(), "Reliability");
    }

    #[test]
    fn a_fresh_initiative_is_active_at_version_one() {
        let initiative = Initiative::new(InitiativeId::new(7), named("Reliability"));

        assert_eq!(initiative.id(), InitiativeId::new(7));
        assert_eq!(initiative.name(), "Reliability");
        assert_eq!(initiative.state(), InitiativeState::Active);
        assert!(!initiative.is_archived());
        assert_eq!(initiative.version(), 1);
    }

    #[test]
    fn renaming_changes_the_name_and_bumps_the_version() {
        let mut initiative = Initiative::new(InitiativeId::new(1), named("Alpha"));

        initiative.rename(named("Beta")).expect("active renames");

        assert_eq!(initiative.name(), "Beta");
        assert_eq!(initiative.version(), 2);
    }

    #[test]
    fn archiving_moves_to_the_terminal_state_and_bumps_the_version() {
        let mut initiative = Initiative::new(InitiativeId::new(1), named("Alpha"));

        initiative.archive().expect("active archives");

        assert_eq!(initiative.state(), InitiativeState::Archived);
        assert!(initiative.is_archived());
        assert_eq!(initiative.version(), 2);
    }

    #[test]
    fn renaming_an_archived_initiative_is_refused() {
        let mut initiative = Initiative::new(InitiativeId::new(1), named("Alpha"));
        initiative.archive().expect("active archives");

        assert_eq!(
            initiative.rename(named("Beta")),
            Err(InitiativeError::ArchivedIsTerminal)
        );
        assert_eq!(initiative.name(), "Alpha", "the refusal changed nothing");
        assert_eq!(initiative.version(), 2, "the refusal changed nothing");
    }

    #[test]
    fn archiving_twice_is_refused() {
        let mut initiative = Initiative::new(InitiativeId::new(1), named("Alpha"));
        initiative.archive().expect("the first archive applies");

        assert_eq!(initiative.archive(), Err(InitiativeError::AlreadyArchived));
        assert_eq!(initiative.version(), 2, "the refusal changed nothing");
    }

    #[test]
    fn restore_rehydrates_every_recorded_fact() {
        let initiative = Initiative::restore(
            InitiativeId::new(9),
            named("Archived work"),
            InitiativeState::Archived,
            4,
        );

        assert_eq!(initiative.id().value(), 9);
        assert_eq!(initiative.name(), "Archived work");
        assert!(initiative.is_archived());
        assert_eq!(initiative.version(), 4);
    }

    #[test]
    fn ids_order_by_their_value() {
        let mut ids = [
            InitiativeId::new(3),
            InitiativeId::new(1),
            InitiativeId::new(2),
        ];
        ids.sort();

        assert_eq!(
            ids.map(|id| id.value()),
            [1, 2, 3],
            "initiatives list in a stable order"
        );
    }
}
