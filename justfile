# Kanban whole-repository commands. Cargo owns Rust, pnpm owns the
# frontend, and just coordinates both.

set shell := ["bash", "-euo", "pipefail", "-c"]

# List the available recipes.
default:
    @just --list

# Install toolchains and dependencies.
bootstrap: need-rust need-web
    pnpm install
    cargo fetch

# fmt, clippy, Rust tests, web lint, typecheck, tests, and contract drift.
check: need-rust need-web
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    pnpm -r run lint
    pnpm -r run typecheck
    pnpm -r run test
    just verify-contracts

# Regenerate contracts and fail when committed artifacts drift.
verify-contracts: need-rust
    cargo run --quiet -p kanban-app --bin kanban-contracts-gen
    git diff --check -- \
      packages/contracts/src/index.ts \
      packages/contracts/src/client.ts \
      packages/contracts/src/types.ts \
      packages/contracts/src/mcp-tools.json \
      packages/contracts/src/schemas
    git diff --exit-code -- \
      packages/contracts/src/index.ts \
      packages/contracts/src/client.ts \
      packages/contracts/src/types.ts \
      packages/contracts/src/mcp-tools.json \
      packages/contracts/src/schemas

# Debug builds of the core and the desktop app.
build: need-rust need-web
    cargo build --workspace
    pnpm -r run build

# Run the core and the desktop app for development.
dev: need-rust need-web
    #!/usr/bin/env bash
    set -euo pipefail
    cargo run -p kanban-service &
    core_pid=$!
    cleanup() { kill "$core_pid" 2>/dev/null || true; }
    trap cleanup EXIT INT TERM
    pnpm --filter desktop dev

# Fail loudly when the Rust toolchain or its components are missing.
[private]
need-rust:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo >/dev/null 2>&1; then
      echo "kanban: cargo is required but was not found on PATH." >&2
      echo "" >&2
      echo "Install Rust with rustup (https://rustup.rs) or:" >&2
      echo "    brew install rust" >&2
      echo "Then re-run the failed just recipe." >&2
      echo "" >&2
      echo "Agents: install the toolchain or ask the user for help; do" >&2
      echo "not edit the justfile to skip this check." >&2
      exit 1
    fi
    for component in fmt clippy; do
      if ! cargo "$component" --version >/dev/null 2>&1; then
        echo "kanban: the cargo '$component' component is missing." >&2
        echo "" >&2
        echo "rustup users: rustup component add rust$component" >&2
        echo "Homebrew users: brew install rust" >&2
        echo "Then re-run the failed just recipe." >&2
        exit 1
      fi
    done

# Fail loudly when the frontend toolchain is missing.
[private]
need-web:
    #!/usr/bin/env bash
    set -euo pipefail
    for tool in node pnpm; do
      if ! command -v "$tool" >/dev/null 2>&1; then
        echo "kanban: $tool is required but was not found on PATH." >&2
        echo "" >&2
        echo "Install Node.js (https://nodejs.org) and pnpm with:" >&2
        echo "    corepack enable" >&2
        echo "Then re-run the failed just recipe." >&2
        echo "" >&2
        echo "Agents: install the toolchain or ask the user for help; do" >&2
        echo "not edit the justfile to skip this check." >&2
        exit 1
      fi
    done
