# Operator MCP acceptance runner

A scripted, repeatable run of the Linear-parity acceptance tests against a
real Kanban core over `POST /mcp/operator`. It uses only the Python 3
standard library (3.9 or newer) and `git`.

Tests 1–10 are the scenarios in
[`docs/mcp-acceptance-tests.md`](../../docs/mcp-acceptance-tests.md); test 11
covers sub-issues as [`docs/sub-issues.md`](../../docs/sub-issues.md) and
[`docs/linear-issue-model.md`](../../docs/linear-issue-model.md) describe
them. Every check asserts Linear's behaviour, not merely that a call
succeeded.

## Run it

Build the core, then run from the repository root:

```sh
cargo build --locked -p kanban-service --bin kanban-service

# Start a core on an empty data root, set up LIN and LTB, run all 11 tests.
python3 -m scripts.acceptance --fresh

# Re-read an existing database (for example a migrated copy of an old one):
# tests 2, 9 and 10 only, and no writes.
python3 -m scripts.acceptance --read-only --project LIN --data-dir /path/to/root
```

Options:

- `--binary PATH`: the `kanban-service` to start (default
  `target/debug/kanban-service`).
- `--data-dir DIR`: the data root. The core's own directory is `DIR/Kanban`,
  or `DIR` itself when it holds `kanban.sqlite`; `XDG_DATA_HOME` is set to its
  parent. `--fresh` needs it empty or absent and otherwise makes a temporary
  one, which it keeps and names at the end so it can be re-read.
- `--port N`: the loopback HTTP port (default 9891).
- `--tests 3,5`: run only those tests. Each fresh test creates its own issues,
  so any subset works after setup. An unknown number, or a selection that
  matches nothing, is refused before a core starts.

A fresh run takes a few seconds. It starts the core with
`--loopback-http 127.0.0.1:PORT`, registers the LIN and LTB projects over the
core socket (no MCP tool registers projects), creates milestones and labels
with the MCP tools, runs the tests, and stops the core with `service.stop`.

In `--read-only` mode the runner never starts or stops a core on the store
it inspects. It attaches to a core already serving that store, or copies the
store and runs against the copy. Only query tools are allowed past the
client. The run ends by checking that every file in the inspected store is
unchanged.

## Output

One line per check, then a per-test summary:

```text
PASS  3.15 add_labels adds without replacing the set -- ['frontend', 'size-small', 'storage']
XFAIL 3.29 label names match ignoring case -- invalid_request field=add_labels 'label case differs'
```

- `PASS`: matches Linear. `FAIL`: does not.
- `XFAIL`: the failure text is exactly the signature the waiver named.
  Any other failure is `FAIL`. `XPASS`: a waived check that now passes;
  turn its `rec.xfail` into `rec.check`.
- `SKIP`: cannot run here (a write in read-only mode, or data the database
  does not hold).

A test's verdict is `pass` (no FAIL), `partial` (some FAIL) or `fail` (only
FAIL). The exit code is 0 only when nothing FAILs; 2 means the runner could
not drive the core.

## Waivers

A future waiver must name the exact failure it still expects. A different
error from the same check is not waived, and neither is a prefix of the
expected text. There are no open waivers.

## Secrets

The bearer is derived in memory from the core's 32-byte installation secret.
Neither the bearer nor the decoded text of that secret is printed, logged
or written, including text that contains a newline. A secret nested in a
value, including a set, is removed before that value is escaped, shortened,
or shown in a difference. Python string and bytes renderings are removed
too, including a secret spliced into a longer value or already escaped, so
a fragment cannot survive rendering.

The core files the secret under an account: `installation` for its default
data directory, or `data-` plus the SHA-256 of the canonical path for any
other. The runner works the account out the same way and tries the other one
if the core refuses the first.

- Linux: the secret is the file `installation-credential/<account>` in the data
  directory. The runner sets `XDG_DATA_HOME` so the directory it starts a core
  on is that core's default.
- macOS: the secret is a login Keychain generic password, service
  `dev.kanban.desktop.installation`, read with the same Security.framework
  call the core uses. `XDG_DATA_HOME` does not apply there, so a fresh run
  uses a `data-…` account. The first read of each item asks whether Python
  may use it; choose Always Allow. With no desktop session (over SSH, say) the
  prompt cannot appear and the read fails with OSStatus -25308.

Each fresh data root on macOS leaves one Keychain item, and the run prints the
command to remove it. Reusing one path keeps it to a single item and a single
prompt:

```sh
rm -rf /tmp/kanban-acceptance && python3 -m scripts.acceptance --fresh --data-dir /tmp/kanban-acceptance
```

## Files

- `__main__.py`: options, core lifecycle, test selection.
- `harness.py`: the core process, its socket, the MCP client, and the
  installation secret (file or Keychain).
- `context.py`: shared helpers, and the read-only tool allow-list.
- `scenarios.py`: setup and the 11 tests.
- `checks.py`: outcomes and the summary.
- `fixtures/long-report.md`: the report test 8 posts word for word.
- `test_acceptance.py`: unit tests for the runner itself, needing no core:
  `python3 -m unittest scripts.acceptance.test_acceptance -v`.
