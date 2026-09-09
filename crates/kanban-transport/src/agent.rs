//! Private, pre-connected application channel inherited by one stdio child.
//! No credential, capability selector, subscription, or operator route is on it.
use kanban_app::agent_authorization::AgentSession;
use kanban_dto::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

const FRAME_LIMIT: u64 = 4 * 1024 * 1024;

/// The service owns process launch; the socket transport owns the connection.
pub trait AgentLauncher: Send + Sync {
    fn serve(&self, capability_id: u64, channel: BufReader<UnixStream>) -> io::Result<()>;
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAttachment {
    pub capability_id: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum AgentRequest {
    List,
    Call { operation: String, payload: Value },
}

pub struct AgentClient(Mutex<BufReader<UnixStream>>);
impl AgentClient {
    pub fn new(channel: UnixStream) -> Self {
        Self(Mutex::new(BufReader::new(channel)))
    }
    pub fn operations(&self) -> Result<Vec<String>, ApiError> {
        serde_json::from_value(self.request(&AgentRequest::List)?).map_err(|_| unavailable())
    }
    pub fn call(&self, operation: &str, payload: &Value) -> Result<Value, ApiError> {
        self.request(&AgentRequest::Call {
            operation: operation.to_owned(),
            payload: payload.clone(),
        })
    }
    fn request(&self, request: &AgentRequest) -> Result<Value, ApiError> {
        let mut stream = self.0.lock().map_err(|_| unavailable())?;
        write(stream.get_mut(), request).map_err(|_| unavailable())?;
        let bytes = read(&mut *stream).map_err(|_| unavailable())?;
        serde_json::from_slice::<Result<Value, ApiError>>(&bytes).map_err(|_| unavailable())?
    }
}

/// Called only after the service has selected and checked this run's binding.
pub fn serve_agent_channel(channel: UnixStream, session: AgentSession) -> io::Result<()> {
    let mut stream = BufReader::new(channel);
    loop {
        let bytes = read(&mut stream)?;
        if bytes.is_empty() {
            return Ok(());
        }
        let request = serde_json::from_slice::<AgentRequest>(&bytes);
        let response = match request {
            Ok(AgentRequest::List) => session
                .operations()
                .and_then(|value| serde_json::to_value(value).map_err(|_| unavailable())),
            Ok(AgentRequest::Call { operation, payload }) => session.call(&operation, &payload),
            Err(_) => Err(ApiError::invalid_request("malformed agent frame")),
        };
        write(stream.get_mut(), &response)?;
    }
}
fn read(stream: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    stream.take(FRAME_LIMIT + 1).read_until(b'\n', &mut line)?;
    if line.len() as u64 > FRAME_LIMIT {
        return Err(io::Error::other("agent frame exceeds limit"));
    }
    Ok(line)
}
fn write(stream: &mut UnixStream, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() as u64 >= FRAME_LIMIT {
        return Err(io::Error::other("agent frame exceeds limit"));
    }
    stream.write_all(&bytes)?;
    stream.write_all(b"\n")?;
    stream.flush()
}
fn unavailable() -> ApiError {
    ApiError::internal("agent channel is unavailable")
}
