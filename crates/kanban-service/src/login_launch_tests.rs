use crate::login_launch::NativeLoginLaunch;
use kanban_app::service_lifecycle::LoginLaunchPort;
use tempfile::TempDir;

#[test]
#[cfg(target_os = "macos")]
fn login_launch_removal_recovers_an_owned_plist_after_external_unload() {
    let dir = TempDir::new().unwrap();
    let label = format!("dev.kanban.test.recovery.{}", std::process::id());
    let adapter = NativeLoginLaunch::isolated(
        "/usr/bin/true".into(),
        dir.path().join("data"),
        dir.path().to_path_buf(),
        label.clone(),
        "/bin/launchctl".into(),
    );
    adapter.set_enabled(true).unwrap();
    assert!(
        std::process::Command::new("/bin/launchctl")
            .arg("bootout")
            .arg(format!("gui/{}/{}", unsafe { libc::geteuid() }, label))
            .status()
            .unwrap()
            .success()
    );
    assert!(adapter.enabled().is_err());
    adapter
        .set_enabled(false)
        .expect("explicit removal repairs an owned but unloaded registration");
    assert!(!adapter.enabled().unwrap());
}

#[test]
fn login_launch_bootstrap_and_readback_failures_never_claim_enabled() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let command = dir.path().join("launchctl-fixture");
    std::fs::write(
        &command,
        "#!/bin/sh\ncase \"$1\" in print|bootout) exit 113;; *) exit 5;; esac\n",
    )
    .unwrap();
    std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
    let adapter = NativeLoginLaunch::isolated(
        "/usr/bin/true".into(),
        dir.path().join("data"),
        dir.path().to_path_buf(),
        "dev.kanban.test.failure".into(),
        command.clone(),
    );
    assert!(adapter.set_enabled(true).is_err());
    assert!(!adapter.enabled().unwrap());
    assert!(!dir.path().join("dev.kanban.test.failure.plist").exists());
    std::fs::write(&command, "#!/bin/sh\nexit 5\n").unwrap();
    assert!(adapter.enabled().is_err());
    assert!(adapter.set_enabled(false).is_err());
}

#[test]
fn login_launch_commands_record_requests_replay_and_report_native_failure() {
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    struct Unavailable(Mutex<u32>);
    impl LoginLaunchPort for Unavailable {
        fn enabled(&self) -> Result<bool, kanban_dto::ApiError> {
            Ok(false)
        }
        fn set_enabled(&self, _: bool) -> Result<(), kanban_dto::ApiError> {
            *self.0.lock().unwrap() += 1;
            Err(kanban_dto::ApiError::internal("fixture native failure"))
        }
    }
    let dir = TempDir::new().unwrap();
    let (_, mut core, observer, _, _) = crate::assemble_core(
        dir.path(),
        crate::prepare_database(dir.path()).unwrap(),
        Arc::new(kanban_app::NoopEventSink),
        dir.path().join("sessions"),
        crate::fast_observation(),
        Arc::new(crate::LocalFleetCloneTool::default()),
    )
    .unwrap();
    let port = Arc::new(Unavailable(Mutex::new(0)));
    core.register_login_launch(port.clone(), "fixture-instance".into())
        .unwrap();
    let state = core.query("service.login_launch.get", &json!({})).unwrap();
    assert_eq!(state["enabled"], false);
    let request = json!({"mutation":{"optimistic_version":0,"idempotency_key":"login"},"instance_id":"fixture-instance","enabled":true});
    let result = core.command("service.login_launch.set", &request).unwrap();
    assert_eq!(result["status"], "change_requested");
    assert_eq!(
        core.command("service.login_launch.set", &request).unwrap(),
        result
    );
    assert_eq!(*port.0.lock().unwrap(), 1);
    let state = core.query("service.login_launch.get", &json!({})).unwrap();
    assert_eq!(state["enabled"], false);
    assert_eq!(state["version"], 1);
    assert_eq!(state["error"], "fixture native failure");
    let mut stale = request.clone();
    stale["mutation"]["idempotency_key"] = json!("stale");
    assert_eq!(
        core.command("service.login_launch.set", &stale)
            .unwrap_err()
            .code,
        kanban_dto::ErrorCode::StaleVersion
    );
    let mut unknown = request.clone();
    unknown["force"] = json!(true);
    assert_eq!(
        core.command("service.login_launch.set", &unknown)
            .unwrap_err()
            .code,
        kanban_dto::ErrorCode::UnknownField
    );
    let mut reused = request.clone();
    reused["enabled"] = json!(false);
    assert_eq!(
        core.command("service.login_launch.set", &reused)
            .unwrap_err()
            .code,
        kanban_dto::ErrorCode::DuplicateIdempotencyKey
    );
    observer.shutdown();
}

#[test]
#[cfg(target_os = "macos")]
fn login_launch_native_registration_is_opt_in_read_back_and_reversible() {
    let dir = TempDir::new().unwrap();
    let label = format!(
        "dev.kanban.test.{}",
        dir.path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace('.', "")
    );
    let adapter = NativeLoginLaunch::isolated(
        "/usr/bin/true".into(),
        dir.path().join("data"),
        dir.path().to_path_buf(),
        label,
        "/bin/launchctl".into(),
    );
    struct Cleanup<'a>(&'a NativeLoginLaunch);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = self.0.set_enabled(false);
        }
    }
    let _cleanup = Cleanup(&adapter);
    assert!(!adapter.enabled().unwrap());
    adapter.set_enabled(true).unwrap();
    assert!(adapter.enabled().unwrap());
    adapter.set_enabled(true).unwrap();
    adapter.set_enabled(false).unwrap();
    assert!(!adapter.enabled().unwrap());
    adapter.set_enabled(false).unwrap();
}
