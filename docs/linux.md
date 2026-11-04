# Linux development

Kanban's core (`kanban-service`), MCP adapter (`kanban-mcp`), and Tauri desktop
shell build and run on Linux x86_64 with Rust stable and Node.js 22+.

## Prerequisites

System packages (Debian/Ubuntu-like):

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential pkg-config git rsync \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
  patchelf
```

`rsync` is required for `just check` / repository gates (not only
`just check-ci-matrix`).

Tooling:

- [Rust](https://rustup.rs) stable (2024 edition; MSRV 1.90+)
- Node.js 22+ with [pnpm](https://pnpm.io) (`corepack enable`)
- [`just`](https://github.com/casey/just) 1.59+
- [`pre-commit`](https://pre-commit.com) for the spelling gate

```bash
rustup default stable
corepack enable
cargo install just --locked
pip install pre-commit   # or apt install pre-commit
```

From the repository root:

```bash
just bootstrap
```

## Platform gaps on Linux

Launch at login and native macOS notifications are **unavailable** on Linux;
the desktop shell does not register a systemd user unit or show Notification
Center alerts. Use ordinary process supervision or your session manager if you
need auto-start.

## Installation credentials on Linux

Managed startup stores the 32-byte installation secret in a mode **`0600`**
regular file inside a mode **`0700`** directory named `installation-credential`
under the data directory. This is **not** Keychain parity: the bytes are stored
**unencrypted**, protected only by `0700`/`0600` permissions and same-user
access, and they are included in home-directory backups. See
[`docs/decisions/linux-file-installation-credential.md`](decisions/linux-file-installation-credential.md).

The store opens the directory with `O_DIRECTORY|O_NOFOLLOW`, verifies owner and
exact mode **`0700`**, and refuses symlinks, FIFOs, hard-linked credential
files, or credential files whose mode is not exactly **`0600`**. Looser modes
(for example `0701` or `0604`) are rejected.

Custom `--data-dir` paths use an isolated file name (`data-<sha256>` of the
canonical path), matching the macOS account naming scheme.

Secrets never appear in environment variables, `config.json`, CLI arguments,
logs, or SQLite.

Default managed data lives at `$XDG_DATA_HOME/Kanban` (usually
`~/.local/share/Kanban`).

## Run the core

```bash
cargo build -p kanban-service -p kanban-mcp
export DATA="$(mktemp -d)"
./target/debug/kanban-service --data-dir "$DATA"
```

In another terminal, health check (reuse the same `DATA` value):

```bash
./target/debug/kanban-service --data-dir "$DATA"   # exits 0 when already running
python3 - <<'PY'
import json, socket
from pathlib import Path
import os
data = Path(os.environ["DATA"])
sock = data / "core.sock"
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(str(sock))
s.sendall((json.dumps({"kind":"query","operation":"health.get","payload":{}})+"\n").encode())
print(s.makefile().readline())
PY
```

## Loopback HTTP credential (Linux)

When loopback HTTP is enabled, read the raw 32-byte secret from:

```text
<data>/installation-credential/installation
```

(or `<data>/installation-credential/data-<sha256>` for a non-default data
directory). Encode it as `kanban_` followed by URL-safe base64 without padding
for the `Authorization: Bearer` header, same as on macOS Keychain. Keep the
derived credential in memory only.

## MCP

While the service is running, point a local MCP client at the adapter beside
`kanban-service`:

```text
--socket /absolute/path/to/core.sock --capability DISPATCH_CAPABILITY_ID
```

Dispatch capability IDs and run-scoped tools behave the same as on macOS.

## Desktop app

Build the Rust core and MCP adapter, then the web assets and Tauri shell:

```bash
cargo build -p kanban-service -p kanban-mcp
just dev
# or: cargo build -p kanban-service -p kanban-mcp && pnpm --filter desktop dev
```

`just dev` builds the core and adapter before starting the desktop shell.
`pnpm --filter desktop run build` produces the web bundle and Tauri binary
only; it does not compile `kanban-service`.

Launch on a desktop session with WebKitGTK (if you are not using `just dev`):

```bash
cargo build -p kanban-service -p kanban-mcp
pnpm --filter desktop dev
```

The shell starts `kanban-service` on demand against the managed data directory
and connects over the Unix socket.

## Repository gates

```bash
just check
```

This runs formatting, Clippy, the full Rust and web test matrices, contract
drift checks, spelling, workflow policy, and packaging smoke tests.

`just check-ci-matrix` additionally requires `brew` and mirrors the GitHub
Actions command list from a fresh clone; it is an occasional audit (see the
README continuous-integration section).

Native macOS packaging (`just package`, `just check-packaging-native`) remains
macOS-only.
