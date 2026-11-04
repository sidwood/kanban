mod common;

use std::path::Path;

use common::cargo_target_dir;

const ISOLATED: &str = "target_dir_resolves_a_relative_override_isolated";
const ISOLATED_RELATIVE_MANIFEST: &str = "target_dir_accepts_relative_manifests_isolated";

fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("src-tauri sits three levels below the workspace root")
        .to_path_buf()
}

#[test]
fn target_dir_resolves_a_relative_override_per_manifest() {
    let exe = std::env::current_exe().expect("test executable");
    let output = std::process::Command::new(exe)
        .env("CARGO_TARGET_DIR", "target")
        .env("KANBAN_TARGET_DIR_ISOLATED", "1")
        .args(["--exact", ISOLATED, "--nocapture", "--test-threads=1"])
        .output()
        .expect("spawn isolated target-directory test");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        output.status.success(),
        "isolated target-directory test failed:\n{combined}",
    );
    assert!(
        combined.contains("running 1 test"),
        "child must run exactly one test, output:\n{combined}",
    );
    assert!(
        combined.contains("test result: ok. 1 passed"),
        "child must report exactly one passing test, output:\n{combined}",
    );
}

#[test]
fn target_dir_resolves_a_relative_override_isolated() {
    if std::env::var_os("KANBAN_TARGET_DIR_ISOLATED").is_none() {
        return;
    }
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let shell_manifest = manifest_dir.join("Cargo.toml");
    assert_eq!(
        cargo_target_dir(&shell_manifest),
        manifest_dir.join("target"),
        "a relative CARGO_TARGET_DIR anchors at the manifest's directory",
    );
    let root = workspace_root();
    assert_eq!(
        cargo_target_dir(&root.join("Cargo.toml")),
        root.join("target"),
        "the root workspace manifest resolves against the repository root",
    );
}

#[test]
fn target_dir_accepts_relative_manifests() {
    let exe = std::env::current_exe().expect("test executable");
    let output = std::process::Command::new(exe)
        .current_dir(workspace_root())
        .env_remove("CARGO_TARGET_DIR")
        .env("KANBAN_TARGET_DIR_RELATIVE_MANIFESTS", "1")
        .args([
            "--exact",
            ISOLATED_RELATIVE_MANIFEST,
            "--nocapture",
            "--test-threads=1",
        ])
        .output()
        .expect("spawn isolated relative-manifest test");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        output.status.success(),
        "isolated relative-manifest test failed:\n{combined}",
    );
    assert!(
        combined.contains("running 1 test"),
        "child must run exactly one test, output:\n{combined}",
    );
    assert!(
        combined.contains("test result: ok. 1 passed"),
        "child must report exactly one passing test, output:\n{combined}",
    );
}

#[test]
fn target_dir_accepts_relative_manifests_isolated() {
    if std::env::var_os("KANBAN_TARGET_DIR_RELATIVE_MANIFESTS").is_none() {
        return;
    }
    let root = workspace_root();
    assert_eq!(
        cargo_target_dir(Path::new("Cargo.toml")),
        root.join("target"),
        "a manifest relative to the working directory resolves the root target",
    );
    assert_eq!(
        cargo_target_dir(Path::new("apps/desktop/src-tauri/Cargo.toml")),
        root.join("apps/desktop/src-tauri").join("target"),
        "a nested relative manifest resolves its own target directory",
    );
}
