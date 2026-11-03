# Linux development

Kanban's core (`kanban-service`), MCP adapter (`kanban-mcp`), and Tauri desktop
shell build and run on Linux x86_64 with Rust stable and Node.js 22+.

## Prerequisites

System packages (Debian/Ubuntu-like):

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential pkg-config git \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
  patchelf
```

Tooling:

- [Rust](https://rustup.rs) stable (2024 edition; 1.90+ as pinned in the repo)
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

## Installation credentials on Linux

Managed startup stores the 32-byte installation secret in a mode `0600` file
inside a mode `0700` directory named `installation-credential` under the data
directory. The store refuses to load if either path is group- or world-accessible.
macOS continues to use Keychain with the same service/account naming; custom
`--data-dir` paths still get isolated accounts (Keychain account or file name).

Secrets never appear in environment variables, `config.json`, CLI arguments, logs,
or SQLite.

Default managed data lives at `$XDG_DATA_HOME/Kanban` (usually
`~/.local/share/Kanban`).

## Run the core

```bash
cargo build -p kanban-service -p kanban-mcp
DATA="$(mktemp -d)"
./target/debug/kanban-service --data-dir "$DATA"
```

In another terminal, health check:

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

(Set `DATA` in the environment to the same directory.)

## MCP

While the service is running, point a local MCP client at the adapter beside
`kanban-service`:

```text
--socket /absolute/path/to/core.sock --capability DISPATCH_CAPABILITY_ID
```

Dispatch capability IDs and run-scoped tools behave the same as on macOS.

## Desktop app

Build the web assets and Tauri shell:

```bash
just build
# or: pnpm --filter desktop run build
```

Launch on a desktop session with WebKitGTK:

```bash
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

`just check-ci-matrix` additionally requires `brew` and `rsync`; it mirrors the
GitHub Actions command list from a fresh clone and is intended as an occasional
audit (see the README continuous-integration section). It is not part of
`just check`.

Native macOS packaging (`just package`, `just check-packaging-native`) remains
macOS-only.
