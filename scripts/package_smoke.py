#!/usr/bin/env python3
"""Mount and test an installed personal preview without development variables."""
# cspell:words codesign hdiutil mountpoint nobrowse fsencode
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

KEYCHAIN_SERVICE = "dev.kanban.desktop.installation"


def run(*arguments, **kwargs):
    return subprocess.run([str(value) for value in arguments], check=True,
                          capture_output=True, text=True, timeout=60, **kwargs)


def credential_status(account):
    result = subprocess.run(
        ["/usr/bin/security", "find-generic-password", "-s", KEYCHAIN_SERVICE, "-a", account],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
    if result.returncode not in (0, 44):
        raise RuntimeError(f"native fixture lookup failed ({result.returncode})")
    return result.returncode


def remove_owned_credential(account):
    if not re.fullmatch(r"data-[0-9a-f]{64}", account):
        raise ValueError("refusing to remove a non-fixture account")
    if credential_status(account) == 0:
        run("/usr/bin/security", "delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a", account)
    if credential_status(account) != 44:
        raise RuntimeError("disposable native credential was not removed")


def require_stopped_data(data):
    if not (data / "kanban.sqlite").is_file() or (data / "core.sock").exists():
        raise RuntimeError("installed probe did not create selected data and stop its socket")


def installed_identity(application):
    return json.loads((application / "Contents/Resources/resources/build-identity.json").read_text())


def installation_directory():
    # macOS's default per-user temporary root can exceed the Unix socket limit.
    return tempfile.TemporaryDirectory(prefix="kanban-installed-", dir="/tmp")


def smoke(image):
    image = image.resolve()
    if not image.is_file():
        raise ValueError("disk image is missing")
    # Security.framework needs the logged-in user's HOME to find native Keychain.
    # The fresh explicit data directory selects only a disposable data-* account.
    environment = {"HOME": os.environ["HOME"], "PATH": "/usr/bin:/bin"}
    run("/usr/bin/hdiutil", "verify", image)
    with installation_directory() as directory:
        root = Path(directory).resolve()
        mount = root / "volume"
        mount.mkdir()
        run("/usr/bin/hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", mount, image)
        try:
            application = root / "Applications/Kanban.app"
            application.parent.mkdir()
            run("/usr/bin/ditto", mount / "Kanban.app", application)
        finally:
            run("/usr/bin/hdiutil", "detach", mount)
        run("/usr/bin/codesign", "--verify", "--deep", "--strict", application)
        executable = application / "Contents/MacOS/kanban-desktop"
        service = application / "Contents/MacOS/kanban-service"
        adapter = application / "Contents/MacOS/kanban-mcp"
        for binary in (executable, service, adapter):
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise RuntimeError(f"required bundled executable is missing: {binary.name}")
        identity = json.loads(run(executable, "--version", env=environment, cwd=root).stdout)
        service_identity = json.loads(run(service, "--version", env=environment, cwd=root).stdout)
        if identity != service_identity or identity != installed_identity(application):
            raise RuntimeError("installed shell, service and resource identities disagree")
        data = root / "data"
        account = "data-" + hashlib.sha256(os.fsencode(data)).hexdigest()
        if credential_status(account) != 44:
            raise RuntimeError("refusing to reuse an existing Keychain account")
        try:
            # The application, not this harness, creates the fresh data directory.
            result = run(executable, "--package-smoke", data, env=environment, cwd=root)
            report = json.loads(result.stdout)
            if report.get("identity") != identity or report.get("stopped") is not True:
                raise RuntimeError("installed probe did not prove identity and graceful stop")
            if report.get("service_binary") != str(service):
                raise RuntimeError("installed shell resolved a non-bundled service")
            health = report.get("health", {})
            if not health.get("connected") or not health.get("herdr", {}).get("connection_diagnostic"):
                raise RuntimeError("installed probe omitted service or Herdr prerequisite health")
            require_stopped_data(data)
            if credential_status(account) != 0:
                raise RuntimeError("installed service did not use the scoped native Keychain account")
        finally:
            remove_owned_credential(account)
        return {"passed": True, "image": str(image), "identity": identity,
                "probe": report, "native_credential_removed": True,
                "environment": {"PATH": environment["PATH"], "HOME": "native user Keychain"},
                "temporary_installation_removed": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dmg", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    report = {"passed": False, "stage": "running"}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report) + "\n")
    try:
        report = smoke(args.dmg)
    except (ValueError, OSError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        report = {"passed": False, "error": str(error)}
        args.report.write_text(json.dumps(report, indent=2) + "\n")
        parser.exit(1, f"installed package smoke: {error}\n")
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
