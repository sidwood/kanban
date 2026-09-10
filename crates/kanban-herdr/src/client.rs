//! The per-session Herdr socket client (DR-HB-12).

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::HerdrError;
use crate::paths::session_socket_in;
use crate::protocol::{HerdrRequest, HerdrResponse, Snapshot};
use crate::session::SessionMapping;

/// How long one request round-trip may block before the observer
/// treats the session as unresponsive (DR-HB-11).
///
/// This bounds the whole round trip, not each read within it: the
/// write, every push event that overtakes the answer, the fragments
/// a partly arrived answer still owes, and the decode all spend the
/// same budget. A session that keeps feeding complete frames, or
/// one response fragment at a time, is therefore unable to hold a
/// request — or the delivery authorisation waiting on it — open
/// past this (KAN-T142-AC2).
pub const SESSION_IO_TIMEOUT: Duration = Duration::from_secs(5);

const MAX_RESPONSE_FRAME_BYTES: usize = 4 * 1024 * 1024;

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

/// The role tab dispatch wakes: the Project Coordinator, never an
/// implementation agent (DR-HB-14, DR-HB-16).
pub const COORDINATOR_ROLE: &str = "coordinator";

/// A wake delivered to the Project Coordinator on dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRequest {
    /// The Dispatch Request that just entered the queue.
    pub dispatch_request_id: u64,
}

/// One connected Herdr session client.
pub struct SessionClient {
    mapping: SessionMapping,
    socket_path: PathBuf,
    stream: UnixStream,
    /// Bytes of a response line that has only partly arrived, carried
    /// across a bounded read so a timeout cannot lose them.
    pending: Vec<u8>,
    pending_scan_from: usize,
    /// Push events that overtook a request's response on the wire,
    /// held for the next [`SessionClient::read_event`] so the stream
    /// keeps its order while requests proceed.
    queued_events: VecDeque<Value>,
    /// The deadline one whole request has to answer inside, restored
    /// on the socket after a bounded event read.
    io_timeout: Duration,
    /// Set once a request ended with its answer still owed, so this
    /// connection can no longer say which request an arriving frame
    /// belongs to.
    abandoned: bool,
    #[cfg(feature = "test-support")]
    decode_pause: Option<Duration>,
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
            pending_scan_from: 0,
            queued_events: VecDeque::new(),
            io_timeout,
            abandoned: false,
            #[cfg(feature = "test-support")]
            decode_pause: None,
        })
    }

    /// Buffer one complete response and pause its decode in transport tests.
    #[cfg(feature = "test-support")]
    pub fn buffer_response_for_test(&mut self, response: HerdrResponse, decode_pause: Duration) {
        let mut frame = serde_json::to_vec(&response).expect("the test response encodes");
        frame.push(b'\n');
        self.pending.extend_from_slice(&frame);
        self.decode_pause = Some(decode_pause);
    }

    /// Buffer raw response bytes in transport tests.
    #[cfg(feature = "test-support")]
    pub fn buffer_raw_response_for_test(&mut self, frame: &[u8]) {
        self.pending.extend_from_slice(frame);
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
        // Restoring the request deadline is hygiene, not outcome: a
        // stream that dropped during the window fails this control
        // call too, and the caller needs the read's disconnection,
        // not the restore's socket error standing in for it.
        let _ = apply_io_deadline(&self.stream, self.io_timeout);
        response
    }

    /// Capture the full session state.
    pub fn snapshot(&mut self) -> Result<Snapshot, HerdrError> {
        self.request(HerdrRequest::Snapshot, |response| match response {
            HerdrResponse::Snapshot(snapshot) => Ok(snapshot),
            HerdrResponse::Error { message } => Err(HerdrError::Remote { message }),
            other => Err(response_mismatch("snapshot", other)),
        })
    }

    /// Start receiving push events on this connection.
    pub fn subscribe(&mut self) -> Result<(), HerdrError> {
        self.request(HerdrRequest::Subscribe, |response| match response {
            HerdrResponse::Subscribed => Ok(()),
            HerdrResponse::Error { message } => Err(HerdrError::Remote { message }),
            other => Err(response_mismatch("subscribed", other)),
        })
    }

    /// Read one push event after subscribing. Events that overtook an
    /// in-flight request are served first, in arrival order.
    pub fn read_event(&mut self) -> Result<Value, HerdrError> {
        if let Some(payload) = self.queued_events.pop_front() {
            return Ok(payload);
        }
        // A subscription is not a request: it carries no deadline of
        // its own, only the window its caller applied, and a window
        // that closes empty leaves the connection open.
        self.usable()?;
        let response = match self.read_response() {
            Ok(response) => response,
            Err(error @ HerdrError::Decode(_)) => {
                self.abandon();
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        match response {
            HerdrResponse::Event { payload } => Ok(payload),
            HerdrResponse::Error { message } => Err(HerdrError::Remote { message }),
            other => {
                self.abandon();
                Err(response_mismatch("event", other))
            }
        }
    }

    /// Wait for a condition with a timeout.
    pub fn wait(&mut self, request: WaitRequest) -> Result<(bool, Value), HerdrError> {
        self.request(
            HerdrRequest::Wait {
                condition: request.condition,
                timeout_ms: request.timeout_ms,
            },
            |response| match response {
                HerdrResponse::WaitResult { met, detail } => Ok((met, detail)),
                HerdrResponse::Error { message } => Err(HerdrError::Remote { message }),
                other => Err(response_mismatch("wait_result", other)),
            },
        )
    }

    /// Prompt one role tab.
    pub fn prompt(&mut self, request: PromptRequest) -> Result<bool, HerdrError> {
        self.request(
            HerdrRequest::Prompt {
                role: request.role,
                message: request.message,
            },
            |response| match response {
                HerdrResponse::PromptResult { accepted } => Ok(accepted),
                HerdrResponse::Error { message } => Err(HerdrError::Remote { message }),
                other => Err(response_mismatch("prompt_result", other)),
            },
        )
    }

    /// Wake the Project Coordinator over this session socket. The
    /// role is fixed: Kanban never uses this path to launch an
    /// implementation agent.
    pub fn wake_coordinator(&mut self, request: WakeRequest) -> Result<bool, HerdrError> {
        self.request(
            HerdrRequest::Wake {
                role: COORDINATOR_ROLE.to_owned(),
                dispatch_request_id: request.dispatch_request_id,
            },
            |response| match response {
                HerdrResponse::WakeResult { accepted } => Ok(accepted),
                HerdrResponse::Error { message } => Err(HerdrError::Remote { message }),
                other => Err(response_mismatch("wake_result", other)),
            },
        )
    }

    /// Send one request and return its answer inside one deadline:
    /// `io_timeout` from the first write to the decoded response,
    /// however many event frames or response fragments arrive in
    /// between. A request that runs out of time abandons the
    /// connection rather than leaving an answer in flight that could
    /// satisfy the next one (KAN-T142-AC2).
    fn request<T>(
        &mut self,
        request: HerdrRequest,
        decode: impl FnOnce(HerdrResponse) -> Result<T, HerdrError>,
    ) -> Result<T, HerdrError> {
        let deadline = Instant::now() + self.io_timeout;
        let answered = self.request_by(request, deadline).and_then(decode);
        let answered = if Instant::now() >= deadline {
            Err(HerdrError::TimedOut)
        } else {
            answered
        };
        match answered {
            // The request's own answer was not proven wholly read, so
            // a frame still in flight has nothing to attribute it to.
            Err(
                HerdrError::TimedOut
                | HerdrError::Read(_)
                | HerdrError::Write(_)
                | HerdrError::Decode(_)
                | HerdrError::Disconnected,
            ) => self.abandon(),
            // A recognized response frame was consumed whole, so the
            // stream is still in step and only the shrunken socket
            // deadline has to be put back.
            _ => {
                let _ = apply_io_deadline(&self.stream, self.io_timeout);
            }
        }
        answered
    }

    fn request_by(
        &mut self,
        request: HerdrRequest,
        deadline: Instant,
    ) -> Result<HerdrResponse, HerdrError> {
        self.usable()?;
        let encoded = serde_json::to_string(&request)
            .map_err(|error| HerdrError::Write(error.to_string()))?;
        self.bound_next_io(deadline)?;
        writeln!(self.stream, "{encoded}").map_err(write_failure)?;
        // The subscription keeps pushing while a request awaits its
        // answer, so events read here are queued for `read_event`
        // instead of being refused as the wrong frame — and they
        // spend this request's deadline rather than renewing it.
        loop {
            match self.read_response_by(deadline)? {
                HerdrResponse::Event { payload } => self.queued_events.push_back(payload),
                response => return Ok(response),
            }
        }
    }

    fn usable(&self) -> Result<(), HerdrError> {
        if self.abandoned {
            return Err(HerdrError::Disconnected);
        }
        Ok(())
    }

    /// Give up on this connection. A request that ended with its
    /// answer still owed leaves the stream unable to tell that late
    /// answer from the next request's, so the socket is shut down and
    /// every later call refuses: a prompt result arriving after its
    /// deadline can then neither answer another request nor be
    /// recorded as a delivery (KAN-T142-AC2).
    fn abandon(&mut self) {
        self.abandoned = true;
        self.pending.clear();
        self.pending_scan_from = 0;
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    /// Bound the next socket operation by whatever is left of
    /// `deadline`, refusing outright once nothing is: a zero timeout
    /// means *no* timeout to the socket layer, so an expired request
    /// must never reach it.
    fn bound_next_io(&self, deadline: Instant) -> Result<(), HerdrError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(HerdrError::TimedOut);
        }
        apply_io_deadline(&self.stream, remaining)
    }

    /// Read one frame with no deadline beyond the one already on the
    /// socket: the observation stream's bound belongs to its caller.
    fn read_response(&mut self) -> Result<HerdrResponse, HerdrError> {
        loop {
            if let Some(response) = self.buffered_response()? {
                return Ok(response);
            }
            self.fill()?;
        }
    }

    fn read_response_by(&mut self, deadline: Instant) -> Result<HerdrResponse, HerdrError> {
        loop {
            // Expiry is read before the buffer is served: bytes
            // already in hand do not make a request that has run out
            // of time a timely one, and working through a long run of
            // queued frames spends the deadline like any other step.
            if Instant::now() >= deadline {
                return Err(HerdrError::TimedOut);
            }
            let buffered = self.buffered_response();
            if Instant::now() >= deadline {
                return Err(HerdrError::TimedOut);
            }
            if let Some(response) = buffered? {
                return Ok(response);
            }
            self.bound_next_io(deadline)?;
            self.fill()?;
        }
    }

    /// The next whole line already buffered, decoded, or `None` when
    /// the line has only partly arrived.
    fn buffered_response(&mut self) -> Result<Option<HerdrResponse>, HerdrError> {
        let newline = self.pending[self.pending_scan_from..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| self.pending_scan_from + offset);
        let Some(newline) = newline else {
            self.pending_scan_from = self.pending.len();
            if self.pending.len() >= MAX_RESPONSE_FRAME_BYTES {
                return Err(response_frame_too_large());
            }
            return Ok(None);
        };
        if newline + 1 > MAX_RESPONSE_FRAME_BYTES {
            return Err(response_frame_too_large());
        }
        let line: Vec<u8> = self.pending.drain(..=newline).collect();
        self.pending_scan_from = 0;
        let text =
            std::str::from_utf8(&line).map_err(|error| HerdrError::Decode(error.to_string()))?;
        #[cfg(feature = "test-support")]
        if let Some(pause) = self.decode_pause.take() {
            std::thread::sleep(pause);
        }
        if text.trim().is_empty() {
            return Err(HerdrError::Disconnected);
        }
        serde_json::from_str(text.trim())
            .map(Some)
            .map_err(|error| HerdrError::Decode(error.to_string()))
    }

    /// One bounded read appending whatever arrived. A read that finds
    /// nothing inside the socket's current deadline leaves the part
    /// already arrived in `pending` for the next call.
    fn fill(&mut self) -> Result<(), HerdrError> {
        let mut chunk = [0u8; 512];
        let remaining = MAX_RESPONSE_FRAME_BYTES.saturating_sub(self.pending.len());
        if remaining == 0 {
            return Err(response_frame_too_large());
        }
        let read = self
            .stream
            .read(&mut chunk[..remaining.min(512)])
            .map_err(read_failure)?;
        if read == 0 {
            return Err(HerdrError::Disconnected);
        }
        self.pending.extend_from_slice(&chunk[..read]);
        Ok(())
    }
}

fn response_frame_too_large() -> HerdrError {
    HerdrError::Decode(format!(
        "Herdr response frame exceeds the {MAX_RESPONSE_FRAME_BYTES}-byte limit"
    ))
}

fn response_mismatch(expected: &str, observed: HerdrResponse) -> HerdrError {
    HerdrError::Decode(format!("expected {expected}, got `{observed:?}`"))
}

fn read_failure(error: std::io::Error) -> HerdrError {
    if expired(&error) {
        HerdrError::TimedOut
    } else {
        HerdrError::Read(error.to_string())
    }
}

fn write_failure(error: std::io::Error) -> HerdrError {
    if expired(&error) {
        HerdrError::TimedOut
    } else {
        HerdrError::Write(error.to_string())
    }
}

fn expired(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
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
