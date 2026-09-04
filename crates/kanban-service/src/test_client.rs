//! The line-based socket client every service test drives the core
//! through, mirroring what a real client does.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{CoreProcess, serve};

/// Boot the core against a scratch data directory.
pub(crate) fn boot(dir: &TempDir) -> CoreProcess {
    serve(dir.path()).expect("the core boots on a scratch data directory")
}

/// One line-based client, mirroring what every real client does.
pub(crate) struct Client {
    reader: BufReader<UnixStream>,
    stream: UnixStream,
}

impl Client {
    pub(crate) fn connect(socket_path: &Path) -> Self {
        let stream = UnixStream::connect(socket_path).expect("the client connects");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("the read timeout applies");
        let reader = BufReader::new(stream.try_clone().expect("the stream clones"));
        Self { reader, stream }
    }

    pub(crate) fn query(&mut self, operation: &str) -> Value {
        self.request("query", operation, json!({}))
    }

    pub(crate) fn query_with(&mut self, operation: &str, payload: Value) -> Value {
        self.request("query", operation, payload)
    }

    pub(crate) fn command(&mut self, operation: &str, payload: Value) -> Value {
        self.request("command", operation, payload)
    }

    /// The error payload of a command the core refused.
    pub(crate) fn command_error(&mut self, operation: &str, payload: Value) -> Value {
        let frame = self.attempt("command", operation, payload);
        assert_eq!(frame["kind"], "error", "the command is refused: {frame}");
        frame["error"].clone()
    }

    /// The payload of a request the core answered, failing the test
    /// when it refused.
    pub(crate) fn request(&mut self, kind: &str, operation: &str, payload: Value) -> Value {
        let frame = self.attempt(kind, operation, payload);
        assert_eq!(frame["kind"], "response", "the {kind} succeeds: {frame}");
        frame["payload"].clone()
    }

    /// The whole frame, so a test can assert on a refusal.
    pub(crate) fn attempt(&mut self, kind: &str, operation: &str, payload: Value) -> Value {
        writeln!(
            self.stream,
            "{}",
            json!({ "kind": kind, "operation": operation, "payload": payload })
        )
        .expect("the client writes");
        self.stream.flush().expect("the client flushes");
        let mut line = String::new();
        let read = self.reader.read_line(&mut line).expect("the client reads");
        assert!(read > 0, "the core answers the request");
        serde_json::from_str(line.trim_end()).expect("a frame decodes")
    }
}
