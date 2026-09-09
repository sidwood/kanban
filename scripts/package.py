#!/usr/bin/env python3
"""Personal macOS packaging; no publication or trusted signing credentials."""
# cspell:words CFLAGS CXXFLAGS RUSTFLAGS IDAT IEND IHDR UDIF UDZO calcsize codesign ffile gname hfsplus koly libisofs mkisofs mountpoint nobrowse rustc unnotarized vers xattr xcrun xorriso
import argparse
from datetime import datetime, timezone
import hashlib
import mmap
import os
from pathlib import Path
import shutil
import shlex
import struct
import subprocess
import tempfile
import tarfile
import platform
import json
import re
import tomllib

from package_inputs import capture_source, staged_inputs, verify_inputs

ROOT = Path(__file__).resolve().parents[1]


def run(*args, **kwargs):
    print("+ " + " ".join(str(arg) for arg in args), flush=True)
    return subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def source_identity(root, allow_dirty=False):
    revision, epoch = subprocess.check_output(
        ["git", "log", "-1", "--format=%H %ct"], cwd=root, text=True).split()
    if not re.fullmatch(r"[0-9a-f]{40}", revision) or not epoch.isdecimal():
        raise ValueError("Git must provide a full revision and decimal source epoch")
    status = subprocess.check_output(
        ["git", "status", "--porcelain", "--untracked-files=all"], cwd=root, text=True)
    dirty = bool(status.strip())
    if dirty and not allow_dirty:
        raise ValueError("dirty checkout: commit source first, or use --allow-dirty for local tooling tests only")
    version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    shell = root / "apps/desktop/src-tauri"
    versions = [version, tomllib.loads((shell / "Cargo.toml").read_text())["package"]["version"],
                json.loads((shell / "tauri.conf.json").read_text())["version"],
                json.loads((root / "apps/desktop/package.json").read_text())["version"]]
    if len(set(versions)) != 1:
        raise ValueError(f"package versions disagree: {versions}")
    return {"version": version, "source_revision": revision, "source_epoch": int(epoch)}, dirty


def release_environment(root, identity, target, inherited):
    environment = dict(inherited)
    environment.pop("RUSTFLAGS", None)
    cargo_home = Path(environment.get("CARGO_HOME", str(Path.home() / ".cargo"))).resolve()
    mappings = [(root.resolve(), "/kanban"), (cargo_home, "/cargo-home")]
    environment.update({
        "KANBAN_SOURCE_REVISION": identity["source_revision"],
        "SOURCE_DATE_EPOCH": str(identity["source_epoch"]),
        "KANBAN_PACKAGE_TARGET": target,
        "CARGO_TARGET_DIR": str(root / "target/package-build"),
        "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(f"--remap-path-prefix={a}={b}" for a, b in mappings),
        "CFLAGS": shlex.join(f"-ffile-prefix-map={a}={b}" for a, b in mappings),
        "CXXFLAGS": shlex.join(f"-ffile-prefix-map={a}={b}" for a, b in mappings),
        "CC_SHELL_ESCAPED_FLAGS": "1",
        "CARGO_INCREMENTAL": "0", "ZERO_AR_DATE": "1", "TZ": "UTC", "LC_ALL": "C",
    })
    return environment


def make_icon(source, destination):
    with tempfile.TemporaryDirectory(prefix="kanban-icon-") as directory:
        png = Path(directory) / "icon.png"
        run("sips", "-z", "512", "512", source, "--out", png)
        raw = png.read_bytes()
    if raw[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("sips did not produce a PNG")
    image = raw[:8]
    position = 8
    while position < len(raw):
        size = struct.unpack_from(">I", raw, position)[0] + 12
        if raw[position + 4:position + 8] in (b"IHDR", b"IDAT", b"IEND"):
            image += raw[position:position + size]
        position += size
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(b"icns" + struct.pack(">I", len(image) + 16)
                            + b"ic09" + struct.pack(">I", len(image) + 8) + image)


def normalized_copy(source, destination, epoch):
    shutil.copytree(source, destination, symlinks=True, copy_function=shutil.copyfile)
    for path in sorted([destination, *destination.rglob("*")]):
        original = source / path.relative_to(destination)
        if not path.is_symlink():
            path.chmod(0o755 if path.is_dir() or original.stat().st_mode & 0o111 else 0o644)
        os.utime(path, (epoch, epoch), follow_symlinks=False)


def clear_xorriso_finder_info(image):
    # libisofs 1.5.8 invents ????/???? Finder codes rejected by strict codesign.
    # Parse the catalog, never replace matching bytes in application file data.
    with image.open("r+b") as stream, mmap.mmap(stream.fileno(), 0) as data:
        def bounds(offset, size, start, end, label):
            if not start <= offset <= end or not 0 <= size <= end - offset:
                raise ValueError(f"invalid HFS+ {label} bounds")

        def number(offset, fmt="I"):
            bounds(offset, struct.calcsize(">" + fmt), 0, len(data), "field")
            return struct.unpack_from(">" + fmt, data, offset)[0]

        if data[:2] != b"ER":
            raise ValueError("expected xorriso Apple partition map")
        sector = number(2, "H")
        if sector < 512 or sector & (sector - 1):
            raise ValueError("invalid Apple partition sector size")
        bounds(0, 2 * sector, 0, len(data), "partition map header")
        count = number(sector + 4)
        if not count:
            raise ValueError("empty Apple partition map")
        bounds(sector, count * sector, 0, len(data), "partition map")
        partitions = []
        for index in range(1, count + 1):
            entry = sector * index
            if data[entry:entry + 2] != b"PM" or number(entry + 4) != count:
                raise ValueError("invalid Apple partition map entry")
            start = sector * number(entry + 8)
            size = sector * number(entry + 12)
            if not size:
                raise ValueError("empty Apple partition")
            bounds(start, size, sector, len(data), "partition")
            if data[entry + 48:entry + 80].rstrip(b"\0") == b"Apple_HFS":
                bounds(start, size, sector * (count + 1), len(data), "HFS+ partition")
                partitions.append((start, start + size))
        if len(partitions) != 1:
            raise ValueError("expected exactly one HFS+ partition")
        volume, partition_end = partitions[0]
        header = volume + 1024
        bounds(header, 512, volume, partition_end, "volume header")
        if data[header:header + 4] != b"H+\0\x04":
            raise ValueError("expected HFS+ volume version 4")
        block_size = number(header + 40)
        if block_size < 512 or block_size & (block_size - 1):
            raise ValueError("invalid HFS+ allocation block size")
        volume_size = number(header + 44) * block_size
        bounds(volume, volume_size, volume, partition_end, "volume")
        bounds(header, 512, volume, volume + volume_size, "volume header")
        logical_size = number(header + 272, "Q")
        start, blocks = number(header + 288), number(header + 292)
        if (not logical_size or logical_size > blocks * block_size
                or number(header + 284) != blocks or any(data[header + 296:header + 352])):
            raise ValueError("fragmented HFS+ catalogs are not supported")
        catalog = volume + start * block_size
        bounds(catalog, blocks * block_size, header + 512, volume + volume_size - 1024, "catalog extent")
        bounds(catalog, 34, catalog, catalog + logical_size, "catalog header")
        node_size = number(catalog + 32, "H")
        if node_size < 512 or node_size & (node_size - 1) or logical_size % node_size:
            raise ValueError("invalid HFS+ catalog node size")
        total_nodes = logical_size // node_size
        if number(catalog + 36) != total_nodes or number(catalog + 40) > total_nodes:
            raise ValueError("invalid HFS+ catalog node count")
        if any(number(catalog + offset) >= total_nodes for offset in (16, 24, 28)):
            raise ValueError("invalid HFS+ catalog node reference")
        if number(catalog + 52) & 6 != 6:
            raise ValueError("unsupported HFS+ catalog key format")
        edits = []
        for node in range(catalog, catalog + logical_size, node_size):
            bounds(node, node_size, catalog, catalog + logical_size, "node")
            kind, height = data[node + 8], data[node + 9]
            if (kind not in (0, 1, 2, 255) or (kind == 1) != (node == catalog)
                    or (kind in (1, 2) and height != 0) or (kind == 255 and height != 1)
                    or (kind == 0 and height < 2)):
                raise ValueError("invalid HFS+ catalog node descriptor")
            if number(node) >= total_nodes or number(node + 4) >= total_nodes:
                raise ValueError("invalid HFS+ catalog node link")
            records = number(node + 10, "H")
            table = node + node_size - 2 * (records + 1)
            bounds(table, 2 * (records + 1), node + 14, node + node_size, "record table")
            offsets = [node + number(node + node_size - 2 * (index + 1), "H")
                       for index in range(records + 1)]
            if (offsets[0] != node + 14
                    or any(offset % 2 or not node + 14 <= offset <= table for offset in offsets)
                    or any(left >= right for left, right in zip(offsets, offsets[1:]))):
                raise ValueError("invalid HFS+ record offsets")
            if kind == 1:
                if offsets != [node + 14, node + 120, node + 248, table]:
                    raise ValueError("invalid HFS+ catalog header records")
                continue
            if kind == 2:
                if records != 1:
                    raise ValueError("invalid HFS+ catalog map records")
                continue
            for key, end in zip(offsets, offsets[1:]):
                bounds(key, 8, key, end, "catalog key")
                key_size = number(key, "H")
                if not 6 <= key_size <= 516 or key_size != 6 + 2 * number(key + 6, "H"):
                    raise ValueError("invalid HFS+ catalog key length")
                bounds(key, 2 + key_size, key, end, "catalog key")
                record = key + 2 + key_size
                if kind == 0:
                    bounds(record, 4, key, end, "index child")
                    if not 0 < number(record) < total_nodes:
                        raise ValueError("invalid HFS+ index child node")
                    continue
                bounds(record, 2, key, end, "record type")
                record_type = number(record, "H")
                if record_type in (3, 4):
                    bounds(record, 10, key, end, "thread record")
                    name_size = number(record + 8, "H")
                    if name_size > 255:
                        raise ValueError("invalid HFS+ thread name length")
                    bounds(record, 10 + 2 * name_size, key, end, "thread name")
                    continue
                if record_type not in (1, 2):
                    raise ValueError("unsupported HFS+ catalog record type")
                bounds(record, 88 if record_type == 1 else 248, key, end, "catalog record")
                if record_type != 2:
                    continue
                mode = number(record + 42, "H")
                if mode & 0o170000 != 0o100000:
                    continue
                finder = record + 48
                if data[finder:finder + 32] == b"????????" + bytes(24):
                    edits.append(finder)
                elif any(data[finder:finder + 32]):
                    raise ValueError("unexpected Finder metadata in xorriso catalog")
        # A late invalid record must not leave an earlier edit in the image.
        for finder in edits:
            data[finder:finder + 8] = bytes(8)
        print(f"HFS_FINDER_DEFAULTS_CLEARED {len(edits)}", flush=True)


def tree_manifest(root):
    entries = []
    for path in sorted([root, *root.rglob("*")]):
        entry = {"path": str(path.relative_to(root)), "mode": path.lstat().st_mode & 0o777}
        if path.is_symlink():
            entry.update({"type": "link", "target": os.readlink(path)})
        elif path.is_file():
            entry.update({"type": "file", "sha256": sha256(path)})
        elif path.is_dir():
            entry["type"] = "directory"
        else:
            raise ValueError(f"unsupported bundle entry: {path}")
        entries.append(entry)
    return entries


def verify_dmg_mount(image, expected_app):
    with tempfile.TemporaryDirectory(prefix="kanban-mount-") as directory:
        mount = Path(directory) / "volume"
        mount.mkdir()
        run("hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", mount, image)
        try:
            app = mount / expected_app.name
            if tree_manifest(app) != tree_manifest(expected_app):
                raise ValueError("mounted app differs from packaged app")
            if (mount / "Applications").readlink() != Path("/Applications"):
                raise ValueError("missing Applications installation link")
            run("codesign", "--verify", "--deep", "--strict", app)
            print("DMG_MOUNT_VERIFIED", flush=True)
        finally:
            run("hdiutil", "detach", mount)


def normalize_udif(output, image_hash):
    with output.open("r+b") as stream:
        stream.seek(-512, os.SEEK_END)
        footer = stream.read(512)
        if footer[:12] != b"koly" + struct.pack(">II", 4, 512):
            raise ValueError("unrecognized UDIF trailer; refusing to normalize")
        if struct.unpack_from(">II", footer, 56) != (1, 1):
            raise ValueError("only single-segment UDIF images are supported")
        # UDIF's random segment UUID is not covered by its data checksums.
        stream.seek(-512 + 64, os.SEEK_END)
        stream.write(image_hash[:16])


def make_dmg(app, output, epoch):
    app, output = app.resolve(), output.resolve()
    if not app.is_dir() or app.suffix != ".app":
        raise ValueError("--app must be an existing .app directory")
    if output.exists():
        raise ValueError(f"refusing to overwrite {output}")
    run("codesign", "--verify", "--deep", "--strict", app)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="kanban-package-") as directory:
        temporary = Path(directory)
        stage = temporary / "stage"
        stage.mkdir()
        normalized_copy(app, stage / app.name, epoch)
        (stage / "Applications").symlink_to("/Applications")
        os.utime(stage / "Applications", (epoch, epoch), follow_symlinks=False)
        os.utime(stage, (epoch, epoch))
        date = datetime.fromtimestamp(epoch, timezone.utc).strftime("%Y%m%d%H%M%S00")
        image = temporary / "payload.iso"
        run("xorriso", "-no_rc", "-as", "mkisofs", "-hfsplus", "-hfsplus-serial-no",
            "4b616e62616e0001", "-R", "-uid", "0", "-gid", "0", "-V", "Kanban",
            "-preparer", "Kanban", f"--modification-date={date}",
            "--set_all_file_dates", date, "-o", image, stage,
            env=os.environ | {"TZ": "UTC", "LC_ALL": "C", "SOURCE_DATE_EPOCH": str(epoch)})
        clear_xorriso_finder_info(image)
        run("hdiutil", "convert", image, "-format", "UDZO", "-tasks", "1", "-o", output)
        normalize_udif(output, bytes.fromhex(sha256(image)))
        run("hdiutil", "verify", output)
        verify_dmg_mount(output, stage / app.name)
    print(f"DMG_SHA256 {sha256(output)} {output}")


def prepare():
    identity = verify_inputs(ROOT, os.environ)
    target = os.environ.get("KANBAN_PACKAGE_TARGET", "")
    if target not in ("aarch64-apple-darwin", "x86_64-apple-darwin"):
        raise ValueError("release hook requires the controlled environment from just package")
    environment = release_environment(ROOT, identity, target, os.environ)
    shell = ROOT / "apps/desktop/src-tauri"
    run("cargo", "build", "--locked", "--release", "--target", target,
        "-p", "kanban-service", "-p", "kanban-mcp", cwd=ROOT, env=environment)
    binaries = shell / "binaries"
    binaries.mkdir(exist_ok=True)
    for name in ("kanban-service", "kanban-mcp"):
        binary = Path(environment["CARGO_TARGET_DIR"]) / target / "release" / name
        destination = binaries / f"{name}-{target}"
        shutil.copyfile(binary, destination)
        destination.chmod(0o755)
    actual = json.loads(subprocess.check_output([str(binaries / f"kanban-service-{target}"), "--version"]))
    if actual != identity:
        raise ValueError(f"built service identity disagrees: {actual} != {identity}")
    resource = shell / "resources/build-identity.json"
    resource.parent.mkdir(exist_ok=True)
    resource.write_text(json.dumps(identity, sort_keys=True, indent=2) + "\n")
    make_icon(shell / "icons/icon.png", shell / "icons/icon.icns")
    run("pnpm", "--filter", "desktop", "run", "build:web", cwd=ROOT, env=environment)


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def make_app_tar(app, output, epoch):
    with tarfile.open(output, "w", format=tarfile.PAX_FORMAT) as archive:
        for path in sorted([app, *app.rglob("*")]):
            info = archive.gettarinfo(str(path), str(path.relative_to(app.parent)))
            info.uid = info.gid = 0
            info.uname = info.gname = "root"
            info.mtime = epoch
            info.pax_headers = {}
            if info.isfile():
                with path.open("rb") as stream:
                    archive.addfile(info, stream)
            else:
                archive.addfile(info)


def build(output, allow_dirty=False):
    if platform.system() != "Darwin":
        raise ValueError("personal packaging currently supports macOS only")
    for tool in ("git", "rustc", "cargo", "node", "pnpm", "xcrun", "xorriso", "sips", "hdiutil", "codesign"):
        if shutil.which(tool) is None:
            raise ValueError(f"required build tool is missing: {tool}; see docs/packaging.md")
    identity, dirty = source_identity(ROOT, allow_dirty=allow_dirty)
    rust = subprocess.check_output(["rustc", "-vV"], text=True)
    target = next(line.split(": ", 1)[1] for line in rust.splitlines() if line.startswith("host: "))
    if target not in ("aarch64-apple-darwin", "x86_64-apple-darwin"):
        raise ValueError(f"unsupported native packaging target: {target}")
    output = output.resolve()
    if output.exists() and any(output.iterdir()):
        raise ValueError(f"output directory must be absent or empty: {output}")
    files = capture_source(ROOT, identity["source_revision"], dirty)
    output.mkdir(parents=True, exist_ok=True)
    app = output / "Kanban.app"
    with staged_inputs(files, identity) as (build_root, inputs):
        environment = release_environment(build_root, identity, target, os.environ)
        environment["KANBAN_PACKAGE_INPUT_SHA256"] = inputs["input_sha256"]
        # Personal previews must not consume developer signing or publication secrets.
        environment = {key: value for key, value in environment.items()
                       if not key.startswith(("APPLE_", "TAURI_SIGNING_"))}
        run("pnpm", "install", "--frozen-lockfile", cwd=build_root, env=environment)
        run("pnpm", "--filter", "desktop", "exec", "tauri", "build", "--ci", "--target", target,
            "--bundles", "app", "--config", "src-tauri/tauri.release.conf.json", "--", "--locked",
            cwd=build_root, env=environment)
        built = Path(environment["CARGO_TARGET_DIR"]) / target / "release/bundle/macos/Kanban.app"
        normalized_copy(built, app, identity["source_epoch"])
    (output / "build-inputs.json").write_text(json.dumps(inputs, sort_keys=True, indent=2) + "\n")
    run("xattr", "-cr", app)
    for name in ("kanban-service", "kanban-mcp", "kanban-desktop"):
        run("codesign", "--force", "--sign", "-", "--timestamp=none", "--options", "runtime",
            app / "Contents/MacOS" / name)
    run("codesign", "--force", "--sign", "-", "--timestamp=none", "--options", "runtime", app)
    run("codesign", "--verify", "--deep", "--strict", app)
    for path in [app, *app.rglob("*")]:
        os.utime(path, (identity["source_epoch"], identity["source_epoch"]), follow_symlinks=False)
    make_app_tar(app, output / "Kanban.app.tar", identity["source_epoch"])
    make_dmg(app, output / "Kanban.dmg", identity["source_epoch"])
    report = {
        "identity": identity, "source_dirty": dirty, "target": target,
        "signing": "ad-hoc; unnotarized personal preview",
        "sha256": {name: sha256(output / name) for name in ("Kanban.app.tar", "Kanban.dmg")},
        "toolchain": {"rustc": rust.strip(), "macos": subprocess.check_output(["sw_vers"], text=True).strip(),
                      "node": subprocess.check_output(["node", "--version"], text=True).strip(),
                      "pnpm": subprocess.check_output(["pnpm", "--version"], text=True).strip(),
                      "xorriso": subprocess.check_output(["xorriso", "-version"], text=True, stderr=subprocess.STDOUT).strip(),
                      "sdk": subprocess.check_output(["xcrun", "--show-sdk-version"], text=True).strip()},
    }
    (output / "packaging-report.json").write_text(json.dumps(report, sort_keys=True, indent=2) + "\n")
    print(json.dumps(report, sort_keys=True, indent=2), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    dmg = commands.add_parser("dmg", help="create a deterministic DMG from a signed app")
    dmg.add_argument("--app", type=Path, required=True)
    dmg.add_argument("--output", type=Path, required=True)
    dmg.add_argument("--epoch", type=int, required=True)
    commands.add_parser("prepare", help="Tauri release-only beforeBuildCommand")
    builder = commands.add_parser("build", help="build personal native release artifacts")
    builder.add_argument("--output", type=Path, default=ROOT / "target/package")
    builder.add_argument("--allow-dirty", action="store_true", help="local tooling tests only, not reproducibility evidence")
    args = parser.parse_args()
    try:
        if args.command == "dmg":
            make_dmg(args.app, args.output, args.epoch)
        elif args.command == "prepare":
            prepare()
        else:
            build(args.output, args.allow_dirty)
    except (ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"kanban packaging: {error}\n")


if __name__ == "__main__":
    main()
