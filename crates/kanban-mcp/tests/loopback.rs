//! Real HTTP and socket clients share the production Core, stores, and authority.
use kanban_app::secrets::InstallationSecret;
use kanban_transport::loopback::LoopbackHttpConfig;
use rmcp::{
    ServiceExt,
    model::CallToolRequestParams,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
#[path = "../../kanban-app/tests/common/mod.rs"]
mod common;

struct Fixture {
    service: Option<kanban_service::CoreProcess>,
    h: common::DispatchHarness,
    secret: Arc<InstallationSecret>,
    capability: u64,
    ticket: u64,
}
impl Fixture {
    fn new(enabled: bool) -> Self {
        let h = common::harness();
        let ticket = common::insert_ticket(&h.database_path, 1, "normal");
        common::assign_lane(&h.database_path, ticket);
        let request = h
            .core
            .command(
                "dispatch.request",
                &json!({"mutation": common::mutation(0,"request"), "ticket_id":ticket}),
            )
            .unwrap();
        let claim = h.core.command("dispatch.claim", &json!({"mutation": common::mutation(1,"claim"), "dispatch_request_id":request["id"]})).unwrap();
        h.core.command("run.acknowledge", &json!({"mutation":common::mutation(2,"acknowledge"), "dispatch_request_id":request["id"]})).unwrap();
        let secret = Arc::new(InstallationSecret::from_key(&[37; 32]));
        let service = kanban_service::serve_with_http(
            h._dir.path(),
            kanban_service::ServiceRuntime {
                mcp_executable: env!("CARGO_BIN_EXE_kanban-mcp").into(),
                herdr_socket_root: h._dir.path().join("herdr"),
                installation_secret: Some(secret.clone()),
            },
            LoopbackHttpConfig {
                bind: enabled.then(|| "127.0.0.1:0".parse().unwrap()),
            },
        )
        .unwrap();
        Self {
            service: Some(service),
            h,
            secret,
            capability: claim["capability"]["id"].as_u64().unwrap(),
            ticket,
        }
    }
    fn service(&self) -> &kanban_service::CoreProcess {
        self.service.as_ref().unwrap()
    }
    fn url(&self) -> String {
        format!("http://{}/mcp", self.service().http_address().unwrap())
    }
    async fn client(&self) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
        let config = StreamableHttpClientTransportConfig::with_uri(self.url())
            .auth_header(self.secret.expose())
            .custom_headers(
                [(
                    reqwest::header::HeaderName::from_static("x-kanban-capability"),
                    self.capability.to_string().parse().unwrap(),
                )]
                .into(),
            );
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(5),
            ().serve(StreamableHttpClientTransport::with_client(http, config)),
        )
        .await
        .unwrap()
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(service) = self.service.take() {
            service.shutdown()
        }
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn loopback_auth_matches_socket_allow_deny_revoke_and_idempotency() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let f = Fixture::new(true);
    let http = f.client().await;
    let other = common::insert_ticket(&f.h.database_path, 2, "normal");
    let mut channel = BufReader::new(UnixStream::connect(f.service().socket_path()).unwrap());
    writeln!(
        channel.get_mut(),
        "{}",
        json!({"kind":"agent","payload":{"capability_id":f.capability}})
    )
    .unwrap();
    let mut attached = String::new();
    channel.read_line(&mut attached).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&attached).unwrap()["kind"],
        "response"
    );
    let channel = channel.into_inner();
    channel.set_nonblocking(true).unwrap();
    let socket = ().serve(tokio::net::UnixStream::from_std(channel).unwrap()).await.unwrap();
    let http_tools = http.list_all_tools().await.unwrap();
    let socket_tools = socket.list_all_tools().await.unwrap();
    assert_eq!(http_tools, socket_tools);
    assert!(http_tools.iter().any(|tool| tool.name == "ticket_get"));
    assert!(!http_tools.iter().any(|tool| tool.name == "ticket_override"));
    for (name, args, denied) in [
        ("ticket_get", json!({"ticket_id":f.ticket}), false),
        ("ticket_get", json!({"ticket_id":other}), true),
        (
            "ticket_get",
            json!({"ticket_id":f.ticket, "operator":true}),
            true,
        ),
        (
            "initiative_create",
            json!({"mutation":common::mutation(0,"operator"), "name":"spoof"}),
            true,
        ),
        (
            "comment_create",
            json!({"mutation":common::mutation(0,"same-replay"), "project_id":1, "target":{"kind":"ticket","id":f.ticket.to_string()}, "text":"one durable comment"}),
            false,
        ),
    ] {
        let first = http.call_tool(call(name, args.clone())).await.unwrap();
        let second = socket.call_tool(call(name, args.clone())).await.unwrap();
        let replay = http.call_tool(call(name, args)).await.unwrap();
        assert_eq!(first.is_error.unwrap_or(false), denied, "{name}");
        assert_eq!(first, second, "socket/HTTP {name}");
        assert_eq!(first, replay, "HTTP replay {name}");
    }
    let conn = rusqlite::Connection::open(&f.h.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM comments", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    conn.execute(
        "UPDATE capabilities SET status = 'settled', settled_at = 100 WHERE id = ?1",
        [i64::try_from(f.capability).unwrap()],
    )
    .unwrap();
    let replay = json!({"mutation":common::mutation(0,"same-replay"), "project_id":1, "target":{"kind":"ticket","id":f.ticket.to_string()}, "text":"one durable comment"});
    assert!(
        socket
            .call_tool(call("comment_create", replay.clone()))
            .await
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    assert!(
        http.call_tool(call("comment_create", replay))
            .await
            .is_err()
    );
    assert!(http.list_all_tools().await.is_err());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM comments", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    http.cancel().await.unwrap();
    socket.cancel().await.unwrap();
}

#[test]
fn loopback_default_off_service_still_serves_only_the_unix_socket() {
    let f = Fixture::new(false);
    assert!(f.service().http_address().is_none());
    assert!(std::os::unix::net::UnixStream::connect(f.service().socket_path()).is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_auth_secret_exclusion_covers_evidence_and_managed_artifacts() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let mut f = Fixture::new(true);
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let args = json!({"mutation":common::mutation(0,"secret-evidence"), "project_id":1, "entity_kind":"ticket", "entity_id":f.ticket.to_string(), "evidence_kind":"managed_file", "content_base64":STANDARD.encode(f.secret.expose())});
    let body = json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"evidence_attach","arguments":args}});
    let response = http
        .post(f.url())
        .bearer_auth(f.secret.expose())
        .header("x-kanban-capability", f.capability)
        .header("accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let result: Value = response.json().await.unwrap();
    assert_eq!(result["result"]["isError"], true);
    assert!(!result.to_string().contains(f.secret.expose()));
    for input in [
        json!({"jsonrpc":"2.0","id":f.secret.expose(),"method":"not-a-method"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"comment_create","arguments":{"mutation":common::mutation(0,"raw-secret"),"project_id":1,"target":{"kind":"ticket","id":f.ticket.to_string()},"text":f.secret.expose()}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":f.secret.expose()}}}),
    ] {
        let response = http
            .post(f.url())
            .bearer_auth(f.secret.expose())
            .header("x-kanban-capability", f.capability)
            .header("accept", "application/json, text/event-stream")
            .json(&input)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert!(!response.text().await.unwrap().contains(f.secret.expose()));
    }
    let client = f.client().await;
    let evidence = client
        .call_tool(call(
            "evidence_list",
            json!({"project_id":1,"entity_kind":"ticket","entity_id":f.ticket.to_string()}),
        ))
        .await
        .unwrap();
    assert_eq!(evidence.structured_content.unwrap()["evidence"], json!([]));
    client.cancel().await.unwrap();
    f.service.take().unwrap().shutdown();
    fn scan(path: &std::path::Path, secret: &str) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                scan(&entry.path(), secret)
            } else if entry.file_type().unwrap().is_file() {
                let bytes = std::fs::read(entry.path()).unwrap();
                for needle in [secret.as_bytes(), &[37; 32]] {
                    assert!(
                        !bytes.windows(needle.len()).any(|part| part == needle),
                        "secret persisted in managed artifact"
                    );
                }
            }
        }
    }
    scan(f.h._dir.path(), f.secret.expose());
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_auth_rechecks_credentials_before_a_recorded_replay() {
    let f = Fixture::new(true);
    let client = f.client().await;
    let args = json!({"mutation":common::mutation(0,"authenticated-replay"), "project_id":1,"target":{"kind":"ticket","id":f.ticket.to_string()},"text":"only once"});
    assert!(
        !client
            .call_tool(call("comment_create", args.clone()))
            .await
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    for token in [None, Some("invalid")] {
        let mut request = http.post(f.url()).header("x-kanban-capability", f.capability).header("accept","application/json, text/event-stream").json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"comment_create","arguments":args}}));
        if let Some(token) = token {
            request = request.bearer_auth(token)
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 401);
        assert!(!response.text().await.unwrap().contains("only once"));
    }
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_auth_service_shutdown_disconnects_live_clients_and_can_restart() {
    let mut f = Fixture::new(true);
    let client = f.client().await;
    let addr = f.service().http_address().unwrap();
    let _idle = std::net::TcpStream::connect(addr).unwrap();
    f.service.take().unwrap().shutdown();
    assert!(std::net::TcpStream::connect(addr).is_err());
    let result = tokio::time::timeout(Duration::from_secs(2), client.list_all_tools())
        .await
        .unwrap();
    assert!(result.is_err());
    client.cancel().await.unwrap();
    let restarted = kanban_service::serve_with_http(
        f.h._dir.path(),
        kanban_service::ServiceRuntime {
            mcp_executable: env!("CARGO_BIN_EXE_kanban-mcp").into(),
            herdr_socket_root: f.h._dir.path().join("herdr"),
            installation_secret: Some(f.secret.clone()),
        },
        LoopbackHttpConfig { bind: Some(addr) },
    )
    .unwrap();
    assert_eq!(restarted.http_address(), Some(addr));
    restarted.shutdown();
}

fn call(name: &str, arguments: Value) -> CallToolRequestParams {
    CallToolRequestParams::new(name.to_owned())
        .with_arguments(arguments.as_object().unwrap().clone())
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_auth_sdk_client_reaches_the_running_core() {
    let f = Fixture::new(true);
    assert!(f.h.database_path.is_file());
    let client = f.client().await;
    let result = client
        .call_tool(call("ticket_get", json!({"ticket_id":f.ticket})))
        .await
        .unwrap();
    assert_eq!(result.structured_content.unwrap()["id"], f.ticket);
    client.cancel().await.unwrap();
}
