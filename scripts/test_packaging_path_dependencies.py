"""Real Cargo regression for the shell's external path dependencies, not app proof."""
# cspell:words repro rustc RUSTFLAGS
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

import package


class ExternalPathReproducibilityTests(unittest.TestCase):
    def test_release_environment_preserves_bytes_across_external_path_roots(self):
        rust = subprocess.check_output(["rustc", "-vV"], text=True)
        target = next(line.removeprefix("host: ") for line in rust.splitlines()
                      if line.startswith("host: "))
        identity = {"version": "0.1.0", "source_revision": "a" * 40,
                    "source_epoch": 1700000000}
        sources = {
            "Cargo.toml": '[workspace]\nmembers = ["crates/core"]\nresolver = "2"\n',
            "crates/core/Cargo.toml": ('[package]\nname = "repro-core"\n'
                                       'version = "0.1.0"\nedition = "2024"\n'),
            "crates/core/src/lib.rs": ('#[inline(never)]\npub fn message() -> String {\n'
                                       '    std::hint::black_box("path dependency").to_owned()\n}\n'),
            "apps/shell/Cargo.toml": ('[package]\nname = "repro-shell"\n'
                                      'version = "0.1.0"\nedition = "2024"\n'
                                      '[workspace]\n[dependencies]\n'
                                      'repro-core = { path = "../../crates/core" }\n'),
            "apps/shell/src/main.rs": 'fn main() { println!("{}", repro_core::message()); }\n',
        }
        products = []
        with tempfile.TemporaryDirectory(prefix="kanban-path-regression-") as directory:
            for label in ("first", "second"):
                root = Path(directory) / label
                for name, content in sources.items():
                    path = root / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(content)
                self.assertTrue(hasattr(package, "staged_inputs"), "missing deterministic compiler input staging")
                files = {name: ("100644", (root / name).read_bytes()) for name in sources}
                with package.staged_inputs(files, identity, Path(directory) / "staging") as (staged, _):
                    self.assertFalse((staged / "target").exists(), "each build needs a fresh target")
                    self.assertFalse((staged / ".git").exists(), "staging must not invent Git metadata")
                    environment = package.release_environment(staged, identity, target, os.environ)
                    manifest = staged / "apps/shell/Cargo.toml"
                    commands = [
                        ["cargo", "generate-lockfile", "--offline", "--manifest-path", str(manifest)],
                        ["cargo", "build", "--locked", "--offline", "--release", "--target", target,
                         "--manifest-path", str(manifest), "--verbose"],
                    ]
                    for command in commands:
                        result = subprocess.run(command, cwd=staged, env=environment, text=True,
                                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
                        print(result.stdout, end="", flush=True)
                        self.assertEqual(result.returncode, 0, result.stdout)
                    suffix = ".exe" if os.name == "nt" else ""
                    binary = Path(environment["CARGO_TARGET_DIR"]) / target / "release" / f"repro-shell{suffix}"
                    self.assertEqual(subprocess.check_output([str(binary)], text=True), "path dependency\n")
                    products.append(binary.read_bytes())
                self.assertFalse(staged.exists(), "owned build inputs and targets must be cleaned")
            hashes = [hashlib.sha256(product).hexdigest() for product in products]
            print(json.dumps({"rustc": rust, "sha256": hashes}, indent=2), flush=True)
            self.assertTrue(products[0] == products[1],
                            f"external path dependencies changed release bytes: {hashes}")


if __name__ == "__main__":
    unittest.main(verbosity=2)
