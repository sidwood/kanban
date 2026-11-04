"""Drive one Kanban core: its process, its socket, and its operator MCP.

Only the Python standard library is used. The bearer credential is
derived in memory from the core's installation secret (a file on
Linux, the login Keychain on macOS) and never printed, logged, or
written anywhere; every message this module produces is passed
through `Redactor` first.
"""

import base64
import ctypes
import hashlib
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


class Unauthorised(HarnessError):
    """The core refused the bearer with HTTP 401."""


class CoreError(Exception):
    """The core refused a socket request with a structured error."""

    def __init__(self, error):
        self.error = error if isinstance(error, dict) else {"message": str(error)}
        super().__init__(self.error.get("message", "core error"))


def derive_bearer(installation_bytes):
    """Return `kanban_` plus the unpadded base64url of the secret bytes."""
    encoded = base64.urlsafe_b64encode(installation_bytes).decode("ascii")
    return "kanban_" + encoded.rstrip("=")


# Where the core keeps its installation secret (crates/kanban-service/src/secrets.rs).
KEYCHAIN_SERVICE = "dev.kanban.desktop.installation"
SECURITY_FRAMEWORK = "/System/Library/Frameworks/Security.framework/Security"
SECRET_LENGTH = 32
KEYCHAIN_ITEM_NOT_FOUND = -25300
KEYCHAIN_REFUSALS = {
    -128: "the Keychain prompt was cancelled",
    -25293: "Keychain access was denied",
    -25308: "the Keychain needs to ask for permission but cannot show a prompt here; "
            "run from a logged-in desktop session with the login keychain unlocked",
}


def home_directory(environ):
    """`$HOME` when set, otherwise the account's home, as the core reads it."""
    home = environ.get("HOME")
    return Path(home) if home else Path(os.path.expanduser("~"))


def managed_data_dir(environ, platform):
    """The core's default data directory, as the `dirs` crate finds it."""
    if platform == "darwin":
        return home_directory(environ) / "Library" / "Application Support" / "Kanban"
    xdg = environ.get("XDG_DATA_HOME")
    if xdg and os.path.isabs(xdg):
        return Path(xdg) / "Kanban"
    return home_directory(environ) / ".local" / "share" / "Kanban"


def credential_accounts(data_dir, environ=None, platform=None):
    """The accounts the secret may sit under, the core's choice first.

    The core uses `installation` for its default data directory and
    `data-` plus the SHA-256 of the canonical path for any other. The
    choice depends on the core's environment, which an attached core
    may not share with the runner, so the other account is offered too.
    """
    environ = os.environ if environ is None else environ
    platform = platform or sys.platform
    data = Path(data_dir).resolve(strict=True)
    default = managed_data_dir(environ, platform)
    try:
        default = default.resolve(strict=True)
    except OSError:
        pass
    hashed = "data-" + hashlib.sha256(os.fsencode(str(data))).hexdigest()
    return ["installation", hashed] if data == default else [hashed, "installation"]


def load_security_framework():
    library = ctypes.CDLL(SECURITY_FRAMEWORK)
    find = library.SecKeychainFindGenericPassword
    find.argtypes = [
        ctypes.c_void_p, ctypes.c_uint32, ctypes.c_char_p, ctypes.c_uint32, ctypes.c_char_p,
        ctypes.POINTER(ctypes.c_uint32), ctypes.POINTER(ctypes.c_void_p), ctypes.c_void_p,
    ]
    find.restype = ctypes.c_int32
    free = library.SecKeychainItemFreeContent
    free.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    free.restype = ctypes.c_int32
    return library


def keychain_secret(account, library=None):
    """Read the core's generic password from the default keychain search
    list, through the same legacy call the core itself makes. macOS may
    ask once whether Python may read the item."""
    library = library or load_security_framework()
    service = KEYCHAIN_SERVICE.encode("utf-8")
    name = account.encode("utf-8")
    length = ctypes.c_uint32(0)
    data = ctypes.c_void_p()
    status = library.SecKeychainFindGenericPassword(
        None, len(service), service, len(name), name, ctypes.byref(length), ctypes.byref(data), None)
    if status == KEYCHAIN_ITEM_NOT_FOUND:
        return None
    if status != 0:
        reason = KEYCHAIN_REFUSALS.get(status, "the Keychain refused the read")
        raise HarnessError(f"cannot read Keychain item {KEYCHAIN_SERVICE} / {account}: "
                           f"{reason} (OSStatus {status})")
    try:
        return ctypes.string_at(data.value, length.value) if data.value else b""
    finally:
        library.SecKeychainItemFreeContent(None, data)


def read_installation_secret(data_dir, account, platform=None, keychain=None):
    """One account's secret bytes, or None when the account holds none."""
    platform = platform or sys.platform
    if platform == "darwin":
        secret = keychain_secret(account, keychain)
    else:
        path = Path(data_dir) / "installation-credential" / account
        try:
            secret = path.read_bytes()
        except FileNotFoundError:
            return None
        except OSError as error:
            raise HarnessError(f"cannot read the installation credential: {error}")
    if secret is not None and len(secret) != SECRET_LENGTH:
        raise HarnessError(f"the installation secret under account {account} is "
                           f"{len(secret)} bytes, not {SECRET_LENGTH}")
    return secret


def bearer_candidates(data_dir, environ=None, platform=None, keychain=None):
    """Yield (account, bearer) for each account that holds a secret."""
    for account in credential_accounts(data_dir, environ, platform):
        secret = read_installation_secret(data_dir, account, platform, keychain)
        if secret is not None:
            yield account, derive_bearer(secret)


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
        if status == 401:
            raise Unauthorised(f"{method} returned HTTP 401: the core refused the bearer")
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

    def environment(self):
        """The environment the core starts with. On Linux this makes the
        data directory the core's default; macOS ignores XDG_DATA_HOME."""
        environment = dict(os.environ)
        environment["XDG_DATA_HOME"] = str(self.data_dir.parent)
        return environment

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
        environment = self.environment()
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
