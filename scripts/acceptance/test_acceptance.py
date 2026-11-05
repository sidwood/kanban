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


if __name__ == "__main__":
    unittest.main()
