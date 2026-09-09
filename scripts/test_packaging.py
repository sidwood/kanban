"""Packaging policy and deterministic disk-image regression tests."""
# cspell:words CFLAGS CXXFLAGS RUSTFLAGS UDIF ffile koly xorriso
import json
import os
from pathlib import Path
import subprocess
import shlex
import struct
import sys
import tarfile
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import package

ROOT = Path(__file__).resolve().parents[1]


class FinderCatalogTests(unittest.TestCase):
    def catalog_image(self):
        """Synthetic contiguous HFS+ catalog; not a mountable native image."""
        data = bytearray(8192)
        data[:2] = b"ER"
        struct.pack_into(">HI", data, 2, 512, 16)
        data[512:514] = b"PM"
        struct.pack_into(">III", data, 516, 1, 4, 12)
        data[560:570] = b"Apple_HFS\0"
        header = 3072
        data[header:header + 4] = b"H+\0\x04"
        struct.pack_into(">II", data, header + 40, 512, 12)
        struct.pack_into(">Q", data, header + 272, 2048)
        struct.pack_into(">III", data, header + 284, 4, 4, 4)
        catalog = 4096
        data[catalog + 8] = 1
        struct.pack_into(">H", data, catalog + 10, 3)
        struct.pack_into(">H 4I 2H 2I", data, catalog + 14, 1, 1, 2, 1, 1, 1024, 516, 2, 0)
        struct.pack_into(">I", data, catalog + 52, 6)
        for index, offset in enumerate((14, 120, 248, 1016)):
            struct.pack_into(">H", data, catalog + 1024 - 2 * (index + 1), offset)
        leaf = catalog + 1024
        data[leaf + 8:leaf + 10] = b"\xff\x01"
        struct.pack_into(">H", data, leaf + 10, 2)
        for index, offset in enumerate((14, 272, 530)):
            struct.pack_into(">H", data, leaf + 1024 - 2 * (index + 1), offset)
        for offset, name in ((14, "a"), (272, "b")):
            key = leaf + offset
            struct.pack_into(">HIH", data, key, 8, 2, 1)
            data[key + 8:key + 10] = name.encode("utf-16be")
            record = key + 10
            struct.pack_into(">H", data, record, 2)
            struct.pack_into(">H", data, record + 42, 0o100644)
            data[record + 48:record + 80] = b"????????" + bytes(24)
        data[7000:7032] = b"????????" + bytes(24)
        return data

    def assert_rejected_unchanged(self, data):
        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / "catalog.bin"
            image.write_bytes(data)
            try:
                with self.assertRaises(ValueError):
                    package.clear_xorriso_finder_info(image)
            finally:
                self.assertEqual(image.read_bytes(), data, "rejected images must remain byte-identical")

    def test_only_regular_file_defaults_are_cleared_and_repeating_is_safe(self):
        for mode in (0o100644, 0o120777):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                data = self.catalog_image()
                second = 5120 + 272 + 10
                struct.pack_into(">H", data, second + 42, mode)
                expected = bytearray(data)
                expected[5192:5200] = bytes(8)
                if mode == 0o100644:
                    expected[second + 48:second + 56] = bytes(8)
                image = Path(directory) / "catalog.bin"
                image.write_bytes(data)
                for _ in range(2):
                    package.clear_xorriso_finder_info(image)
                    self.assertEqual(image.read_bytes(), expected)

    def test_invalid_leaf_records_are_rejected_without_any_edits(self):
        leaf = 5120
        second_key = leaf + 272
        second_record = second_key + 10
        cases = {
            "record outside leaf and catalog": (leaf + 1022, 2048),
            "record in node descriptor": (leaf + 1022, 12),
            "unaligned record": (leaf + 1022, 15),
            "record in offset table": (leaf + 1022, 1020),
            "duplicate records": (leaf + 1020, 14),
            "late record outside leaf": (leaf + 1020, 2048),
            "reversed records": (leaf + 1020, 12),
            "free space outside leaf": (leaf + 1018, 1024),
            "offset table overlaps descriptor": (leaf + 10, 505),
            "late key crosses record": (second_key, 1024),
            "short key": (second_key, 0),
            "odd key length": (second_key, 7),
            "key name length disagrees": (second_key + 6, 2),
            "unknown record type": (second_record, 5),
            "missing key length": (leaf + 1018, 273),
            "missing record type": (leaf + 1018, 282),
            "truncated mode": (leaf + 1018, 324),
            "truncated Finder field": (leaf + 1018, 354),
            "truncated file body": (leaf + 1018, 528),
        }
        for name, (offset, value) in cases.items():
            with self.subTest(layout=name):
                data = self.catalog_image()
                struct.pack_into(">H", data, offset, value)
                self.assert_rejected_unchanged(data)
        for record_type, end in ((1, 368), (3, 290), (4, 290)):
            with self.subTest(truncated_record_type=record_type):
                data = self.catalog_image()
                struct.pack_into(">H", data, second_record, record_type)
                struct.pack_into(">H", data, leaf + 1018, end)
                self.assert_rejected_unchanged(data)
        for record_type in (3, 4):
            with self.subTest(truncated_thread_name=record_type):
                data = self.catalog_image()
                struct.pack_into(">H", data, second_record, record_type)
                struct.pack_into(">H", data, second_record + 8, 255)
                self.assert_rejected_unchanged(data)

    def test_record_offset_cannot_redirect_a_finder_write_outside_catalog(self):
        data = self.catalog_image()
        leaf, catalog_end = 5120, 6144
        data[catalog_end:catalog_end + 258] = data[leaf + 14:leaf + 272]
        struct.pack_into(">H", data, leaf + 10, 1)
        struct.pack_into(">H", data, leaf + 1022, 1024)
        struct.pack_into(">H", data, leaf + 1020, 1282)
        self.assert_rejected_unchanged(data)

    def test_invalid_container_bounds_are_rejected_without_any_edits(self):
        header = 3072
        cases = {
            "zero sector": (2, "H", 0),
            "short sector": (2, "H", 256),
            "non power of two sector": (2, "H", 513),
            "sector beyond image": (2, "H", 32768),
            "partition signature": (512, "H", 0),
            "empty partition map": (516, "I", 0),
            "map beyond image": (516, "I", 32),
            "partition overlaps map": (520, "I", 1),
            "partition beyond image": (520, "I", 16),
            "empty partition": (524, "I", 0),
            "partition exceeds image": (524, "I", 13),
            "volume header exceeds partition": (524, "I", 2),
            "catalog exceeds partition": (524, "I", 6),
            "zero allocation block": (header + 40, "I", 0),
            "short allocation block": (header + 40, "I", 256),
            "non power of two allocation block": (header + 40, "I", 513),
            "empty volume": (header + 44, "I", 0),
            "volume exceeds partition": (header + 44, "I", 13),
            "catalog exceeds volume": (header + 44, "I", 7),
            "empty catalog": (header + 272, "Q", 0),
            "short catalog header": (header + 272, "Q", 32),
            "partial catalog node": (header + 272, "Q", 1023),
            "catalog allocation mismatch": (header + 284, "I", 5),
            "catalog overlaps volume header": (header + 288, "I", 2),
            "catalog extent exceeds partition": (header + 288, "I", 11),
            "catalog extent exceeds volume": (header + 292, "I", 13),
            "fragmented catalog": (header + 296, "I", 1),
        }
        for name, (offset, fmt, value) in cases.items():
            with self.subTest(layout=name):
                data = self.catalog_image()
                struct.pack_into(">" + fmt, data, offset, value)
                self.assert_rejected_unchanged(data)
        data = self.catalog_image()
        struct.pack_into(">I", data, 516, 2)
        data[1024:1026] = b"PM"
        struct.pack_into(">III", data, 1028, 1, 2, 1)
        with self.subTest(layout="inconsistent partition map counts"):
            self.assert_rejected_unchanged(data)

    def test_invalid_node_layouts_are_rejected_without_any_edits(self):
        catalog, leaf = 4096, 5120
        cases = {
            "missing header node": (catalog + 8, "B", 0),
            "header height": (catalog + 9, "B", 1),
            "header record count": (catalog + 10, "H", 2),
            "header record offset": (catalog + 1022, "H", 16),
            "truncated header record": (catalog + 1020, "H", 118),
            "truncated user record": (catalog + 1018, "H", 246),
            "node size too small": (catalog + 32, "H", 256),
            "node size exceeds catalog": (catalog + 32, "H", 4096),
            "node count exceeds catalog": (catalog + 36, "I", 3),
            "node count omits leaf": (catalog + 36, "I", 1),
            "free nodes exceed total": (catalog + 40, "I", 3),
            "root outside catalog": (catalog + 16, "I", 2),
            "first leaf outside catalog": (catalog + 24, "I", 2),
            "last leaf outside catalog": (catalog + 28, "I", 2),
            "unsupported key format": (catalog + 52, "I", 0),
            "unknown node kind": (leaf + 8, "B", 3),
            "duplicate header node": (leaf + 8, "B", 1),
            "leaf height": (leaf + 9, "B", 2),
            "next node outside catalog": (leaf, "I", 2),
            "previous node outside catalog": (leaf + 4, "I", 2),
        }
        for name, (offset, fmt, value) in cases.items():
            with self.subTest(layout=name):
                data = self.catalog_image()
                struct.pack_into(">" + fmt, data, offset, value)
                self.assert_rejected_unchanged(data)

    def indexed_catalog_image(self):
        data = self.catalog_image()
        struct.pack_into(">Q", data, 3072 + 272, 3072)
        struct.pack_into(">III", data, 3072 + 284, 6, 4, 6)
        struct.pack_into(">HI", data, 4096 + 14, 2, 2)
        struct.pack_into(">I", data, 4096 + 36, 3)
        node = 6144
        data[node + 8:node + 10] = b"\0\x02"
        struct.pack_into(">H", data, node + 10, 1)
        struct.pack_into(">H", data, node + 1022, 14)
        struct.pack_into(">H", data, node + 1020, 28)
        struct.pack_into(">H I 2H I", data, node + 14, 8, 2, 1, ord("a"), 1)
        return data

    def test_late_invalid_index_node_leaves_earlier_leaf_unchanged(self):
        node = 6144
        cases = {
            "index height": (node + 9, "B", 1),
            "index table bounds": (node + 1022, "H", 2048),
            "index key bounds": (node + 14, "H", 1024),
            "truncated child pointer": (node + 1020, "H", 26),
            "child outside catalog": (node + 24, "I", 3),
        }
        for name, (offset, fmt, value) in cases.items():
            with self.subTest(layout=name):
                data = self.indexed_catalog_image()
                struct.pack_into(">" + fmt, data, offset, value)
                self.assert_rejected_unchanged(data)

    def test_valid_index_node_is_preserved(self):
        data = self.indexed_catalog_image()
        expected = bytearray(data)
        for key in (5120 + 14, 5120 + 272):
            expected[key + 10 + 48:key + 10 + 56] = bytes(8)
        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / "catalog.bin"
            image.write_bytes(data)
            package.clear_xorriso_finder_info(image)
            self.assertEqual(image.read_bytes(), expected)

    def test_truncated_images_are_rejected_without_any_edits(self):
        data = self.catalog_image()
        for length in (0, 1, 2, 3, 4, 511, 512, 516, 520, 524, 560, 570, 1023,
                       3072, 3076, 3112, 3344, 3360, 3424, 3583, 4096, 4128,
                       4130, 5119, 5120, 5192, 5392, 6143, 6144, 8191):
            with self.subTest(length=length):
                self.assert_rejected_unchanged(data[:length])

    def test_late_unexpected_finder_metadata_leaves_all_bytes_unchanged(self):
        data = self.catalog_image()
        data[5120 + 272 + 10 + 48] = ord("X")
        self.assert_rejected_unchanged(data)


class PackagingTests(unittest.TestCase):
    def test_release_bundle_declares_its_service_and_resources(self):
        baseline = json.loads((ROOT / "apps/desktop/src-tauri/tauri.conf.json").read_text())
        self.assertEqual(baseline["bundle"], {"active": False, "icon": []})
        overlay = ROOT / "apps/desktop/src-tauri/tauri.release.conf.json"
        self.assertTrue(overlay.exists(), "generated inputs belong only to release builds")
        release = json.loads(overlay.read_text())
        self.assertNotIn("app", release, "release must inherit the restrictive WebView posture")
        self.assertEqual(release["build"]["beforeBuildCommand"],
                         "python3 ../../scripts/package.py prepare")
        config = baseline | release
        self.assertTrue(config["bundle"]["active"], "personal installation needs a bundle")
        self.assertEqual(config["bundle"]["targets"], ["app"])
        self.assertEqual(config["bundle"]["externalBin"], ["binaries/kanban-service", "binaries/kanban-mcp"])
        self.assertTrue(config["bundle"]["icon"])
        self.assertIn("resources/build-identity.json", config["bundle"]["resources"])
        self.assertNotIn("herdr", json.dumps(config["bundle"]))
        self.assertFalse(config["app"]["withGlobalTauri"])
        self.assertNotIn("localhost:1420", config["app"]["security"]["csp"])


    def test_release_environment_pins_identity_and_remaps_both_compilers(self):
        self.assertTrue(hasattr(package, "release_environment"), "missing controlled build environment")
        identity = {"version": "0.1.0", "source_revision": "a" * 40, "source_epoch": 1700000000}
        environment = package.release_environment(ROOT, identity, "aarch64-apple-darwin", {
            "PATH": "/usr/bin", "HOME": "/Users/fixture", "CARGO_HOME": "/cache/cargo",
            "RUSTFLAGS": "-C opt-level=0", "SOURCE_DATE_EPOCH": "1",
            "CARGO_ENCODED_RUSTFLAGS": "inherited", "CARGO_TARGET_DIR": "/wrong",
        })
        self.assertEqual(environment["KANBAN_SOURCE_REVISION"], "a" * 40)
        self.assertEqual(environment["SOURCE_DATE_EPOCH"], "1700000000")
        self.assertEqual(environment["KANBAN_PACKAGE_TARGET"], "aarch64-apple-darwin")
        self.assertEqual(environment["CARGO_TARGET_DIR"], str(ROOT / "target/package-build"))
        self.assertNotIn("RUSTFLAGS", environment)
        self.assertEqual(environment["CARGO_INCREMENTAL"], "0")
        self.assertEqual(environment["ZERO_AR_DATE"], "1")
        self.assertIn(f"--remap-path-prefix={ROOT}=/kanban", environment["CARGO_ENCODED_RUSTFLAGS"])
        self.assertIn("--remap-path-prefix=/cache/cargo=/cargo-home", environment["CARGO_ENCODED_RUSTFLAGS"])
        self.assertIn(f"-ffile-prefix-map={ROOT}=/kanban", environment["CFLAGS"])
        self.assertEqual(environment["CFLAGS"], environment["CXXFLAGS"])


    def test_source_identity_matches_git_and_rust_workspace(self):
        self.assertTrue(hasattr(package, "source_identity"), "missing source identity")
        identity, dirty = package.source_identity(ROOT, allow_dirty=True)
        revision, epoch = subprocess.check_output(
            ["git", "log", "-1", "--format=%H %ct"], cwd=ROOT, text=True).split()
        self.assertEqual(identity, {"version": "0.1.0", "source_revision": revision, "source_epoch": int(epoch)})
        self.assertIsInstance(dirty, bool)
        if dirty:
            with self.assertRaisesRegex(ValueError, "dirty"):
                package.source_identity(ROOT)


    def test_ordinary_check_runs_only_portable_packaging_tests(self):
        recipes = (ROOT / "justfile").read_text()
        check = recipes.split("check: need-rust need-web\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("    just check-packaging\n", check + "\n")
        self.assertNotIn("check-packaging-native", check)
        self.assertNotIn("check-packaging-build", check)
        packaging = recipes.split("check-packaging:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("scripts/test_package_inputs.py", packaging)
        self.assertIn("scripts/test_packaging_path_dependencies.py", packaging)


    def test_udif_normalization_changes_only_its_unsigned_segment_id(self):
        self.assertTrue(hasattr(package, "normalize_udif"), "missing validated UDIF boundary")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.dmg"
            footer = bytearray(512)
            footer[:12] = b"koly" + struct.pack(">II", 4, 512)
            struct.pack_into(">II", footer, 56, 1, 1)
            footer[64:80] = b"x" * 16
            before = b"unit fixture payload" + footer
            path.write_bytes(before)
            package.normalize_udif(path, bytes(range(32)))
            after = path.read_bytes()
            self.assertEqual(after[:-448], before[:-448])
            self.assertEqual(after[-448:-432], bytes(range(16)))
            self.assertEqual(after[-432:], before[-432:])
            path.write_bytes(b"not a disk image" + bytes(512))
            with self.assertRaisesRegex(ValueError, "trailer"):
                package.normalize_udif(path, bytes(32))
            struct.pack_into(">II", footer, 56, 1, 2)
            path.write_bytes(b"unit fixture payload" + footer)
            with self.assertRaisesRegex(ValueError, "single-segment"):
                package.normalize_udif(path, bytes(32))

    def test_app_archive_contains_every_byte_with_stable_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archives = []
            for index in range(2):
                source = root / str(index) / "Kanban.app"
                source.mkdir(parents=True)
                for name in (["binary", "resource"] if index == 0 else ["resource", "binary"]):
                    path = source / name
                    path.write_bytes(b"fixture payload ????????\0\0")
                    path.chmod(0o700 if name == "binary" else 0o600)
                    os.utime(path, (100 + index, 100 + index))
                (source / "link").symlink_to("resource")
                normalized = root / f"normalized-{index}" / "Kanban.app"
                package.normalized_copy(source, normalized, 1700000000)
                archive = root / f"{index}.tar"
                package.make_app_tar(normalized, archive, 1700000000)
                archives.append(archive.read_bytes())
                with tarfile.open(archive) as tar:
                    self.assertEqual(tar.getmember("Kanban.app/binary").mode, 0o755)
                    self.assertEqual(tar.getmember("Kanban.app/resource").mode, 0o644)
                    self.assertEqual(tar.getmember("Kanban.app/link").linkname, "resource")
                    for item in tar.getmembers():
                        self.assertEqual((item.uid, item.gid, item.mtime), (0, 0, 1700000000))
                    self.assertEqual(tar.extractfile("Kanban.app/resource").read(), b"fixture payload ????????\0\0")
            self.assertEqual(archives[0], archives[1])


    def test_native_compiler_remapping_preserves_paths_with_spaces(self):
        root = Path("/checkout with spaces")
        identity = {"version": "0.1.0", "source_revision": "a" * 40, "source_epoch": 1700000000}
        environment = package.release_environment(root, identity, "aarch64-apple-darwin", {})
        self.assertIn("-ffile-prefix-map=/checkout with spaces=/kanban", shlex.split(environment["CFLAGS"]))
        self.assertEqual(environment["CC_SHELL_ESCAPED_FLAGS"], "1")


if __name__ == "__main__":
    unittest.main()
