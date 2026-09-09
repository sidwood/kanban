"""Source provenance and owned staging boundary tests."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import package_inputs

ROOT = Path(__file__).resolve().parents[1]
IDENTITY = {"version": "0.1.0", "source_revision": "a" * 40, "source_epoch": 1700000000}


class SourceSnapshotTests(unittest.TestCase):
    def test_git_export_matches_real_revision_and_dirty_export_matches_working_bytes(self):
        self.assertTrue(hasattr(package_inputs, "capture_source"), "missing independently verified source export")
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        clean = package_inputs.capture_source(ROOT, revision, False)
        expected = subprocess.check_output(["git", "show", f"{revision}:Cargo.toml"], cwd=ROOT)
        self.assertEqual(clean["Cargo.toml"], ("100644", expected))
        paths = subprocess.check_output(["git", "ls-tree", "-rz", "--name-only", revision], cwd=ROOT)
        self.assertEqual(set(clean), set(filter(None, os.fsdecode(paths).split("\0"))))
        dirty = package_inputs.capture_source(ROOT, revision, True)
        self.assertEqual(dirty["scripts/package.py"][1], (ROOT / "scripts/package.py").read_bytes())
        self.assertEqual(dirty["scripts/package_inputs.py"][1], (ROOT / "scripts/package_inputs.py").read_bytes())
        self.assertFalse(any(path.startswith(("target/", "temp/", "node_modules/", ".git/")) for path in dirty))


class InputStagingTests(unittest.TestCase):
    def test_release_hook_requires_exact_staged_source_and_identity(self):
        self.assertTrue(hasattr(package_inputs, "verify_inputs"), "missing staged release-hook verification")
        with tempfile.TemporaryDirectory() as directory:
            files = {"source": ("100644", b"exact source")}
            with package_inputs.staged_inputs(files, IDENTITY, Path(directory) / "staging") as (root, receipt):
                environment = {"KANBAN_PACKAGE_INPUT_SHA256": receipt["input_sha256"],
                               "KANBAN_SOURCE_REVISION": IDENTITY["source_revision"],
                               "SOURCE_DATE_EPOCH": str(IDENTITY["source_epoch"])}
                self.assertEqual(package_inputs.verify_inputs(root, environment), IDENTITY)
                with self.assertRaises(ValueError):
                    package_inputs.verify_inputs(root, environment | {"KANBAN_SOURCE_REVISION": "b" * 40})
                with self.assertRaises(ValueError):
                    package_inputs.verify_inputs(root, {})
                (root / "source").write_bytes(b"changed")
                with self.assertRaises(ValueError):
                    package_inputs.verify_inputs(root, environment)
                (root / "source").write_bytes(b"exact source")
                manifest = root.parent / "inputs.json"
                changed = json.loads(manifest.read_text())
                changed["identity"]["version"] = "99.0.0"
                manifest.write_text(json.dumps(changed))
                with self.assertRaises(ValueError):
                    package_inputs.verify_inputs(root, environment)

    def test_staging_rejects_unsafe_paths_and_borrowed_roots(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory) / "staging"
            for name, mode in [("../escape", "100644"), ("/escape", "100644"),
                               (".git/HEAD", "100644"), ("link", "120000")]:
                with self.subTest(name=name), self.assertRaises(ValueError):
                    with package_inputs.staged_inputs({name: (mode, b"not written")}, IDENTITY, base):
                        self.fail("unsafe input accepted")
            borrowed = Path(directory) / "borrowed"
            borrowed.mkdir(mode=0o755)
            with self.assertRaises(ValueError):
                with package_inputs.staged_inputs({"file": ("100644", b"bytes")}, IDENTITY, borrowed):
                    self.fail("borrowed root accepted")
            link = Path(directory) / "link"
            link.symlink_to(borrowed)
            with self.assertRaises(ValueError):
                with package_inputs.staged_inputs({"file": ("100644", b"bytes")}, IDENTITY, link):
                    self.fail("symlink root accepted")
            self.assertEqual(list(borrowed.iterdir()), [])

    def test_lock_prevents_overlap_and_cleanup_preserves_unowned_files(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory) / "staging"
            files = {"script": ("100755", b"exact source")}
            with package_inputs.staged_inputs(files, IDENTITY, base) as (root, receipt):
                sentinel = base / "not-ours"
                sentinel.write_bytes(b"keep")
                self.assertEqual((root / "script").stat().st_mode & 0o777, 0o755)
                self.assertEqual((root / "script").read_bytes(), b"exact source")
                self.assertFalse((root / ".git").exists())
                with self.assertRaisesRegex(ValueError, "busy"):
                    with package_inputs.staged_inputs(files, IDENTITY, base):
                        self.fail("overlapping stage accepted")
                self.assertEqual(receipt["manifest"]["files"][0]["sha256"], hashlib.sha256(b"exact source").hexdigest())
            self.assertFalse(root.exists())
            self.assertEqual(sentinel.read_bytes(), b"keep")
            previous = root.parent
            previous.mkdir()
            (previous / "previous-run").write_bytes(b"keep")
            with self.assertRaises(FileExistsError):
                with package_inputs.staged_inputs(files, IDENTITY, base):
                    self.fail("previous run overwritten")
            self.assertEqual((previous / "previous-run").read_bytes(), b"keep")


if __name__ == "__main__":
    unittest.main(verbosity=2)
