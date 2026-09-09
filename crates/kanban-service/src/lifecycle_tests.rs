use crate::test_client::{Client, boot};

#[test]
fn lifecycle_startup_does_not_reuse_an_unhealthy_socket_or_prepare_its_database() {
    let dir = TempDir::new().unwrap();
    let listener = std::os::unix::net::UnixListener::bind(dir.path().join("core.sock")).unwrap();
    let started = std::time::Instant::now();
    let result = crate::run_with_runtime(
        dir.path(),
        crate::ServiceRuntime {
            mcp_executable: "/usr/bin/false".into(),
            herdr_socket_root: dir.path().join("isolated-herdr"),
            installation_secret: None,
        },
    );
    drop(listener);
    assert!(
        result.is_err(),
        "a socket accepting connections is not a healthy core"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(12),
        "readiness failure must be bounded"
    );
    assert!(
        !dir.path().join("kanban.sqlite").exists(),
        "a follower must not prepare another owner's database"
    );
}

#[test]
fn lifecycle_stop_requires_confirmation_rejects_unknown_stale_and_replays() {
    let dir = TempDir::new().unwrap();
    let core = boot(&dir);
    let mut client = Client::connect(core.socket_path());
    let warning = client.query("service.stop_warning");
    let request = json!({"mutation":{"optimistic_version":0,"idempotency_key":"stop-1"}, "instance_id":warning["instance_id"], "warning_id":warning["warning_id"], "confirmed":true});
    let mut cancelled = request.clone();
    cancelled["confirmed"] = json!(false);
    assert_eq!(
        client.command_error("service.stop", cancelled)["code"],
        "invalid_request"
    );
    let mut unknown = request.clone();
    unknown["force"] = json!(true);
    assert_eq!(
        client.command_error("service.stop", unknown)["code"],
        "unknown_field"
    );
    let mut stale = request.clone();
    stale["mutation"]["optimistic_version"] = json!(9);
    assert_eq!(
        client.command_error("service.stop", stale)["code"],
        "stale_version"
    );
    let mut stale_warning = request.clone();
    stale_warning["warning_id"] = json!("old");
    assert_eq!(
        client.command_error("service.stop", stale_warning)["code"],
        "invalid_request"
    );
    let accepted = client.command("service.stop", request.clone());
    assert_eq!(accepted["status"], "stop_requested");
    assert_eq!(client.command("service.stop", request.clone()), accepted);
    let mut reused = request;
    reused["confirmed"] = json!(false);
    assert_eq!(
        client.command_error("service.stop", reused)["code"],
        "invalid_request"
    );
    core.shutdown();
}

use serde_json::json;
use tempfile::TempDir;

#[test]
fn lifecycle_startup_honours_selected_data_directory_and_rejects_extra_arguments() {
    let selected = std::path::PathBuf::from("/tmp/kanban selected data");
    let options = crate::ServiceOptions::parse([selected.clone().into_os_string()]).unwrap();
    assert_eq!(options.data_dir, selected);
    assert!(!options.launch_once);
    let options = crate::ServiceOptions::parse([
        "--launch-once".into(),
        "--data-dir".into(),
        selected.clone().into_os_string(),
    ])
    .unwrap();
    assert_eq!(options.data_dir, selected);
    assert!(options.launch_once);
    assert!(crate::ServiceOptions::parse(["--surprise".into()]).is_err());
    assert!(crate::ServiceOptions::parse(["one".into(), "two".into()]).is_err());
}

#[test]
fn lifecycle_stop_flushes_response_removes_socket_and_restarts() {
    let dir = TempDir::new().unwrap();
    let core = boot(&dir);
    let socket = core.socket_path().to_path_buf();
    let mut client = Client::connect(&socket);
    let old = client.query("service.stop_warning");
    let owner = std::thread::spawn(move || core.wait_for_stop());
    assert_eq!(client.command("service.stop", json!({"mutation":{"optimistic_version":0,"idempotency_key":"shutdown"},"instance_id":old["instance_id"],"warning_id":old["warning_id"],"confirmed":true}))["status"], "stop_requested");
    owner.join().unwrap();
    assert!(!socket.exists(), "clean shutdown removes the owned socket");
    let restarted = boot(&dir);
    let mut client = Client::connect(restarted.socket_path());
    assert_ne!(
        client.query("service.stop_warning")["instance_id"],
        old["instance_id"]
    );
    assert_eq!(client.command_error("service.stop", json!({"mutation":{"optimistic_version":0,"idempotency_key":"old-warning"},"instance_id":old["instance_id"],"warning_id":old["warning_id"],"confirmed":true}))["code"], "invalid_request");
    restarted.shutdown();
}

#[test]
fn lifecycle_stop_warning_comes_from_current_health() {
    let dir = TempDir::new().unwrap();
    let core = boot(&dir);
    let mut client = Client::connect(core.socket_path());
    let health = client.query("health.get");
    let warning = client.query("service.stop_warning");
    assert_eq!(warning["instance_id"], health["service"]["started_at"]);
    assert_eq!(warning["version"], 0);
    assert_eq!(
        warning["capabilities"][0],
        "Application commands, database access, and live updates"
    );
    assert!(warning["capabilities"].as_array().unwrap().contains(&json!(
        "Schedule activation, daily backups, Attention Items, and notifications"
    )));
    assert!(!warning["warning_id"].as_str().unwrap().is_empty());
    assert_eq!(client.query("health.get")["connected"], true);
    core.shutdown();
}
