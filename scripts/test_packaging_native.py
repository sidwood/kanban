"""Real macOS fixture proof; intentionally separate from portable unit tests."""
# cspell:words APPL codesign mountpoint nobrowse xattr xorrisorc
import hashlib
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/package.py"
EPOCH = 1700000000


def run(*args):
    result = subprocess.run([str(arg) for arg in args], capture_output=True)
    if result.returncode:
        raise RuntimeError(f"{args}: {result.stdout.decode()}\n{result.stderr.decode()}")
    return result.stdout


sys.path.insert(0, str(ROOT / "scripts"))
import package


class NativePackagingTests(unittest.TestCase):
    def test_icon_is_repeatable_and_native_readable(self):
        self.assertTrue(hasattr(package, "make_icon"), "missing generated release icon")
        with tempfile.TemporaryDirectory(prefix="kanban-icon-") as directory:
            paths = [Path(directory) / f"icon-{index}.icns" for index in range(2)]
            for path in paths:
                package.make_icon(ROOT / "apps/desktop/src-tauri/icons/icon.png", path)
                description = run("sips", "-g", "pixelWidth", "-g", "pixelHeight", path).decode()
                self.assertIn("pixelWidth: 512", description)
                self.assertIn("pixelHeight: 512", description)
            self.assertEqual(paths[0].read_bytes(), paths[1].read_bytes())

    def test_independent_images_have_identical_bytes_and_mount(self):
        self.assertTrue(SCRIPT.exists(), "the native image packaging entry point is missing")
        with tempfile.TemporaryDirectory(prefix="kanban-native-package-") as directory:
            root = Path(directory)
            source = root / "fixture.c"
            source.write_text('#include <stdio.h>\nint main(void) { puts("packaging fixture"); return 0; }\n')
            binary = root / "native-fixture"
            run("clang", "-O2", source, "-o", binary)
            images = []
            for index in range(2):
                app = root / f"independent-{index}" / "Kanban.app"
                macos = app / "Contents/MacOS"
                resources = app / "Contents/Resources"
                macos.mkdir(parents=True)
                resources.mkdir()
                shutil.copyfile(binary, macos / "native-fixture")
                (macos / "native-fixture").chmod(0o755)
                (app / "Contents/Info.plist").write_bytes(plistlib.dumps({
                    "CFBundleExecutable": "native-fixture",
                    "CFBundleIdentifier": "dev.kanban.packaging-fixture",
                    "CFBundleName": "Kanban",
                    "CFBundlePackageType": "APPL",
                    "CFBundleVersion": "1",
                }, sort_keys=True))
                shutil.copyfile(ROOT / "apps/desktop/src-tauri/icons/icon.png", resources / "icon.png")
                for name in (["alpha", "omega"] if index == 0 else ["omega", "alpha"]):
                    (resources / name).write_text(f"packaging fixture: {name}\n")
                run("codesign", "--force", "--sign", "-", "--timestamp=none", app)
                for path in [app, *app.rglob("*")]:
                    os.utime(path, (EPOCH + index * 100, EPOCH + index * 100))
                image = root / f"fixture-{index}.dmg"
                environment = dict(os.environ)
                if index == 1:
                    home = root / "different-home"
                    home.mkdir()
                    (home / ".xorrisorc").write_text("-this-command-must-never-be-read\n")
                    environment["HOME"] = str(home)
                process = subprocess.run([sys.executable, str(SCRIPT), "dmg", "--app", str(app),
                                          "--output", str(image), "--epoch", str(EPOCH)],
                                         text=True, capture_output=True, env=environment)
                self.assertEqual(process.returncode, 0, process.stdout + process.stderr)
                self.assertIn("DMG_MOUNT_VERIFIED", process.stdout,
                              "packaging must verify the mounted app before reporting success")
                print(process.stdout, end="", flush=True)
                images.append(image)
                time.sleep(2)
            self.assertEqual(images[0].read_bytes(), images[1].read_bytes(),
                             "DMG reproducibility is full bytes, not canonical content")
            for image in images:
                print(f"FULL_BYTES_SHA256 {hashlib.sha256(image.read_bytes()).hexdigest()} {image.name}", flush=True)
                print(run("hdiutil", "verify", image).decode(), end="", flush=True)
                mount = root / image.stem
                mount.mkdir()
                run("hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", mount, image)
                try:
                    self.assertEqual((mount / "Applications").readlink(), Path("/Applications"))
                    installed = mount / "Kanban.app"
                    print(run("xattr", "-lr", installed).decode(), flush=True)
                    run("codesign", "--verify", "--deep", "--strict", installed)
                    self.assertEqual(run(installed / "Contents/MacOS/native-fixture"), b"packaging fixture\n")
                    self.assertEqual((installed / "Contents/Resources/icon.png").read_bytes(),
                                     (ROOT / "apps/desktop/src-tauri/icons/icon.png").read_bytes())
                    print(f"NATIVE_MOUNT_CODESIGN_EXEC_OK {image.name}", flush=True)
                finally:
                    run("hdiutil", "detach", mount)


if __name__ == "__main__":
    unittest.main(verbosity=2)
