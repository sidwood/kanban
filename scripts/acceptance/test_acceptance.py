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

    def test_a_printable_secret_is_removed_before_it_can_be_cut(self):
        """Raw text of a printable secret must not survive shortening or a difference."""
        redact = harness.Redactor()
        secret = b"0123456789" + bytes(range(ord("a"), ord("v") + 1))
        other = b"Z" * 32
        text = secret.decode("ascii")
        self.assertEqual(len(secret), 32)
        self.assertTrue(text.isprintable())
        redact.add(secret)
        redact.add(other)
        lines = []
        recorder = checks.Recorder(emit=lines.append, redact=redact)
        recorder.begin(1, text)
        matched = recorder.equal("raw bytes", secret, secret)
        differed = recorder.equal("raw bytes differ", secret, other)
        recorder.equal("text boundary", "x" * 140 + text, "x" * 140 + text)
        recorder.equal("text difference", {"token": "x" * 130 + text}, {"token": "other"})
        recorder.abort("a" * 280 + text)
        recorder.abort(secret)
        rendered = "\n".join(lines + recorder.summary_lines())
        self.assertTrue(matched)
        self.assertFalse(differed)
        self.assertNotIn(text, rendered)
        self.assertNotIn(text[:12], rendered)
        self.assertNotIn(text[-8:], rendered)
        self.assertNotIn(other.decode("ascii"), rendered)

    def test_a_binary_secret_bytes_rendering_is_removed_before_it_can_be_cut(self):
        """The Python rendering of arbitrary secret bytes must not survive shortening."""
        redact = harness.Redactor()
        secret = bytes([255, 254]) + bytes(range(30))
        other = b"\x11" * 32
        rendering = repr(secret)
        self.assertEqual(len(secret), 32)
        with self.assertRaises(UnicodeDecodeError):
            secret.decode("utf-8")
        redact.add(secret)
        redact.add(other)
        lines = []
        recorder = checks.Recorder(emit=lines.append, redact=redact)
        recorder.begin(1, rendering)
        matched = recorder.equal("raw bytes", secret, secret)
        differed = recorder.equal("raw bytes differ", secret, other)
        recorder.equal("repr boundary", "m" * 140 + rendering, "m" * 140 + rendering)
        recorder.equal("repr difference", {"token": secret}, {"token": other})
        recorder.abort("a" * 280 + rendering)
        recorder.abort(secret)
        rendered = "\n".join(lines + recorder.summary_lines())
        self.assertTrue(matched)
        self.assertFalse(differed)
        self.assertNotIn(rendering, rendered)
        self.assertNotIn(rendering[:16], rendered)
        self.assertNotIn(rendering[-12:], rendered)
        self.assertNotIn(repr(other), rendered)

    def test_an_embedded_secret_leaves_no_fragment_of_its_bytes_rendering(self):
        """A secret spliced into longer bytes must not survive shortening."""
        binary = bytes([255, 254]) + bytes(range(30))
        quoted = b"0123456789'" + bytes(range(ord("a"), ord("a") + 21))
        binary_embedded = b"x" * 140 + binary
        quoted_embedded = b'"' * 140 + quoted
        self.assertEqual(len(quoted), 32)
        self.assertIn("\\xff\\xfe", repr(binary_embedded)[:157])
        self.assertIn("\\'", repr(quoted_embedded)[:157])
        redact = harness.Redactor()
        redact.add(binary)
        redact.add(quoted)
        lines = []
        recorder = checks.Recorder(emit=lines.append, redact=redact)
        recorder.begin(1, "Secrets")
        recorder.equal("binary boundary", binary_embedded, binary_embedded)
        recorder.equal("binary difference", binary_embedded, b"other")
        recorder.abort(binary_embedded)
        matched = recorder.equal("quoted boundary", quoted_embedded, quoted_embedded)
        differed = recorder.equal("quoted difference", quoted_embedded, b"other")
        recorder.abort(quoted_embedded)
        rendered = "\n".join(lines + recorder.summary_lines())
        self.assertTrue(matched)
        self.assertFalse(differed)
        self.assertNotIn("\\xff\\xfe", rendered)
        self.assertNotIn("\\'", rendered)
        self.assertNotIn(quoted.decode("ascii"), rendered)

    def assert_decoded_secret_is_hidden(self, secret):
        """The exact recorder paths must hide a complete decoded secret."""
        self.assertEqual(len(secret), 32)
        text = secret.decode("utf-8")
        redact = harness.Redactor()
        redact.add(secret)
        self.assertNotIn(text, redact(text))
        bearer = harness.derive_bearer(secret)
        self.assertNotIn(bearer, redact(bearer))
        lines = []
        recorder = checks.Recorder(emit=lines.append, redact=redact)
        recorder.begin(1, "Secrets")
        matched = recorder.equal("boundary", "x" * 140 + text, "x" * 140 + text)
        recorder.abort("a" * 280 + text)
        differed = recorder.equal("nested", {"value": "x" * 128 + text}, {"value": "other"})
        rendered = "\n".join(lines + recorder.summary_lines())
        self.assertTrue(matched)
        self.assertFalse(differed)
        self.assertNotIn(text[:12], rendered)
        self.assertNotIn(bearer, rendered)

    def test_a_decoded_secret_containing_a_newline_is_removed_before_clipping(self):
        secret = b"0123456789" + b"abcde" + b"\n" + bytes(range(ord("g"), ord("v") + 1))
        self.assertEqual(secret.decode("utf-8")[15], "\n")
        self.assert_decoded_secret_is_hidden(secret)

    def test_a_unicode_secret_containing_a_backslash_is_removed_before_repr(self):
        self.assert_decoded_secret_is_hidden(("abcdefghijklmno\\" + "é" * 8).encode())

    def assert_escaped_renderings_hide(self, secret, prefix):
        """An already escaped rendering must not keep a secret prefix."""
        self.assertEqual(len(secret), 32)
        self.assertGreaterEqual(len(prefix), 8)
        text = secret.decode("utf-8")
        self.assertTrue(text.startswith(prefix))
        other = b"Z" * 32
        self.assertNotEqual(secret, other)
        redact = harness.Redactor()
        redact.add(secret)
        redact.add(other)
        message = "x" * 240 + text
        described = harness.ToolResult(
            False, {"code": "internal", "message": message}
        ).describe()
        renderings = {
            "set": repr({"x" * 140 + text}),
            "frozenset": repr(frozenset({"x" * 130 + text})),
            "nested": repr({"value": {"x" * 120 + text}}),
            "list": repr(["x" * 140 + text]),
            "tuple": repr(("x" * 140 + text,)),
            "dict": repr({"value": "x" * 140 + text}),
            "tool_error": described,
        }
        self.assertTrue(described.startswith("internal "))
        self.assertIn(repr(message), described)
        for name, rendering in renderings.items():
            self.assertIn(prefix, rendering, name)
            self.assertNotIn(prefix, redact(rendering), name)
        lines = []
        recorder = checks.Recorder(emit=lines.append, redact=redact)
        recorder.begin(1, "Secrets")
        matched = recorder.equal("raw secret", secret, secret)
        differed = recorder.equal("raw secret differs", secret, other)
        recorder.equal("set", {"x" * 140 + text}, {"x" * 140 + text})
        recorder.equal(
            "frozenset", frozenset({"x" * 130 + text}), frozenset({"x" * 130 + text})
        )
        recorder.equal(
            "nested set", {"value": {"x" * 120 + text}}, {"value": "other"}
        )
        recorder.equal("list", ["x" * 140 + text], ["x" * 140 + text])
        recorder.abort("save_issue refused: " + described)
        output = "\n".join(lines + recorder.summary_lines())
        self.assertTrue(matched)
        self.assertFalse(differed)
        self.assertNotIn(prefix, output)
        self.assertIn("[redacted]", output)
        self.assertNotIn(other.decode("ascii"), output)
        self.assertNotIn(harness.derive_bearer(secret), output)

    def test_an_escaped_installation_secret_is_hidden_in_set_and_tool_errors(self):
        secret = ("abcdefghijklmno\\" + "é" * 8).encode()
        self.assertEqual(secret.decode("utf-8")[15], "\\")
        self.assert_escaped_renderings_hide(secret, "abcdefghijklmno")

    def test_repr_escapes_of_an_installation_secret_are_hidden(self):
        newline = b"0123456789\n" + ("é" * 10).encode() + b"Z"
        self.assert_escaped_renderings_hide(newline, "0123456789")
        quoted = b"0123456789'" + ("é" * 10).encode() + b"Z"
        self.assert_escaped_renderings_hide(quoted, "0123456789")
        separator = b"0123456789" + "\u2028".encode() * 7 + b"Z"
        self.assert_escaped_renderings_hide(separator, "0123456789")
        text = quoted.decode("utf-8")
        redact = harness.Redactor()
        redact.add(quoted)
        forced = repr('note " ' + text)
        self.assertIn("\\'", forced)
        self.assertIn("é", forced)
        self.assertIn('"', repr({text}))
        self.assertNotIn("0123456789", redact(forced))
        self.assertNotIn("0123456789", redact(repr({text})))

    def test_an_already_rendered_secret_is_hidden_before_a_tool_error_is_escaped(self):
        """A refusal must remove a rendered secret before the message is escaped."""
        secret = ("abcdefghijklmno\\" + "é" * 8).encode()
        self.assertEqual(len(secret), 32)
        prefix = "abcdefgh"
        redact = harness.Redactor()
        redact.add(secret)
        renderings = (
            ("string-repr", repr(secret.decode())),
            ("bytes-repr", repr(secret)),
            ("decoded", secret.decode()),
        )
        for name, rendering in renderings:
            for padding in (0, 240, 250):
                message = "x" * padding + rendering
                self.assertNotIn(prefix, redact(message), (name, padding))
                lines = []
                recorder = checks.Recorder(emit=lines.append, redact=redact)
                recorder.begin(1, "Tool refusal")

                class Refusing:
                    def __init__(self, text):
                        self.text = text
                        self.result = None

                    def call(self, tool, arguments):
                        self.result = harness.ToolResult(
                            False, {"code": "internal", "message": self.text}
                        )
                        return self.result

                client = Refusing(message)
                ctx = context.Context(client, None, recorder, "redaction", False)
                with self.assertRaises(context.StepError) as raised:
                    ctx.must("save_issue", {})
                self.assertEqual(client.result.payload["message"], message)
                self.assertEqual(client.result.code, "internal")
                self.assertIsNone(client.result.field)
                self.assertIs(client.result.error, client.result.payload)
                self.assertIn(repr(message), client.result.describe())
                recorder.abort(raised.exception)
                output = "\n".join(lines + recorder.summary_lines())
                detail = (name, padding)
                self.assertNotIn(prefix, str(raised.exception), detail)
                self.assertIn("[redacted]", str(raised.exception), detail)
                self.assertNotIn(prefix, output, detail)
                self.assertIn("[redacted]", output, detail)

    def test_a_refused_save_hides_a_rendered_secret_before_escaping(self):
        secret = ("abcdefghijklmno\\" + "é" * 8).encode()
        prefix = "abcdefgh"
        message = "x" * 240 + repr(secret.decode())
        redact = harness.Redactor()
        redact.add(secret)
        lines = []
        recorder = checks.Recorder(emit=lines.append, redact=redact)
        recorder.begin(1, "Tool refusal")

        class Client:
            def __init__(self):
                self.result = None

            def call(self, tool, arguments):
                if tool == "get_issue":
                    return harness.ToolResult(True, {"identifier": "LIN-1", "version": 3})
                self.result = harness.ToolResult(
                    False, {"code": "internal", "field": "title", "message": message}
                )
                return self.result

        client = Client()
        ctx = context.Context(client, None, recorder, "redaction", False)
        with self.assertRaises(context.StepError) as raised:
            ctx.save_must("LIN-1", title="kept")
        self.assertEqual(client.result.payload["message"], message)
        self.assertEqual((client.result.code, client.result.field), ("internal", "title"))
        recorder.abort(raised.exception)
        output = "\n".join(lines + recorder.summary_lines())
        self.assertNotIn(prefix, str(raised.exception))
        self.assertNotIn(prefix, output)
        self.assertIn("[redacted]", str(raised.exception))

    def test_a_recorded_refusal_hides_a_rendered_secret_before_escaping(self):
        secret = ("abcdefghijklmno\\" + "é" * 8).encode()
        prefix = "abcdefgh"
        message = "x" * 250 + repr(secret)
        redact = harness.Redactor()
        redact.add(secret)
        lines = []

        class Remembering(checks.Recorder):
            def __init__(self):
                super().__init__(emit=lines.append, redact=redact)
                self.reasons = []

            def check(self, description, ok, reason=""):
                self.reasons.append(reason)
                return super().check(description, ok, reason)

        recorder = Remembering()
        recorder.begin(1, "Errors")
        result = harness.ToolResult(
            False, {"code": "internal", "field": "title", "message": message}
        )
        ctx = context.Context(None, None, recorder, "redaction", False)
        self.assertEqual(result.payload["message"], message)
        ctx.refused("a rendered secret stays out of the reason", result, "not_found", "title")
        ctx.refused("a matched refusal still hides the message", result, "internal", "title")
        output = "\n".join(lines + recorder.summary_lines())
        self.assertGreaterEqual(len(recorder.reasons), 2)
        for reason in recorder.reasons:
            self.assertNotIn(prefix, reason)
            self.assertIn("[redacted]", reason)
        self.assertNotIn(prefix, output)
        self.assertEqual(result.payload["message"], message)
        self.assertEqual(result.field, "title")

    def test_a_legacy_refusal_hides_a_rendered_secret_before_formatting(self):
        """A socket refusal must remove a rendered secret before the error is formatted."""
        secret = ("abcdefghijklmno\\" + "é" * 8).encode()
        self.assertEqual(len(secret), 32)
        prefix = "abcdefgh"
        redact = harness.Redactor()
        redact.add(secret)
        renderings = (
            ("decoded", secret.decode()),
            ("string-repr", repr(secret.decode())),
            ("bytes-repr", repr(secret)),
        )
        lines = []
        recorder = checks.Recorder(emit=lines.append, redact=redact)
        recorder.begin(1, "Legacy refusal")

        class Socket:
            def __init__(self):
                self.seen = None
                self.error = None
                self.result = {"id": 7, "title": "kept"}

            def command(self, operation, payload):
                self.seen = (operation, payload)
                if self.error is not None:
                    raise harness.CoreError(self.error)
                return self.result

        socket_client = Socket()
        ctx = context.Context(None, socket_client, recorder, "redaction", False, pause=0)
        self.assertIs(ctx.legacy("ticket.edit", {"ticket_id": 7}), socket_client.result)
        self.assertEqual(socket_client.seen, ("ticket.edit", {"ticket_id": 7}))
        plain = {
            "code": "invalid_request",
            "message": "it's a plain refusal",
            "field": "title",
        }
        socket_client.error = plain
        with self.assertRaises(context.StepError) as raised:
            ctx.legacy("ticket.edit", {})
        self.assertEqual(
            str(raised.exception),
            "ticket.edit was refused: "
            "{'code': 'invalid_request', 'message': \"it's a plain refusal\", 'field': 'title'}",
        )
        self.assertIs(socket_client.error, plain)

        for name, rendering in renderings:
            for padding in (0, 200, 240, 250):
                message = "x" * padding + rendering
                body = {"code": "internal", "message": message}
                lines = []
                recorder = checks.Recorder(emit=lines.append, redact=redact)
                recorder.begin(1, "Legacy refusal")
                socket_client = Socket()
                socket_client.error = body
                ctx = context.Context(None, socket_client, recorder, "redaction", False, pause=0)
                with self.assertRaises(context.StepError) as raised:
                    ctx.legacy("ticket.edit", {})
                self.assertIs(body["message"], message)
                self.assertEqual(body["code"], "internal")
                recorder.abort(raised.exception)
                output = "\n".join(lines + recorder.summary_lines())
                detail = (name, padding)
                self.assertNotIn(prefix, str(raised.exception), detail)
                self.assertIn("[redacted]", str(raised.exception), detail)
                self.assertNotIn(prefix, output, detail)
                if padding < 240:
                    self.assertIn("[redacted]", output, detail)

    def test_a_nested_message_hides_a_rendered_secret_before_formatting(self):
        """A dict or list message must lose a rendered secret before it is escaped."""
        secret = ("abcdefghijklmno\\" + "é" * 8).encode()
        self.assertEqual(len(secret), 32)
        prefix = "abcdefgh"
        redact = harness.Redactor()
        redact.add(secret)
        plain_message = {"note": "it's plain", "n": 1, "items": ["kept", None]}
        plain = harness.ToolResult(
            False, {"code": "internal", "field": "title", "message": plain_message}
        )
        self.assertEqual(plain.describe(redact), plain.describe())
        self.assertEqual(plain.describe(), "internal field=title " + repr(plain_message))
        self.assertIs(plain.payload["message"], plain_message)
        self.assertEqual(
            harness.ToolResult(False, {"code": "internal", "message": ""}).describe(redact),
            "internal",
        )
        self.assertEqual(
            harness.ToolResult(False, {"code": "internal", "message": 0}).describe(redact),
            "internal",
        )
        listed = ["it's plain", 2]
        listed_result = harness.ToolResult(False, {"code": "internal", "message": listed})
        self.assertEqual(listed_result.describe(redact), "internal " + repr(listed))
        self.assertIs(listed_result.payload["message"], listed)

        renderings = (
            ("decoded", secret.decode()),
            ("string-repr", repr(secret.decode())),
            ("bytes-repr", repr(secret)),
        )
        for name, rendering in renderings:
            shapes = (
                ("dict", {"note": rendering}),
                ("list", [rendering]),
                ("nested", {"items": [rendering, {"inner": rendering}]}),
            )
            for shape, message in shapes:
                result = harness.ToolResult(False, {"code": "internal", "message": message})
                self.assertIs(result.payload["message"], message)
                self.assertIn(prefix, result.describe(), (name, shape))
                described = result.describe(redact)
                detail = (name, shape)
                self.assertNotIn(prefix, described, detail)
                self.assertIn("[redacted]", described, detail)
                self.assertIs(result.payload["message"], message)
                lines = []
                recorder = checks.Recorder(emit=lines.append, redact=redact)
                recorder.begin(1, "Tool refusal")

                class Refusing:
                    def __init__(self):
                        self.result = result

                    def call(self, tool, arguments):
                        return self.result

                ctx = context.Context(Refusing(), None, recorder, "redaction", False)
                with self.assertRaises(context.StepError) as raised:
                    ctx.must("save_issue", {})
                self.assertNotIn(prefix, str(raised.exception), detail)
                self.assertIn("[redacted]", str(raised.exception), detail)
                recorder.abort(raised.exception)
                output = "\n".join(lines + recorder.summary_lines())
                self.assertNotIn(prefix, output, detail)
                self.assertIn("[redacted]", output, detail)

                class Socket:
                    def command(self, operation, payload):
                        raise harness.CoreError({"code": "internal", "message": message})

                lines = []
                recorder = checks.Recorder(emit=lines.append, redact=redact)
                recorder.begin(1, "Legacy refusal")
                ctx = context.Context(None, Socket(), recorder, "redaction", False, pause=0)
                with self.assertRaises(context.StepError) as raised:
                    ctx.legacy("ticket.edit", {})
                recorder.abort(raised.exception)
                output = "\n".join(lines + recorder.summary_lines())
                self.assertNotIn(prefix, str(raised.exception), detail)
                self.assertIn("[redacted]", str(raised.exception), detail)
                self.assertNotIn(prefix, output, detail)

        byte_message = {"note": secret}
        byte_result = harness.ToolResult(False, {"code": "internal", "message": byte_message})
        self.assertIs(byte_result.payload["message"], byte_message)
        self.assertNotIn(prefix, byte_result.describe(redact))
        self.assertIn(prefix, byte_result.describe())
        field_message = repr(secret.decode())
        field_result = harness.ToolResult(
            False, {"code": {"note": field_message}, "field": [field_message], "message": "kept"}
        )
        self.assertIs(field_result.payload["field"], field_result.error["field"])
        self.assertNotIn(prefix, field_result.describe(redact))
        self.assertEqual(field_result.payload["message"], "kept")

    def test_a_register_refusal_hides_a_rendered_secret_before_formatting(self):
        """project.register must remove a rendered secret before it formats the refusal."""
        secret = ("abcdefghijklmno\\" + "é" * 8).encode()
        prefix = "abcdefgh"
        redact = harness.Redactor()
        redact.add(secret)
        messages = (
            repr(secret.decode()),
            {"note": repr(secret.decode())},
            [repr(secret)],
        )
        for message in messages:
            lines = []
            recorder = checks.Recorder(emit=lines.append, redact=redact)
            recorder.begin(0, "Setup")

            class Socket:
                def command(self, operation, payload):
                    raise harness.CoreError({"code": "internal", "message": message})

            ctx = context.Context(None, Socket(), recorder, "redaction", False, pause=0)
            with self.assertRaises(context.StepError) as raised:
                scenarios.register_project(ctx, "LIN", "Linear parity", "repository")
            self.assertNotIn(prefix, str(raised.exception))
            self.assertIn("[redacted]", str(raised.exception))
            recorder.abort(raised.exception)
            output = "\n".join(lines + recorder.summary_lines())
            self.assertNotIn(prefix, output)
            self.assertIn("[redacted]", output)

    def test_a_stop_refusal_hides_a_rendered_secret_before_formatting(self):
        """service.stop must remove a rendered secret before it formats the refusal."""
        import socket
        import threading

        secret = ("abcdefghijklmno\\" + "é" * 8).encode()
        prefix = "abcdefgh"
        redact = harness.Redactor()
        redact.add(secret)

        def reason_for(message):
            with tempfile.TemporaryDirectory(prefix="ka-stop-", dir="/tmp") as root:
                data_dir = Path(root) / "Kanban"
                data_dir.mkdir()
                path = data_dir / "core.sock"
                replies = [
                    {
                        "kind": "response",
                        "payload": {"version": 1, "instance_id": "instance", "warning_id": "warning"},
                    },
                    {"kind": "error", "error": {"code": "internal", "message": message}},
                ]
                server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                server.bind(str(path))
                server.listen(2)
                server.settimeout(5)

                def serve():
                    try:
                        for reply in replies:
                            connection, _address = server.accept()
                            with connection:
                                buffer = b""
                                while b"\n" not in buffer:
                                    chunk = connection.recv(65536)
                                    if not chunk:
                                        break
                                    buffer += chunk
                                connection.sendall((json.dumps(reply) + "\n").encode())
                    except OSError:
                        return

                thread = threading.Thread(target=serve)
                thread.start()
                try:
                    core = harness.CoreProcess(
                        Path(root) / "unused", data_dir, 9, Path(root) / "core.log"
                    )
                    return core.stop(redact=redact)
                finally:
                    server.close()
                    thread.join(timeout=5)

        plain = reason_for("it's a plain refusal")
        self.assertEqual(plain, "service.stop failed: it's a plain refusal")
        leaked = reason_for({"note": repr(secret.decode())})
        self.assertNotIn(prefix, leaked)
        self.assertIn("[redacted]", leaked)
        self.assertNotIn(prefix, redact(f"core stopped: {leaked}"))

    def test_a_plain_tool_error_keeps_its_refusal_text(self):
        message = "it's a plain refusal"
        lines = []
        recorder = checks.Recorder(emit=lines.append)
        recorder.begin(1, "Tool refusal")

        class Refusing:
            def __init__(self):
                self.result = None

            def call(self, tool, arguments):
                self.result = harness.ToolResult(
                    False, {"code": "invalid_request", "field": "title", "message": message}
                )
                return self.result

        client = Refusing()
        ctx = context.Context(client, None, recorder, "redaction", False)
        with self.assertRaises(context.StepError) as raised:
            ctx.must("save_issue", {})
        expected = "invalid_request field=title " + repr(message)
        self.assertEqual(str(raised.exception), f"save_issue was refused: {expected}")
        self.assertEqual(client.result.describe(), expected)
        self.assertEqual(client.result.payload["message"], message)
        self.assertTrue(recorder.equal("raw message", message, message))
        self.assertFalse(recorder.equal("redaction token", message, "[redacted]"))
        result = harness.ToolResult(False, {"code": "internal", "message": ""})
        self.assertEqual(result.describe(), "internal")
        self.assertEqual(harness.ToolResult(True, {"identifier": "LIN-1"}).describe(), "accepted")

    def test_a_matching_refusal_keeps_the_expected_signature_text(self):
        result = harness.ToolResult(
            False, {"code": "invalid_request", "field": "state", "message": "unknown state"}
        )
        lines = []
        recorder = checks.Recorder(emit=lines.append)
        recorder.begin(1, "Errors")
        ctx = context.Context(None, None, recorder, "run", False)
        ctx.refused("unknown state", result, "invalid_request", "state")
        self.assertTrue(lines[-1].startswith("PASS "))
        self.assertIn("invalid_request field=state 'unknown state'", lines[-1])
        self.assertEqual(result.payload["message"], "unknown state")

    def test_scenario_and_context_render_tool_errors_through_the_recorder(self):
        for module in (scenarios, context):
            source = Path(module.__file__).read_text()
            self.assertNotIn(".describe()", source, module.__name__)
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

            def stop(self, redact=None):
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

    def test_a_snapshot_keeps_private_credential_permissions(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "source"
            private = source / "installation-credential"
            private.mkdir(parents=True, mode=0o700)
            credential = private / "installation"
            credential.write_bytes(b"0123456789" + b"abcdefghijklmnopqrstuv")
            credential.chmod(0o600)
            (source / "kanban.sqlite").write_bytes(b"database")
            before = (
                credential.read_bytes(),
                credential.stat().st_mode & 0o777,
                private.stat().st_mode & 0o777,
                (source / "kanban.sqlite").read_bytes(),
            )
            destination = Path(tmp) / "copy"
            harness.copy_store(source, destination)
            copied_dir = destination / "installation-credential"
            copied = copied_dir / "installation"
            self.assertEqual(copied_dir.stat().st_mode & 0o777, 0o700)
            self.assertEqual(copied.stat().st_mode & 0o777, 0o600)
            self.assertEqual(copied.read_bytes(), credential.read_bytes())
            self.assertEqual((destination / "kanban.sqlite").read_bytes(), b"database")
            self.assertEqual(
                (
                    credential.read_bytes(),
                    credential.stat().st_mode & 0o777,
                    private.stat().st_mode & 0o777,
                    (source / "kanban.sqlite").read_bytes(),
                ),
                before,
            )



class LinkedStoreFingerprintTests(unittest.TestCase):
    """A store symlink is identified by the file it resolves to."""

    SIDECARS = ("-wal", "-shm", "-journal")

    def digest(self, data):
        return hashlib.sha256(data).hexdigest()

    def unlinked_walk(self, root):
        """The fingerprint of a store whose entries are not links."""
        root = Path(root)
        found = {}
        if not root.exists():
            return found
        for path in sorted(root.rglob("*")):
            relative = path.relative_to(root).as_posix()
            try:
                if path.is_symlink():
                    found[relative] = ("symlink", os.readlink(path))
                elif path.is_dir():
                    found[relative] = ("dir",)
                elif path.is_file():
                    found[relative] = ("file", self.digest(path.read_bytes()))
                else:
                    found[relative] = ("other", path.stat().st_mode)
            except OSError as error:
                found[relative] = ("unreadable", type(error).__name__)
        return found

    def test_a_changed_linked_database_and_its_write_ahead_log_are_named(self):
        import sqlite3
        from contextlib import redirect_stdout
        from scripts.acceptance import __main__ as cli

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inspected = root / "inspected"
            data = inspected / "Kanban"
            data.mkdir(parents=True)
            target = root / "linked.sqlite"
            with sqlite3.connect(target) as database:
                database.execute("create table attention_items (value integer)")
                database.execute("insert into attention_items values (1)")
            wal = Path(str(target) + "-wal")
            wal.write_bytes(b"wal-before")
            (data / "kanban.sqlite").symlink_to(target)

            def mutate():
                with sqlite3.connect(target) as database:
                    database.execute("update attention_items set value = 2")
                target.write_bytes(target.read_bytes() + b"changed")
                wal.write_bytes(b"wal-after")

            def selected(ctx):
                mutate()
                ctx.rec.check("controlled read scenario completes", True)

            class Projects:
                def call(self, name, arguments):
                    return harness.ToolResult(True, {"projects": [{"code": "LIN", "id": 1}]})

            captured = StringIO()
            with redirect_stdout(captured), \
                    patch.object(cli, "socket_answers", return_value=True), \
                    patch.object(cli, "port_in_use", return_value=True), \
                    patch.object(cli, "CoreProcess", side_effect=AssertionError("must not start a core")), \
                    patch.object(cli, "connect", return_value=(Projects(), "synthetic")), \
                    patch.object(cli.scenarios, "TESTS", [(9, "Lookups", selected, True)]), \
                    patch("scripts.acceptance.harness.keychain_secret",
                          side_effect=AssertionError("keychain")):
                status = cli.main([
                    "--read-only", "--project", "LIN", "--data-dir", str(inspected),
                    "--binary", "/nonexistent", "--tests", "9",
                ])
            guard = [
                line for line in captured.getvalue().splitlines()
                if line.startswith(("PASS", "FAIL")) and "inspected store" in line
            ]

            self.assertEqual(len(guard), 1, captured.getvalue())
            self.assertNotEqual(status, 0, captured.getvalue())
            self.assertTrue(guard[0].startswith("FAIL"), guard[0])
            names = [part.strip() for part in guard[0].split("changed: ", 1)[1].split(",")]
            self.assertIn("Kanban/kanban.sqlite", names)
            self.assertIn("Kanban/kanban.sqlite-wal", names)

    def test_linked_database_sidecars_record_present_absent_and_changed_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            store = root / "store"
            outside = root / "outside"
            store.mkdir()
            outside.mkdir()
            target = outside / "kanban.sqlite"
            target.write_bytes(b"database-v1")
            link = store / "kanban.sqlite"
            link.symlink_to(target)
            absent = harness.store_fingerprint(store)
            self.assertEqual(absent["kanban.sqlite"][1], os.readlink(link))
            self.assertEqual(absent["kanban.sqlite"][2], "file")
            self.assertEqual(absent["kanban.sqlite"][3], self.digest(b"database-v1"))
            for suffix in self.SIDECARS:
                self.assertEqual(absent["kanban.sqlite" + suffix], ("absent",))
            self.assertEqual(
                absent["kanban.sqlite"][4],
                tuple((suffix[1:], absent["kanban.sqlite" + suffix]) for suffix in self.SIDECARS),
            )

            for suffix in self.SIDECARS:
                Path(str(target) + suffix).write_bytes(b"present" + suffix.encode())
            present = harness.store_fingerprint(store)
            for suffix in self.SIDECARS:
                body = b"present" + suffix.encode()
                self.assertEqual(present["kanban.sqlite" + suffix], ("file", self.digest(body)))
            appeared = harness.store_changes(absent, present)
            self.assertEqual(appeared, sorted(appeared))
            self.assertEqual(
                appeared,
                ["kanban.sqlite", "kanban.sqlite-journal", "kanban.sqlite-shm", "kanban.sqlite-wal"],
            )

            target.write_bytes(b"database-v2")
            self.assertEqual(
                harness.store_changes(present, harness.store_fingerprint(store)),
                ["kanban.sqlite"],
            )
            present = harness.store_fingerprint(store)
            Path(str(target) + "-wal").write_bytes(b"wal-v2")
            Path(str(target) + "-shm").unlink()
            changed = harness.store_changes(present, harness.store_fingerprint(store))
            self.assertEqual(changed, ["kanban.sqlite", "kanban.sqlite-shm", "kanban.sqlite-wal"])
            after = harness.store_fingerprint(store)
            self.assertEqual(after["kanban.sqlite-wal"], ("file", self.digest(b"wal-v2")))
            self.assertEqual(after["kanban.sqlite-shm"], ("absent",))
            self.assertEqual(after["kanban.sqlite-journal"], present["kanban.sqlite-journal"])

    def test_a_sqlite_header_link_records_sidecars_beside_its_target(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            store = root / "store"
            outside = root / "outside"
            store.mkdir()
            outside.mkdir()
            target = outside / "payload.bin"
            target.write_bytes(b"SQLite format 3\x00" + b"page")
            Path(str(target) + "-wal").write_bytes(b"header-wal")
            (store / "payload.bin").symlink_to(target)
            fingerprint = harness.store_fingerprint(store)
            self.assertEqual(fingerprint["payload.bin-wal"], ("file", self.digest(b"header-wal")))
            self.assertEqual(fingerprint["payload.bin-shm"], ("absent",))
            self.assertEqual(fingerprint["payload.bin-journal"], ("absent",))

    def test_a_linked_text_file_keeps_its_raw_target_and_content(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            store = root / "store"
            outside = root / "outside"
            store.mkdir()
            outside.mkdir()
            target = outside / "notes.txt"
            target.write_bytes(b"hello")
            Path(str(target) + "-wal").write_bytes(b"ignore-me")
            link = store / "notes.txt"
            link.symlink_to("../outside/../outside/notes.txt")
            fingerprint = harness.store_fingerprint(store)
            self.assertEqual(os.readlink(link), "../outside/../outside/notes.txt")
            self.assertEqual(fingerprint["notes.txt"][1], os.readlink(link))
            self.assertEqual(
                fingerprint["notes.txt"],
                ("symlink", "../outside/../outside/notes.txt", "file", self.digest(b"hello")),
            )
            self.assertNotIn("notes.txt-wal", fingerprint)
            before = fingerprint
            Path(str(target) + "-wal").write_bytes(b"still-ignored")
            self.assertEqual(harness.store_changes(before, harness.store_fingerprint(store)), [])
            target.write_bytes(b"changed")
            self.assertEqual(
                harness.store_changes(before, harness.store_fingerprint(store)),
                ["notes.txt"],
            )

    def test_an_in_store_link_records_the_target_content(self):
        with tempfile.TemporaryDirectory() as tmp:
            store = Path(tmp)
            (store / "attention.bin").write_bytes(b"initial")
            (store / "link").symlink_to("attention.bin")
            before = harness.store_fingerprint(store)
            self.assertEqual(before["link"][1], "attention.bin")
            self.assertEqual(before["link"][3], self.digest(b"initial"))
            (store / "attention.bin").write_bytes(b"changed")
            self.assertEqual(
                harness.store_changes(before, harness.store_fingerprint(store)),
                ["attention.bin", "link"],
            )

    def test_a_dangling_link_is_recorded_as_dangling(self):
        with tempfile.TemporaryDirectory() as tmp:
            store = Path(tmp)
            link = store / "missing"
            link.symlink_to("missing-target")
            fingerprint = harness.store_fingerprint(store)
            self.assertEqual(fingerprint["missing"], ("symlink", "missing-target", "dangling"))
            self.assertFalse(any(key.startswith("missing-") for key in fingerprint))
            self.assertEqual(harness.store_changes(fingerprint, harness.store_fingerprint(store)), [])

    def test_a_looping_link_stops_within_a_fixed_hop_bound(self):
        with tempfile.TemporaryDirectory() as tmp:
            store = Path(tmp)
            (store / "self").symlink_to("self")
            (store / "a").symlink_to("b")
            (store / "b").symlink_to("a")
            short = store / "short"
            long = store / "long"
            short.mkdir()
            long.mkdir()
            self.chain(short, 8, b"short-chain")
            self.chain(long, 41, b"too-far")
            fingerprint = harness.store_fingerprint(store)
            self.assertEqual(fingerprint["self"], ("symlink", "self", "loop"))
            self.assertEqual(fingerprint["a"], ("symlink", "b", "loop"))
            self.assertEqual(fingerprint["b"], ("symlink", "a", "loop"))
            self.assertEqual(fingerprint["short/hop-7"][2], "file")
            self.assertEqual(fingerprint["short/hop-7"][3], self.digest(b"short-chain"))
            self.assertEqual(fingerprint["long/hop-40"][2], "loop")
            self.assertNotIn(self.digest(b"too-far"), fingerprint["long/hop-40"])

    def chain(self, directory, hops, content):
        final = directory / "final.txt"
        final.write_bytes(content)
        current = final
        for index in range(hops):
            link = directory / f"hop-{index}"
            link.symlink_to(current.name)
            current = link
        return current

    def test_retargeting_a_link_records_the_new_text_and_content(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            store = root / "store"
            outside = root / "outside"
            store.mkdir()
            outside.mkdir()
            (outside / "first.txt").write_bytes(b"one")
            (outside / "second.txt").write_bytes(b"two")
            link = store / "current"
            link.symlink_to("../outside/first.txt")
            before = harness.store_fingerprint(store)
            link.unlink()
            link.symlink_to("../outside/second.txt")
            after = harness.store_fingerprint(store)
            self.assertEqual(before["current"][1], "../outside/first.txt")
            self.assertEqual(
                after["current"],
                ("symlink", "../outside/second.txt", "file", self.digest(b"two")),
            )
            self.assertEqual(harness.store_changes(before, after), ["current"])

    def test_a_link_to_a_directory_is_not_walked(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            store = root / "store"
            external = root / "external"
            store.mkdir()
            external.mkdir()
            (external / "secret.bin").write_bytes(b"secret")
            link = store / "tree"
            link.symlink_to(external)
            before = harness.store_fingerprint(store)
            self.assertEqual(before["tree"], ("symlink", os.readlink(link), "dir"))
            self.assertNotIn("tree/secret.bin", before)
            (external / "secret.bin").write_bytes(b"changed")
            self.assertEqual(harness.store_changes(before, harness.store_fingerprint(store)), [])

    def test_an_in_store_sidecar_keeps_its_own_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            store = root / "store"
            outside = root / "outside"
            store.mkdir()
            outside.mkdir()
            target = outside / "db.sqlite"
            target.write_bytes(b"database")
            Path(str(target) + "-wal").write_bytes(b"external-wal")
            (store / "kanban.sqlite-wal").write_bytes(b"local-wal")
            (store / "kanban.sqlite").symlink_to(target)
            before = harness.store_fingerprint(store)
            self.assertEqual(before["kanban.sqlite-wal"], ("file", self.digest(b"local-wal")))
            self.assertEqual(
                before["kanban.sqlite"][4][0],
                ("wal", ("file", self.digest(b"external-wal"))),
            )
            Path(str(target) + "-wal").write_bytes(b"external-wal-2")
            changed = harness.store_changes(before, harness.store_fingerprint(store))
            self.assertIn("kanban.sqlite", changed)
            self.assertNotIn("kanban.sqlite-wal", changed)
            self.assertEqual(
                harness.store_fingerprint(store)["kanban.sqlite-wal"],
                ("file", self.digest(b"local-wal")),
            )

    def test_an_unlinked_store_fingerprint_matches_the_file_walk(self):
        with tempfile.TemporaryDirectory() as tmp:
            store = Path(tmp)
            nested = store / "nested"
            nested.mkdir()
            (store / "kanban.sqlite").write_bytes(b"database")
            (store / "kanban.sqlite-wal").write_bytes(b"wal")
            (store / "kanban.sqlite-shm").write_bytes(b"shm")
            (store / "kanban.sqlite-journal").write_bytes(b"journal")
            (nested / "note.txt").write_bytes(b"note")
            (store / "empty").write_bytes(b"")
            fingerprint = harness.store_fingerprint(store)
            self.assertEqual(fingerprint, self.unlinked_walk(store))
            for relative in fingerprint:
                self.assertTrue((store / relative).exists(), relative)
            before = fingerprint
            (store / "kanban.sqlite-wal").write_bytes(b"wal-changed")
            self.assertEqual(
                harness.store_changes(before, harness.store_fingerprint(store)),
                ["kanban.sqlite-wal"],
            )
            (nested / "note.txt").write_bytes(b"changed")
            self.assertEqual(
                harness.store_changes(before, harness.store_fingerprint(store)),
                ["kanban.sqlite-wal", "nested/note.txt"],
            )

    def test_hard_linked_relative_symlinks_resolve_from_their_own_directories(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            store = root / "store"
            outside = root / "outside"
            nested = outside / "nested"
            store.mkdir()
            nested.mkdir(parents=True)
            target = root / "database"
            target.write_bytes(b"SQLite format 3\x00" + b"before")
            first = nested / "database"
            second = outside / "database"
            first.symlink_to("../database")
            os.link(first, second, follow_symlinks=False)
            self.assertEqual(first.lstat().st_ino, second.lstat().st_ino)
            link = store / "kanban.sqlite"
            link.symlink_to(first)
            before = harness.store_fingerprint(store)
            self.assertEqual(before["kanban.sqlite"][1], os.readlink(link))
            self.assertEqual(before["kanban.sqlite"][2], "file")
            self.assertEqual(before["kanban.sqlite"][3], self.digest(target.read_bytes()))
            self.assertEqual(before["kanban.sqlite-wal"], ("absent",))
            self.assertEqual(before["kanban.sqlite-shm"], ("absent",))
            self.assertEqual(before["kanban.sqlite-journal"], ("absent",))
            target.write_bytes(b"SQLite format 3\x00" + b"after!")
            Path(str(target) + "-wal").write_bytes(b"wal bytes")
            after = harness.store_fingerprint(store)
            changed = harness.store_changes(before, after)
            self.assertEqual(changed, sorted(changed))
            self.assertIn("kanban.sqlite", changed)
            self.assertIn("kanban.sqlite-wal", changed)
            self.assertEqual(after["kanban.sqlite"][2], "file")
            self.assertEqual(after["kanban.sqlite"][3], self.digest(target.read_bytes()))
            self.assertEqual(after["kanban.sqlite-wal"], ("file", self.digest(b"wal bytes")))
            self.assertEqual(link.read_bytes(), target.read_bytes())

    def test_a_loop_in_a_target_parent_is_not_a_dangling_link(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            store = root / "store"
            outside = root / "outside"
            store.mkdir()
            outside.mkdir()
            cycle = outside / "cycle"
            cycle.symlink_to("cycle", target_is_directory=True)
            link = store / "cycle-payload"
            link.symlink_to("../outside/cycle/payload")
            before = harness.store_fingerprint(store)
            self.assertEqual(
                before["cycle-payload"],
                ("symlink", os.readlink(link), "loop"),
            )
            cycle.unlink()
            cycle.mkdir()
            after = harness.store_fingerprint(store)
            self.assertEqual(
                after["cycle-payload"],
                ("symlink", "../outside/cycle/payload", "dangling"),
            )
            self.assertEqual(harness.store_changes(before, after), ["cycle-payload"])

    def test_an_unreadable_target_is_not_a_dangling_link(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            store = root / "store"
            store.mkdir()
            target = root / "payload"
            target.write_bytes(b"private")
            (store / "link").symlink_to("../payload")
            lstat = os.lstat

            def inspect(path, *args, **kwargs):
                if Path(path) == target:
                    raise PermissionError("target is not accessible")
                return lstat(path, *args, **kwargs)

            with patch("scripts.acceptance.harness.os.lstat", side_effect=inspect):
                fingerprint = harness.store_fingerprint(store)
            self.assertEqual(
                fingerprint["link"],
                ("symlink", "../payload", "unreadable", "PermissionError"),
            )


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

    def test_a_duplicate_cancellation_must_match_the_transition_entry(self):
        entered = "2026-11-05T00:00:02.000Z"
        history = [
            {"state": "Ready", "state_type": "unstarted", "started_at": "2026-11-05T00:00:01.000Z"},
            {"state": "Duplicate", "state_type": "duplicate", "started_at": entered},
        ]
        fresh = {
            "state": "Duplicate", "state_type": "duplicate", "canceled_at": entered,
            "state_history": history,
        }
        stale = dict(fresh, canceled_at="1999-01-01T00:00:00.000Z")
        self.assertFalse(scenarios.stamp_problems(fresh))
        problems = scenarios.stamp_problems(stale)
        self.assertTrue(any(entered in problem for problem in problems))



if __name__ == "__main__":
    unittest.main()
class SubIssueClosureTests(unittest.TestCase):
    def context(
        self,
        corrupt_waiting_parent=False,
        corrupt_done_waiting=False,
        corrupt_done_created=False,
        corrupt_terminal_created=False,
        corrupt_idle_created=False,
        corrupt_closed_again=False,
    ):
        class MemoryMcp:
            def __init__(self):
                self.issues = {}
                self.next_id = 1
                self.states = []
                self.auto_close = False
                self.corrupt_waiting_parent = corrupt_waiting_parent
                self.corrupt_done_waiting = corrupt_done_waiting
                self.corrupt_done_created = corrupt_done_created
                self.corrupt_terminal_created = corrupt_terminal_created
                self.corrupt_idle_created = corrupt_idle_created
                self.corrupt_closed_again = corrupt_closed_again
                self.closed_parent_reads = 0

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
                if self.corrupt_waiting_parent and self.waiting(key):
                    record["version"] = 99
                    record["updated_at"] = "2099-01-01T00:00:00.000Z"
                    record["state_history"] = record["state_history"] + [{
                        "state": "Done", "state_type": "completed",
                        "started_at": "2099-01-01T00:00:00.000Z",
                    }]
                if self.corrupt_done_waiting and self.done_waiting(key):
                    record["version"] = 99
                    record["updated_at"] = "2099-01-01T00:00:00.000Z"
                    record["state_history"] = record["state_history"] + [{
                        "state": "Done", "state_type": "completed",
                        "started_at": "2099-01-01T00:00:00.000Z",
                    }]
                if self.corrupt_done_created and self.done_waiting(key):
                    record["created_at"] = "1999-01-01T00:00:00.000Z"
                if self.corrupt_terminal_created and self.terminal_closed(key):
                    record["created_at"] = "1999-01-01T00:00:00.000Z"
                if self.corrupt_idle_created and self.idle_closed(key):
                    record["created_at"] = "1999-01-01T00:00:00.000Z"
                if self.corrupt_closed_again and self.closed_again(key):
                    self.closed_parent_reads += 1
                    if self.closed_parent_reads >= 2:
                        record["created_at"] = "1999-01-01T00:00:00.000Z"
                return record

            def children_of(self, key):
                return [item for item in self.issues.values() if item.get("parent") == key]

            def waiting(self, key):
                stored = self.issues[key]
                if "Mixed closed children" not in stored["title"]:
                    return False
                children = self.children_of(key)
                closed = [
                    item for item in children if item["state"] in {"Cancelled", "Superseded"}
                ]
                return bool(children) and 0 < len(closed) < len(children)

            def done_waiting(self, key):
                stored = self.issues[key]
                if stored["title"] != "T11 Auto-close parent":
                    return False
                children = self.children_of(key)
                done = [item for item in children if item["state"] == "Done"]
                return bool(children) and 0 < len(done) < len(children)

            def terminal_closed(self, key):
                stored = self.issues[key]
                if stored["title"] not in {
                    "Done parent", "Cancelled parent", "Superseded parent", "Duplicate parent",
                }:
                    return False
                children = self.children_of(key)
                return bool(children) and all(item["state"] == "Done" for item in children)

            def idle_closed(self, key):
                stored = self.issues[key]
                if stored["title"] != "T11 Parent":
                    return False
                children = self.children_of(key)
                closed = {"Done", "Cancelled", "Superseded", "Duplicate"}
                return bool(children) and all(item["state"] in closed for item in children)

            def closed_again(self, key):
                stored = self.issues[key]
                if stored["title"] != "T11 Auto-close parent":
                    return False
                children = self.children_of(key)
                return bool(children) and all(item["state"] == "Done" for item in children)

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

    def clock_results(self, corrupt=False):
        ctx = self.context(corrupt_waiting_parent=corrupt)
        ctx.rec.begin(11, "Sub-issues")
        scenarios.test_sub_issues(ctx)
        return {result[2]: result[1] for result in ctx.rec.tests[-1].results}

    def test_a_waiting_parent_keeps_its_clock_while_a_child_remains(self):
        results = self.clock_results()
        for child in ("cancelled", "superseded"):
            description = (
                f"a {child} child leaves the parent version, timestamps and history unchanged"
            )
            self.assertEqual(results[description], checks.PASS)

    def test_a_waiting_parent_rejects_a_changed_clock_while_a_child_remains(self):
        results = self.clock_results(corrupt=True)
        for child in ("cancelled", "superseded"):
            description = (
                f"a {child} child leaves the parent version, timestamps and history unchanged"
            )
            self.assertEqual(results[description], checks.FAIL)

    def closure_results(self, **flags):
        ctx = self.context(**flags)
        ctx.rec.begin(11, "Sub-issues")
        scenarios.test_sub_issues(ctx)
        return {result[2]: result[1] for result in ctx.rec.tests[-1].results}

    def test_a_done_child_keeps_the_parent_clock_while_a_sibling_is_open(self):
        results = self.closure_results()
        description = (
            "a done child leaves the parent version, timestamps and history unchanged "
            "while a sibling is open"
        )
        self.assertEqual(results[description], checks.PASS)

    def test_a_done_child_rejects_a_moved_parent_clock_while_a_sibling_is_open(self):
        results = self.closure_results(corrupt_done_waiting=True)
        description = (
            "a done child leaves the parent version, timestamps and history unchanged "
            "while a sibling is open"
        )
        self.assertEqual(results[description], checks.FAIL)

    def test_a_done_child_rejects_a_replaced_created_at_while_a_sibling_is_open(self):
        results = self.closure_results(corrupt_done_created=True)
        description = (
            "a done child leaves the parent version, timestamps and history unchanged "
            "while a sibling is open"
        )
        self.assertEqual(results[description], checks.FAIL)

    def test_a_terminal_parent_rejects_a_replaced_created_at(self):
        results = self.closure_results(corrupt_terminal_created=True)
        for state in ("Done", "Cancelled", "Superseded", "Duplicate"):
            description = (
                f"a {state} parent keeps its version and timestamps when its last child closes"
            )
            self.assertEqual(results[description], checks.FAIL, state)

    def test_an_idle_parent_rejects_a_replaced_created_at(self):
        results = self.closure_results(corrupt_idle_created=True)
        self.assertEqual(
            results["with auto-close off, the parent keeps its version and timestamps"],
            checks.FAIL,
        )

    def test_an_already_closed_parent_rejects_a_replaced_created_at(self):
        results = self.closure_results(corrupt_closed_again=True)
        self.assertEqual(results["an already closed parent is not closed again"], checks.FAIL)


class FixtureNameTests(unittest.TestCase):
    def test_new_query_and_closure_fixtures_describe_their_behaviour(self):
        source = Path("scripts/acceptance/scenarios.py").read_text()
        required = (
            '"marigold beacon"',
            '"plain heading"',
            '"control heading"',
            '"Closed-child progress"',
            '"Cancelled child"',
            '"Superseded child"',
            '"Duplicate child"',
            '"Mixed closed children"',
            '"Mixed cancelled child"',
            '"Mixed superseded child"',
            '"Mixed duplicate child"',
            "f\"{state['name']} parent\"",
            "f\"Child of {state['name']}\"",
        )
        forbidden = (
            '"T2 marigold beacon"',
            '"T2 plain heading"',
            '"T2 control heading"',
            '"T11 Closed-child progress"',
            '"T11 Cancelled child"',
            '"T11 Superseded child"',
            '"T11 Duplicate child"',
            '"T11 Mixed closed children"',
            '"T11 Mixed cancelled child"',
            '"T11 Mixed superseded child"',
            '"T11 Mixed duplicate child"',
            "f\"T11 {state['name']} parent\"",
            "f\"T11 Child of {state['name']}\"",
        )
        inherited = (
            '"T2 aurora frontend task"',
            '"T11 Parent"',
            '"T11 Auto-close parent"',
            'f"T4 Born {state}"',
        )
        for title in required:
            self.assertIn(title, source)
        for title in forbidden:
            self.assertNotIn(title, source)
        for title in inherited:
            self.assertIn(title, source)
