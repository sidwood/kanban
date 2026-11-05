"""Unit tests for the acceptance runner itself; no core is needed.

    python3 -m unittest scripts.acceptance.test_acceptance -v
"""

import base64
import ctypes
import hashlib
import json
import os
import tempfile
import unittest
from io import StringIO
from unittest.mock import patch
from pathlib import Path

from scripts.acceptance import checks, context, harness, scenarios


def hashed_account(path):
    """The account the core gives a data directory that is not the default."""
    return "data-" + hashlib.sha256(os.fsencode(str(Path(path).resolve()))).hexdigest()


class CredentialAccountTests(unittest.TestCase):
    """The account mirrors the core: `installation` for the platform's
    managed data directory, `data-<sha256 of the canonical path>` for
    any other."""

    def setUp(self):
        self.root = Path(tempfile.mkdtemp()).resolve()
        self.addCleanup(lambda: __import__("shutil").rmtree(self.root, ignore_errors=True))

    def directory(self, *parts):
        path = self.root.joinpath(*parts)
        path.mkdir(parents=True)
        return path

    def test_linux_default_directory_is_the_installation_account(self):
        data = self.directory("share", "Kanban")
        environ = {"HOME": str(self.root / "home"), "XDG_DATA_HOME": str(data.parent)}
        self.assertEqual(harness.credential_accounts(data, environ, "linux"),
                         ["installation", hashed_account(data)])

    def test_linux_other_directory_is_a_hashed_account(self):
        data = self.directory("elsewhere", "Kanban")
        environ = {"HOME": str(self.root / "home"), "XDG_DATA_HOME": str(self.root / "share")}
        self.assertEqual(harness.credential_accounts(data, environ, "linux"),
                         [hashed_account(data), "installation"])

    def test_linux_ignores_a_relative_xdg_data_home(self):
        data = self.directory("home", ".local", "share", "Kanban")
        environ = {"HOME": str(self.root / "home"), "XDG_DATA_HOME": "relative/share"}
        self.assertEqual(harness.credential_accounts(data, environ, "linux")[0], "installation")

    def test_macos_default_is_application_support_whatever_xdg_says(self):
        data = self.directory("home", "Library", "Application Support", "Kanban")
        elsewhere = self.directory("xdg", "Kanban")
        environ = {"HOME": str(self.root / "home"), "XDG_DATA_HOME": str(elsewhere.parent)}
        self.assertEqual(harness.credential_accounts(data, environ, "darwin")[0], "installation")
        self.assertEqual(harness.credential_accounts(elsewhere, environ, "darwin")[0],
                         hashed_account(elsewhere))

    def test_the_hash_covers_the_canonical_path(self):
        real = self.directory("real", "Kanban")
        link = self.root / "link"
        link.symlink_to(real.parent)
        environ = {"HOME": str(self.root / "home")}
        self.assertEqual(harness.credential_accounts(link / "Kanban", environ, "linux")[0],
                         hashed_account(real))


class FakeSecurity:
    """Stands in for Security.framework's legacy generic-password calls."""

    def __init__(self, status=0, secret=b"\x07" * 32):
        self.status = status
        self.secret = secret
        self.lookups = []
        self.freed = 0
        self._buffer = ctypes.create_string_buffer(secret, len(secret))

        def find(keychain, service_length, service, account_length, account, length, data, item):
            self.lookups.append((keychain, service[:service_length], account[:account_length], item))
            if self.status == 0:
                length._obj.value = len(self.secret)
                data._obj.value = ctypes.addressof(self._buffer)
            return self.status

        def free(attributes, data):
            self.freed += 1
            return 0

        self.SecKeychainFindGenericPassword = find
        self.SecKeychainItemFreeContent = free


class InstallationSecretTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp()).resolve()
        self.addCleanup(lambda: __import__("shutil").rmtree(self.root, ignore_errors=True))
        self.data = self.root / "Kanban"
        (self.data / "installation-credential").mkdir(parents=True)

    def store_file(self, account, secret):
        (self.data / "installation-credential" / account).write_bytes(secret)

    def test_linux_reads_the_account_file(self):
        self.store_file("installation", b"\x01" * 32)
        self.assertEqual(harness.read_installation_secret(self.data, "installation", "linux"), b"\x01" * 32)

    def test_linux_missing_account_is_none(self):
        self.assertIsNone(harness.read_installation_secret(self.data, "installation", "linux"))

    def test_a_secret_that_is_not_32_bytes_is_refused(self):
        self.store_file("installation", b"\x01" * 31)
        with self.assertRaises(harness.HarnessError):
            harness.read_installation_secret(self.data, "installation", "linux")

    def test_macos_reads_the_generic_password_and_frees_it(self):
        security = FakeSecurity(secret=b"\x09" * 32)
        secret = harness.read_installation_secret(self.data, "data-abc", "darwin", keychain=security)
        self.assertEqual(secret, b"\x09" * 32)
        self.assertEqual(security.lookups,
                         [(None, b"dev.kanban.desktop.installation", b"data-abc", None)])
        self.assertEqual(security.freed, 1)

    def test_macos_missing_item_is_none(self):
        security = FakeSecurity(status=-25300)
        self.assertIsNone(harness.read_installation_secret(self.data, "installation", "darwin",
                                                           keychain=security))
        self.assertEqual(security.freed, 0)

    def test_macos_refusals_explain_the_keychain_prompt(self):
        for status in (-128, -25293, -25308, -61):
            with self.subTest(status=status):
                with self.assertRaises(harness.HarnessError) as raised:
                    harness.read_installation_secret(self.data, "installation", "darwin",
                                                     keychain=FakeSecurity(status=status))
                self.assertIn("Keychain", str(raised.exception))
                self.assertIn(str(status), str(raised.exception))

    def test_bearer_candidates_try_the_core_account_first_and_skip_missing_ones(self):
        environ = {"HOME": str(self.root / "home"), "XDG_DATA_HOME": str(self.root)}
        hashed = hashed_account(self.data)
        self.store_file("installation", b"\x01" * 32)
        self.store_file(hashed, b"\x02" * 32)
        found = list(harness.bearer_candidates(self.data, environ, "linux"))
        self.assertEqual(found, [("installation", harness.derive_bearer(b"\x01" * 32)),
                                 (hashed, harness.derive_bearer(b"\x02" * 32))])
        (self.data / "installation-credential" / "installation").unlink()
        self.assertEqual([account for account, _ in harness.bearer_candidates(self.data, environ, "linux")],
                         [hashed])

    def test_bearer_candidates_register_the_installation_secret(self):
        secret = b"\x11" * 32
        self.store_file("installation", secret)
        environ = {"HOME": str(self.root / "home"), "XDG_DATA_HOME": str(self.root)}
        redact = harness.Redactor()
        found = list(harness.bearer_candidates(self.data, environ, "linux", redact=redact))
        self.assertEqual([account for account, _bearer in found], ["installation"])
        rendered = redact("hex " + secret.hex() + " bearer " + harness.derive_bearer(secret))
        self.assertNotIn(secret.hex(), rendered)
        self.assertNotIn(harness.derive_bearer(secret), rendered)



class BearerTests(unittest.TestCase):
    def test_bearer_is_prefixed_unpadded_base64url(self):
        secret = bytes(range(250, 256)) + b"\xfb\xff"
        bearer = harness.derive_bearer(secret)
        self.assertTrue(bearer.startswith("kanban_"))
        encoded = bearer[len("kanban_"):]
        self.assertNotIn("=", encoded)
        self.assertNotIn("+", encoded)
        self.assertNotIn("/", encoded)
        padded = encoded + "=" * (-len(encoded) % 4)
        self.assertEqual(base64.urlsafe_b64decode(padded), secret)

    def test_redactor_removes_every_registered_secret(self):
        redact = harness.Redactor()
        redact.add("kanban_SECRET")
        self.assertEqual(redact("Bearer kanban_SECRET leaked kanban_SECRET"),
                         "Bearer [redacted] leaked [redacted]")
        self.assertEqual(redact("nothing here"), "nothing here")

    def test_a_truncated_bearer_leaves_no_fragment(self):
        """A secret that begins inside the shortened line must not survive as a prefix."""
        redact = harness.Redactor()
        secret = harness.derive_bearer(bytes(range(32)))
        redact.add(secret)
        lines = []
        recorder = checks.Recorder(emit=lines.append, redact=redact)
        recorder.begin(1, "Secrets")
        recorder.equal("equal boundary", "x" * 130 + secret, "x" * 130 + secret)
        recorder.abort("x" * 270 + secret)
        rendered = "\n".join(lines + recorder.summary_lines())
        self.assertNotIn(secret, rendered)
        self.assertNotIn(secret[:20], rendered)
        self.assertNotIn(secret[-12:], rendered)

    def test_a_truncated_difference_leaves_no_secret_fragment(self):
        redact = harness.Redactor()
        secret = harness.derive_bearer(bytes(range(32)))
        redact.add(secret)
        lines = []
        recorder = checks.Recorder(emit=lines.append, redact=redact)
        recorder.begin(1, "Secrets")
        recorder.equal("wrapped", {"token": "x" * 130 + secret + "A"}, {"token": "other"})
        rendered = "\n".join(lines + recorder.summary_lines())
        self.assertNotIn(secret[:16], rendered)
        self.assertNotIn(secret[-12:], rendered)

    def test_a_truncated_installation_secret_leaves_no_fragment(self):
        redact = harness.Redactor()
        secret = bytes(range(32))
        redact.add(secret)
        lines = []
        recorder = checks.Recorder(emit=lines.append, redact=redact)
        recorder.begin(1, "Secrets")
        recorder.equal("hex boundary", "y" * 140 + secret.hex(), "y" * 140 + secret.hex())
        rendered = "\n".join(lines + recorder.summary_lines())
        self.assertNotIn(secret.hex(), rendered)
        self.assertNotIn(secret.hex()[:16], rendered)
        self.assertNotIn(harness.derive_bearer(secret), rendered)

class CoreProcessTests(unittest.TestCase):
    def test_a_socket_path_too_long_is_refused_before_the_core_starts(self):
        import sys
        import tempfile
        from pathlib import Path
        with tempfile.TemporaryDirectory() as root:
            data_dir = Path(root) / ("d" * 120) / "Kanban"
            core = harness.CoreProcess(sys.executable, data_dir, 1, Path(root) / "core.log")
            with self.assertRaises(harness.HarnessError) as raised:
                core.start()
            self.assertIn("too long for a Unix socket", str(raised.exception))
            self.assertIsNone(core.process)
            self.assertFalse(data_dir.parent.exists())


class UnauthorisedTests(unittest.TestCase):
    def test_a_rejected_bearer_raises_unauthorised(self):
        import http.server
        import threading

        class Refuse(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers.get("content-length", 0)))
                self.send_response(401)
                self.send_header("content-length", "0")
                self.end_headers()

            def log_message(self, *arguments):
                pass

        server = http.server.HTTPServer(("127.0.0.1", 0), Refuse)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        client = harness.McpClient("127.0.0.1", server.server_address[1], "kanban_wrong")
        with self.assertRaises(harness.Unauthorised):
            client.initialize()


class RpcBodyTests(unittest.TestCase):
    def test_json_body_is_decoded(self):
        body = json.dumps({"jsonrpc": "2.0", "id": 3, "result": {}}).encode()
        self.assertEqual(harness.parse_rpc_body("application/json", body, 3)["id"], 3)

    def test_event_stream_picks_the_message_for_the_request(self):
        body = (
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n"
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\n"
            "data: \"result\":{\"ok\":true}}\n\n"
        )
        message = harness.parse_rpc_body("text/event-stream", body.encode(), 7)
        self.assertEqual(message, {"jsonrpc": "2.0", "id": 7, "result": {"ok": True}})

    def test_empty_json_body_is_none(self):
        self.assertIsNone(harness.parse_rpc_body("application/json", b"", 1))


class ToolResultTests(unittest.TestCase):
    def test_structured_content_is_the_payload(self):
        result = harness.tool_result_from_rpc(
            {"result": {"structuredContent": {"identifier": "LIN-1"}, "content": []}})
        self.assertTrue(result.ok)
        self.assertEqual(result.payload, {"identifier": "LIN-1"})

    def test_text_content_is_the_fallback_payload(self):
        result = harness.tool_result_from_rpc(
            {"result": {"content": [{"type": "text", "text": "{\"a\": 1}"}]}})
        self.assertEqual(result.payload, {"a": 1})

    def test_is_error_marks_a_refusal_with_code_and_field(self):
        result = harness.tool_result_from_rpc({"result": {"isError": True, "structuredContent": {
            "code": "invalid_request", "field": "milestone_id", "message": "nope"}}})
        self.assertFalse(result.ok)
        self.assertEqual((result.code, result.field), ("invalid_request", "milestone_id"))
        self.assertIn("field=milestone_id", result.describe())

    def test_json_rpc_error_is_a_refusal(self):
        result = harness.tool_result_from_rpc({"error": {"code": -32602, "message": "bad params"}})
        self.assertFalse(result.ok)
        self.assertEqual(result.code, "rpc_error")


class RecorderTests(unittest.TestCase):
    def recorder(self):
        lines = []
        return checks.Recorder(emit=lines.append), lines

    def test_a_matching_waiver_does_not_fail_the_run(self):
        recorder, lines = self.recorder()
        recorder.begin(1, "Create")
        recorder.check("passes", True)
        expected = "invalid_request field=state 'unknown state'"
        recorder.xfail("still open", False, expected, expected=expected)
        recorder.xfail("now closed", True, expected=expected)
        self.assertEqual(recorder.exit_code(), 0)
        self.assertEqual(recorder.tests[0].verdict, "pass")
        self.assertTrue(lines[-2].startswith("XFAIL 1.02 still open -- " + expected))
        self.assertTrue(lines[-1].startswith("XPASS 1.03 now closed"))

    def test_an_unrelated_failure_is_not_waived(self):
        recorder, lines = self.recorder()
        recorder.begin(3, "Update")
        recorder.xfail(
            "add_labels matches a label name ignoring case",
            False,
            "internal 'synthetic unrelated backend failure'",
            expected="invalid_request field=add_labels 'label case differs'",
        )
        self.assertEqual(recorder.exit_code(), 1)
        self.assertTrue(lines[-1].startswith("FAIL "))
        self.assertNotIn("XFAIL", "\n".join(lines))

    def test_a_prefix_of_the_expected_failure_is_not_waived(self):
        recorder, lines = self.recorder()
        recorder.begin(1, "Create")
        recorder.xfail("gap", False, "internal 'other'", expected="internal")
        self.assertEqual(recorder.exit_code(), 1)
        self.assertTrue(lines[-1].startswith("FAIL "))

    def test_a_waiver_without_a_signature_is_refused(self):
        recorder, _lines = self.recorder()
        recorder.begin(3, "Update")
        with self.assertRaises(TypeError):
            recorder.xfail("gap", False, "anything")

    def test_an_empty_waiver_signature_is_not_a_waiver(self):
        recorder, lines = self.recorder()
        recorder.begin(3, "Update")
        recorder.xfail("gap", False, "internal error", expected="")
        self.assertEqual(recorder.exit_code(), 1)
        self.assertTrue(lines[-1].startswith("FAIL "))

    def test_closed_linear_behaviours_are_ordinary_checks(self):
        source = Path(scenarios.__file__).read_text()
        self.assertNotIn("rec.xfail(", source)


    def test_any_fail_fails_the_run_and_marks_the_test_partial(self):
        recorder, _ = self.recorder()
        recorder.begin(5, "Relations")
        recorder.check("passes", True)
        recorder.equal("differs", "a", "b")
        self.assertEqual(recorder.exit_code(), 1)
        self.assertEqual(recorder.tests[0].verdict, "partial")

    def test_a_test_with_only_failures_fails(self):
        recorder, _ = self.recorder()
        recorder.begin(6, "Duplicate merge")
        recorder.abort("boom")
        self.assertEqual(recorder.tests[0].verdict, "fail")

    def test_skips_alone_neither_pass_nor_fail(self):
        recorder, _ = self.recorder()
        recorder.begin(2, "Read")
        recorder.skip("no data", "empty")
        self.assertEqual(recorder.tests[0].verdict, "skipped")
        self.assertEqual(recorder.exit_code(), 0)

    def test_equal_names_the_first_difference_of_two_strings(self):
        recorder, lines = self.recorder()
        recorder.begin(1, "Create")
        recorder.equal("round trip", "abcX", "abcY")
        self.assertIn("offset 3", lines[-1])

    def test_summary_is_redacted(self):
        lines = []
        recorder = checks.Recorder(emit=lines.append, redact=lambda text: text.replace("SECRET", "[r]"))
        recorder.begin(1, "Create")
        recorder.check("leak", False, "SECRET")
        self.assertNotIn("SECRET", "\n".join(lines + recorder.summary_lines()))


class RecordRuleTests(unittest.TestCase):
    def record(self, **overrides):
        record = {
            "identifier": "LIN-1", "url": "kanban://issue/LIN-1", "ticket_id": 1, "project_id": 1,
            "title": "T", "description": "", "state": "Done", "state_type": "completed", "priority": 0,
            "labels": [], "relations": [], "version": 3,
            "created_at": "2026-11-04T10:00:00.000Z", "updated_at": "2026-11-04T10:00:02.000Z",
            "completed_at": "2026-11-04T10:00:02.000Z",
            "state_history": [
                {"state": "Draft", "state_type": "backlog", "started_at": "2026-11-04T10:00:00.000Z",
                 "ended_at": "2026-11-04T10:00:02.000Z"},
                {"state": "Done", "state_type": "completed", "started_at": "2026-11-04T10:00:02.000Z"},
            ],
        }
        record.update(overrides)
        return record

    def test_a_well_formed_record_has_no_problems(self):
        record = self.record()
        self.assertEqual(scenarios.record_problems(record), [])
        self.assertEqual(scenarios.history_problems(record), [])
        self.assertEqual(scenarios.lifecycle_problems(record), [])

    def test_a_gap_in_the_history_is_found(self):
        record = self.record()
        record["state_history"][0]["ended_at"] = "2026-11-04T10:00:01.000Z"
        self.assertTrue(scenarios.history_problems(record))

    def test_an_empty_history_is_found(self):
        self.assertEqual(scenarios.history_problems(self.record(state_history=[])), ["no state history rows"])

    def test_a_completed_issue_without_completed_at_is_found(self):
        record = self.record()
        del record["completed_at"]
        self.assertTrue(scenarios.lifecycle_problems(record))

    def test_a_reopened_issue_keeping_completed_at_is_found(self):
        self.assertTrue(scenarios.lifecycle_problems(self.record(state="Active", state_type="started",
                                                                 started_at="2026-11-04T10:00:03.000Z")))

    def test_an_unknown_field_is_reported(self):
        self.assertIn("unexpected surprise", scenarios.record_problems(self.record(surprise=1)))

    def test_one_appended_history_row_is_accepted(self):
        before = self.record()
        after = self.record()
        after["state_history"] = [dict(row) for row in before["state_history"]]
        after["state_history"][-1]["ended_at"] = "2026-11-04T10:00:05.000Z"
        after["state_history"].append({"state": "Active", "state_type": "started",
                                       "started_at": "2026-11-04T10:00:05.000Z"})
        self.assertEqual(scenarios.history_appended(before, after, "Active", "started"), [])
        self.assertTrue(scenarios.history_appended(before, before, "Active", "started"))


class TimestampTests(unittest.TestCase):
    def test_parses_rfc3339_variants(self):
        self.assertIsNotNone(context.parse_timestamp("2026-11-04T22:11:36.293Z"))
        self.assertIsNotNone(context.parse_timestamp("2026-11-04T22:11:36Z"))
        self.assertIsNotNone(context.parse_timestamp("2026-11-04T22:11:36.123456789+01:00"))

    def test_rejects_other_shapes(self):
        for text in ("2026-11-04", "2026-11-04 22:11:36Z", "yesterday", None, "2026-02-30T00:00:00Z"):
            self.assertIsNone(context.parse_timestamp(text), text)

    def test_orders_across_offsets(self):
        earlier = context.parse_timestamp("2026-11-04T10:00:00+01:00")
        later = context.parse_timestamp("2026-11-04T09:30:00Z")
        self.assertLess(earlier, later)


class ReadOnlyGuardTests(unittest.TestCase):
    class FakeMcp:
        def __init__(self):
            self.called = []

        def call(self, tool, arguments):
            self.called.append(tool)
            return harness.ToolResult(True, {})

    def test_read_only_refuses_writes_before_they_reach_the_core(self):
        mcp = self.FakeMcp()
        ctx = context.Context(mcp, None, checks.Recorder(emit=lambda line: None), "run", read_only=True)
        for tool in ("save_issue", "comment_create", "create_label", "retire_label",
                     "update_project_settings", "resolve_comment"):
            with self.assertRaises(context.StepError):
                ctx.call(tool, {})
        with self.assertRaises(context.StepError):
            ctx.legacy("ticket.create", {})
        ctx.call("get_issue", {"identifier": "LIN-1"})
        self.assertEqual(mcp.called, ["get_issue"])

    def test_every_read_tool_is_an_operator_tool(self):
        self.assertTrue(context.READ_TOOLS <= scenarios.OPERATOR_TOOLS)

    def test_idempotency_keys_never_repeat_within_a_run(self):
        ctx = context.Context(None, None, None, "run", read_only=False)
        keys = {ctx.idempotency_key("x") for _ in range(100)}
        self.assertEqual(len(keys), 100)


class FieldDiffTests(unittest.TestCase):
    def test_changed_fields_ignores_allowed_names(self):
        before = {"title": "a", "version": 1, "labels": ["x"]}
        after = {"title": "b", "version": 2, "labels": ["x"], "due_date": "2030-01-01"}
        self.assertEqual(context.changed_fields(before, after, ("version",)), ["due_date", "title"])


class SelectionTests(unittest.TestCase):
    """An unknown or empty selection must not start a core."""

    def run_main(self, argv):
        from scripts.acceptance import __main__ as cli

        stderr = StringIO()
        with patch("scripts.acceptance.__main__.CoreProcess") as core, \
                patch("scripts.acceptance.__main__.connect",
                      side_effect=harness.HarnessError("should not connect")), \
                patch("scripts.acceptance.harness.keychain_secret",
                      side_effect=AssertionError("keychain")), \
                patch("sys.stderr", stderr):
            try:
                status = cli.main(argv)
            except SystemExit as exited:
                status = exited.code
        return status, core, stderr.getvalue()

    def test_an_unknown_test_number_is_rejected_before_a_core_starts(self):
        status, core, errors = self.run_main(["--fresh", "--tests", "99", "--binary", "/nonexistent"])
        self.assertEqual(status, 2)
        self.assertIn("unknown", errors)
        core.assert_not_called()

    def test_an_unknown_number_among_real_ones_is_rejected_before_a_core_starts(self):
        status, core, errors = self.run_main(["--fresh", "--tests", "2,99", "--binary", "/nonexistent"])
        self.assertEqual(status, 2)
        self.assertIn("unknown", errors)
        core.assert_not_called()

    def test_an_empty_selection_is_rejected_before_a_core_starts(self):
        status, core, errors = self.run_main(["--fresh", "--tests", ",", "--binary", "/nonexistent"])
        self.assertEqual(status, 2)
        self.assertIn("no selected", errors)
        core.assert_not_called()

    def test_a_read_only_selection_with_no_readable_test_is_rejected_before_a_core_starts(self):
        with tempfile.TemporaryDirectory() as root:
            data = Path(root) / "Kanban"
            data.mkdir()
            (data / "kanban.sqlite").write_bytes(b"")
            status, core, errors = self.run_main([
                "--read-only", "--project", "LIN", "--data-dir", root,
                "--tests", "1", "--binary", "/nonexistent",
            ])
        self.assertEqual(status, 2)
        self.assertIn("no selected", errors)
        core.assert_not_called()


class StartupFailureTests(unittest.TestCase):
    def test_a_core_that_never_opens_its_port_is_reaped(self):
        import signal

        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            script = root / "sleeping-core"
            script.write_text("#!/bin/sh\nexec sleep 30\n")
            script.chmod(0o755)
            data = root / "Kanban"
            with harness.socket.socket() as probe:
                probe.bind(("127.0.0.1", 0))
                port = probe.getsockname()[1]
            core = harness.CoreProcess(script, data, port, root / "core.log")

            def kill():
                process = core.process
                if process is not None and process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)

            self.addCleanup(kill)
            with self.assertRaises(harness.HarnessError):
                core.start(wait_seconds=0.4)
            self.assertIsNotNone(core.process)
            self.assertIsNotNone(core.process.poll())

    def test_a_startup_failure_reaps_the_spawned_child(self):
        import signal
        import subprocess
        import sys

        from scripts.acceptance import __main__ as cli

        spawned = {}

        def start(self, wait_seconds=60):
            self.process = subprocess.Popen(["sleep", "30"], start_new_session=True)
            spawned["process"] = self.process
            raise harness.HarnessError("the core did not come up")

        def kill():
            process = spawned.get("process")
            if process is not None and process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)

        self.addCleanup(kill)
        with tempfile.TemporaryDirectory() as root, \
                patch.object(harness.CoreProcess, "start", start), \
                patch("scripts.acceptance.harness.keychain_secret",
                      side_effect=AssertionError("keychain")):
            status = cli.main(["--fresh", "--data-dir", root, "--binary", sys.executable, "--tests", "1"])
        self.assertEqual(status, 2)
        self.assertIsNotNone(spawned["process"].poll())

class ReadOnlyStoreTests(unittest.TestCase):
    def files(self, root):
        return {
            path.relative_to(root).as_posix(): path.read_bytes()
            for path in Path(root).rglob("*")
            if path.is_file()
        }

    def inside(self, path, root):
        path = Path(path).resolve()
        root = Path(root).resolve()
        return path == root or root in path.parents

    def test_read_only_runs_a_core_only_against_a_snapshot(self):
        from scripts.acceptance import __main__ as cli

        class FakeCore:
            started = []

            def __init__(self, binary, data_dir, port, log_path, host="127.0.0.1"):
                self.data_dir = Path(data_dir)
                self.log_path = Path(log_path)
                self.socket_path = self.data_dir / "core.sock"
                self.process = None
                self.host = host
                self.port = port
                FakeCore.started.append(self)

            def environment(self):
                return {}

            def start(self):
                self.process = object()
                self.data_dir.mkdir(parents=True, exist_ok=True)
                self.log_path.parent.mkdir(parents=True, exist_ok=True)
                self.log_path.write_text("core log")
                (self.data_dir / "kanban.sqlite").write_bytes(b"mutated-by-core")

            def stop(self):
                return None

            def reap(self):
                return None

        class FakeMcp:
            def unauthorised_initialize_status(self):
                return 401

            def call(self, name, arguments):
                payloads = {
                    "list_projects": {"projects": [{"code": "LIN", "id": 1, "name": "Linear"}]},
                    "list_issues": {"issues": []},
                    "list_issue_statuses": {"states": []},
                    "list_issue_labels": {"labels": []},
                    "list_milestones": {"milestones": []},
                }
                return harness.ToolResult(True, payloads.get(name, {}))

        FakeCore.started = []
        with tempfile.TemporaryDirectory() as root:
            inspected = Path(root) / "store"
            data = inspected / "Kanban"
            data.mkdir(parents=True)
            (data / "kanban.sqlite").write_bytes(b"original-database")
            (data / "sidecar.txt").write_text("keep")
            before = self.files(inspected)
            with patch("scripts.acceptance.__main__.CoreProcess", FakeCore), \
                    patch("scripts.acceptance.__main__.socket_answers", return_value=False), \
                    patch("scripts.acceptance.__main__.connect", return_value=(FakeMcp(), "installation")), \
                    patch("scripts.acceptance.harness.keychain_secret",
                          side_effect=AssertionError("keychain")):
                cli.main([
                    "--read-only", "--project", "LIN", "--data-dir", str(inspected),
                    "--binary", "/nonexistent", "--tests", "2",
                ])
            self.assertEqual(self.files(inspected), before)
            self.assertTrue(FakeCore.started)
            for core in FakeCore.started:
                self.assertFalse(self.inside(core.data_dir, inspected))
                self.assertFalse(self.inside(core.log_path, inspected))

    def test_read_only_guard_notices_a_change_outside_issue_rows(self):
        from io import StringIO
        from contextlib import redirect_stdout
        from scripts.acceptance import __main__ as cli

        class FakeMcp:
            def __init__(self, mutate):
                self.mutate = mutate

            def unauthorised_initialize_status(self):
                return 401

            def call(self, name, arguments):
                if name == "list_issues":
                    self.mutate()
                payloads = {
                    "list_projects": {"projects": [{"code": "LIN", "id": 1, "name": "Linear"}]},
                    "list_issues": {"issues": []},
                    "list_issue_statuses": {"states": []},
                    "list_issue_labels": {"labels": []},
                    "list_milestones": {"milestones": []},
                }
                return harness.ToolResult(True, payloads.get(name, {}))

        with tempfile.TemporaryDirectory() as root:
            inspected = Path(root)
            (inspected / "kanban.sqlite").write_bytes(b"original-database")
            captured = StringIO()

            def mutate():
                (inspected / "sidecar.txt").write_text("written during the run")

            with redirect_stdout(captured), \
                    patch("scripts.acceptance.__main__.socket_answers", return_value=True), \
                    patch("scripts.acceptance.__main__.port_in_use", return_value=True), \
                    patch("scripts.acceptance.__main__.connect",
                          return_value=(FakeMcp(mutate), "installation")), \
                    patch("scripts.acceptance.harness.keychain_secret",
                          side_effect=AssertionError("keychain")):
                cli.main([
                    "--read-only", "--project", "LIN", "--data-dir", str(inspected),
                    "--binary", "/nonexistent", "--tests", "2",
                ])
            failed = [
                line for line in captured.getvalue().splitlines()
                if line.startswith("FAIL") and "inspected store" in line
            ]
            self.assertTrue(failed)



class TextQueryTests(unittest.TestCase):
    def context(self, title_only):
        class MemoryMcp:
            def __init__(self):
                self.issues = []
                self.next_id = 1
                self.queries = []

            def call(self, name, arguments):
                if name == "save_issue":
                    return harness.ToolResult(True, self.save(arguments))
                if name == "get_issue":
                    return harness.ToolResult(True, self.by_key(arguments["identifier"]))
                if name == "list_issues":
                    return harness.ToolResult(True, {"issues": self.matching(arguments)})
                if name == "list_issue_statuses":
                    return harness.ToolResult(True, {"states": [
                        {"name": "Draft", "state_type": "backlog"},
                        {"name": "Done", "state_type": "completed"},
                    ]})
                return harness.ToolResult(True, {})

            def save(self, arguments):
                if arguments.get("identifier"):
                    return {"identifier": arguments["identifier"]}
                number = self.next_id
                self.next_id += 1
                key = f"LIN-{number}"
                self.issues.append({
                    "identifier": key,
                    "title": arguments.get("title", ""),
                    "description": arguments.get("description") or "",
                    "project_id": arguments.get("project_id", 1),
                    "updated_at": f"2026-11-05T00:00:{number:02d}.000Z",
                    "created_at": "2026-11-05T00:00:00.000Z",
                    "state": arguments.get("state", "Draft"),
                    "state_type": "backlog",
                    "labels": list(arguments.get("add_labels") or []),
                    "relations": [],
                    "version": 1,
                    "ticket_id": number,
                    "priority": 0,
                    "url": f"kanban://issue/{key}",
                    "state_history": [],
                })
                return {"identifier": key}

            def by_key(self, key):
                return next(issue for issue in self.issues if issue["identifier"] == key)

            def matching(self, arguments):
                query = arguments.get("query")
                if query:
                    self.queries.append(query)
                    folded = query.casefold()
                    if title_only:
                        return [issue for issue in self.issues if folded in issue["title"].casefold()]
                    return [
                        issue for issue in self.issues
                        if folded in issue["title"].casefold() or folded in issue["description"].casefold()
                    ]
                return list(self.issues)

        mcp = MemoryMcp()
        recorder = checks.Recorder(emit=lambda line: None)
        ctx = context.Context(mcp, None, recorder, "run", read_only=False, pause=0)
        ctx.projects = {"LIN": 1, "LTB": 2}
        ctx.project_code = "LIN"
        ctx.other_code = "LTB"
        ctx.milestones = {"M1": 1, "M2": 2}
        ctx.mcp = mcp
        return ctx

    def test_a_title_only_listing_fails_description_search(self):
        ctx = self.context(title_only=True)
        ctx.rec.begin(2, "Read")
        scenarios.test_read(ctx)
        absent_from_titles = [
            query for query in ctx.mcp.queries
            if all(query.casefold() not in issue["title"].casefold() for issue in ctx.mcp.issues)
        ]
        self.assertTrue(absent_from_titles)
        failed = [
            result for result in ctx.rec.tests[-1].results
            if result[1] == checks.FAIL and "description token" in result[2]
        ]
        self.assertTrue(failed)

    def test_title_and_description_tokens_match_only_their_issue(self):
        ctx = self.context(title_only=False)
        ctx.rec.begin(2, "Read")
        scenarios.test_read(ctx)
        results = {result[2]: result[1] for result in ctx.rec.tests[-1].results}
        self.assertEqual(results["a title token matches only the issue whose title holds it"], checks.PASS)
        self.assertEqual(
            results["a description token matches only the issue whose description holds it"],
            checks.PASS,
        )


class StateMoveTests(unittest.TestCase):
    workflow = (
        ("Draft", "backlog"), ("Parked", "backlog"), ("Blocked", "backlog"),
        ("Scheduled", "unstarted"), ("Ready", "unstarted"), ("Active", "started"),
        ("In Review", "started"), ("Approved", "started"), ("Landing", "started"),
        ("Done", "completed"), ("Cancelled", "canceled"), ("Superseded", "canceled"),
        ("Duplicate", "duplicate"),
    )

    def context(self):
        kinds = dict(self.workflow)

        class MemoryMcp:
            def __init__(self):
                self.issues = {}
                self.next_id = 1
                self.moves = []

            def call(self, name, arguments):
                if name == "list_issue_statuses":
                    return harness.ToolResult(True, {"states": [
                        {"name": name, "state_type": kind} for name, kind in kinds.items()
                    ]})
                if name == "save_issue":
                    return harness.ToolResult(True, self.save(arguments))
                if name == "get_issue":
                    return harness.ToolResult(True, self.issues[arguments["identifier"]])
                return harness.ToolResult(True, {})

            def save(self, arguments):
                if arguments.get("identifier"):
                    issue = self.issues[arguments["identifier"]]
                    if "state" in arguments and arguments["state"] != issue["state"]:
                        self.moves.append((issue["state"], arguments["state"]))
                        kind = kinds[arguments["state"]]
                        issue["state_history"].append({
                            "state": arguments["state"], "state_type": kind,
                            "started_at": "2026-11-05T00:00:02.000Z",
                        })
                        issue["state"] = arguments["state"]
                        issue["state_type"] = kind
                        issue["updated_at"] = "2026-11-05T00:00:02.000Z"
                        issue["version"] += 1
                    return {"identifier": arguments["identifier"]}
                number = self.next_id
                self.next_id += 1
                key = f"LIN-{number}"
                state = arguments.get("state", "Draft")
                kind = kinds.get(state, "backlog")
                self.issues[key] = {
                    "identifier": key, "title": arguments.get("title", ""),
                    "state": state, "state_type": kind, "version": 1,
                    "created_at": "2026-11-05T00:00:01.000Z",
                    "updated_at": "2026-11-05T00:00:01.000Z",
                    "state_history": [{
                        "state": state, "state_type": kind,
                        "started_at": "2026-11-05T00:00:01.000Z",
                    }],
                }
                return {"identifier": key}

        mcp = MemoryMcp()
        recorder = checks.Recorder(emit=lambda line: None)
        ctx = context.Context(mcp, None, recorder, "run", read_only=False, pause=0)
        ctx.projects = {"LIN": 1}
        ctx.project_code = "LIN"
        ctx.mcp = mcp
        return ctx

    def test_every_workflow_state_moves_to_every_other(self):
        ctx = self.context()
        ctx.rec.begin(4, "State moves")
        scenarios.test_state_moves(ctx)
        names = [name for name, _kind in self.workflow]
        expected = {(source, dest) for source in names for dest in names if source != dest}
        self.assertTrue(expected <= set(ctx.mcp.moves))
        descriptions = [result[2] for result in ctx.rec.tests[-1].results]
        for source, dest in expected:
            related = [line for line in descriptions if line.startswith(f"{source} -> {dest}:")]
            self.assertTrue(any("lands" in line for line in related), source + " -> " + dest)
            self.assertTrue(any("history" in line for line in related))
            self.assertTrue(any("stamp" in line for line in related))



if __name__ == "__main__":
    unittest.main()
class SubIssueClosureTests(unittest.TestCase):
    def context(self):
        class MemoryMcp:
            def __init__(self):
                self.issues = {}
                self.next_id = 1
                self.states = []
                self.auto_close = False

            def call(self, name, arguments):
                if name == "save_issue":
                    return harness.ToolResult(True, self.save(arguments))
                if name == "get_issue":
                    return harness.ToolResult(True, self.issue(arguments["identifier"]))
                if name == "update_project_settings":
                    if "auto_close_sub_issues" in arguments:
                        self.auto_close = arguments["auto_close_sub_issues"]
                    return harness.ToolResult(True, {"auto_close_sub_issues": self.auto_close})
                if name == "list_issue_statuses":
                    return harness.ToolResult(True, {"states": [
                        {"name": "Draft", "state_type": "backlog"},
                        {"name": "Active", "state_type": "started"},
                        {"name": "Done", "state_type": "completed"},
                        {"name": "Cancelled", "state_type": "canceled"},
                        {"name": "Superseded", "state_type": "canceled"},
                        {"name": "Duplicate", "state_type": "duplicate"},
                    ]})
                if name == "list_issues":
                    return harness.ToolResult(True, {"issues": []})
                return harness.ToolResult(True, {})

            def save(self, arguments):
                if arguments.get("identifier"):
                    issue = self.issues[arguments["identifier"]]
                    if "state" in arguments:
                        issue["state"] = arguments["state"]
                        self.states.append(arguments["state"])
                    return {"identifier": arguments["identifier"]}
                number = self.next_id
                self.next_id += 1
                key = f"LIN-{number}"
                state = arguments.get("state", "Draft")
                self.issues[key] = {
                    "title": arguments.get("title", ""),
                    "state": state,
                    "project_id": arguments.get("project_id", 1),
                    "priority": arguments.get("priority", 0),
                    "labels": list(arguments.get("add_labels") or []),
                    "parent": arguments.get("parent_id"),
                    "milestone_id": arguments.get("milestone_id"),
                }
                if "state" in arguments:
                    self.states.append(state)
                return {"identifier": key}

            def issue(self, key):
                stored = self.issues[key]
                record = {
                    "identifier": key,
                    "title": stored["title"],
                    "project_id": stored["project_id"],
                    "state": stored["state"],
                    "state_type": "backlog",
                    "priority": stored["priority"],
                    "labels": stored["labels"],
                    "version": 1,
                    "created_at": "2026-11-05T00:00:01.000Z",
                    "updated_at": "2026-11-05T00:00:01.000Z",
                    "state_history": [{
                        "state": stored["state"], "state_type": "backlog",
                        "started_at": "2026-11-05T00:00:01.000Z",
                    }],
                    "children": [],
                }
                if stored.get("parent"):
                    record["parent"] = stored["parent"]
                if stored.get("milestone_id") is not None:
                    record["milestone_id"] = stored["milestone_id"]
                return record

        mcp = MemoryMcp()
        recorder = checks.Recorder(emit=lambda line: None)
        ctx = context.Context(mcp, None, recorder, "run", read_only=False, pause=0)
        ctx.projects = {"LIN": 1, "LTB": 2}
        ctx.project_code = "LIN"
        ctx.milestones = {"M1": 1, "M2": 2}
        ctx.mcp = mcp
        return ctx

    def test_closed_children_and_terminal_parents_keep_their_clock(self):
        ctx = self.context()
        ctx.rec.begin(11, "Sub-issues")
        scenarios.test_sub_issues(ctx)
        self.assertTrue({"Cancelled", "Superseded", "Duplicate"} <= set(ctx.mcp.states))
        descriptions = [result[2] for result in ctx.rec.tests[-1].results]
        for state in ("Done", "Cancelled", "Superseded", "Duplicate"):
            matched = [
                line for line in descriptions
                if state in line and "version" in line and "timestamp" in line
            ]
            self.assertTrue(matched, state)


