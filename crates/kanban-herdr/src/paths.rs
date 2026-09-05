//! Managed Herdr session socket locations.

use std::path::{Path, PathBuf};

use crate::error::HerdrError;

/// The root directory holding one socket per named Herdr session.
pub fn herdr_sessions_dir() -> Result<PathBuf, HerdrError> {
    dirs::data_dir()
        .map(|dir| dir.join("Herdr").join("sessions"))
        .ok_or(HerdrError::HomeUnknown)
}

/// The per-session Herdr socket for `session_name`.
pub fn session_socket_path(session_name: &str) -> Result<PathBuf, HerdrError> {
    Ok(herdr_sessions_dir()?.join(format!("{session_name}.sock")))
}

/// Resolve a socket path inside `root` for tests and injected roots.
pub fn session_socket_in(root: &Path, session_name: &str) -> PathBuf {
    root.join(format!("{session_name}.sock"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{herdr_sessions_dir, session_socket_in, session_socket_path};

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
    fn session_socket_path_names_the_session_file() {
        assert_eq!(
            session_socket_path("kanban-main").expect("home is known in tests"),
            herdr_sessions_dir()
                .expect("home is known in tests")
                .join("kanban-main.sock")
        );
    }

    #[test]
    fn session_socket_in_uses_the_injected_root() {
        assert_eq!(
            session_socket_in(Path::new("/tmp/herdr"), "wave-main"),
            Path::new("/tmp/herdr/wave-main.sock")
        );
    }
}
