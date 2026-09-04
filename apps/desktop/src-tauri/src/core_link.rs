//! The shell's client of the core's Unix socket: typed request
//! frames out, response frames back, and the ordered event stream
//! (ADR-0003). No domain rules live here.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use kanban_dto::{ApiError, EventEnvelope};
use kanban_transport::frame::{FrameKind, RequestFrame, ResponseFrame};
use serde_json::Value;

/// How long one request may wait for the core's answer before the
/// shell reports the core as unresponsive.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// One guarded connection for queries and commands. Requests are
/// serialised through the mutex so a frame and its answer always
/// belong together, and the channel is held only while the boundary
/// between the two is certain: an answer that timed out, arrived
/// truncated, or could not be decoded may still be queued on that
/// socket, so the channel goes and the next request dials a new one.
pub struct CoreLink {
    socket_path: PathBuf,
    channel: Mutex<Option<Channel>>,
}

struct Channel {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl CoreLink {
    /// Connect to the core's socket. The core must be serving.
    pub fn connect(socket_path: &Path) -> std::io::Result<Self> {
        let channel = dial(socket_path)?;
        Ok(Self {
            socket_path: socket_path.to_owned(),
            channel: Mutex::new(Some(channel)),
        })
    }

    /// The socket this link is bound to.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Serve one named query.
    pub fn query(&self, operation: &str, payload: &Value) -> Result<Value, ApiError> {
        self.request(FrameKind::Query, operation, payload)
    }

    /// Serve one named command through the mutation guard.
    pub fn command(&self, operation: &str, payload: &Value) -> Result<Value, ApiError> {
        self.request(FrameKind::Command, operation, payload)
    }

    /// Send one frame and read its answer. Taking the channel out of
    /// its slot is the invalidation: it is put back only when the
    /// answer proves where this operation ended, so every other way
    /// out drops the connection before the error reaches the caller.
    /// The operation itself is never retried — a mutation whose
    /// outcome is unknown must stay unknown.
    fn request(
        &self,
        kind: FrameKind,
        operation: &str,
        payload: &Value,
    ) -> Result<Value, ApiError> {
        let frame = RequestFrame {
            kind,
            operation: Some(operation.to_owned()),
            payload: Some(payload.clone()),
        };
        let line = serde_json::to_string(&frame)
            .map_err(|_| ApiError::internal("the request frame could not be encoded"))?;
        let mut slot = self
            .channel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut channel = match slot.take() {
            Some(channel) => channel,
            // The previous operation left an uncertain boundary
            // behind; this one gets a connection of its own, dialled
            // once.
            None => dial(&self.socket_path)
                .map_err(|_| ApiError::internal("the core connection could not be reopened"))?,
        };
        match channel.exchange(&line) {
            Ok(ResponseFrame::Response { payload }) => {
                *slot = Some(channel);
                Ok(payload)
            }
            // A refusal is still this operation's answer, so the
            // connection is sound and stays.
            Ok(ResponseFrame::Error { error }) => {
                *slot = Some(channel);
                Err(error)
            }
            // Only a subscribed connection carries these; a request
            // connection that sees one is talking to the wrong stream
            // and cannot say which answer comes next.
            Ok(
                ResponseFrame::Event { .. }
                | ResponseFrame::Subscribed {}
                | ResponseFrame::Evicted {},
            ) => Err(ApiError::internal(
                "the core's event stream leaked onto a request connection",
            )),
            Err(failure) => Err(failure),
        }
    }
}

impl Channel {
    /// Write one request line and read the one answer that follows.
    fn exchange(&mut self, line: &str) -> Result<ResponseFrame, ApiError> {
        writeln!(self.writer, "{line}")
            .and_then(|_| self.writer.flush())
            .map_err(|_| ApiError::internal("the core connection is not writable"))?;
        let mut answer = String::new();
        let read = self
            .reader
            .read_line(&mut answer)
            .map_err(|_| ApiError::internal("the core connection is not readable"))?;
        if read == 0 {
            return Err(ApiError::internal(
                "the core closed the connection before answering",
            ));
        }
        serde_json::from_str(answer.trim_end())
            .map_err(|_| ApiError::internal("the core's answer could not be decoded"))
    }
}

/// Open one request connection, bounded by the read timeout.
fn dial(socket_path: &Path) -> std::io::Result<Channel> {
    let stream = UnixStream::connect(socket_path)?;
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    let writer = stream.try_clone()?;
    let reader = BufReader::new(stream);
    Ok(Channel { reader, writer })
}

/// Read the ordered event stream from a dedicated connection,
/// handing each envelope to `on_event` in arrival order, until the
/// core closes the socket. An evicted subscription is renewed in
/// place; the frames that did not fit the queue are gone, exactly as
/// the broker intends.
pub fn forward_events<F>(socket_path: &Path, mut on_event: F) -> std::io::Result<()>
where
    F: FnMut(EventEnvelope),
{
    let mut channel = subscribe(socket_path)?;
    let mut line = String::new();
    loop {
        line.clear();
        if channel.reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let frame: ResponseFrame = match serde_json::from_str(line.trim_end()) {
            Ok(frame) => frame,
            Err(_) => continue,
        };
        match frame {
            ResponseFrame::Event { event } => on_event(event),
            // The broker dropped this subscription for reading too
            // slowly; subscribe again and carry on from now.
            ResponseFrame::Evicted {} => {
                channel = subscribe(socket_path)?;
            }
            // The acknowledgement of (re)subscribing; nothing to do.
            ResponseFrame::Subscribed {} => {}
            // Queries and commands never travel on this connection.
            ResponseFrame::Response { .. } | ResponseFrame::Error { .. } => {}
        }
    }
}

/// Open one connection and put it into the subscribed state.
fn subscribe(socket_path: &Path) -> std::io::Result<Channel> {
    let stream = UnixStream::connect(socket_path)?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let frame = RequestFrame {
        kind: FrameKind::Subscribe,
        operation: None,
        payload: None,
    };
    let line = serde_json::to_string(&frame).expect("a subscribe frame encodes");
    writeln!(writer, "{line}").and_then(|_| writer.flush())?;
    let mut acknowledgement = String::new();
    reader.read_line(&mut acknowledgement)?;
    match serde_json::from_str::<ResponseFrame>(acknowledgement.trim_end()) {
        Ok(ResponseFrame::Subscribed {}) => Ok(Channel { reader, writer }),
        _ => Err(std::io::Error::other(
            "the core did not acknowledge the subscription",
        )),
    }
}
