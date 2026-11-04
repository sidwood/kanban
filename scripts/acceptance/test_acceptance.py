"""Unit tests for the acceptance runner itself; no core is needed.

    python3 -m unittest scripts.acceptance.test_acceptance -v
"""

import base64
import json
import unittest

from scripts.acceptance import checks, context, harness, scenarios


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

    def test_xfail_and_xpass_never_fail_the_run(self):
        recorder, lines = self.recorder()
        recorder.begin(1, "Create")
        recorder.check("passes", True)
        recorder.xfail("still open", False, "gap")
        recorder.xfail("now closed", True)
        self.assertEqual(recorder.exit_code(), 0)
        self.assertEqual(recorder.tests[0].verdict, "pass")
        self.assertTrue(lines[-2].startswith("XFAIL 1.02 still open -- gap"))
        self.assertTrue(lines[-1].startswith("XPASS 1.03 now closed"))

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
