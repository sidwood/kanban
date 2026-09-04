//! Configuration-specific guards for `KANBAN_CORE_BIN`: debug and test
//! builds honour the override; release builds resolve only the trusted
//! packaged core beside the shell (DR-SS-09).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use kanban_desktop_lib::locate_core_binary;

/// Env-var tests share one lock: std::env::set_var races otherwise.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const RELEASE_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/release_path_resolution/kanban-service"
);

const MALICIOUS_OVERRIDE: &str = "/malicious/kanban-service-override";

/// The release probe is built outside the test crate graph so it
/// carries `not(debug_assertions)` path resolution.
fn release_probe_exe() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--bin",
            "core-binary-probe",
            "--manifest-path",
        ])
        .arg(&manifest)
        .arg("-q")
        .status()
        .expect("the release probe builds");
    assert!(status.success(), "the release probe must compile");
    Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release/core-binary-probe")
}

/// Lay out a shell directory with no packaged core beside the probe.
fn shell_without_core(dir: &Path, probe: &Path) -> PathBuf {
    let shell = dir.join("kanban-desktop");
    std::fs::copy(probe, &shell).expect("the probe copies into the fixture");
    let mut permissions = std::fs::metadata(&shell)
        .expect("the shell binary exists")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&shell, permissions).expect("the shell binary is executable");
    shell
}

/// Lay out a packaged shell directory: the probe stands in for the
/// desktop binary and the fixture names the trusted core beside it.
fn packaged_shell_dir(dir: &Path, probe: &Path) -> PathBuf {
    let shell = dir.join("kanban-desktop");
    std::fs::copy(probe, &shell).expect("the probe copies into the fixture");
    let mut permissions = std::fs::metadata(&shell)
        .expect("the shell binary exists")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&shell, permissions).expect("the shell binary is executable");
    std::fs::copy(RELEASE_FIXTURE, dir.join("kanban-service"))
        .expect("the trusted core fixture copies beside the shell");
    shell
}

#[test]
#[cfg(debug_assertions)]
fn core_binary_debug_honours_kanban_core_bin_override() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Safety: the lock above serialises every env-writing test.
    unsafe { std::env::set_var("KANBAN_CORE_BIN", MALICIOUS_OVERRIDE) };
    let located = locate_core_binary().expect("the override resolves");
    assert_eq!(
        located,
        PathBuf::from(MALICIOUS_OVERRIDE),
        "debug builds honour KANBAN_CORE_BIN"
    );
    // Safety: still under the lock.
    unsafe { std::env::remove_var("KANBAN_CORE_BIN") };
}

#[test]
fn core_binary_release_ignores_kanban_core_bin_override() {
    let probe = release_probe_exe();
    let dir = tempfile::tempdir().expect("a scratch directory is available");
    let shell = packaged_shell_dir(dir.path(), &probe);
    let trusted = dir.path().join("kanban-service");

    let output = Command::new(&shell)
        .env("KANBAN_CORE_BIN", MALICIOUS_OVERRIDE)
        .output()
        .expect("the release probe runs");

    assert!(
        output.status.success(),
        "the release probe resolves a trusted core: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let resolved = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    assert_eq!(
        resolved,
        trusted.display().to_string(),
        "release builds ignore KANBAN_CORE_BIN and resolve the packaged core"
    );
    assert_ne!(
        resolved, MALICIOUS_OVERRIDE,
        "the override must not win in release"
    );
}

#[test]
fn core_binary_release_fails_without_packaged_core() {
    let probe = release_probe_exe();
    let dir = tempfile::tempdir().expect("a scratch directory is available");
    let shell = shell_without_core(dir.path(), &probe);

    let output = Command::new(&shell)
        .output()
        .expect("the release probe runs");

    assert!(
        !output.status.success(),
        "release builds must not fall back to the workspace debug core"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no kanban-service binary found"),
        "expected a locate failure, got: {stderr}"
    );
}
