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
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// One guarded connection for queries and commands. Requests are
/// serialised through the mutex so a frame and its answer always
/// belong together.
pub struct CoreLink {
    socket_path: PathBuf,
    channel: Mutex<Channel>,
}

struct Channel {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl CoreLink {
    /// Connect to the core's socket. The core must be serving.
    pub fn connect(socket_path: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(socket_path)?;
        stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
        let writer = stream.try_clone()?;
        let reader = BufReader::new(stream);
        Ok(Self {
            socket_path: socket_path.to_owned(),
            channel: Mutex::new(Channel { reader, writer }),
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
        let mut channel = self
            .channel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        writeln!(channel.writer, "{line}")
            .and_then(|_| channel.writer.flush())
            .map_err(|_| ApiError::internal("the core connection is not writable"))?;
        let mut line = String::new();
        channel
            .reader
            .read_line(&mut line)
            .map_err(|_| ApiError::internal("the core connection is not readable"))?;
        let frame: ResponseFrame = serde_json::from_str(line.trim_end())
            .map_err(|_| ApiError::internal("the core's answer could not be decoded"))?;
        match frame {
            ResponseFrame::Response { payload } => Ok(payload),
            ResponseFrame::Error { error } => Err(error),
            // Only a subscribed connection carries these; a request
            // connection that sees one is talking to the wrong stream.
            ResponseFrame::Event { .. }
            | ResponseFrame::Subscribed {}
            | ResponseFrame::Evicted {} => Err(ApiError::internal(
                "the core's event stream leaked onto a request connection",
            )),
        }
    }
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
