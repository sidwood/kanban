use kanban_desktop_lib::ensure_core_running;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn request(socket: &Path, kind: &str, operation: &str, payload: Value) -> Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    writeln!(
        stream,
        "{}",
        json!({"kind":kind,"operation":operation,"payload":payload})
    )
    .unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    let frame: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(frame["kind"], "response", "{frame}");
    frame["payload"].clone()
}
fn stop(socket: &Path, key: &str) {
    let warning = request(socket, "query", "service.stop_warning", json!({}));
    let response = request(
        socket,
        "command",
        "service.stop",
        json!({"mutation":{"optimistic_version":warning["version"],"idempotency_key":key},"instance_id":warning["instance_id"],"warning_id":warning["warning_id"],"confirmed":true}),
    );
    assert_eq!(response["status"], "stop_requested");
    let until = Instant::now() + Duration::from_secs(15);
    while socket.exists() {
        assert!(Instant::now() < until, "service failed to shut down");
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
fn shell_quit_fixture() {
    let Some(data) = std::env::var_os("KANBAN_LIFECYCLE_FIXTURE_DATA") else {
        return;
    };
    let socket = Path::new(&data).join("core.sock");
    let spawned = ensure_core_running(&socket).unwrap();
    assert!(
        ensure_core_running(&socket).unwrap().is_none(),
        "reuse does not spawn"
    );
    if let Some(child) = spawned {
        std::fs::write(Path::new(&data).join("fixture.pid"), child.id().to_string()).unwrap();
    }
}
#[test]
fn lifecycle_real_on_demand_survives_shell_process_exit_stops_and_restarts() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let build = Command::new("cargo")
        .current_dir(&root)
        .args([
            "test",
            "-p",
            "kanban-service",
            "--test",
            "lifecycle_process",
            "--no-run",
            "--message-format=json",
        ])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let binary = String::from_utf8_lossy(&build.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| {
            value["target"]["name"] == "lifecycle_process" && value["executable"].is_string()
        })
        .unwrap()["executable"]
        .as_str()
        .unwrap()
        .to_owned();
    let dir = tempfile::TempDir::new().unwrap();
    let data = dir.path().join("chosen data");
    let socket = data.join("core.sock");
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if self.0.join("core.sock").exists()
                && let Ok(pid) = std::fs::read_to_string(self.0.join("fixture.pid"))
            {
                let _ = Command::new("kill").arg(pid).status();
            }
        }
    }
    let _cleanup = Cleanup(data.clone());
    for key in ["first-stop", "restart-stop"] {
        let shell = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "shell_quit_fixture"])
            .env("KANBAN_LIFECYCLE_FIXTURE_DATA", &data)
            .env("KANBAN_CORE_BIN", &binary)
            .output()
            .unwrap();
        assert!(
            shell.status.success(),
            "{} {}",
            String::from_utf8_lossy(&shell.stdout),
            String::from_utf8_lossy(&shell.stderr)
        );
        assert!(
            data.join("kanban.sqlite").is_file(),
            "the real service uses the shell-selected data directory"
        );
        assert_eq!(
            request(&socket, "query", "health.get", json!({}))["connected"],
            true,
            "the real service survives shell process exit"
        );
        stop(&socket, key);
        assert!(!socket.exists());
    }
}
