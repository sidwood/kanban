"""Owned build-input staging; source identity never comes from a synthetic repository."""
# cspell:words CREAT NOFOLLOW RDWR fstat getuid lstat nlink rustc
from contextlib import contextmanager
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess

# Cargo hashes external-workspace dependency paths before rustc remapping.
STAGING_BASE = Path("/private/tmp/kanban-package-inputs")


def capture_source(root, revision, dirty):
    files = {}
    if dirty:
        names = subprocess.check_output(
            ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=root)
        for name in sorted(set(filter(None, os.fsdecode(names).split("\0")))):
            path = root / name
            try:
                info = path.lstat()
            except FileNotFoundError:
                continue
            if not stat.S_ISREG(info.st_mode) or path.resolve() != root.resolve() / name:
                raise ValueError(f"unsupported build input: {name}")
            files[name] = ("100755" if info.st_mode & 0o111 else "100644", path.read_bytes())
    else:
        listing = subprocess.check_output(["git", "ls-tree", "-rz", revision], cwd=root)
        entries = []
        for line in filter(None, listing.split(b"\0")):
            header, name = line.split(b"\t", 1)
            mode, kind, oid = header.split()
            if kind != b"blob" or mode not in (b"100644", b"100755"):
                raise ValueError(f"unsupported build input: {os.fsdecode(name)}")
            entries.append((os.fsdecode(name), mode.decode(), oid))
        stream = io.BytesIO(subprocess.check_output(
            ["git", "cat-file", "--batch"], cwd=root,
            input=b"".join(oid + b"\n" for _, _, oid in entries)))
        algorithm = subprocess.check_output(["git", "rev-parse", "--show-object-format"], cwd=root, text=True).strip()
        for name, mode, expected in entries:
            oid, kind, size = stream.readline().split()
            content = stream.read(int(size))
            actual = hashlib.new(algorithm, b"blob " + size + b"\0" + content).hexdigest().encode()
            if oid != expected or kind != b"blob" or actual != expected or stream.read(1) != b"\n":
                raise ValueError(f"Git source object verification failed: {name}")
            files[name] = (mode, content)
        if stream.read():
            raise ValueError("unexpected Git source object data")
    return files


def input_manifest(files, identity):
    entries = []
    for name, (mode, content) in sorted(files.items()):
        path = Path(name)
        if (not name or path.is_absolute() or str(path) != name
                or any(part in ("..", ".git") for part in path.parts)
                or mode not in ("100644", "100755")):
            raise ValueError(f"unsupported build input: {name}")
        entries.append({"path": name, "mode": mode, "sha256": hashlib.sha256(content).hexdigest()})
    return {"identity": identity, "files": entries}


def input_digest(manifest):
    return hashlib.sha256(json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def verify_inputs(root, environment):
    try:
        manifest = json.loads((root.parent / "inputs.json").read_text())
    except (OSError, ValueError) as error:
        raise ValueError("release hook requires verified staged inputs from just package") from error
    digest = input_digest(manifest)
    identity = manifest["identity"]
    if (root.name != "source" or root.parent.name != digest
            or environment.get("KANBAN_PACKAGE_INPUT_SHA256") != digest
            or environment.get("KANBAN_SOURCE_REVISION") != identity["source_revision"]
            or environment.get("SOURCE_DATE_EPOCH") != str(identity["source_epoch"])):
        raise ValueError("release hook input identity disagrees with the controlled environment")
    files = {}
    input_manifest({entry["path"]: (entry["mode"], b"") for entry in manifest["files"]}, identity)
    for entry in manifest["files"]:
        path = root / entry["path"]
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or path.resolve() != root.resolve() / entry["path"]:
            raise ValueError(f"staged source type changed: {entry['path']}")
        files[entry["path"]] = ("100755" if info.st_mode & 0o111 else "100644", path.read_bytes())
    if input_manifest(files, identity) != manifest:
        raise ValueError("staged source bytes or modes changed")
    return identity


@contextmanager
def staged_inputs(files, identity, base=STAGING_BASE):
    manifest = input_manifest(files, identity)
    digest = input_digest(manifest)
    base.mkdir(mode=0o700, exist_ok=True)
    info = base.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
        raise ValueError("build staging requires an owned private directory")
    base = base.resolve()
    descriptor = os.open(base / "lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "r+b") as lock:
        info = os.fstat(lock.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or stat.S_IMODE(info.st_mode) != 0o600 or info.st_nlink != 1):
            raise ValueError("build staging requires an owned private lock")
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ValueError("build staging is busy; retry after the active package build") from error
        directory = base / digest
        # Never reuse targets, or delete a previous invocation's interrupted work.
        directory.mkdir(mode=0o700)
        try:
            root = directory / "source"
            root.mkdir()
            for name, (mode, content) in sorted(files.items()):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content)
                path.chmod(0o755 if mode == "100755" else 0o644)
            (directory / "inputs.json").write_text(json.dumps(manifest, sort_keys=True, indent=2) + "\n")
            yield root, {"input_sha256": digest, "build_root": str(root), "fresh_targets": True,
                         "manifest": manifest}
        finally:
            shutil.rmtree(directory)
