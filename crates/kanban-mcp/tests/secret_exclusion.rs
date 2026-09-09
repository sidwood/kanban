//! Planted credentials exercise the same service boundary as installed clients.
use kanban_app::secrets::InstallationSecret;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;

#[test]
fn secret_exclusion_managed_service_refuses_credential_payloads() {
    let dir = tempfile::TempDir::new().unwrap();
    let secret = Arc::new(InstallationSecret::from_key(&[9; 32]));
    let runtime = kanban_service::ServiceRuntime {
        mcp_executable: env!("CARGO_BIN_EXE_kanban-mcp").into(),
        herdr_socket_root: dir.path().join("herdr"),
        installation_secret: Some(secret.clone()),
    };
    let service = kanban_service::serve_with_runtime(dir.path(), runtime).unwrap();
    let mut channel = BufReader::new(UnixStream::connect(service.socket_path()).unwrap());
    channel
        .get_ref()
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut responses = Vec::new();
    for kind in ["query", "command"] {
        writeln!(
            channel.get_mut(),
            "{}",
            json!({"kind":kind,"operation":secret.expose(),"payload":{}})
        )
        .unwrap();
        let mut response = String::new();
        channel.read_line(&mut response).unwrap();
        responses.push(response);
    }
    for frame in [
        json!({"kind": secret.expose(), "payload": {}}),
        json!({"kind": "query", (secret.expose()): true}),
        json!({"kind": "agent", "payload": {"capability_id": secret.expose()}}),
    ] {
        let mut malformed = BufReader::new(UnixStream::connect(service.socket_path()).unwrap());
        writeln!(malformed.get_mut(), "{frame}").unwrap();
        let mut response = String::new();
        malformed.read_line(&mut response).unwrap();
        responses.push(response);
    }
    service.shutdown();
    for response in responses {
        let result: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(result["kind"], "error");
        assert!(
            !response.contains(secret.expose()),
            "the transport must not reflect installation credentials"
        );
    }
    scan(dir.path(), secret.expose());
}

fn scan(path: &std::path::Path, secret: &str) {
    for entry in std::fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            scan(&entry.path(), secret);
        } else if entry.file_type().unwrap().is_file() {
            let bytes = std::fs::read(entry.path()).unwrap();
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes()),
                "credential leaked into a managed artifact"
            );
        }
    }
}

#[test]
fn secret_exclusion_diagnostics_scrub_credentials_without_plaintext_config() {
    let dir = tempfile::TempDir::new().unwrap();
    let secret = Arc::new(InstallationSecret::from_key(&[12; 32]));
    let service = kanban_service::serve_with_runtime(
        dir.path(),
        kanban_service::ServiceRuntime {
            mcp_executable: env!("CARGO_BIN_EXE_kanban-mcp").into(),
            herdr_socket_root: dir.path().join("herdr"),
            installation_secret: Some(secret.clone()),
        },
    )
    .unwrap();
    // A pre-existing leaked log is adversarial input, not a product log write.
    std::fs::write(dir.path().join("logs/canary.log"), secret.expose()).unwrap();
    let mut channel = BufReader::new(UnixStream::connect(service.socket_path()).unwrap());
    writeln!(
        channel.get_mut(),
        "{}",
        json!({"kind":"query","operation":"diagnostics.export","payload":{}})
    )
    .unwrap();
    let mut response = String::new();
    channel.read_line(&mut response).unwrap();
    service.shutdown();
    let response: Value = serde_json::from_str(&response).unwrap();
    let bundle = response["payload"]["bundle_dir"]
        .as_str()
        .expect("the diagnostic bundle is created");
    scan(std::path::Path::new(bundle), secret.expose());
    assert!(
        std::fs::read_to_string(std::path::Path::new(bundle).join("logs/canary.log"))
            .unwrap()
            .contains("[REDACTED]")
    );
}

#[test]
fn secret_exclusion_logs_scrub_the_native_credential() {
    let dir = tempfile::TempDir::new().unwrap();
    let secret = InstallationSecret::from_key(&[11; 32]);
    let logs = kanban_service::logs::LogWriter::open(dir.path())
        .unwrap()
        .with_installation_secret(Some(&secret));
    logs.append(
        &kanban_service::logs::LogRecord::new(
            kanban_service::logs::LogLevel::Info,
            secret.expose(),
            secret.expose(),
        )
        .with_fields(json!({"note": secret.expose()})),
    )
    .unwrap();
    scan(dir.path(), secret.expose());
    assert!(!format!("{logs:?}").contains(secret.expose()));
}
