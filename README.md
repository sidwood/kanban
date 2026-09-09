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
  gates, spelling, web lint, typecheck, and tests.
- `just verify-contracts` — regenerate contracts and fail on drift,
  including a generated file nobody committed.
- `just build` — debug builds of the core and the desktop app.
- `just dev` — run the core and the desktop app.

## Local MCP clients

Build the service and adapter together with `just build` or `just dev`.
While the service is running, configure a local MCP client to launch the
`kanban-mcp` executable beside `kanban-service` with these arguments:

```text
--socket /absolute/path/to/Kanban/core.sock --capability DISPATCH_CAPABILITY_ID
```

Use the absolute managed socket path (normally under
`~/Library/Application Support/Kanban/`) and the numeric capability ID
returned by dispatch. The run must already be acknowledged and executing.
This ID selects a grant; it is not an installation credential.

The client process only relays stdio. The service launches the actual
adapter with an inherited, run-bound channel and a cleared environment.
Generated tools expose only that grant's operations; calls still check
scope and expiry, including before replay and mutation. Ending the client
connection or stopping the service cleans up its adapter.

The Unix socket trusts the current macOS user, like the native UI. This
is not an OS sandbox against another process running as that same user.
MCP tool arguments cannot replace the service-selected grant.

Managed startup keeps installation credentials in macOS Keychain under
service `dev.kanban.desktop.installation`, account `installation`.
Credentials never belong in client configuration, prompts, SQLite, or
environment variables. Keychain failure refuses managed startup; there
is no plaintext fallback. HTTP is not enabled by this stdio integration.

## Continuous integration

GitHub Actions runs the repository gates (`just check`) on every pull
request and push to `main`, with locked dependency installs and actions
pinned to immutable commits. Every `${{ }}` expression it evaluates must
be on a short allow-list and every checkout states
`persist-credentials: false`, so neither a repository secret nor the
workflow token is reachable by untrusted pull-request code. The workflow
owns no publication or planning input either: `just check-workflows`
regression-tests that policy against the document a pinned YAML parser
reads, so it needs the locked web workspace `just bootstrap` installs,
and `just check-ci-matrix` re-runs the command matrix from a fresh
checkout with no `temp/` directory. That audit needs `brew` and `rsync`
on `PATH` beyond the development tools above: it installs locally what
the workflow's setup steps provide on GitHub, and overlays the working
tree onto its fresh clone.

## License

This project is licensed under the [MIT License](LICENSE).
