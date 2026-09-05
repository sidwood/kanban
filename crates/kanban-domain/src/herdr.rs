//! The Herdr session vocabulary: the session a Project's observation
//! resolves to, named or default (CONTEXT.md, DR-PH-07).

use crate::project::RegistrationError;
use crate::project::anchored;

/// The session a Project's Herdr binding resolves to: Herdr's default
/// session when the Project names none, or exactly one named session
/// (DR-PH-07). Absence is a first-class choice, not a missing value:
/// commands and connectors pass no session selection for it (DR-HB-20).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HerdrSession {
    /// Herdr's own default session.
    Default,
    /// One session Herdr serves under a name.
    Named(String),
}

impl HerdrSession {
    /// Accept one named session, refusing names that are not a single
    /// safe path segment.
    pub fn named(raw: &str) -> Result<Self, RegistrationError> {
        Ok(Self::Named(herdr_session_name(raw)?))
    }

    /// The named session, if this is one; the default session has no
    /// name to offer.
    pub fn as_name(&self) -> Option<&str> {
        match self {
            Self::Default => None,
            Self::Named(name) => Some(name),
        }
    }

    /// Whether this is Herdr's default session.
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Default)
    }
}

/// Reject Herdr session names that are not one safe path segment.
pub fn validate_herdr_session_name(raw: &str) -> Result<String, RegistrationError> {
    herdr_session_name(raw)
}

/// Accept one non-empty path segment that cannot escape a parent
/// directory when joined under the Herdr sessions root.
pub(crate) fn herdr_session_name(raw: &str) -> Result<String, RegistrationError> {
    let trimmed = anchored("Herdr session name", raw)?;
    if !is_single_safe_path_segment(&trimmed) {
        return Err(RegistrationError::InvalidHerdrSession);
    }
    Ok(trimmed)
}

fn is_single_safe_path_segment(segment: &str) -> bool {
    !segment.starts_with('/')
        && !segment.contains('\\')
        && !segment.contains('/')
        && segment != "."
        && segment != ".."
}

#[cfg(test)]
mod session_selection {
    use super::{HerdrSession, validate_herdr_session_name};
    use crate::project::RegistrationError;

    #[test]
    fn a_named_session_accepts_one_safe_segment() {
        let session = HerdrSession::named(" kanban-main ").expect("the name validates");

        assert_eq!(session, HerdrSession::Named("kanban-main".to_owned()));
        assert_eq!(session.as_name(), Some("kanban-main"));
        assert!(!session.is_default());
    }

    #[test]
    fn the_default_session_carries_no_name() {
        assert_eq!(HerdrSession::Default.as_name(), None);
        assert!(HerdrSession::Default.is_default());
    }

    #[test]
    fn a_named_session_refuses_unsafe_segments() {
        for refused in [
            "/absolute",
            "foo/bar",
            "..",
            "../escape",
            "still/../escape",
            ".",
        ] {
            assert_eq!(
                HerdrSession::named(refused),
                Err(RegistrationError::InvalidHerdrSession),
                "session `{refused}` must be refused"
            );
        }
    }

    #[test]
    fn validation_rejects_a_blank_session_name() {
        assert_eq!(
            validate_herdr_session_name(" "),
            Err(RegistrationError::Blank("Herdr session name"))
        );
    }
}
