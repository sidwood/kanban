//! The per-session Herdr socket client (DR-HB-12).

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::error::HerdrError;
use crate::paths::session_socket_in;
use crate::protocol::{HerdrRequest, HerdrResponse, Snapshot};
use crate::session::SessionMapping;

/// How long one request round-trip may block before the observer
/// treats the session as unresponsive (DR-HB-11).
pub const SESSION_IO_TIMEOUT: Duration = Duration::from_secs(5);

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
    socket_path: PathBuf,
    stream: UnixStream,
    /// Bytes of a response line that has only partly arrived, carried
    /// across a bounded read so a timeout cannot lose them.
    pending: Vec<u8>,
    /// Push events that overtook a request's response on the wire,
    /// held for the next [`SessionClient::read_event`] so the stream
    /// keeps its order while requests proceed.
    queued_events: VecDeque<Value>,
    /// The request deadline restored after a bounded event read.
    io_timeout: Duration,
}

impl SessionClient {
    /// Connect to one named session under `socket_root` with no
    /// request traffic: the snapshot handshake belongs to the caller.
    /// A caller that must stay interruptible can register its socket
    /// duplicate before the first blocking read (see
    /// [`SessionClient::duplicate_socket`]); `connect` performs the
    /// handshake up front instead.
    pub fn open(mapping: SessionMapping, socket_root: &Path) -> Result<Self, HerdrError> {
        Self::open_with_io_timeout(mapping, socket_root, SESSION_IO_TIMEOUT)
    }

    /// Connect with an explicit request I/O deadline, for tests that
    /// need a shorter window than production observation.
    pub fn open_with_io_timeout(
        mapping: SessionMapping,
        socket_root: &Path,
        io_timeout: Duration,
    ) -> Result<Self, HerdrError> {
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
        apply_io_deadline(&stream, io_timeout)?;
        Ok(Self {
            mapping,
            socket_path: path,
            stream,
            pending: Vec::new(),
            queued_events: VecDeque::new(),
            io_timeout,
        })
    }

    /// Connect to one named session under `socket_root` and verify the
    /// workspace mapping through an initial snapshot.
    pub fn connect(mapping: SessionMapping, socket_root: &Path) -> Result<Self, HerdrError> {
        let mut client = Self::open(mapping, socket_root)?;
        let snapshot = client.snapshot()?;
        client.mapping.verify_snapshot(&snapshot)?;
        Ok(client)
    }

    /// The mapping this client serves.
    pub fn mapping(&self) -> &SessionMapping {
        &self.mapping
    }

    /// A second handle to this client's socket. Shutting the duplicate
    /// down wakes a read blocked on this client without touching its
    /// buffered state, which is how an observer's owner stops a
    /// thread parked on [`SessionClient::read_event`].
    pub fn duplicate_socket(&self) -> Result<UnixStream, HerdrError> {
        self.stream
            .try_clone()
            .map_err(|source| HerdrError::Connect {
                path: self.socket_path.display().to_string(),
                source: source.to_string(),
            })
    }

    /// Read one push event, bounding the wait at `window`: no frame
    /// inside the window reports [`HerdrError::TimedOut`] with the
    /// connection still open, and a line that is only partly arrived
    /// when the window ends is kept for the next read, so nothing is
    /// lost. The socket blocks without a deadline again once this
    /// returns.
    pub fn read_event_within(&mut self, window: Duration) -> Result<Value, HerdrError> {
        apply_io_deadline(&self.stream, window)?;
        let response = self.read_event();
        apply_io_deadline(&self.stream, self.io_timeout)?;
        response
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

    /// Read one push event after subscribing. Events that overtook an
    /// in-flight request are served first, in arrival order.
    pub fn read_event(&mut self) -> Result<Value, HerdrError> {
        if let Some(payload) = self.queued_events.pop_front() {
            return Ok(payload);
        }
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
        // The subscription keeps pushing while a request awaits its
        // answer, so events read here are queued for `read_event`
        // instead of being refused as the wrong frame.
        loop {
            match self.read_response()? {
                HerdrResponse::Event { payload } => self.queued_events.push_back(payload),
                response => return Ok(response),
            }
        }
    }

    fn read_response(&mut self) -> Result<HerdrResponse, HerdrError> {
        loop {
            if let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=newline).collect();
                let text = std::str::from_utf8(&line)
                    .map_err(|error| HerdrError::Decode(error.to_string()))?;
                if text.trim().is_empty() {
                    return Err(HerdrError::Disconnected);
                }
                return serde_json::from_str(text.trim())
                    .map_err(|error| HerdrError::Decode(error.to_string()));
            }
            // The line is incomplete, so more bytes must arrive; a
            // bounded read that finds none leaves the part already
            // arrived in `pending` for the next call.
            let mut chunk = [0u8; 512];
            let read = self.stream.read(&mut chunk).map_err(|error| {
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
                    HerdrError::TimedOut
                } else {
                    HerdrError::Read(error.to_string())
                }
            })?;
            if read == 0 {
                return Err(HerdrError::Disconnected);
            }
            self.pending.extend_from_slice(&chunk[..read]);
        }
    }
}

fn apply_io_deadline(stream: &UnixStream, deadline: Duration) -> Result<(), HerdrError> {
    stream
        .set_read_timeout(Some(deadline))
        .map_err(|error| HerdrError::Read(error.to_string()))?;
    stream
        .set_write_timeout(Some(deadline))
        .map_err(|error| HerdrError::Write(error.to_string()))?;
    Ok(())
}
