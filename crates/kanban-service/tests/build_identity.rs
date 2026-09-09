//! Version diagnostics must not start Core or require a development environment.
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn installed_version_reports_embedded_identity_without_starting_core() {
    let home = tempfile::TempDir::new().expect("isolated home");
    let mut child = Command::new(env!("CARGO_BIN_EXE_kanban-service"))
        .arg("--version")
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .current_dir(home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the built service starts");
    let deadline = Instant::now() + Duration::from_secs(3);
    while child.try_wait().expect("child status").is_none() {
        if Instant::now() >= deadline {
            child.kill().expect("reap an unexpectedly running service");
            child.wait().expect("child reaped");
            panic!("--version must exit without starting the durable service");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("version output");
    assert!(output.status.success(), "version diagnostics must succeed");
    let identity: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON identity");
    assert_eq!(identity, kanban_dto::build_identity::json());
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}
