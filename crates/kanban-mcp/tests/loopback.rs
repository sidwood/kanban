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
use std::io::{BufRead, Write};
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
        let ticket = common::insert_ready_ticket(&h.database_path, 1, "normal");
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
    let comment = json!({"mutation":common::mutation(0,"same-replay"), "project_id":1, "target":{"kind":"ticket","id":f.ticket.to_string()}, "text":"one durable comment", "author":"operator"});
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
        ("comment_create", comment.clone(), false),
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
    // The replay reuses the original request verbatim, so its
    // idempotency fingerprint matches: only the revoked authority can
    // explain a refusal, never a request-body mismatch.
    let refused = socket
        .call_tool(call("comment_create", comment.clone()))
        .await
        .unwrap();
    assert!(
        refused.is_error.unwrap_or(false),
        "a revoked capability cannot replay its recorded request"
    );
    let refusal = refused.structured_content.unwrap();
    assert_eq!(refusal["code"], json!("invalid_request"));
    assert_eq!(
        refusal["message"],
        json!("operation is outside the authenticated run capability"),
        "the refusal names the authority, not the idempotency key: {refusal}"
    );
    assert!(
        http.call_tool(call("comment_create", comment))
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
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"comment_create","arguments":{"mutation":common::mutation(0,"raw-secret"),"project_id":1,"target":{"kind":"ticket","id":f.ticket.to_string()},"text":f.secret.expose(),"author":"operator"}}}),
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
    let args = json!({"mutation":common::mutation(0,"authenticated-replay"), "project_id":1,"target":{"kind":"ticket","id":f.ticket.to_string()},"text":"only once","author":"operator"});
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

/// A client on the installation-authenticated operator route: bearer
/// credential only, no capability header.
async fn operator_client(
    service: &kanban_service::CoreProcess,
    secret: &InstallationSecret,
) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let uri = format!("http://{}/mcp/operator", service.http_address().unwrap());
    let config = StreamableHttpClientTransportConfig::with_uri(uri).auth_header(secret.expose());
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

#[tokio::test(flavor = "multi_thread")]
async fn operator_route_serves_one_comment_stream_per_issue() {
    let h = common::harness();
    let secret = Arc::new(InstallationSecret::from_key(&[41; 32]));
    let service = kanban_service::serve_with_http(
        h._dir.path(),
        kanban_service::ServiceRuntime {
            mcp_executable: env!("CARGO_BIN_EXE_kanban-mcp").into(),
            herdr_socket_root: h._dir.path().join("herdr"),
            installation_secret: Some(secret.clone()),
        },
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
    )
    .unwrap();
    let client = operator_client(&service, &secret).await;

    let issue = client
        .call_tool(call(
            "save_issue",
            json!({"mutation":common::mutation(0,"stream-issue"), "project_id":1, "title":"Stream host"}),
        ))
        .await
        .unwrap();
    let issue = issue.structured_content.unwrap();
    let key = issue["identifier"].as_str().unwrap().to_owned();
    let ticket_id = issue["ticket_id"].as_u64().unwrap();

    let by_key = client
        .call_tool(call(
            "comment_create",
            json!({"mutation":common::mutation(0,"stream-by-key"), "project_id":1,
                   "target":{"kind":"ticket","id":key}, "text":"Posted by key", "author":"lin"}),
        ))
        .await
        .unwrap();
    let by_key = by_key.structured_content.unwrap();
    assert_eq!(
        by_key["target"],
        json!({"kind":"ticket","id":ticket_id.to_string()}),
        "a comment addressed by key stores the issue id"
    );
    client
        .call_tool(call(
            "comment_create",
            json!({"mutation":common::mutation(0,"stream-by-id"), "project_id":1,
                   "target":{"kind":"ticket","id":ticket_id.to_string()}, "text":"Posted by id", "author":"lin"}),
        ))
        .await
        .unwrap();

    let listed_by_id = client
        .call_tool(call(
            "list_comments",
            json!({"target":{"kind":"ticket","id":ticket_id.to_string()}, "limit":10}),
        ))
        .await
        .unwrap();
    let listed_by_key = client
        .call_tool(call(
            "list_comments",
            json!({"target":{"kind":"ticket","id":key}, "limit":10}),
        ))
        .await
        .unwrap();
    let stream = listed_by_id.structured_content.unwrap();
    assert_eq!(
        stream["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|comment| comment["body"].clone())
            .collect::<Vec<_>>(),
        vec![json!("Posted by key"), json!("Posted by id")],
        "both address forms read and write one stream"
    );
    assert_eq!(
        listed_by_key.structured_content.unwrap()["comments"],
        stream["comments"]
    );

    let unknown = client
        .call_tool(call(
            "comment_create",
            json!({"mutation":common::mutation(0,"stream-unknown"), "project_id":1,
                   "target":{"kind":"ticket","id":"CORE-999"}, "text":"Ghost", "author":"lin"}),
        ))
        .await
        .unwrap();
    assert!(unknown.is_error.unwrap_or(false));
    assert_eq!(
        unknown.structured_content.unwrap()["code"],
        json!("not_found")
    );

    let root_id = by_key["id"].as_u64().unwrap();
    let reply = client
        .call_tool(call(
            "comment_create",
            json!({"mutation":common::mutation(0,"stream-reply"), "project_id":1,
                   "target":{"kind":"ticket","id":ticket_id.to_string()}, "text":"Reply", "author":"lin",
                   "parent_comment_id":root_id}),
        ))
        .await
        .unwrap();
    assert!(!reply.is_error.unwrap_or(true));
    let nested = client
        .call_tool(call(
            "comment_create",
            json!({"mutation":common::mutation(0,"stream-nested"), "project_id":1,
                   "target":{"kind":"ticket","id":ticket_id.to_string()}, "text":"Reply to the reply", "author":"lin",
                   "parent_comment_id":reply.structured_content.unwrap()["id"].as_u64().unwrap()}),
        ))
        .await
        .unwrap();
    assert!(nested.is_error.unwrap_or(false));
    let nested = nested.structured_content.unwrap();
    assert_eq!(nested["code"], json!("invalid_request"));
    assert_eq!(nested["field"], json!("parent_comment_id"));

    let unnamed = client
        .call_tool(call(
            "comment_create",
            json!({"mutation":common::mutation(0,"stream-unnamed"), "project_id":1,
                   "target":{"kind":"ticket","id":ticket_id.to_string()}, "text":"No author"}),
        ))
        .await
        .unwrap();
    assert!(unnamed.is_error.unwrap_or(false));
    let unnamed = unnamed.structured_content.unwrap();
    assert_eq!(unnamed["code"], json!("invalid_request"));
    assert_eq!(unnamed["field"], json!("author"));

    let resolved = client
        .call_tool(call(
            "resolve_comment",
            json!({"mutation":common::mutation(1,"stream-resolve"), "comment_id":root_id, "resolved":true}),
        ))
        .await
        .unwrap();
    assert_eq!(
        resolved.structured_content.unwrap()["version"],
        json!(2),
        "resolving bumps the comment version"
    );

    // Comments stored before targets resolved carry whichever
    // reference spelling their caller passed; they keep listing
    // through the operator route beside canonical rows.
    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    let code = key
        .rsplit_once('-')
        .expect("the key carries a code")
        .0
        .to_owned();
    for (n, stored) in [
        format!("0{ticket_id}"),
        format!("+{ticket_id}"),
        format!("{code}-0{ticket_id}"),
    ]
    .iter()
    .enumerate()
    {
        conn.execute(
            "INSERT INTO comments (project_id, entity_kind, entity_id, version, author)
             VALUES (1, 'ticket', ?1, 1, 'legacy')",
            [stored],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO comment_revisions (comment_id, revision, text)
             VALUES (?1, 1, ?2)",
            rusqlite::params![id, format!("legacy-{n}")],
        )
        .unwrap();
    }
    drop(conn);
    let listed = client
        .call_tool(call(
            "list_comments",
            json!({"target":{"kind":"ticket","id":key}, "limit":10}),
        ))
        .await
        .unwrap();
    let stream = listed.structured_content.unwrap();
    let bodies: Vec<_> = stream["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["body"].clone())
        .collect();
    assert_eq!(
        bodies,
        vec![
            json!("Posted by key"),
            json!("Posted by id"),
            json!("Reply"),
            json!("legacy-0"),
            json!("legacy-1"),
            json!("legacy-2"),
        ],
        "historically stored spellings keep listing over the operator route"
    );

    // The advertised contract demands the author the runtime refuses
    // to default.
    let tools = client.list_all_tools().await.unwrap();
    let create_tool = tools
        .iter()
        .find(|tool| tool.name == "comment_create")
        .expect("the create tool is listed");
    let create_schema = serde_json::to_value(&create_tool.input_schema).unwrap();
    let required: Vec<_> = create_schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    assert!(
        required.contains(&"author"),
        "the live input schema must require author: {required:?}"
    );

    client.cancel().await.unwrap();
    service.shutdown();
}

fn wait_for_live_event(
    reader: &mut std::io::BufReader<std::os::unix::net::UnixStream>,
    want: &str,
    ticket_id: u64,
) -> Value {
    reader
        .get_mut()
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut line = String::new();
    let mut seen = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let Ok(frame) = serde_json::from_str::<Value>(line.trim()) else {
                    continue;
                };
                if frame["kind"] != "event" {
                    continue;
                }
                let event = frame["event"].clone();
                seen.push(event["event_type"].clone());
                if event["event_type"] == want && event["payload"]["id"] == json!(ticket_id) {
                    return event;
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(error) => panic!("the event stream stopped: {error}"),
        }
    }
    panic!("no {want} for ticket {ticket_id}; saw {seen:?}");
}
fn collect_ticket_edits(
    reader: &mut std::io::BufReader<std::os::unix::net::UnixStream>,
    ids: &[u64],
) -> Vec<Value> {
    reader
        .get_mut()
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut remaining: Vec<u64> = ids.to_vec();
    let mut found = Vec::new();
    let mut line = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !remaining.is_empty() && std::time::Instant::now() < deadline {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let Ok(frame) = serde_json::from_str::<Value>(line.trim()) else {
                    continue;
                };
                if frame["kind"] != "event" || frame["event"]["event_type"] != "ticket.edited" {
                    continue;
                }
                let Some(id) = frame["event"]["payload"]["id"].as_u64() else {
                    continue;
                };
                if let Some(position) = remaining.iter().position(|wanted| *wanted == id) {
                    remaining.swap_remove(position);
                    found.push(frame["event"].clone());
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(error) => panic!("the event stream stopped: {error}"),
        }
    }
    assert!(
        remaining.is_empty(),
        "missing ticket.edited for {remaining:?}; saw {found:?}"
    );
    found
}

fn subscribe_live_events(
    service: &kanban_service::CoreProcess,
) -> std::io::BufReader<std::os::unix::net::UnixStream> {
    let mut stream = std::os::unix::net::UnixStream::connect(service.socket_path()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(stream, "{{\"kind\":\"subscribe\"}}").unwrap();
    stream.flush().unwrap();
    let mut reader = std::io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(
        line.contains("\"kind\":\"subscribed\""),
        "the socket subscribed: {line}"
    );
    reader
}

#[tokio::test(flavor = "multi_thread")]
async fn operator_route_applies_an_omitted_mutation_and_publishes_the_edit() {
    let h = common::harness();
    let secret = Arc::new(InstallationSecret::from_key(&[47; 32]));
    let service = kanban_service::serve_with_http(
        h._dir.path(),
        kanban_service::ServiceRuntime {
            mcp_executable: env!("CARGO_BIN_EXE_kanban-mcp").into(),
            herdr_socket_root: h._dir.path().join("herdr"),
            installation_secret: Some(secret.clone()),
        },
        LoopbackHttpConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        },
    )
    .unwrap();
    let mut events = subscribe_live_events(&service);
    let client = operator_client(&service, &secret).await;

    let tools = client.list_all_tools().await.unwrap();
    let save_tool = tools
        .iter()
        .find(|tool| tool.name == "save_issue")
        .expect("save_issue is listed");
    let save_schema = serde_json::to_value(&save_tool.input_schema).unwrap();
    let required = save_schema["required"].as_array();
    assert!(
        required.is_none_or(|fields| !fields.iter().any(|field| field == "mutation")),
        "the live operator schema must not require mutation: {save_schema}"
    );

    let created = client
        .call_tool(call(
            "save_issue",
            json!({ "project_id": 1, "title": "Written without a mutation" }),
        ))
        .await
        .unwrap();
    assert!(
        !created.is_error.unwrap_or(false),
        "an omitted mutation still creates: {created:?}"
    );
    let created = created.structured_content.unwrap();
    let identifier = created["identifier"].as_str().unwrap().to_owned();
    let ticket_id = created["ticket_id"].as_u64().unwrap();
    let created_event = wait_for_live_event(&mut events, "ticket.created", ticket_id);
    assert_eq!(
        created_event["payload"]["title"],
        json!("Written without a mutation")
    );

    let renamed = client
        .call_tool(call(
            "save_issue",
            json!({ "identifier": identifier, "title": "Renamed without a mutation" }),
        ))
        .await
        .unwrap();
    assert!(!renamed.is_error.unwrap_or(false), "{renamed:?}");
    let edited = wait_for_live_event(&mut events, "ticket.edited", ticket_id);
    assert_eq!(
        edited["payload"]["title"],
        json!("Renamed without a mutation")
    );

    let stale = client
        .call_tool(call(
            "save_issue",
            json!({
                "mutation": common::mutation(0, "stale-operator-write"),
                "identifier": identifier,
                "title": "Stale writer",
            }),
        ))
        .await
        .unwrap();
    assert!(
        stale.is_error.unwrap_or(false),
        "a sent stale version is refused"
    );
    assert_eq!(
        stale.structured_content.unwrap()["code"],
        json!("stale_version")
    );
    let kept = client
        .call_tool(call("get_issue", json!({ "identifier": identifier })))
        .await
        .unwrap();
    assert_eq!(
        kept.structured_content.unwrap()["title"],
        json!("Renamed without a mutation")
    );

    let other = client
        .call_tool(call(
            "save_issue",
            json!({ "project_id": 1, "title": "Blocked neighbour" }),
        ))
        .await
        .unwrap();
    let other = other.structured_content.unwrap();
    let other_key = other["identifier"].as_str().unwrap().to_owned();
    let other_before = client
        .call_tool(call("get_issue", json!({ "identifier": other_key })))
        .await
        .unwrap()
        .structured_content
        .unwrap()["version"]
        .clone();
    let blocked = client
        .call_tool(call(
            "save_issue",
            json!({ "identifier": identifier, "add_blocks": [other_key] }),
        ))
        .await
        .unwrap();
    assert!(!blocked.is_error.unwrap_or(false), "{blocked:?}");
    let host = client
        .call_tool(call("get_issue", json!({ "identifier": identifier })))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert!(
        host["relations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|relation| relation["kind"] == "blocks" && relation["identifier"] == other_key),
        "the block is visible on the writer: {}",
        host["relations"]
    );
    let neighbour = client
        .call_tool(call("get_issue", json!({ "identifier": other_key })))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert!(
        neighbour["relations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|relation| {
                relation["kind"] == "blocked-by" && relation["identifier"] == identifier
            }),
        "the other end lists the same edge: {}",
        neighbour["relations"]
    );
    assert!(
        neighbour["version"].as_u64().unwrap() > other_before.as_u64().unwrap(),
        "the other end of the block is versioned"
    );
    let other_id = other["ticket_id"]
        .as_u64()
        .expect("the neighbour has an id");
    let edits = collect_ticket_edits(&mut events, &[ticket_id, other_id]);
    assert!(
        edits
            .iter()
            .any(|event| event["payload"]["id"] == json!(other_id)),
        "the waiting issue is named on the live socket: {edits:?}"
    );
    assert!(
        edits
            .iter()
            .any(|event| event["payload"]["id"] == json!(ticket_id)),
        "the writer is still named: {edits:?}"
    );

    client.cancel().await.unwrap();
    service.shutdown();
}
