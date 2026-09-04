# Kanban whole-repository commands. Cargo owns Rust, pnpm owns the
# frontend, and just coordinates both.

set shell := ["bash", "-euo", "pipefail", "-c"]

# List the available recipes.
default:
    @just --list

# Enable the repository hooks, then install toolchains and dependencies.
bootstrap: enable-hooks need-rust need-web
    pnpm install
    cargo fetch

# Regenerate contracts and fail when committed artifacts drift. A
# generated file that nobody staged is drift too, so it is refused
# rather than left invisible to git diff.
verify-contracts: need-rust
    #!/usr/bin/env bash
    set -euo pipefail
    cargo run --quiet -p kanban-app --bin kanban-contracts-gen
    generated=(
      packages/contracts/src/index.ts
      packages/contracts/src/client.ts
      packages/contracts/src/types.ts
      packages/contracts/src/mcp-tools.json
      packages/contracts/src/schemas
    )
    git diff --check -- "${generated[@]}"
    git diff --exit-code -- "${generated[@]}"
    untracked="$(git ls-files --others --exclude-standard -- "${generated[@]}")"
    if [[ -n "$untracked" ]]; then
      echo "kanban: generation left untracked contract artifacts:" >&2
      echo "$untracked" | sed 's/^/    /' >&2
      echo "" >&2
      echo "Stage them with the change that introduced them." >&2
      echo "" >&2
      echo "Agents: commit the generated files; do not delete them to pass." >&2
      exit 1
    fi

# Debug builds of the core and the desktop app.
build: need-rust need-web
    cargo build --workspace
    pnpm -r run build

# fmt, clippy, Rust tests, contract drift, the repository gates, web
# lint, typecheck, and web tests.
check: need-rust need-web
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    just check-shell
    just verify-contracts
    just check-gates
    pnpm -r run lint
    pnpm -r run typecheck
    pnpm -r run test

# The Tauri shell workspace: fmt, clippy, and tests for the crate in
# apps/desktop/src-tauri, which is deliberately outside the root
# Cargo workspace.
check-shell: need-rust need-web
    #!/usr/bin/env bash
    set -euo pipefail
    pnpm --filter desktop run build:web
    cargo build -p kanban-fake-core --bin fake-core
    cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml --check
    cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
    cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml

# Prove the commit hooks and contract verification reject bad input.
check-gates: need-rust need-web
    scripts/check_gates.sh

# Run the core and the desktop app for development. The shell starts
# the core on demand; quitting the window leaves it running.
dev: need-rust need-web
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -p kanban-service
    pnpm --filter desktop dev

# Development helper: stop a core a dev session left running. The
# product's own stop path with warnings lands in KAN-T63.
stop-core:
    pkill -x kanban-service || true

# Point this repository at the tracked hooks in .githooks. The hooks are
# never copied into .git/hooks; that directory stays Git's own.
[private]
enable-hooks:
    #!/usr/bin/env bash
    set -euo pipefail
    for hook in commit-msg pre-commit; do
      if [[ ! -x ".githooks/$hook" ]]; then
        echo "kanban: .githooks/$hook is missing or not executable." >&2
        echo "" >&2
        echo "Restore the tracked hook, then re-run just bootstrap:" >&2
        echo "    git checkout -- .githooks/$hook" >&2
        echo "    chmod +x .githooks/$hook" >&2
        echo "" >&2
        echo "Agents: repair the hook; do not disable it." >&2
        exit 1
      fi
    done
    if ! command -v pre-commit >/dev/null 2>&1; then
      echo "kanban: the pre-commit runner is required but was not found." >&2
      echo "" >&2
      echo "Install it with: brew install pre-commit" >&2
      echo "Then re-run just bootstrap." >&2
      echo "" >&2
      echo "Agents: install the runner or ask the user for help; do not" >&2
      echo "commit with the hooks disabled." >&2
      exit 1
    fi
    git config --local core.hooksPath .githooks
    configured="$(git config --get core.hooksPath || true)"
    if [[ "$configured" != ".githooks" ]]; then
      echo "kanban: core.hooksPath is \"$configured\" after configuration." >&2
      echo "" >&2
      echo "A higher-priority Git scope is overriding it. Clear it with:" >&2
      echo "    git config --worktree --unset core.hooksPath" >&2
      echo "Then re-run just bootstrap." >&2
      exit 1
    fi

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
