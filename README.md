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
