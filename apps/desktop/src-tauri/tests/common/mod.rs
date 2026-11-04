//! Shared helpers for desktop integration tests.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve the workspace target directory honouring `CARGO_TARGET_DIR`.
pub fn cargo_target_dir(manifest: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("CARGO_TARGET_DIR")
        && Path::new(&dir).is_absolute()
    {
        return PathBuf::from(dir);
    }
    let manifest = std::path::absolute(manifest).expect("the manifest path can be made absolute");
    let output = Command::new("cargo")
        .current_dir(manifest.parent().expect("the manifest has a parent"))
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(&manifest)
        .output()
        .expect("cargo metadata runs");
    assert!(output.status.success(), "cargo metadata must succeed");
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata is JSON");
    PathBuf::from(
        metadata["target_directory"]
            .as_str()
            .expect("target_directory in cargo metadata"),
    )
}
