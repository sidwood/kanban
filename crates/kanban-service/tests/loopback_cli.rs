#[test]
fn loopback_default_off_production_argv_reuses_desktop_selected_data() {
    for explicit in [false, true] {
        let dir = tempfile::TempDir::new().unwrap();
        let data = dir.path().join("selected data");
        let core = kanban_service::serve_with_runtime(
            &data,
            kanban_service::ServiceRuntime {
                mcp_executable: "/usr/bin/false".into(),
                herdr_socket_root: data.join("isolated-herdr"),
                installation_secret: None,
            },
        )
        .unwrap();
        assert!(core.http_address().is_none());
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_kanban-service"));
        if explicit {
            command.arg("--data-dir");
        }
        let output = command
            .arg(&data)
            .env_clear()
            .env("HOME", dir.path().join("unused-home"))
            .output()
            .unwrap();
        core.shutdown();
        assert!(
            output.status.success(),
            "desktop data selection was rejected"
        );
        assert!(!dir.path().join("unused-home").exists());
    }
}

#[test]
fn loopback_auth_production_argv_accepts_opt_in_with_selected_data() {
    let dir = tempfile::TempDir::new().unwrap();
    let core = kanban_service::serve_with_runtime(
        dir.path(),
        kanban_service::ServiceRuntime {
            mcp_executable: "/usr/bin/false".into(),
            herdr_socket_root: dir.path().join("isolated-herdr"),
            installation_secret: None,
        },
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_kanban-service"))
        .arg("--loopback-http")
        .arg("127.0.0.1:0")
        .arg("--data-dir")
        .arg(dir.path())
        .env_clear()
        .env("HOME", dir.path().join("unused-home"))
        .output()
        .unwrap();
    assert!(
        core.http_address().is_none(),
        "reuse must not reconfigure a live core"
    );
    core.shutdown();
    assert!(output.status.success(), "explicit HTTP opt-in was rejected");
}

#[test]
fn loopback_auth_cli_refuses_invalid_enablement_before_managed_start() {
    let dir = tempfile::TempDir::new().unwrap();
    for args in [
        vec!["--loopback-http"],
        vec!["--loopback-http", "0.0.0.0:9876"],
        vec!["--loopback-http", "[::]:9876"],
        vec!["--loopback-http", "localhost:9876"],
        vec!["--loopback-http", "127.0.0.1:65536"],
        vec!["--loopback-http", "127.0.0.1"],
        vec!["--loopback-http", ""],
        vec!["--loopback-http=127.0.0.1:9876"],
        vec![
            "--loopback-http",
            "127.0.0.1:9876",
            "--loopback-http",
            "127.0.0.1:9876",
        ],
        vec!["--launch-once", "--launch-once"],
        vec!["--data-dir"],
        vec!["--data-dir", "--loopback-http", "127.0.0.1:9876"],
        vec!["--data-dir", ""],
        vec!["--data-dir", "relative"],
        vec!["--data-dir", "/unused", "--data-dir", "/unused"],
        vec!["/unused", "--data-dir", "/unused"],
        vec!["/unused", "/unused"],
        vec!["--loopback-http", "127.0.0.1:9876", "--unknown"],
        vec!["--http-secret", "planted-secret"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_kanban-service"))
            .args(args)
            .env_clear()
            .env("HOME", dir.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("planted-secret"));
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }
}

#[test]
fn loopback_auth_production_argv_rejects_non_utf8_bind_without_echoing_input() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::TempDir::new().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_kanban-service"))
        .arg("--loopback-http")
        .arg(std::ffi::OsString::from_vec(b"planted-secret\xff".to_vec()))
        .env_clear()
        .env("HOME", dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("planted-secret"));
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
}
