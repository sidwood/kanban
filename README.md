# Kanban

A local-first desktop control plane for human and agent work across projects,
repositories, Herdr sessions, plans, specifications, and tickets.

Production source lives in this repository. Temporary planning and fleet
artifacts live under ignored `temp/`.

## Development

Requires Rust, Node.js, pnpm, and `just` on `PATH`.

- `just bootstrap` — install dependencies for both workspaces.
- `just check` — fmt, clippy, Rust tests, web lint, typecheck, tests, and contract drift.
- `just verify-contracts` — regenerate contracts and fail on drift.
- `just build` — debug builds of the core and the desktop app.
- `just dev` — run the core and the desktop app.
