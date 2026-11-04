"""Drive one Kanban core: its process, its socket, and its operator MCP.

Only the Python standard library is used. The bearer credential is
derived in memory from the installation file and never printed,
logged, or written anywhere; every message this module produces is
passed through `Redactor` first.
"""

import base64
import http.client
import json
import os
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

PROTOCOL_VERSION = "2025-06-18"
# A Unix socket path, with its terminating NUL, must fit in 104 bytes on
# macOS and 108 on Linux.
SOCKET_PATH_LIMIT = 104 if sys.platform == "darwin" else 108


class HarnessError(Exception):
    """The harness itself could not drive the core."""


class CoreError(Exception):
    """The core refused a socket request with a structured error."""

    def __init__(self, error):
        self.error = error if isinstance(error, dict) else {"message": str(error)}
        super().__init__(self.error.get("message", "core error"))


def derive_bearer(installation_bytes):
    """Return `kanban_` plus the unpadded base64url of the secret bytes."""
    encoded = base64.urlsafe_b64encode(installation_bytes).decode("ascii")
    return "kanban_" + encoded.rstrip("=")


def read_bearer(data_dir):
    """Derive the bearer for one data directory, in memory only."""
    path = Path(data_dir) / "installation-credential" / "installation"
    try:
        secret = path.read_bytes()
    except OSError as error:
        raise HarnessError(f"cannot read the installation credential: {error}")
    if not secret:
        raise HarnessError("the installation credential is empty")
    return derive_bearer(secret)


class Redactor:
    """Strip known secrets from any text before it leaves the process."""

    def __init__(self):
        self._secrets = []

    def add(self, secret):
        if secret:
            self._secrets.append(secret)

    def __call__(self, text):
        text = str(text)
        for secret in self._secrets:
            text = text.replace(secret, "[redacted]")
        return text


def parse_rpc_body(content_type, body, request_id):
    """Return the JSON-RPC message for `request_id` from a JSON or SSE body."""
    text = body.decode("utf-8") if isinstance(body, bytes) else body
    if "text/event-stream" in (content_type or ""):
        messages = []
        data_lines = []
        for line in text.splitlines() + [""]:
            if line.startswith("data:"):
                data_lines.append(line[5:].lstrip(" "))
            elif line == "" and data_lines:
                messages.append(json.loads("\n".join(data_lines)))
                data_lines = []
        for message in messages:
            if isinstance(message, dict) and message.get("id") == request_id:
                return message
        if messages:
            return messages[-1]
        raise HarnessError("the event stream carried no JSON-RPC message")
    if not text.strip():
        return None
    return json.loads(text)


class ToolResult:
    """One `tools/call` outcome: a payload, or a structured error."""

    def __init__(self, ok, payload):
        self.ok = ok
        self.payload = payload

    @property
    def error(self):
        return None if self.ok else (self.payload or {})

    @property
    def code(self):
        return (self.error or {}).get("code")

    @property
    def field(self):
        return (self.error or {}).get("field")

    def describe(self):
        if self.ok:
            return "accepted"
        error = self.error or {}
        parts = [str(error.get("code"))]
        if error.get("field"):
            parts.append(f"field={error['field']}")
        if error.get("message"):
            parts.append(repr(error["message"]))
        return " ".join(parts)


def tool_result_from_rpc(message):
    """Decode a `tools/call` JSON-RPC response into a `ToolResult`."""
    if message is None:
        raise HarnessError("tools/call returned an empty body")
    if "error" in message:
        return ToolResult(False, {
            "code": "rpc_error",
            "message": message["error"].get("message"),
            "rpc_code": message["error"].get("code"),
        })
    result = message.get("result") or {}
    payload = result.get("structuredContent")
    if payload is None:
        for item in result.get("content") or []:
            if item.get("type") == "text":
                try:
                    payload = json.loads(item.get("text", ""))
                except ValueError:
                    payload = {"text": item.get("text")}
                break
    return ToolResult(not result.get("isError", False), payload)


class McpClient:
    """A minimal Streamable HTTP client for `/mcp/operator`."""

    def __init__(self, host, port, bearer, path="/mcp/operator", timeout=30):
        self.host = host
        self.port = port
        self.path = path
        self.timeout = timeout
        self._bearer = bearer
        self.session_id = None
        self._next_id = 0
        self.calls = 0

    def _post(self, body, authorised=True):
        connection = http.client.HTTPConnection(self.host, self.port, timeout=self.timeout)
        headers = {
            "content-type": "application/json",
            "accept": "application/json, text/event-stream",
        }
        if authorised:
            headers["authorization"] = "Bearer " + self._bearer
        if self.session_id:
            headers["mcp-session-id"] = self.session_id
        try:
            connection.request("POST", self.path, body=json.dumps(body), headers=headers)
            response = connection.getresponse()
            raw = response.read()
            return response.status, dict(response.getheaders()), raw
        finally:
            connection.close()

    def _rpc(self, method, params=None, notify=False):
        body = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            body["params"] = params
        request_id = None
        if not notify:
            self._next_id += 1
            request_id = self._next_id
            body["id"] = request_id
        status, headers, raw = self._post(body)
        lowered = {key.lower(): value for key, value in headers.items()}
        if status >= 400:
            raise HarnessError(f"{method} returned HTTP {status}")
        if lowered.get("mcp-session-id"):
            self.session_id = lowered["mcp-session-id"]
        if notify:
            return None
        return parse_rpc_body(lowered.get("content-type"), raw, request_id)

    def unauthorised_initialize_status(self):
        """POST initialize with no credential and return the HTTP status."""
        saved = self.session_id
        self.session_id = None
        try:
            status, _, _ = self._post(self._initialize_body(0), authorised=False)
        finally:
            self.session_id = saved
        return status

    @staticmethod
    def _initialize_body(request_id):
        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "kanban-acceptance", "version": "1"},
            },
        }

    def initialize(self):
        message = self._rpc("initialize", self._initialize_body(0)["params"])
        if not message or "result" not in message:
            raise HarnessError("initialize returned no result")
        self._rpc("notifications/initialized", notify=True)
        return message["result"]

    def list_tools(self):
        message = self._rpc("tools/list", {})
        return [tool["name"] for tool in (message.get("result") or {}).get("tools", [])]

    def call(self, name, arguments):
        self.calls += 1
        message = self._rpc("tools/call", {"name": name, "arguments": arguments})
        return tool_result_from_rpc(message)


class SocketClient:
    """One JSON line out and one JSON line back per core request."""

    def __init__(self, path, timeout=30):
        self.path = str(path)
        self.timeout = timeout

    def request(self, kind, operation, payload):
        frame = {"kind": kind, "operation": operation, "payload": payload}
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
            connection.settimeout(self.timeout)
            connection.connect(self.path)
            connection.sendall((json.dumps(frame) + "\n").encode("utf-8"))
            buffer = b""
            while b"\n" not in buffer:
                chunk = connection.recv(65536)
                if not chunk:
                    break
                buffer += chunk
        line = buffer.split(b"\n", 1)[0]
        if not line:
            raise HarnessError(f"{operation}: the core closed the socket without a reply")
        reply = json.loads(line)
        if reply.get("kind") == "error":
            raise CoreError(reply.get("error"))
        if reply.get("kind") != "response":
            raise HarnessError(f"{operation}: unexpected reply kind {reply.get('kind')!r}")
        return reply.get("payload")

    def command(self, operation, payload):
        return self.request("command", operation, payload)

    def query(self, operation, payload=None):
        return self.request("query", operation, payload if payload is not None else {})


def port_in_use(host, port):
    try:
        with socket.create_connection((host, port), timeout=1):
            return True
    except OSError:
        return False


def socket_answers(path):
    """Whether a core already serves this socket."""
    try:
        SocketClient(path, timeout=5).query("health.get", {})
        return True
    except (OSError, HarnessError, CoreError, ValueError):
        return False


class CoreProcess:
    """Start a core on a data directory and stop it cleanly.

    `data_dir` is the core's own directory (it holds kanban.sqlite and
    core.sock); `XDG_DATA_HOME` is set to its parent, as a managed
    start would see it.
    """

    def __init__(self, binary, data_dir, port, log_path, host="127.0.0.1"):
        self.binary = Path(binary)
        self.data_dir = Path(data_dir)
        self.host = host
        self.port = port
        self.process = None
        self.log_path = Path(log_path)

    @property
    def socket_path(self):
        return self.data_dir / "core.sock"

    def last_log_line(self):
        try:
            lines = self.log_path.read_text(encoding="utf-8", errors="replace").strip().splitlines()
        except OSError:
            return "no log"
        return lines[-1] if lines else "empty log"

    def start(self, wait_seconds=60):
        if not self.binary.is_file():
            raise HarnessError(f"core binary not found: {self.binary}")
        if port_in_use(self.host, self.port):
            raise HarnessError(f"something already listens on {self.host}:{self.port}; pick --port")
        length = len(str(self.socket_path).encode())
        if length >= SOCKET_PATH_LIMIT:
            raise HarnessError(f"{self.socket_path} is {length} bytes, too long for a Unix socket "
                               f"(at most {SOCKET_PATH_LIMIT - 1}); choose a shorter --data-dir")
        self.data_dir.parent.mkdir(parents=True, exist_ok=True)
        environment = dict(os.environ)
        environment["XDG_DATA_HOME"] = str(self.data_dir.parent)
        log = open(self.log_path, "ab")
        try:
            self.process = subprocess.Popen(
                [str(self.binary), str(self.data_dir), "--loopback-http", f"{self.host}:{self.port}"],
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=subprocess.STDOUT,
                env=environment,
                start_new_session=True,
            )
        finally:
            log.close()
        deadline = time.monotonic() + wait_seconds
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise HarnessError(f"the core exited with {self.process.returncode}: "
                                   f"{self.last_log_line()} (log {self.log_path})")
            if self.socket_path.exists() and port_in_use(self.host, self.port):
                return
            time.sleep(0.25)
        raise HarnessError(f"the core did not come up within {wait_seconds}s; see {self.log_path}")

    def stop(self, wait_seconds=30):
        """Ask the core to stop through `service.stop`, then reap it."""
        reason = None
        try:
            client = SocketClient(self.socket_path, timeout=10)
            warning = client.query("service.stop_warning", {})
            client.command("service.stop", {
                "mutation": {
                    "optimistic_version": warning["version"],
                    "idempotency_key": f"acceptance-stop-{time.time_ns()}",
                },
                "instance_id": warning["instance_id"],
                "warning_id": warning["warning_id"],
                "confirmed": True,
            })
        except (OSError, HarnessError, CoreError, KeyError) as error:
            reason = f"service.stop failed: {error}"
        if self.process is None:
            return reason
        try:
            self.process.wait(timeout=wait_seconds)
        except subprocess.TimeoutExpired:
            os.killpg(self.process.pid, signal.SIGTERM)
            self.process.wait(timeout=10)
            reason = (reason or "") + " the core ignored service.stop and was terminated"
        return reason
