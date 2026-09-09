use super::*;
#[path = "../../kanban-app/tests/common/mod.rs"]
mod common;

#[test]
fn loopback_auth_failed_start_stops_observers_before_returning() {
    let h = common::harness();
    let root = h._dir.path().join("herdr");
    let result = serve_configured_with_mcp(
        h._dir.path(),
        root.clone(),
        fast_observation(),
        Arc::new(LocalFleetCloneTool::default()),
        h._dir.path().join("unused"),
        None,
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
    );
    assert!(matches!(result, Err(ServiceError::LoopbackHttp)));
    let fixture = kanban_herdr::fixture::ScriptedSession::bind(
        &root,
        "kanban-main",
        "/workspaces/kanban.seed",
        kanban_herdr::fixture::SessionScript::default(),
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        fixture.requests_seen(),
        0,
        "failed startup left a Herdr observer running"
    );
    assert!(std::os::unix::net::UnixStream::connect(h._dir.path().join("core.sock")).is_err());
}

#[test]
fn loopback_auth_never_reads_a_configuration_secret_as_fallback() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"http_secret":"planted-secret"}"#,
    )
    .unwrap();
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = held.local_addr().unwrap();
    drop(held);
    let result = serve_with_http(
        dir.path(),
        ServiceRuntime {
            mcp_executable: dir.path().join("unused"),
            herdr_socket_root: dir.path().join("herdr"),
            installation_secret: None,
        },
        LoopbackHttpConfig {
            bind: Some(address),
        },
    );
    assert!(matches!(result, Err(ServiceError::LoopbackHttp)));
    assert!(std::net::TcpStream::connect(address).is_err());
    assert!(std::os::unix::net::UnixStream::connect(dir.path().join("core.sock")).is_err());
    let fallback = serve_with_runtime(
        dir.path(),
        ServiceRuntime {
            mcp_executable: dir.path().join("unused"),
            herdr_socket_root: dir.path().join("herdr"),
            installation_secret: None,
        },
    )
    .unwrap();
    assert!(fallback.http_address().is_none());
    fallback.shutdown();
}

#[test]
fn loopback_auth_production_orchestration_preserves_owned_credential_failure() {
    let dir = tempfile::TempDir::new().unwrap();
    let result = run_with_args(
        [
            dir.path().as_os_str().to_owned(),
            "--loopback-http".into(),
            "127.0.0.1:0".into(),
        ],
        Path::new("/usr/bin/false"),
        |canonical| {
            assert_eq!(canonical, dir.path().canonicalize().unwrap());
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(canonical.join("core.lock"))
                .unwrap();
            assert!(matches!(
                lock.try_lock(),
                Err(std::fs::TryLockError::WouldBlock)
            ));
            assert!(!canonical.join("kanban.sqlite").exists());
            Err(ServiceError::InstallationSecret)
        },
    );
    assert!(matches!(result, Err(ServiceError::InstallationSecret)));
    assert!(!dir.path().join("kanban.sqlite").exists());
    assert!(!dir.path().join("core.sock").exists());
    let owner = startup::StartupOwner::acquire(dir.path()).unwrap();
    drop(owner);
}

#[test]
fn loopback_auth_production_orchestration_reuses_without_credentials_or_reconfiguration() {
    let dir = tempfile::TempDir::new().unwrap();
    let core = crate::test_client::boot(&dir);
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = held.local_addr().unwrap();
    drop(held);
    run_with_args(
        [
            "--loopback-http".into(),
            address.to_string().into(),
            "--data-dir".into(),
            dir.path().as_os_str().to_owned(),
        ],
        Path::new("/usr/bin/false"),
        |_| panic!("healthy reuse must not request native credentials"),
    )
    .unwrap();
    assert!(core.http_address().is_none());
    assert!(std::net::TcpStream::connect(address).is_err());
    core.shutdown();
}

#[test]
fn loopback_default_off_cli_requires_explicit_numeric_loopback_enablement() {
    assert!(ServiceOptions::parse([]).unwrap().http.bind.is_none());
    let options =
        ServiceOptions::parse(["--loopback-http".into(), "127.0.0.1:9876".into()]).unwrap();
    assert_eq!(options.http.bind.unwrap().to_string(), "127.0.0.1:9876");
    for args in [
        vec!["--loopback-http"],
        vec!["--loopback-http", "0.0.0.0:9876"],
        vec!["--loopback-http", "localhost:9876"],
        vec!["--http-secret", "planted-secret"],
        vec!["--loopback-http", "127.0.0.1:9876", "extra"],
    ] {
        let error = ServiceOptions::parse(args.iter().map(Into::into))
            .err()
            .unwrap();
        assert!(!error.to_string().contains("planted-secret"));
    }
}
