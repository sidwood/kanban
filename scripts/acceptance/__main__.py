"""Run the operator MCP acceptance tests against one Kanban core.

    python3 -m scripts.acceptance --fresh
    python3 -m scripts.acceptance --read-only --project LIN --data-dir DIR

See scripts/acceptance/README.md.
"""

import argparse
import os
import secrets
import shutil
import subprocess
import sys
import tempfile
import time
import traceback
from pathlib import Path

from . import scenarios
from .checks import Recorder
from .context import Context, StepError
from .harness import (
    KEYCHAIN_SERVICE,
    CoreProcess,
    HarnessError,
    McpClient,
    Redactor,
    SocketClient,
    Unauthorised,
    bearer_candidates,
    copy_store,
    credential_accounts,
    disposable_directory,
    port_in_use,
    socket_answers,
    store_changes,
    store_fingerprint,
)

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_BINARY = ROOT / "target" / "debug" / "kanban-service"
READ_ONLY_TESTS = (2, 9, 10)
VALID_TESTS = {number for number, *_rest in scenarios.TESTS}


def parse_arguments(argv):
    parser = argparse.ArgumentParser(
        prog="python3 -m scripts.acceptance",
        description="Run Kanban's operator MCP acceptance tests (Linear parity).",
    )
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--fresh", action="store_true",
                      help="start a core on an empty data directory, set up LIN and LTB, run all tests")
    mode.add_argument("--read-only", action="store_true",
                      help="attach to an existing database and run tests 2, 9 and 10 without writing")
    parser.add_argument("--project", help="team (project) code to read; required with --read-only")
    parser.add_argument("--binary", default=str(DEFAULT_BINARY),
                        help="kanban-service binary (default: target/debug/kanban-service)")
    parser.add_argument("--data-dir",
                        help="data root: the core's directory is DIR/Kanban (or DIR itself when it "
                             "holds kanban.sqlite). --fresh needs it empty or absent and defaults to "
                             "a new temporary directory")
    parser.add_argument("--port", type=int, default=9891, help="loopback HTTP port (default 9891)")
    parser.add_argument("--tests", help="comma-separated test numbers to run, for example 3,5")
    arguments = parser.parse_args(argv)
    if arguments.read_only and not arguments.project:
        parser.error("--read-only needs --project CODE")
    if arguments.read_only and not arguments.data_dir:
        parser.error("--read-only needs --data-dir pointing at the existing database")
    if arguments.tests is not None:
        try:
            parsed = {int(part) for part in arguments.tests.split(",") if part.strip()}
        except ValueError:
            parser.error("--tests takes comma-separated numbers")
        if not parsed:
            parser.error("no selected test can run")
        unknown = sorted(parsed - VALID_TESTS)
        if unknown:
            parser.error("unknown test number: " + ", ".join(str(number) for number in unknown))
        arguments.tests = parsed
    allowed = set(READ_ONLY_TESTS) if arguments.read_only else set(VALID_TESTS)
    selected = allowed if arguments.tests is None else arguments.tests & allowed
    if not selected:
        parser.error("no selected test can run")
    arguments.selected = selected
    return arguments


def git(*command, cwd=None):
    environment = dict(os.environ)
    environment.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull})
    subprocess.run(["git", *command], cwd=cwd, check=True, env=environment,
                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)


def make_repositories(root):
    """One-commit git repositories, each with a `.seed` clone beside it."""
    repositories = {}
    for code in ("LIN", "LTB"):
        repository = root / "repos" / code.lower()
        repository.mkdir(parents=True)
        git("-c", "init.defaultBranch=main", "init", "-q", str(repository))
        git("-c", "user.name=Acceptance", "-c", "user.email=acceptance@example.invalid",
            "commit", "-q", "--allow-empty", "-m", "Seed", cwd=repository)
        git("clone", "-q", str(repository), str(repository) + ".seed")
        repositories[code] = repository
    return repositories


def resolve_data_dir(root):
    """The core's own directory under a data root."""
    root = Path(root).resolve()
    if (root / "kanban.sqlite").exists():
        return root
    return root / "Kanban"


def repository_commit():
    try:
        return subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, check=True,
                              capture_output=True, text=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"



def run_test(recorder, ctx, number, title, function):
    recorder.begin(number, title)
    try:
        function(ctx)
    except StepError as error:
        recorder.abort(error)
    except Exception as error:  # report and keep going with the next test
        frame = traceback.extract_tb(error.__traceback__)[-1]
        recorder.abort(f"{type(error).__name__}: {error} (at {Path(frame.filename).name}:{frame.lineno})")


def main(argv=None):
    arguments = parse_arguments(sys.argv[1:] if argv is None else argv)
    redact = Redactor()

    def emit(line):
        print(redact(line), flush=True)

    host = "127.0.0.1"
    snapshot_root = None
    core = None
    started = False
    repositories = None
    before_store = None
    if arguments.fresh:
        root = Path(arguments.data_dir or tempfile.mkdtemp(prefix="kanban-acceptance-")).resolve()
        if root.exists() and any(root.iterdir()):
            emit(f"error: --fresh needs an empty data directory; {root} is not empty")
            return 2
        root.mkdir(parents=True, exist_ok=True)
        data_dir = root / "Kanban"
        log_path = root / "core.log"
        run_data_dir = data_dir
    else:
        root = Path(arguments.data_dir).resolve()
        data_dir = resolve_data_dir(root)
        if not (data_dir / "kanban.sqlite").exists():
            emit(f"error: no kanban.sqlite under {root}")
            return 2
        before_store = store_fingerprint(root)
        run_data_dir = data_dir
        log_path = None

    mode = "fresh" if arguments.fresh else f"read-only, team {arguments.project}"
    emit(f"Kanban operator MCP acceptance ({mode})")
    emit(f"repository {repository_commit()}; binary {arguments.binary}")
    emit(f"data directory {data_dir}; loopback {host}:{arguments.port}")

    recorder = Recorder(emit=emit, redact=redact)
    try:
        if arguments.read_only and socket_answers(data_dir / "core.sock"):
            if not port_in_use(host, arguments.port):
                emit(f"error: a core serves {data_dir / 'core.sock'} but not {host}:{arguments.port}; "
                     "restart it with --loopback-http or pass its --port")
                return 2
            emit("attached to the core already serving this data directory")
            environ = os.environ
        elif arguments.read_only:
            snapshot_root = disposable_directory(root)
            run_data_dir = snapshot_root / data_dir.name
            copy_store(data_dir, run_data_dir)
            log_path = snapshot_root / "core.log"
            core = CoreProcess(arguments.binary, run_data_dir, arguments.port, log_path, host=host)
            core.start()
            started = True
            emit(f"core started on a disposable snapshot; log {log_path}")
            environ = core.environment()
        else:
            repositories = make_repositories(root)
            core = CoreProcess(arguments.binary, data_dir, arguments.port, log_path, host=host)
            core.start()
            started = True
            emit(f"core started; log {log_path}")
            environ = core.environment()
        mcp, account = connect(host, arguments.port, run_data_dir, environ, redact)
        emit(f"installation secret: account {account} in {secret_store(run_data_dir)}")
        run_id = time.strftime("%Y%m%d%H%M%S") + "-" + secrets.token_hex(3)
        socket_client = None if arguments.read_only else SocketClient(core.socket_path)
        ctx = Context(mcp, socket_client, recorder, run_id, arguments.read_only)
        if arguments.fresh:
            try:
                scenarios.setup_fresh(ctx, repositories)
            except Exception as error:
                recorder.abort(f"setup failed, so no test can run: {error}")
                return finish(recorder, emit, root, arguments, account)
        else:
            projects = ctx.must("list_projects", {})["projects"]
            match = [entry for entry in projects if entry["code"] == arguments.project]
            if not match:
                emit(f"error: no team {arguments.project}; the database holds "
                     + ", ".join(entry["code"] for entry in projects))
                return 2
            ctx.project_code = arguments.project
            ctx.projects = {entry["code"]: entry["id"] for entry in projects}
        for number, title, function, _read_only_capable in scenarios.TESTS:
            if number not in arguments.selected:
                continue
            run_test(recorder, ctx, number, title, function)
        if arguments.read_only:
            changed = store_changes(before_store, store_fingerprint(root))
            recorder.begin(0, "Read-only guard")
            recorder.check(
                "the run left the inspected store unchanged",
                not changed,
                "changed: " + ", ".join(changed[:10]) if changed else f"{len(before_store)} files unchanged",
            )
        return finish(recorder, emit, root, arguments, account)
    except (HarnessError, OSError, subprocess.CalledProcessError) as error:
        emit(f"error: {error}")
        for line in recorder.summary_lines():
            emit(line)
        return 2
    finally:
        if core is not None and core.process is not None:
            if started:
                problem = core.stop()
                emit(f"core stopped{': ' + problem if problem else ''}")
            else:
                core.reap()
                emit("core stopped")
        if snapshot_root is not None:
            shutil.rmtree(snapshot_root, ignore_errors=True)


def secret_store(data_dir):
    if sys.platform == "darwin":
        return "the login Keychain"
    return str(data_dir / "installation-credential")


def connect(host, port, data_dir, environ, redact):
    """An initialised MCP client, trying each account that holds a secret
    until the core accepts one."""
    refused = []
    for account, bearer in bearer_candidates(data_dir, environ, redact=redact):
        redact.add(bearer)
        mcp = McpClient(host, port, bearer)
        try:
            mcp.initialize()
            return mcp, account
        except Unauthorised:
            refused.append(account)
    if refused:
        raise HarnessError(f"the core refused the secret under account {', '.join(refused)} "
                           f"in {secret_store(data_dir)}")
    raise HarnessError(f"no installation secret for {data_dir} in {secret_store(data_dir)} "
                       f"(looked for account {', '.join(credential_accounts(data_dir, environ))})")


def finish(recorder, emit, root, arguments, account):
    for line in recorder.summary_lines():
        emit(line)
    if arguments.fresh:
        emit(f"data root kept at {root}; re-read it with: python3 -m scripts.acceptance "
             f"--read-only --project LIN --data-dir {root}")
        if sys.platform == "darwin" and account != "installation":
            emit(f"the core added a login Keychain item for this data root; once you are done with it, "
                 f"remove it with: security delete-generic-password -s {KEYCHAIN_SERVICE} -a {account}")
    return recorder.exit_code()


if __name__ == "__main__":
    sys.exit(main())
