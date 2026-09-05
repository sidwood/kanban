//! Scripted Herdr socket fixture for client tests.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};

use kanban_domain::HerdrSession;

use crate::protocol::{HerdrRequest, HerdrResponse, Snapshot};

/// A running scripted Herdr session socket.
pub struct ScriptedSession {
    root: PathBuf,
    socket_path: PathBuf,
    requests: Arc<AtomicUsize>,
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
        let directory = match session.as_name() {
            None => root.to_path_buf(),
            Some(name) => root.join("sessions").join(name),
        };
        std::fs::create_dir_all(&directory).expect("the fixture session directory is created");
        let socket_path = directory.join("herdr.sock");
        if socket_path.exists() {
            std::fs::remove_file(&socket_path).expect("stale fixture socket is cleared");
        }
        let listener = UnixListener::bind(&socket_path).expect("the fixture socket binds");
        let script = Arc::new(script);
        let product_workspace = product_workspace.to_owned();
        let session_name = session.as_name().unwrap_or("default").to_owned();
        let accept_connections = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(AtomicUsize::new(0));
        let served_requests = requests.clone();
        let server = thread::Builder::new()
            .name(format!("herdr-fixture-{session_name}"))
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let index = accept_connections.fetch_add(1, Ordering::Relaxed);
                    serve_connection(
                        stream,
                        &session_name,
                        &product_workspace,
                        herdr_workspace.clone(),
                        script.clone(),
                        index,
                        &served_requests,
                    );
                }
            })
            .expect("the fixture server starts");
        Self {
            root: root.to_path_buf(),
            socket_path,
            requests,
            server,
        }
    }

    /// The bound socket path clients connect to.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// The injected Herdr config root, shared by default and named sessions.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// How many request lines the session has read across every
    /// connection: one counted request proves the client wrote it and
    /// is now blocked waiting for an answer.
    pub fn requests_seen(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
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
    subscribe_error: Option<String>,
    close_after_events: bool,
    hold_before_close: Option<Duration>,
    flap: bool,
    silent: bool,
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

    /// Refuse every subscribe request with this message, so a test
    /// observes connections that never reach a live subscription.
    pub fn with_subscribe_error(mut self, message: &str) -> Self {
        self.subscribe_error = Some(message.to_owned());
        self
    }

    /// Close the first connection once it has delivered every scripted
    /// event, so a test observes one reconnect; later connections stay
    /// open like a settled session.
    pub fn close_after_events(mut self) -> Self {
        self.close_after_events = true;
        self
    }

    /// Hold the first connection open for `hold` after subscribing —
    /// long enough for a live subscription to settle — then close it;
    /// later connections stay open like a healthy session.
    pub fn close_after_hold(mut self, hold: Duration) -> Self {
        self.hold_before_close = Some(hold);
        self
    }

    /// Drop every connection the moment it subscribes, so a test
    /// observes a session that is live and gone at once, over and
    /// over.
    pub fn with_flapping_subscriptions(mut self) -> Self {
        self.flap = true;
        self
    }

    /// Read every request and answer none, so a client is blocked on
    /// its first response — the snapshot handshake — for as long as
    /// the test wants.
    pub fn with_silent_handshake(mut self) -> Self {
        self.silent = true;
        self
    }
}

fn serve_connection(
    stream: UnixStream,
    session_name: &str,
    product_workspace: &str,
    herdr_workspace: Option<String>,
    script: Arc<SessionScript>,
    connection_index: usize,
    requests: &Arc<AtomicUsize>,
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
        requests.fetch_add(1, Ordering::Relaxed);
        if script.silent {
            continue;
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
                if let Some(message) = &script.subscribe_error {
                    HerdrResponse::Error {
                        message: message.clone(),
                    }
                } else {
                    subscribed = true;
                    HerdrResponse::Subscribed
                }
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
            if script.flap {
                // Subscribe succeeded, then the session drops at once:
                // a live subscription that cannot settle.
                return;
            }
            while event_index < script.events.len() {
                let event = script.events[event_index].clone();
                event_index += 1;
                if write_response(&mut writer, HerdrResponse::Event { payload: event }).is_err() {
                    return;
                }
            }
            if script.close_after_events && connection_index == 0 {
                return;
            }
            if let Some(hold) = script.hold_before_close
                && connection_index == 0
            {
                thread::sleep(hold);
                return;
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
