"""Installed diagnostics are native-only and fail closed."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from scripts import package_smoke

ROOT = Path(__file__).resolve().parents[1]


class InstalledSmokeTests(unittest.TestCase):
    def test_installation_supports_a_long_default_temporary_directory(self):
        import socket
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as outer:
            long_default = Path(outer) / ("nested-" * 20)
            long_default.mkdir()
            with patch.object(tempfile, "tempdir", str(long_default)):
                with package_smoke.installation_directory() as directory:
                    root = Path(directory).resolve()
                    (root / "data").mkdir()
                    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
                        try:
                            listener.bind(str(root / "data/core.sock"))
                        except OSError as error:
                            self.fail(f"installation must support its real Unix socket: {error}")

    def test_cleanup_rejects_other_accounts_before_native_access(self):
        from unittest.mock import patch
        with patch.object(package_smoke, "credential_status", side_effect=AssertionError("native access forbidden")):
            for account in ("installation", "data-" + "q" * 64):
                with self.assertRaises(ValueError):
                    package_smoke.remove_owned_credential(account)

    def test_identity_uses_the_tauri_resource_layout(self):
        with tempfile.TemporaryDirectory() as directory:
            app = Path(directory) / "Kanban.app"
            resource = app / "Contents/Resources/resources/build-identity.json"
            resource.parent.mkdir(parents=True)
            identity = {"version": "0.1.0", "source_revision": "fixture", "source_epoch": 0}
            resource.write_text(json.dumps(identity))
            self.assertEqual(package_smoke.installed_identity(app), identity)

    def test_selected_storage_matches_the_service_database_name(self):
        import sqlite3
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with sqlite3.connect(root / "kanban.sqlite") as database:
                database.execute("CREATE TABLE fixture (id INTEGER PRIMARY KEY)")
            package_smoke.require_stopped_data(root)
            (root / "core.sock").touch()
            with self.assertRaisesRegex(RuntimeError, "stop its socket"):
                package_smoke.require_stopped_data(root)

    def test_missing_image_fails_with_a_failed_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            report = root / "receipt.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts/package_smoke.py"),
                 "--dmg", str(root / "missing.dmg"), "--report", str(report)],
                capture_output=True, text=True, timeout=15)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("disk image is missing", result.stderr)
            self.assertFalse(json.loads(report.read_text())["passed"])


if __name__ == "__main__":
    unittest.main()
