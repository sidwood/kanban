//! Managed Herdr session socket locations.

use std::path::{Path, PathBuf};

use kanban_domain::HerdrSession;
use kanban_domain::validate_herdr_session_name;

use crate::error::HerdrError;

const SESSION_SOCKET: &str = "herdr.sock";

fn validated_session_name(session_name: &str) -> Result<&str, HerdrError> {
    validate_herdr_session_name(session_name)
        .map(|_| session_name)
        .map_err(|_| HerdrError::InvalidSessionName)
}

/// Herdr's config root, containing the default socket and named sessions.
pub fn herdr_sessions_dir() -> Result<PathBuf, HerdrError> {
    dirs::home_dir()
        .map(|dir| dir.join(".config/herdr"))
        .ok_or(HerdrError::HomeUnknown)
}

/// The per-session Herdr socket for `session`. The default session is
/// addressed by its own well-known socket; no session name travels
/// with the connection (DR-HB-20).
pub fn session_socket_path(session: &HerdrSession) -> Result<PathBuf, HerdrError> {
    session_socket_in(&herdr_sessions_dir()?, session)
}

/// Resolve a socket under an explicit Herdr config root, bypassing home discovery.
pub fn session_socket_in(root: &Path, session: &HerdrSession) -> Result<PathBuf, HerdrError> {
    let directory = match session.as_name() {
        None => root.to_path_buf(),
        Some(name) => root.join("sessions").join(validated_session_name(name)?),
    };
    Ok(directory.join(SESSION_SOCKET))
}

#[cfg(test)]
mod session_socket_paths {
    use std::path::Path;

    use kanban_domain::HerdrSession;

    use super::{herdr_sessions_dir, session_socket_in, session_socket_path};
    use crate::error::HerdrError;

    #[test]
    fn herdr_sessions_dir_matches_the_installed_cli_config_root() {
        assert_eq!(
            herdr_sessions_dir().expect("home is known in tests"),
            dirs::home_dir()
                .expect("home is known in tests")
                .join(".config/herdr")
        );
    }

    #[test]
    fn a_named_session_socket_matches_the_installed_cli_layout() {
        let session = HerdrSession::named("kanban-main").expect("the name validates");
        assert_eq!(
            session_socket_path(&session).expect("home is known in tests"),
            dirs::home_dir()
                .expect("home is known in tests")
                .join(".config/herdr/sessions/kanban-main/herdr.sock")
        );
    }

    #[test]
    fn the_default_session_socket_is_well_known() {
        assert_eq!(
            session_socket_path(&HerdrSession::Default).expect("home is known in tests"),
            dirs::home_dir()
                .expect("home is known in tests")
                .join(".config/herdr/herdr.sock")
        );
        assert_eq!(
            session_socket_in(Path::new("/tmp/herdr"), &HerdrSession::Default)
                .expect("the default session needs no name"),
            Path::new("/tmp/herdr/herdr.sock")
        );
    }

    #[test]
    fn session_socket_in_uses_the_injected_root() {
        let session = HerdrSession::named("wave-main").expect("the name validates");
        assert_eq!(
            session_socket_in(Path::new("/tmp/herdr"), &session).expect("the name validates"),
            Path::new("/tmp/herdr/sessions/wave-main/herdr.sock")
        );
    }

    #[test]
    fn session_socket_paths_refuse_unsafe_session_names() {
        for session in ["/absolute", "foo/bar", "..", "../escape"] {
            // The enum stays open: the socket join re-validates so an
            // unvalidated name can never escape the sessions root.
            let refused = HerdrSession::Named(session.to_owned());
            assert_eq!(
                session_socket_path(&refused),
                Err(HerdrError::InvalidSessionName),
                "session `{session}` must be refused before joining"
            );
        }
    }

    #[test]
    fn session_socket_in_refuses_unsafe_session_names() {
        let refused = HerdrSession::Named("../escape".to_owned());
        assert_eq!(
            session_socket_in(Path::new("/tmp/herdr"), &refused),
            Err(HerdrError::InvalidSessionName)
        );
    }
}
