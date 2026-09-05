//! The per-session Herdr socket client (DR-HB-12).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use serde_json::Value;

use crate::error::HerdrError;
use crate::paths::session_socket_in;
use crate::protocol::{HerdrRequest, HerdrResponse, Snapshot};
use crate::session::SessionMapping;

/// A wait request sent to one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitRequest {
    /// The condition Herdr should watch for.
    pub condition: String,
    /// How long to wait before returning.
    pub timeout_ms: u64,
}

/// A prompt delivered to one role tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRequest {
    /// The role tab to address.
    pub role: String,
    /// The operator message.
    pub message: String,
}

/// One connected Herdr session client.
pub struct SessionClient {
    mapping: SessionMapping,
    reader: BufReader<UnixStream>,
    stream: UnixStream,
}

impl SessionClient {
    /// Connect to the session this mapping resolves to under
    /// `socket_root` and verify the workspace mapping through an
    /// initial snapshot.
    pub fn connect(mapping: SessionMapping, socket_root: &Path) -> Result<Self, HerdrError> {
        let path = session_socket_in(socket_root, mapping.session())?;
        if !path.exists() {
            return Err(HerdrError::SocketMissing {
                path: path.display().to_string(),
            });
        }
        let stream = UnixStream::connect(&path).map_err(|source| HerdrError::Connect {
            path: path.display().to_string(),
            source: source.to_string(),
        })?;
        let reader = BufReader::new(stream.try_clone().map_err(|source| HerdrError::Connect {
            path: path.display().to_string(),
            source: source.to_string(),
        })?);
        let mut client = Self {
            mapping,
            reader,
            stream,
        };
        let snapshot = client.snapshot()?;
        client.mapping.verify_snapshot(&snapshot)?;
        Ok(client)
    }

    /// The mapping this client serves.
    pub fn mapping(&self) -> &SessionMapping {
        &self.mapping
    }

    /// Capture the full session state.
    pub fn snapshot(&mut self) -> Result<Snapshot, HerdrError> {
        self.request(HerdrRequest::Snapshot)
            .and_then(|response| match response {
                HerdrResponse::Snapshot(snapshot) => Ok(snapshot),
                HerdrResponse::Error { message } => Err(HerdrError::Remote { message }),
                other => Err(HerdrError::Decode(format!(
                    "expected snapshot, got `{other:?}`"
                ))),
            })
    }

    /// Start receiving push events on this connection.
    pub fn subscribe(&mut self) -> Result<(), HerdrError> {
        match self.request(HerdrRequest::Subscribe)? {
            HerdrResponse::Subscribed => Ok(()),
            HerdrResponse::Error { message } => Err(HerdrError::Remote { message }),
            other => Err(HerdrError::Decode(format!(
                "expected subscribed, got `{other:?}`"
            ))),
        }
    }

    /// Read one push event after subscribing.
    pub fn read_event(&mut self) -> Result<Value, HerdrError> {
        match self.read_response()? {
            HerdrResponse::Event { payload } => Ok(payload),
            HerdrResponse::Error { message } => Err(HerdrError::Remote { message }),
            other => Err(HerdrError::Decode(format!(
                "expected event, got `{other:?}`"
            ))),
        }
    }

    /// Wait for a condition with a timeout.
    pub fn wait(&mut self, request: WaitRequest) -> Result<(bool, Value), HerdrError> {
        match self.request(HerdrRequest::Wait {
            condition: request.condition,
            timeout_ms: request.timeout_ms,
        })? {
            HerdrResponse::WaitResult { met, detail } => Ok((met, detail)),
            HerdrResponse::Error { message } => Err(HerdrError::Remote { message }),
            other => Err(HerdrError::Decode(format!(
                "expected wait_result, got `{other:?}`"
            ))),
        }
    }

    /// Prompt one role tab.
    pub fn prompt(&mut self, request: PromptRequest) -> Result<bool, HerdrError> {
        match self.request(HerdrRequest::Prompt {
            role: request.role,
            message: request.message,
        })? {
            HerdrResponse::PromptResult { accepted } => Ok(accepted),
            HerdrResponse::Error { message } => Err(HerdrError::Remote { message }),
            other => Err(HerdrError::Decode(format!(
                "expected prompt_result, got `{other:?}`"
            ))),
        }
    }

    fn request(&mut self, request: HerdrRequest) -> Result<HerdrResponse, HerdrError> {
        let encoded = serde_json::to_string(&request)
            .map_err(|error| HerdrError::Write(error.to_string()))?;
        writeln!(self.stream, "{encoded}").map_err(|error| HerdrError::Write(error.to_string()))?;
        self.read_response()
    }

    fn read_response(&mut self) -> Result<HerdrResponse, HerdrError> {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .map_err(|error| HerdrError::Read(error.to_string()))?;
        if line.trim().is_empty() {
            return Err(HerdrError::Disconnected);
        }
        serde_json::from_str(line.trim()).map_err(|error| HerdrError::Decode(error.to_string()))
    }
}
