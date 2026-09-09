#[path = "common/http_response.rs"]
mod http_response;

use http_response::read_response;
use kanban_app::secrets::InstallationSecret;
use kanban_transport::loopback::{LoopbackHttp, LoopbackHttpConfig};
use rmcp::ServerHandler;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

fn secret() -> Arc<InstallationSecret> {
    Arc::new(InstallationSecret::from_key(&[71; 32]))
}

fn post(addr: SocketAddr, credential: Option<&str>, extra: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream, "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nX-Kanban-Capability: 1\r\nContent-Length: {}\r\n", body.len()).unwrap();
    if let Some(credential) = credential {
        write!(stream, "Authorization: Bearer {credential}\r\n").unwrap();
    }
    write!(stream, "{extra}\r\n{body}").unwrap();
    read_response(&mut stream).unwrap()
}

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}"#;

#[test]
fn loopback_auth_reads_early_refusal_with_a_late_body() {
    let secret = secret();
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret.clone()),
        |_| -> Result<Probe, _> { panic!("early refusal must precede application dispatch") },
    )
    .unwrap();
    let addr = server.local_addr().unwrap();
    for (credential, extra, status) in [
        (None, "", "HTTP/1.1 401 Unauthorized"),
        (Some("invalid"), "", "HTTP/1.1 401 Unauthorized"),
        (
            Some(secret.expose()),
            "Origin: null\r\n",
            "HTTP/1.1 403 Forbidden",
        ),
    ] {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: 2\r\n"
        )
        .unwrap();
        if let Some(credential) = credential {
            write!(stream, "Authorization: Bearer {credential}\r\n").unwrap();
        }
        write!(stream, "{extra}\r\n").unwrap();
        let mut first = [0; 1];
        stream
            .read_exact(&mut first)
            .expect("refusal before request body");
        stream.write_all(b"{}").unwrap();
        let response = read_response(&mut first.as_slice().chain(&mut stream)).unwrap();
        assert!(response.lines().next() == Some(status));
        assert!(response.split_once("\r\n\r\n").unwrap().1 == "request refused");
        assert!(!response.contains(secret.expose()));
    }
    server.shutdown().unwrap();
}

#[test]
fn loopback_auth_precedes_mcp_for_every_request() {
    let secret = secret();
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret.clone()),
        |_| Ok(Probe),
    )
    .unwrap();
    let addr = server.local_addr().unwrap();
    for (credential, status) in [
        (None, "401"),
        (Some("invalid"), "401"),
        (Some(secret.expose()), "200"),
    ] {
        let response = post(addr, credential, "", INITIALIZE);
        assert!(
            response.starts_with(&format!("HTTP/1.1 {status}")),
            "{response}"
        );
        assert!(!response.contains(secret.expose()));
        if status == "401" {
            assert!(
                response
                    .to_ascii_lowercase()
                    .contains("www-authenticate: bearer")
            );
        }
    }
    server.shutdown().unwrap();
    assert!(TcpStream::connect(addr).is_err());
}

#[test]
fn loopback_auth_rejects_nonloopback_bindings_before_listening() {
    for address in ["0.0.0.0:0", "[::]:0", "192.0.2.1:0"] {
        let result = LoopbackHttp::start(
            LoopbackHttpConfig {
                bind: Some(address.parse().unwrap()),
            },
            Some(secret()),
            |_| Ok(Probe),
        );
        let rejected = result.is_err();
        drop(result);
        assert!(rejected, "nonloopback binding was accepted: {address}");
    }
}

#[test]
fn loopback_auth_refuses_browser_origins_even_with_a_valid_credential() {
    let secret = secret();
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret.clone()),
        |_| Ok(Probe),
    )
    .unwrap();
    let addr = server.local_addr().unwrap();
    for origin in [
        "https://attacker.example",
        "null",
        &format!("http://{addr}"),
    ] {
        let response = post(
            addr,
            Some(secret.expose()),
            &format!("Origin: {origin}\r\n"),
            INITIALIZE,
        );
        assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    }
}

#[test]
fn loopback_auth_pins_the_http_authority_to_the_bound_listener() {
    let secret = secret();
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret.clone()),
        |_| Ok(Probe),
    )
    .unwrap();
    let addr = server.local_addr().unwrap();
    for host in [
        "attacker.example",
        "localhost",
        "127.0.0.1:1",
        "127.0.0.2:1",
    ] {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(stream, "POST /mcp HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nAuthorization: Bearer {}\r\nX-Kanban-Capability: 1\r\nContent-Length: {}\r\n\r\n{INITIALIZE}", secret.expose(), INITIALIZE.len()).unwrap();
        let response = read_response(&mut stream).unwrap();
        assert!(response.starts_with("HTTP/1.1 403"), "{host}: {response}");
    }
}

#[test]
fn loopback_auth_secret_exclusion_wraps_framework_envelopes() {
    let secret = secret();
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret.clone()),
        |_| Ok(Probe),
    )
    .unwrap();
    let addr = server.local_addr().unwrap();
    for body in [
        format!(
            r#"{{"jsonrpc":"2.0","id":"{}","method":"tools/list"}}"#,
            secret.expose()
        ),
        format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{{"_meta":{{"io.modelcontextprotocol/protocolVersion":"{}"}}}}}}"#,
            secret.expose()
        ),
        INITIALIZE.replace("fixture", secret.expose()),
        INITIALIZE.replace(
            "fixture",
            &secret
                .expose()
                .chars()
                .map(|c| format!("\\u{:04x}", c as u32))
                .collect::<String>(),
        ),
    ] {
        let response = post(addr, Some(secret.expose()), "", &body);
        assert!(
            !response.contains(secret.expose()),
            "credential was reflected"
        );
        assert!(
            response.starts_with("HTTP/1.1 400"),
            "protected envelope was accepted"
        );
    }
}

#[test]
fn loopback_auth_secret_exclusion_covers_headers_before_sdk_errors() {
    let secret = secret();
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret.clone()),
        |_| Ok(Probe),
    )
    .unwrap();
    let addr = server.local_addr().unwrap();
    for header in ["MCP-Protocol-Version", "Mcp-Method", "Mcp-Name", "X-Extra"] {
        let response = post(
            addr,
            Some(secret.expose()),
            &format!("{header}: {}\r\n", secret.expose()),
            INITIALIZE,
        );
        assert!(
            !response.contains(secret.expose()),
            "header credential reflected"
        );
        assert!(
            response.starts_with("HTTP/1.1 400"),
            "protected header accepted"
        );
    }
}

#[derive(Clone)]
struct LeakingProbe(Arc<InstallationSecret>);
impl ServerHandler for LeakingProbe {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        let mut info = rmcp::model::ServerInfo::default();
        info.server_info.name = self.0.expose().into();
        info
    }
}

#[test]
fn loopback_auth_secret_exclusion_checks_outbound_framework_payloads() {
    let secret = secret();
    let handler_secret = secret.clone();
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret.clone()),
        move |_| Ok(LeakingProbe(handler_secret.clone())),
    )
    .unwrap();
    let response = post(
        server.local_addr().unwrap(),
        Some(secret.expose()),
        "",
        INITIALIZE,
    );
    assert!(
        !response.contains(secret.expose()),
        "outbound credential reflected"
    );
    assert!(response.starts_with("HTTP/1.1 500"));
}

#[test]
fn loopback_auth_rejects_ambiguous_identity_headers_and_routes() {
    let secret = secret();
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret.clone()),
        |_| Ok(Probe),
    )
    .unwrap();
    let addr = server.local_addr().unwrap();
    for header in [
        format!("Authorization: Bearer {}\r\n", secret.expose()),
        "X-Kanban-Capability: 2\r\n".into(),
    ] {
        let response = post(addr, Some(secret.expose()), &header, INITIALIZE);
        assert!(
            response.starts_with("HTTP/1.1 400"),
            "ambiguous identity accepted"
        );
    }
}

fn exchange(addr: SocketAddr, wire: &str) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(wire.as_bytes()).unwrap();
    read_response(&mut stream).unwrap()
}

#[test]
fn loopback_auth_has_only_a_stateless_mcp_route_and_numeric_run_identity() {
    let secret = secret();
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret.clone()),
        |_| Ok(Probe),
    )
    .unwrap();
    let addr = server.local_addr().unwrap();
    for (method, path, capability, status) in [
        ("POST", "/other", "1", 404),
        ("POST", "/mcp?operator=true", "1", 404),
        ("GET", "/mcp", "1", 405),
        ("DELETE", "/mcp", "1", 405),
        ("OPTIONS", "/mcp", "1", 405),
        ("POST", "/mcp", "+1", 400),
        ("POST", "/mcp", "0", 400),
        ("POST", "/mcp", "operator", 400),
    ] {
        let response = exchange(
            addr,
            &format!(
                "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nAuthorization: Bearer {}\r\nX-Kanban-Capability: {capability}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n\r\n{INITIALIZE}",
                secret.expose(),
                INITIALIZE.len()
            ),
        );
        assert!(
            response.starts_with(&format!("HTTP/1.1 {status}")),
            "{method} {path} {capability}: {response}"
        );
    }
}

#[test]
fn loopback_auth_times_out_an_incomplete_body_without_reflection() {
    let secret = secret();
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret.clone()),
        |_| Ok(Probe),
    )
    .unwrap();
    let addr = server.local_addr().unwrap();
    let response = exchange(
        addr,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nAuthorization: Bearer {}\r\nX-Kanban-Capability: 1\r\nContent-Length: 999\r\n\r\n{{",
            secret.expose()
        ),
    );
    assert!(response.starts_with("HTTP/1.1 408"));
    assert!(!response.contains(secret.expose()));
}

#[test]
fn loopback_auth_times_out_an_incomplete_http_header() {
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret()),
        |_| Ok(Probe),
    )
    .unwrap();
    let mut stream = TcpStream::connect(server.local_addr().unwrap()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(b"POST /mcp HTTP/1.1\r\nHost:").unwrap();
    let mut first = [0; 1];
    let response = if stream.read(&mut first).unwrap() == 0 {
        String::new()
    } else {
        read_response(&mut first.as_slice().chain(&mut stream)).unwrap()
    };
    assert!(response.is_empty() || response.starts_with("HTTP/1.1 408"));
}

#[test]
fn loopback_auth_oversized_reflected_metadata_is_refused_without_secret_output() {
    let secret = secret();
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret.clone()),
        |_| Ok(Probe),
    )
    .unwrap();
    let id = format!("{}{}", "x".repeat(4 * 1024 * 1024), secret.expose());
    let body = serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/list"}).to_string();
    let response = post(
        server.local_addr().unwrap(),
        Some(secret.expose()),
        "",
        &body,
    );
    assert!(response.starts_with("HTTP/1.1 413"));
    assert!(!response.contains(secret.expose()));
}

#[test]
fn loopback_auth_shutdown_releases_idle_and_partial_clients_for_rebinding() {
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret()),
        |_| Ok(Probe),
    )
    .unwrap();
    let addr = server.local_addr().unwrap();
    let mut idle = TcpStream::connect(addr).unwrap();
    let mut partial = TcpStream::connect(addr).unwrap();
    idle.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    partial
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    partial.write_all(b"POST /mcp HTTP/1.1\r\nHost:").unwrap();
    let start = std::time::Instant::now();
    drop(server);
    assert!(start.elapsed() < Duration::from_secs(2));
    for client in [&mut idle, &mut partial] {
        match client.read(&mut [0; 1]) {
            Ok(0) => (),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                ) => {}
            other => panic!("client remained open: {other:?}"),
        }
    }
    assert!(TcpStream::connect(addr).is_err());
    let replacement = LoopbackHttp::start(
        LoopbackHttpConfig { bind: Some(addr) },
        Some(secret()),
        |_| Ok(Probe),
    )
    .unwrap();
    assert_eq!(replacement.local_addr(), Some(addr));
    replacement.shutdown().unwrap();
}

#[test]
fn loopback_auth_bind_and_missing_key_fail_closed() {
    let bound = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let config = LoopbackHttpConfig {
        bind: Some(bound.local_addr().unwrap()),
    };
    assert!(LoopbackHttp::start(config, Some(secret()), |_| Ok(Probe)).is_err());
    drop(bound);
    assert!(LoopbackHttp::start(config, None, |_| Ok(Probe)).is_err());
    assert!(TcpStream::connect(config.bind.unwrap()).is_err());
    let server = LoopbackHttp::start(config, Some(secret()), |_| {
        Err::<Probe, _>(kanban_dto::ApiError::internal("planted failure"))
    })
    .unwrap();
    let response = post(
        server.local_addr().unwrap(),
        Some(secret().expose()),
        "",
        INITIALIZE,
    );
    assert!(response.starts_with("HTTP/1.1 403"));
    assert!(!response.contains("planted failure"));
}

#[test]
fn loopback_auth_accepts_the_selected_ipv6_loopback_listener() {
    let secret = secret();
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("[::1]:0".parse().unwrap()),
        },
        Some(secret.clone()),
        |_| Ok(Probe),
    )
    .unwrap();
    let response = post(
        server.local_addr().unwrap(),
        Some(secret.expose()),
        "",
        INITIALIZE,
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
}

#[test]
fn loopback_auth_secret_exclusion_checks_shadowed_raw_json_values() {
    let secret = secret();
    let server = LoopbackHttp::start(
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
        Some(secret.clone()),
        |_| Ok(Probe),
    )
    .unwrap();
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{{"_meta":{{"x":"{}","x":"safe"}}}}}}"#,
        secret.expose()
    );
    let response = post(
        server.local_addr().unwrap(),
        Some(secret.expose()),
        "",
        &body,
    );
    assert!(response.starts_with("HTTP/1.1 400"));
    assert!(!response.contains(secret.expose()));
}

#[derive(Clone)]
struct Probe;
impl ServerHandler for Probe {}

#[test]
fn loopback_default_off_has_no_listener_or_authentication_dependency() {
    let server = LoopbackHttp::start(LoopbackHttpConfig::default(), None, |_| {
        panic!("disabled HTTP must not create an application session");
        #[allow(unreachable_code)]
        Ok(Probe)
    })
    .unwrap();
    assert!(server.local_addr().is_none());
    server.shutdown().unwrap();
}
