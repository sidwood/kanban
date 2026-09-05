//! Managed Herdr session socket locations.

use std::path::{Path, PathBuf};

use kanban_domain::HerdrSession;
use kanban_domain::validate_herdr_session_name;

use crate::error::HerdrError;

/// The socket file name Herdr serves its default session under.
const DEFAULT_SESSION_SOCKET: &str = "default.sock";

fn validated_session_name(session_name: &str) -> Result<&str, HerdrError> {
    validate_herdr_session_name(session_name)
        .map(|_| session_name)
        .map_err(|_| HerdrError::InvalidSessionName)
}

/// The root directory holding one socket per Herdr session.
pub fn herdr_sessions_dir() -> Result<PathBuf, HerdrError> {
    dirs::data_dir()
        .map(|dir| dir.join("Herdr").join("sessions"))
        .ok_or(HerdrError::HomeUnknown)
}

/// The per-session Herdr socket for `session`. The default session is
/// addressed by its own well-known socket; no session name travels
/// with the connection (DR-HB-20).
pub fn session_socket_path(session: &HerdrSession) -> Result<PathBuf, HerdrError> {
    Ok(herdr_sessions_dir()?.join(session_socket_file(session)?))
}

/// Resolve a socket path inside `root` for tests and injected roots.
pub fn session_socket_in(root: &Path, session: &HerdrSession) -> Result<PathBuf, HerdrError> {
    Ok(root.join(session_socket_file(session)?))
}

/// The socket file one session serves under: `default.sock` for the
/// default session, one validated segment per named session.
fn session_socket_file(session: &HerdrSession) -> Result<String, HerdrError> {
    match session.as_name() {
        None => Ok(DEFAULT_SESSION_SOCKET.to_owned()),
        Some(name) => Ok(format!("{}.sock", validated_session_name(name)?)),
    }
}

#[cfg(test)]
mod session_socket_paths {
    use std::path::Path;

    use kanban_domain::HerdrSession;

    use super::{herdr_sessions_dir, session_socket_in, session_socket_path};
    use crate::error::HerdrError;

    #[test]
    fn herdr_sessions_dir_sits_under_application_support() {
        let dir = herdr_sessions_dir().expect("home is known in tests");
        let components: Vec<_> = dir
            .components()
            .map(|component| component.as_os_str())
            .collect();
        let tail = components
            .iter()
            .rev()
            .take(4)
            .rev()
            .cloned()
            .collect::<Vec<_>>();
        let expected: Vec<_> = ["Library", "Application Support", "Herdr", "sessions"]
            .iter()
            .map(std::ffi::OsStr::new)
            .collect();
        assert_eq!(tail, expected);
    }

    #[test]
    fn a_named_session_socket_names_the_session_file() {
        let session = HerdrSession::named("kanban-main").expect("the name validates");
        assert_eq!(
            session_socket_path(&session).expect("home is known in tests"),
            herdr_sessions_dir()
                .expect("home is known in tests")
                .join("kanban-main.sock")
        );
    }

    #[test]
    fn the_default_session_socket_is_well_known() {
        assert_eq!(
            session_socket_in(Path::new("/tmp/herdr"), &HerdrSession::Default)
                .expect("the default session needs no name"),
            Path::new("/tmp/herdr/default.sock")
        );
    }

    #[test]
    fn session_socket_in_uses_the_injected_root() {
        let session = HerdrSession::named("wave-main").expect("the name validates");
        assert_eq!(
            session_socket_in(Path::new("/tmp/herdr"), &session).expect("the name validates"),
            Path::new("/tmp/herdr/wave-main.sock")
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
