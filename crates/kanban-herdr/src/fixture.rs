//! Scripted Herdr socket fixture for client tests.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use serde_json::{Value, json};

use kanban_domain::HerdrSession;

use crate::paths::session_socket_in;
use crate::protocol::{HerdrRequest, HerdrResponse, Snapshot};

/// A running scripted Herdr session socket.
pub struct ScriptedSession {
    socket_path: PathBuf,
    server: JoinHandle<()>,
}

impl ScriptedSession {
    /// Bind one named session socket under `root` and serve `script`.
    /// The Herdr workspace identity reported by snapshots is the
    /// product workspace's final path segment.
    pub fn bind(
        root: &Path,
        session_name: &str,
        product_workspace: &str,
        script: SessionScript,
    ) -> Self {
        Self::bind_session(
            root,
            HerdrSession::named(session_name).expect("the fixture session name validates"),
            product_workspace,
            None,
            script,
        )
    }

    /// Bind Herdr's default session socket under `root` and serve
    /// `script`.
    pub fn bind_default(root: &Path, product_workspace: &str, script: SessionScript) -> Self {
        Self::bind_session(root, HerdrSession::Default, product_workspace, None, script)
    }

    /// Bind one named session socket whose snapshots report an
    /// explicit Herdr workspace identity, for bindings where that
    /// identity differs from the product workspace's final segment.
    pub fn bind_with_workspace(
        root: &Path,
        session_name: &str,
        product_workspace: &str,
        herdr_workspace: &str,
        script: SessionScript,
    ) -> Self {
        Self::bind_session(
            root,
            HerdrSession::named(session_name).expect("the fixture session name validates"),
            product_workspace,
            Some(herdr_workspace.to_owned()),
            script,
        )
    }

    /// Bind one session socket, named or default, under `root`.
    fn bind_session(
        root: &Path,
        session: HerdrSession,
        product_workspace: &str,
        herdr_workspace: Option<String>,
        script: SessionScript,
    ) -> Self {
        std::fs::create_dir_all(root).expect("the fixture root is created");
        let socket_path = session_socket_in(root, &session).expect("the session resolves a socket");
        if socket_path.exists() {
            std::fs::remove_file(&socket_path).expect("stale fixture socket is cleared");
        }
        let listener = UnixListener::bind(&socket_path).expect("the fixture socket binds");
        let script = Arc::new(script);
        let product_workspace = product_workspace.to_owned();
        let session_name = session.as_name().unwrap_or("default").to_owned();
        let server = thread::Builder::new()
            .name(format!("herdr-fixture-{session_name}"))
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    serve_connection(
                        stream,
                        &session_name,
                        &product_workspace,
                        herdr_workspace.clone(),
                        script.clone(),
                    );
                }
            })
            .expect("the fixture server starts");
        Self {
            socket_path,
            server,
        }
    }

    /// The bound socket path clients connect to.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// The directory holding the session socket.
    pub fn root(&self) -> &Path {
        self.socket_path.parent().expect("the socket has a parent")
    }
}

impl Drop for ScriptedSession {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
        self.server.thread().unpark();
    }
}

/// Scripted responses keyed by request method.
#[derive(Debug, Clone, Default)]
pub struct SessionScript {
    events: Vec<Value>,
    wait_met: bool,
    wait_detail: Value,
    prompt_accepted: bool,
}

impl SessionScript {
    /// Push events delivered after subscribe.
    pub fn with_events(mut self, events: Vec<Value>) -> Self {
        self.events = events;
        self
    }

    /// The wait response Herdr should return.
    pub fn with_wait(mut self, met: bool, detail: Value) -> Self {
        self.wait_met = met;
        self.wait_detail = detail;
        self
    }

    /// Whether prompt requests are accepted.
    pub fn with_prompt_accepted(mut self, accepted: bool) -> Self {
        self.prompt_accepted = accepted;
        self
    }
}

fn serve_connection(
    stream: UnixStream,
    session_name: &str,
    product_workspace: &str,
    herdr_workspace: Option<String>,
    script: Arc<SessionScript>,
) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut writer = stream;
    let mut subscribed = false;
    let mut event_index = 0;

    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        let request: HerdrRequest = match serde_json::from_str(line.trim()) {
            Ok(request) => request,
            Err(error) => {
                let _ = write_response(
                    &mut writer,
                    HerdrResponse::Error {
                        message: error.to_string(),
                    },
                );
                continue;
            }
        };

        let response = match request {
            HerdrRequest::Snapshot => HerdrResponse::Snapshot(snapshot(
                session_name,
                product_workspace,
                herdr_workspace.as_deref(),
            )),
            HerdrRequest::Subscribe => {
                subscribed = true;
                HerdrResponse::Subscribed
            }
            HerdrRequest::Wait {
                condition,
                timeout_ms,
            } => HerdrResponse::WaitResult {
                met: script.wait_met,
                detail: script.wait_detail.clone().as_object().map_or_else(
                    || json!({ "condition": condition, "timeout_ms": timeout_ms }),
                    |object| {
                        let mut detail = object.clone();
                        detail.insert("condition".to_owned(), json!(condition));
                        detail.insert("timeout_ms".to_owned(), json!(timeout_ms));
                        Value::Object(detail)
                    },
                ),
            },
            HerdrRequest::Prompt {
                role: _,
                message: _,
            } => HerdrResponse::PromptResult {
                accepted: script.prompt_accepted,
            },
        };

        if write_response(&mut writer, response).is_err() {
            break;
        }

        if subscribed {
            while event_index < script.events.len() {
                let event = script.events[event_index].clone();
                event_index += 1;
                if write_response(&mut writer, HerdrResponse::Event { payload: event }).is_err() {
                    return;
                }
            }
        }
    }
}

fn snapshot(
    session_name: &str,
    product_workspace: &str,
    herdr_workspace: Option<&str>,
) -> Snapshot {
    Snapshot {
        session: session_name.to_owned(),
        product_workspace: product_workspace.to_owned(),
        herdr_workspace: herdr_workspace.map(str::to_owned).unwrap_or_else(|| {
            Path::new(product_workspace)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(product_workspace)
                .to_owned()
        }),
        state: json!({ "roles": [] }),
        captured_at: "2026-09-05T04:46:00Z".to_owned(),
    }
}

fn write_response(stream: &mut UnixStream, response: HerdrResponse) -> std::io::Result<()> {
    let encoded = serde_json::to_string(&response).expect("fixture responses encode");
    writeln!(stream, "{encoded}")
}
