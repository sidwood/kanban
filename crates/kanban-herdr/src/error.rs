use std::fmt;

/// Why the Herdr client refused or failed.
#[derive(Debug, PartialEq, Eq)]
pub enum HerdrError {
    /// The socket path could not be resolved.
    HomeUnknown,
    /// The per-session socket is missing or not reachable.
    SocketMissing { path: String },
    /// The socket could not be opened.
    Connect { path: String, source: String },
    /// A frame could not be written.
    Write(String),
    /// A frame could not be read.
    Read(String),
    /// The response was not valid JSON for the protocol.
    Decode(String),
    /// Herdr reported an error on the wire.
    Remote { message: String },
    /// The session mapping did not match the product workspace.
    WorkspaceMismatch { expected: String, observed: String },
    /// The subscription ended before the wait completed.
    Disconnected,
    /// A bounded read found nothing waiting inside its window; the
    /// connection is still open and a partially arrived line is kept.
    TimedOut,
    /// The session name is not one safe path segment.
    InvalidSessionName,
}

impl fmt::Display for HerdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HomeUnknown => write!(f, "the Herdr sessions directory could not be resolved"),
            Self::SocketMissing { path } => {
                write!(f, "the Herdr session socket `{path}` is not available")
            }
            Self::Connect { path, source } => {
                write!(f, "could not connect to `{path}`: {source}")
            }
            Self::Write(message) => write!(f, "could not write to the Herdr socket: {message}"),
            Self::Read(message) => write!(f, "could not read from the Herdr socket: {message}"),
            Self::Decode(message) => write!(f, "invalid Herdr frame: {message}"),
            Self::Remote { message } => write!(f, "Herdr refused: {message}"),
            Self::WorkspaceMismatch { expected, observed } => write!(
                f,
                "the Herdr workspace `{observed}` does not map to the product workspace `{expected}`"
            ),
            Self::Disconnected => write!(f, "the Herdr session disconnected"),
            Self::TimedOut => write!(f, "no Herdr frame arrived inside the window"),
            Self::InvalidSessionName => {
                write!(f, "a Herdr session name must be one safe path segment")
            }
        }
    }
}

impl std::error::Error for HerdrError {}
