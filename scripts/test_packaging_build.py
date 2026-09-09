"""Expensive real release hook and app build, separate from fixture and CI tests."""
# cspell:words codesign unnotarized
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReleaseBuildTests(unittest.TestCase):
    def test_independent_source_roots_produce_identical_complete_artifacts(self):
        patch = subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=ROOT)
        untracked = subprocess.check_output(
            ["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=ROOT).split(b"\0")
        with tempfile.TemporaryDirectory(prefix="kanban-cross-root-") as temporary:
            directory = Path(os.environ.get("KANBAN_PACKAGING_TEST_ROOT", temporary))
            if directory != Path(temporary):
                directory.mkdir(parents=True, exist_ok=False)
            reports = []
            for name in ("first", "second"):
                clone = directory / name
                subprocess.run(["git", "clone", "--no-local", str(ROOT), str(clone)], check=True)
                if patch:
                    subprocess.run(["git", "apply", "--binary", "-"], input=patch, cwd=clone, check=True)
                for raw in filter(None, untracked):
                    relative = Path(os.fsdecode(raw))
                    (clone / relative).parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(ROOT / relative, clone / relative)
                    shutil.copymode(ROOT / relative, clone / relative)
                missing = {generated: not (clone / generated).exists() for generated in
                           ("target", "node_modules", "apps/desktop/dist", "apps/desktop/src-tauri/binaries")}
                self.assertTrue(all(missing.values()), missing)
                preflight = {"checkout": str(clone), "absent_before_build": missing,
                             "revision": subprocess.check_output(
                                 ["git", "rev-parse", "HEAD"], cwd=clone, text=True).strip(),
                             "status": subprocess.check_output(
                                 ["git", "status", "--porcelain"], cwd=clone, text=True)}
                (directory / f"{name}-preflight.json").write_text(json.dumps(preflight, indent=2) + "\n")
                output = directory / f"{name}-artifacts"
                with (directory / f"{name}.log").open("w") as log:
                    result = subprocess.run([sys.executable, "scripts/package.py", "build", "--allow-dirty",
                                             "--output", str(output)], cwd=clone, stdout=log,
                                            stderr=subprocess.STDOUT)
                self.assertEqual(result.returncode, 0, str(directory / f"{name}.log"))
                report = json.loads((output / "packaging-report.json").read_text())
                reports.append(report)
                print(json.dumps({"checkout": str(clone), "report": report}, indent=2), flush=True)
            self.assertEqual(reports[0]["identity"], reports[1]["identity"])
            self.assertEqual(reports[0]["toolchain"], reports[1]["toolchain"])
            inputs = [json.loads((directory / f"{name}-artifacts/build-inputs.json").read_text())
                      for name in ("first", "second")]
            self.assertEqual(inputs[0], inputs[1])
            self.assertTrue(inputs[0]["fresh_targets"])
            self.assertFalse(Path(inputs[0]["build_root"]).exists())
            for name in ("Kanban.app.tar", "Kanban.dmg"):
                left = (directory / "first-artifacts" / name).read_bytes()
                right = (directory / "second-artifacts" / name).read_bytes()
                self.assertTrue(left == right, f"cross-root {name} mismatch: "
                                f"{hashlib.sha256(left).hexdigest()} != {hashlib.sha256(right).hexdigest()}")

    def test_just_package_builds_real_release_sidecars_and_app(self):
        with tempfile.TemporaryDirectory(prefix="kanban-release-test-") as directory:
            output = Path(directory) / "artifacts"
            result = subprocess.run(["just", "package", "--allow-dirty", "--output", str(output)], cwd=ROOT)
            self.assertEqual(result.returncode, 0, "just package must build, not describe, the release")
            report = json.loads((output / "packaging-report.json").read_text())
            app = output / "Kanban.app"
            self.assertEqual(plistlib.loads((app / "Contents/Info.plist").read_bytes())["CFBundleShortVersionString"],
                             report["identity"]["version"])
            resource = json.loads((app / "Contents/Resources/resources/build-identity.json").read_text())
            self.assertEqual(resource, report["identity"])
            service = app / "Contents/MacOS/kanban-service"
            version = json.loads(subprocess.check_output([str(service), "--version"], text=True))
            self.assertEqual(version, resource)
            for binary in ("kanban-desktop", "kanban-service", "kanban-mcp"):
                path = app / "Contents/MacOS" / binary
                self.assertTrue(path.is_file())
                self.assertTrue(path.stat().st_mode & 0o111)
            executables = {path.name for path in (app / "Contents/MacOS").iterdir()
                           if path.is_file() and path.stat().st_mode & 0o111}
            self.assertEqual(executables, {"kanban-desktop", "kanban-service", "kanban-mcp"})
            self.assertFalse(any(path.name.lower().startswith("herdr") for path in app.rglob("*")))
            subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
            for name in ("Kanban.dmg", "Kanban.app.tar"):
                self.assertEqual(hashlib.sha256((output / name).read_bytes()).hexdigest(), report["sha256"][name])
            self.assertEqual(report["signing"], "ad-hoc; unnotarized personal preview")
            print(json.dumps(report, indent=2), flush=True)


if __name__ == "__main__":
    unittest.main(verbosity=2)
