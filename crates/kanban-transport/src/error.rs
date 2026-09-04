//! The transport error type.

use std::path::PathBuf;

/// Everything that can go wrong while binding or serving the
/// current-user-only socket.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The socket directory could not be created, tightened, or
    /// read.
    #[error("the socket directory {path} is unusable: {source}")]
    Directory {
        /// The directory the socket was to live in.
        path: PathBuf,
        /// The underlying filesystem error.
        source: std::io::Error,
    },
    /// Another core is already serving at the socket path.
    #[error("refusing to bind {path}: another core is already serving there")]
    SocketInUse {
        /// The socket path another core holds.
        path: PathBuf,
    },
    /// The socket path is occupied by something that is not a
    /// socket.
    #[error("refusing to replace {path}: it is not a socket")]
    SocketPathOccupied {
        /// The occupied path.
        path: PathBuf,
    },
    /// A stale socket could not be removed before binding.
    #[error("the stale socket at {path} could not be removed: {source}")]
    StaleSocket {
        /// The stale socket path.
        path: PathBuf,
        /// The underlying filesystem error.
        source: std::io::Error,
    },
    /// Binding the socket failed.
    #[error("binding the socket at {path} failed: {source}")]
    Bind {
        /// The socket path that refused to bind.
        path: PathBuf,
        /// The underlying filesystem error.
        source: std::io::Error,
    },
    /// The socket's owner-only permissions could not be applied.
    #[error("securing the socket at {path} failed: {source}")]
    Secure {
        /// The socket path that could not be secured.
        path: PathBuf,
        /// The underlying filesystem error.
        source: std::io::Error,
    },
    /// Serving could not start: the accept thread could not be
    /// spawned.
    #[error("serving the socket at {path} failed: {source}")]
    Serve {
        /// The socket path that could not be served.
        path: PathBuf,
        /// The underlying thread-spawn failure.
        source: std::io::Error,
    },
}
