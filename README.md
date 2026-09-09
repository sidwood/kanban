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
An explicitly selected non-default data directory uses account `data-`
followed by the lowercase SHA-256 of its canonical path bytes instead.
Startup acquires installation ownership before requesting credentials.
Credentials never belong in client configuration, prompts, SQLite, or
environment variables. Keychain failure refuses managed startup; there
is no plaintext fallback. HTTP is not enabled by this stdio integration.

## Optional loopback HTTP

HTTP is disabled by default: no TCP listener starts with the desktop or
with a plain `kanban-service` invocation. Enable it only for a deliberate
service start, for example:

```text
kanban-service --loopback-http 127.0.0.1:9876
```

Both a positional absolute data directory and `--data-dir /absolute/path`
work with or without HTTP. `--launch-once` forwards the selected directory
and explicit HTTP address to its detached child. No environment variable
enables HTTP; missing values, unknown flags, and duplicate options fail.

This does not reconfigure an already running Core. Stop that Core through
its lifecycle controls before starting with different options. Omit the
flag on the next start to disable HTTP again. The bind address must be a
numeric loopback address; IPv6 `--loopback-http [::1]:9876` also works.
A bind or authentication setup failure refuses startup rather than
silently running without the requested transport.

Use a native MCP client with Streamable HTTP at the exact configured
`http://ADDRESS/mcp` URL. Each request needs:

- `Authorization: Bearer <Keychain-derived credential>`.
- `X-Kanban-Capability: <numeric dispatch capability ID>` for an acknowledged,
  executing Run, as with the socket adapter.

A native client must retrieve the existing 32-byte installation item from
Keychain using the service/account above, then encode it as `kanban_`
followed by URL-safe base64 without padding. Keep the derived credential
in memory only; never put it in configuration, command arguments,
environment variables, prompts, logs, or SQLite. The Core creates the
Keychain item on managed startup and never reads a configuration secret.
There is no credential-printing or plaintext provisioning command.

HTTP adds installation authentication, not Operator authority. It uses
the same generated tools and application authorization as the socket MCP
adapter. Every call checks the live Run scope before idempotency replay;
HTTP has no durable session or authorization cache. It supports stateless
POST responses; GET, DELETE and browser preflight return method refusal.
Browser Origins are all refused, including `null`; the Host must exactly
match the numeric listener authority. This interface is for native local
clients, not WebViews, browsers, proxies, or remote access.

Complete request envelopes are checked before the MCP SDK can reflect
identifiers or protocol metadata; authentication headers never reach the
SDK. Request and response bodies are bounded to 4 MiB, and incomplete
HTTP headers or bodies time out after two seconds. Core shutdown closes
HTTP clients and its listener together with the socket transport.

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
