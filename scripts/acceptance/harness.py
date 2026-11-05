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
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
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


def bearer_candidates(data_dir, environ=None, platform=None, keychain=None, redact=None):
    """Yield (account, bearer) for each account that holds a secret."""
    for account in credential_accounts(data_dir, environ, platform):
        secret = read_installation_secret(data_dir, account, platform, keychain)
        if secret is not None:
            if redact is not None:
                redact.add(secret)
            yield account, derive_bearer(secret)


class Redactor:
    """Strip known secrets from any text before it leaves the process.

    Complete values are removed, including decoded text, its Python
    string escapes, and the bytes rendering of an installation secret,
    so a later shortening cannot leave a fragment.
    """

    def __init__(self):
        self._secrets = []

    def add(self, secret):
        if isinstance(secret, (bytes, bytearray)):
            for rendering in secret_renderings(bytes(secret)):
                self._remember(rendering)
            return
        self._remember(secret)

    def _remember(self, secret):
        if not isinstance(secret, str) or len(secret) < 8 or secret in self._secrets:
            return
        self._secrets.append(secret)

    def __call__(self, text):
        text = str(text)
        for secret in sorted(self._secrets, key=len, reverse=True):
            text = text.replace(secret, "[redacted]")
        return text


def redact_structure(value, redact):
    """Remove known secrets from strings before a value is formatted.

    A container is copied only when a nested string changes, so a value
    with no secret keeps its identity and its rendering. Objects this
    display does not walk are left unchanged.
    """
    if redact is None:
        return value
    cleaned = _redact_walk(value, redact)
    try:
        unchanged = cleaned == value
    except TypeError:
        unchanged = False
    return value if unchanged else cleaned


def _redact_walk(value, redact):
    if isinstance(value, str):
        return redact(value)
    if isinstance(value, dict):
        return {
            _redact_walk(key, redact): _redact_walk(item, redact)
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [_redact_walk(item, redact) for item in value]
    if isinstance(value, tuple):
        return tuple(_redact_walk(item, redact) for item in value)
    if isinstance(value, frozenset):
        return frozenset(_redact_walk(item, redact) for item in value)
    if isinstance(value, set):
        return {_redact_walk(item, redact) for item in value}
    return value


def refusal_text(error, redact=None):
    """The text a core refusal already shows, with secrets removed first."""
    if isinstance(error, CoreError):
        message = error.error.get("message", "core error")
        if redact is not None:
            message = redact_structure(message, redact)
        return message
    return error


def secret_renderings(secret):
    """Forms of an installation secret that must never appear.

    Encodings, the decoded text, and every Python string or bytes
    rendering, including a secret spliced into a longer value or
    already escaped. Decoded text is registered even when it contains
    a newline, so shortening cannot leave a fragment of it.
    """
    standard = base64.b64encode(secret).decode("ascii")
    url = base64.urlsafe_b64encode(secret).decode("ascii")
    renderings = [secret.hex(), standard, standard.rstrip("="), url, url.rstrip("="), repr(secret)]
    renderings.extend(bytes_repr_bodies(secret))
    if len(secret) == SECRET_LENGTH:
        renderings.append(derive_bearer(secret))
    try:
        text = secret.decode("utf-8")
    except UnicodeDecodeError:
        text = ""
    if text:
        renderings.append(text)
        renderings.extend(text_repr_bodies(text))
    return [rendering for rendering in renderings if len(rendering) >= 8]


def text_repr_bodies(text):
    """Escaped interiors Python may splice into a larger string rendering.

    The quote Python picks depends on the surrounding text, so both
    choices are removed. Other characters are escaped the same way.
    """
    bodies = []
    single = repr(text + '"')
    if single.startswith("'") and single.endswith("'"):
        bodies.append(single[1:-2])
    if '"' not in text:
        double = repr(text + "'")
        if double.startswith('"') and double.endswith('"'):
            bodies.append(double[1:-2])
    return bodies


def bytes_repr_bodies(secret):
    """Escaped interiors Python may splice into a larger bytes rendering.

    The quote Python picks depends on the surrounding bytes, so both
    choices are removed. Other bytes are escaped the same way in either.
    """
    return [escaped_bytes(secret, "'"), escaped_bytes(secret, '"')]


def escaped_bytes(secret, quote):
    """How Python renders these bytes inside a `quote`-delimited literal."""
    pieces = []
    for byte in secret:
        if byte == ord(quote) or byte == 0x5C:
            pieces.append("\\" + chr(byte))
        elif byte == 0x09:
            pieces.append("\\t")
        elif byte == 0x0A:
            pieces.append("\\n")
        elif byte == 0x0D:
            pieces.append("\\r")
        elif 0x20 <= byte < 0x7F:
            pieces.append(chr(byte))
        else:
            pieces.append("\\x%02x" % byte)
    return "".join(pieces)


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

    def describe(self, redact=None):
        """One line for a tool outcome.

        When `redact` is given, known secrets are removed from nested
        text before it is escaped. The stored payload is left unchanged.
        """
        if self.ok:
            return "accepted"
        error = self.error or {}

        def clean(value):
            if redact is None:
                return value
            return redact_structure(value, redact)

        parts = [str(clean(error.get("code")))]
        if error.get("field"):
            parts.append(f"field={clean(error['field'])}")
        if error.get("message"):
            parts.append(repr(clean(error["message"])))
        rendered = " ".join(parts)
        if redact is None:
            return rendered
        return redact(rendered)


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


def store_fingerprint(root):
    """Content identity of every file under `root`. Reads only.

    A symlink keeps its original text and also records the file it
    resolves to. A database file also records the three SQLite sidecar
    files beside that target, present or absent. A dangling or looping
    link is recorded as such. Following stops after a fixed number of
    hops and does not walk a directory.
    """
    root = Path(root)
    found = {}
    if not root.exists():
        return found
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        try:
            if path.is_symlink():
                found[relative] = _linked_identity(path)
            elif path.is_dir():
                found[relative] = ("dir",)
            elif path.is_file():
                found[relative] = ("file", hashlib.sha256(path.read_bytes()).hexdigest())
            else:
                found[relative] = ("other", path.stat().st_mode)
        except OSError as error:
            found[relative] = ("unreadable", type(error).__name__)
    _name_linked_sidecars(found)
    return found


_SYMLINK_HOP_LIMIT = 40
_SIDECAR_SUFFIXES = ("-wal", "-shm", "-journal")
_SQLITE_HEADER = b"SQLite format 3\x00"


def _file_digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _linked_identity(path):
    raw = os.readlink(path)
    kind, detail = _follow_link(path)
    if kind == "file":
        resolved, digest = detail
        identity = ("symlink", raw, "file", digest)
        if _names_a_database(path, resolved):
            return identity + (_sidecar_states(resolved),)
        return identity
    if kind == "other":
        return ("symlink", raw, "other", detail)
    if kind == "unreadable":
        return ("symlink", raw, "unreadable", detail)
    return ("symlink", raw, kind)


def _follow_link(start):
    """Follow `start` until a non-link, a loop, or the hop limit."""
    current = Path(start)
    seen = set()
    for _hop in range(_SYMLINK_HOP_LIMIT):
        try:
            if not current.is_symlink():
                return _classify_target(current)
            token = _link_token(current)
        except OSError as error:
            return "unreadable", type(error).__name__
        if token in seen:
            return "loop", None
        seen.add(token)
        try:
            raw = os.readlink(current)
        except OSError as error:
            return "unreadable", type(error).__name__
        nxt = Path(raw)
        if not nxt.is_absolute():
            nxt = current.parent / nxt
        current = nxt
    try:
        if not current.is_symlink():
            return _classify_target(current)
    except OSError as error:
        return "unreadable", type(error).__name__
    return "loop", None


def _link_token(path):
    stat = path.lstat()
    return stat.st_dev, stat.st_ino


def _classify_target(path):
    try:
        if path.is_symlink():
            return "loop", None
        if not path.exists():
            return "dangling", None
        if path.is_dir():
            return "dir", None
        if path.is_file():
            return "file", (path, _file_digest(path))
        return "other", path.stat().st_mode
    except OSError as error:
        return "unreadable", type(error).__name__


def _names_a_database(link, resolved):
    resolved = Path(resolved)
    if _database_name(link.name) or _database_name(resolved.name):
        return True
    try:
        with resolved.open("rb") as handle:
            return handle.read(len(_SQLITE_HEADER)).startswith(_SQLITE_HEADER)
    except OSError:
        return False


def _database_name(name):
    return name == "kanban.sqlite" or name.endswith(".sqlite") or name.endswith(".sqlite3")


def _sidecar_states(resolved):
    resolved = Path(resolved)
    return tuple(
        (suffix[1:], _sidecar_state(Path(str(resolved) + suffix)))
        for suffix in _SIDECAR_SUFFIXES
    )


def _sidecar_state(path):
    try:
        if path.is_symlink():
            kind, detail = _follow_link(path)
            if kind == "file":
                return ("file", detail[1])
            if kind == "other":
                return ("other", detail)
            if kind == "unreadable":
                return ("unreadable", detail)
            return (kind,)
        if not path.exists():
            return ("absent",)
        if path.is_dir():
            return ("dir",)
        if path.is_file():
            return ("file", _file_digest(path))
        return ("other", path.stat().st_mode)
    except OSError as error:
        return ("unreadable", type(error).__name__)


def _name_linked_sidecars(found):
    """Name sidecar states on free paths so a change can be reported."""
    named = {}
    for relative, record in list(found.items()):
        if len(record) < 5 or record[0] != "symlink" or record[2] != "file":
            continue
        for suffix, state in record[4]:
            key = relative + "-" + suffix
            if key not in found and key not in named:
                named[key] = state
    found.update(named)



def store_changes(before, after):
    """Relative paths added, removed, or changed between two fingerprints."""
    return sorted(key for key in set(before) | set(after) if before.get(key) != after.get(key))


def copy_store(source, destination):
    """Copy a store's files without preserving links back into it.

    Permission bits are copied too. A credential directory must stay
    private, or the core refuses to read the installation secret.
    """
    source = Path(source)
    destination = Path(destination)
    destination.mkdir(parents=True, exist_ok=True)
    for path in sorted(source.rglob("*")):
        target = destination / path.relative_to(source)
        if path.is_dir() and not path.is_symlink():
            target.mkdir(parents=True, exist_ok=True)
            target.chmod(path.stat().st_mode & 0o777)
            continue
        if not path.is_file():
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, target, follow_symlinks=True)
        target.chmod(path.stat().st_mode & 0o777)


def disposable_directory(inspected):
    """A temporary directory that is not inside the inspected store."""
    inspected = Path(inspected).resolve()
    system_temp = Path(tempfile.gettempdir()).resolve()
    parent = None
    if inspected == system_temp or inspected in system_temp.parents:
        parent = inspected.parent
    path = Path(tempfile.mkdtemp(prefix="kanban-acceptance-", dir=None if parent is None else str(parent)))
    resolved = path.resolve()
    if resolved == inspected or inspected in resolved.parents:
        shutil.rmtree(path, ignore_errors=True)
        raise HarnessError("the snapshot would sit inside the inspected store")
    return path


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
        try:
            deadline = time.monotonic() + wait_seconds
            while time.monotonic() < deadline:
                if self.process.poll() is not None:
                    raise HarnessError(f"the core exited with {self.process.returncode}: "
                                       f"{self.last_log_line()} (log {self.log_path})")
                if self.socket_path.exists() and port_in_use(self.host, self.port):
                    return
                time.sleep(0.25)
            raise HarnessError(f"the core did not come up within {wait_seconds}s; see {self.log_path}")
        except Exception:
            self.reap()
            raise

    def reap(self, wait_seconds=5):
        """Terminate and collect a core this runner spawned."""
        process = self.process
        if process is None or process.poll() is not None:
            return
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except (ProcessLookupError, PermissionError, OSError):
            try:
                process.terminate()
            except OSError:
                pass
        try:
            process.wait(timeout=wait_seconds)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError, OSError):
                try:
                    process.kill()
                except OSError:
                    pass
            process.wait(timeout=wait_seconds)

    def stop(self, wait_seconds=30, redact=None):
        """Ask the core to stop through `service.stop`, then reap it.

        When `redact` is given, secrets are removed from a refusal
        before that refusal is formatted.
        """
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
            shown = refusal_text(error, redact)
            reason = f"service.stop failed: {shown}"
            if redact is not None:
                reason = redact(reason)
        if self.process is None:
            return reason
        try:
            self.process.wait(timeout=wait_seconds)
        except subprocess.TimeoutExpired:
            os.killpg(self.process.pid, signal.SIGTERM)
            self.process.wait(timeout=10)
            reason = (reason or "") + " the core ignored service.stop and was terminated"
        return reason
