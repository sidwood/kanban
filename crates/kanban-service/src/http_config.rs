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

#[test]
fn configured_operator_identity_is_preserved_over_http() {
    use kanban_app::secrets::InstallationSecret;
    use serde_json::{Value, json};
    use std::io::{Read, Write};

    let h = common::harness();
    let configured = "  Ada  ";
    std::fs::write(
        h._dir.path().join("config.json"),
        json!({"operator_name": configured}).to_string(),
    )
    .unwrap();
    let secret = Arc::new(InstallationSecret::from_key(&[61; 32]));
    let service = serve_with_http(
        h._dir.path(),
        ServiceRuntime {
            mcp_executable: h._dir.path().join("unused"),
            herdr_socket_root: h._dir.path().join("herdr"),
            installation_secret: Some(secret.clone()),
        },
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
    )
    .unwrap();
    let address = service.http_address().unwrap();
    let call = |name: &str, arguments: Value| {
        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        })
        .to_string();
        let mut client = std::net::TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            client,
            "POST /mcp/operator HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
            secret.expose(),
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("HTTP/1.1 200"), "{response}");
        let response: Value = serde_json::from_str(body).unwrap();
        assert!(response.get("error").is_none(), "{response}");
        assert_ne!(response["result"]["isError"], json!(true), "{response}");
        response["result"]["structuredContent"].clone()
    };
    let saved = call(
        "save_issue",
        json!({"team": "CORE", "title": "Configured identity", "assignee": "me"}),
    );
    let issue = call("get_issue", json!({"id": saved["identifier"]}));
    call(
        "save_comment",
        json!({"issue": saved["identifier"], "body": "Exact author"}),
    );
    let comments = call(
        "list_comments",
        json!({"target": {"kind": "ticket", "id": saved["identifier"]}}),
    );
    let explicit = call(
        "save_issue",
        json!({"team": "CORE", "title": "Explicit identity", "assignee": configured}),
    );
    let mine = call("list_issues", json!({"assignee": "me"}));
    service.shutdown();

    assert_eq!(issue["assignee"], json!("  Ada  "));
    assert_eq!(issue["creator"], json!("  Ada  "));
    assert_eq!(comments["comments"][0]["author"], json!("  Ada  "));
    let issues = mine["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 2);
    for identifier in [&saved["identifier"], &explicit["identifier"]] {
        assert!(
            issues
                .iter()
                .any(|issue| &issue["identifier"] == identifier)
        );
    }
}

#[test]
fn blank_configured_operator_names_are_unset() {
    let dir = tempfile::TempDir::new().unwrap();
    assert_eq!(configured_operator_name(dir.path()), None);
    for name in ["", " ", "\t\r\n", "\u{2003}"] {
        std::fs::write(
            dir.path().join("config.json"),
            serde_json::json!({"operator_name": name}).to_string(),
        )
        .unwrap();
        assert_eq!(configured_operator_name(dir.path()), None);
    }
}
