# Kanban

A local-first desktop control plane for human and agent work across projects,
repositories, Herdr sessions, plans, specifications, and tickets.

Production source lives in this repository. Temporary planning and fleet
artifacts live under ignored `temp/`.

## Development

Requires Rust, Node.js, pnpm, `just`, and `pre-commit` on `PATH`.

- `just bootstrap` — point the repository at the tracked hooks in
  `.githooks` and install dependencies for both workspaces.
- `just check` — fmt, clippy, Rust tests, contract drift, the repository
  gates, web lint, typecheck, and tests.
- `just verify-contracts` — regenerate contracts and fail on drift,
  including a generated file nobody committed.
- `just build` — debug builds of the core and the desktop app.
- `just dev` — run the core and the desktop app.

## License

This project is licensed under the [MIT License](LICENSE).
