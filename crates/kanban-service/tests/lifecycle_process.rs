use kanban_service::{ServiceOptions, ServiceRuntime, run_with_runtime};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command};
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
fn wait_ready(socket: &Path, child: &mut Child) {
    let until = Instant::now() + Duration::from_secs(15);
    while UnixStream::connect(socket).is_err() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "real runtime exited before readiness"
        );
        assert!(Instant::now() < until, "real runtime never became ready");
        std::thread::sleep(Duration::from_millis(20));
    }
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
}
#[cfg(target_os = "macos")]
fn delete_credential(account: &str) -> bool {
    Command::new("/usr/bin/security")
        .args([
            "delete-generic-password",
            "-s",
            "dev.kanban.desktop.installation",
            "-a",
            account,
        ])
        .output()
        .unwrap()
        .status
        .success()
}
#[cfg(target_os = "macos")]
fn credential_present(account: &str) -> bool {
    // Metadata-only readback avoids another executable requesting secret access.
    let output = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            "dev.kanban.desktop.installation",
            "-a",
            account,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success() || output.status.code() == Some(44),
        "native metadata readback failed"
    );
    output.status.success()
}
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| {
        arg == "--data-dir" || arg == "--launch-once" || Path::new(arg).is_absolute()
    }) {
        let options = ServiceOptions::parse(args).unwrap();
        if options.launch_once {
            // The short-lived login helper exits so launchd cannot kill the core on removal.
            drop(
                kanban_service::launch_detached(
                    &std::env::current_exe().unwrap(),
                    &options.data_dir,
                )
                .unwrap(),
            );
            return;
        }
        let runtime = ServiceRuntime {
            mcp_executable: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug/kanban-mcp"),
            herdr_socket_root: options.data_dir.join("isolated-herdr"),
            installation_secret: None,
        };
        run_with_runtime(&options.data_dir, runtime).unwrap();
        return;
    }
    let dir = tempfile::TempDir::new().unwrap();
    let data = dir.path().join("selected data");
    let socket = data.join("core.sock");
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(&exe).arg(&data).spawn().unwrap();
    wait_ready(&socket, &mut child);
    assert!(data.join("kanban.sqlite").exists());
    assert_eq!(
        request(&socket, "query", "health.get", json!({}))["connected"],
        true
    );
    let reused = Command::new(&exe)
        .arg("--data-dir")
        .arg(&data)
        .status()
        .unwrap();
    assert!(reused.success());
    assert!(child.try_wait().unwrap().is_none());
    stop(&socket, "first-stop");
    assert!(child.wait().unwrap().success());
    assert!(!socket.exists());
    let mut child = kanban_service::launch_detached(&exe, &data).unwrap();
    wait_ready(&socket, &mut child);
    stop(&socket, "restart-stop");
    assert!(child.wait().unwrap().success());
    assert!(!socket.exists());

    #[cfg(target_os = "macos")]
    {
        use kanban_app::service_lifecycle::LoginLaunchPort;
        let login_data = dir.path().join("login data");
        let socket = login_data.join("core.sock");
        let adapter = kanban_service::login_launch::NativeLoginLaunch::for_installation(
            exe.clone(),
            login_data.clone(),
            dir.path().join("LaunchAgents"),
        );
        struct RegistrationCleanup<'a>(&'a dyn LoginLaunchPort);
        impl Drop for RegistrationCleanup<'_> {
            fn drop(&mut self) {
                let _ = self.0.set_enabled(false);
            }
        }
        let _registration = RegistrationCleanup(&adapter);
        assert!(!adapter.enabled().unwrap());
        adapter.set_enabled(true).unwrap();
        assert!(adapter.enabled().unwrap());
        let until = Instant::now() + Duration::from_secs(15);
        while UnixStream::connect(&socket).is_err() {
            assert!(
                Instant::now() < until,
                "native login helper failed to start the real service"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        adapter.set_enabled(false).unwrap();
        assert!(!adapter.enabled().unwrap());
        assert_eq!(
            request(&socket, "query", "health.get", json!({}))["connected"],
            true,
            "unregistering login must not stop the detached service"
        );
        stop(&socket, "login-stop");
        while socket.exists() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "macos")]
    {
        use sha2::{Digest, Sha256};
        use std::os::unix::ffi::OsStrExt;
        let data = dir.path().join("native CLI data");
        let home = std::env::var_os("HOME")
            .expect("native Keychain discovery needs the current user home");
        std::fs::create_dir_all(&data).unwrap();

        let account = format!(
            "data-{:x}",
            Sha256::digest(data.canonicalize().unwrap().as_os_str().as_bytes())
        );
        struct CredentialCleanup(String);
        impl Drop for CredentialCleanup {
            fn drop(&mut self) {
                let _ = delete_credential(&self.0);
            }
        }
        let _credential = CredentialCleanup(account.clone());
        println!("native disposable credential account: {account}");
        assert!(!credential_present(&account));
        let mut child = Command::new(env!("CARGO_BIN_EXE_kanban-service"))
            .arg("--data-dir")
            .arg(&data)
            .env_clear()
            .env("HOME", &home)
            .spawn()
            .unwrap();
        let socket = data.join("core.sock");
        wait_ready(&socket, &mut child);
        assert!(credential_present(&account));
        assert_eq!(
            request(&socket, "query", "health.get", json!({}))["connected"],
            true
        );
        stop(&socket, "native-cli-stop");
        assert!(child.wait().unwrap().success());
        assert!(delete_credential(&account));
        assert!(!credential_present(&account));
        println!(
            "lifecycle_native_cli: actual kanban-service, cleared environment with native HOME, isolated data directory, native disposable Keychain account, health and confirmed stop passed; credential removed"
        );
    }
    println!(
        "lifecycle_process: selected data directory, real SQLite health, reuse, clean stop, detached restart passed"
    );
}
