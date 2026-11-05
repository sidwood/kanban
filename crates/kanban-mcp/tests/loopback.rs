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
        Self::configured(enabled, None)
    }
    fn configured(enabled: bool, operator_name: Option<&str>) -> Self {
        let h = common::harness();
        if let Some(name) = operator_name {
            std::fs::write(
                h._dir.path().join("config.json"),
                json!({"operator_name":name}).to_string(),
            )
            .unwrap();
        }
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

/// Comments carried over from the ticket model recorded no author. The
/// operator route lists them under a placeholder rather than a blank
/// name, and leaves the stored row as it was.
#[tokio::test(flavor = "multi_thread")]
async fn operator_route_lists_an_unrecorded_author_as_unknown() {
    let h = common::harness();
    let secret = Arc::new(InstallationSecret::from_key(&[43; 32]));
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
            json!({"mutation":common::mutation(0,"unsigned-issue"), "project_id":1, "title":"Unsigned host"}),
        ))
        .await
        .unwrap();
    let issue = issue.structured_content.unwrap();
    let key = issue["identifier"].as_str().unwrap().to_owned();
    let ticket_id = issue["ticket_id"].as_u64().unwrap();

    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    conn.execute(
        "INSERT INTO comments (project_id, entity_kind, entity_id, version, author)
         VALUES (1, 'ticket', ?1, 1, '')",
        [ticket_id.to_string()],
    )
    .unwrap();
    let unsigned = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO comment_revisions (comment_id, revision, text) VALUES (?1, 1, 'Imported')",
        [unsigned],
    )
    .unwrap();
    drop(conn);
    client
        .call_tool(call(
            "comment_create",
            json!({"mutation":common::mutation(0,"unsigned-signed"), "project_id":1,
                   "target":{"kind":"ticket","id":key}, "text":"Signed", "author":"lin"}),
        ))
        .await
        .unwrap();

    let listed = client
        .call_tool(call(
            "list_comments",
            json!({"target":{"kind":"ticket","id":key}, "limit":10}),
        ))
        .await
        .unwrap();
    let authors: Vec<_> = listed.structured_content.unwrap()["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["body"].clone(), row["author"].clone()))
        .collect();
    assert_eq!(
        authors,
        vec![
            (json!("Imported"), json!("Unknown author")),
            (json!("Signed"), json!("lin")),
        ]
    );

    let conn = rusqlite::Connection::open(&h.database_path).unwrap();
    let stored: String = conn
        .query_row(
            "SELECT author FROM comments WHERE id = ?1",
            [unsigned],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, "", "listing never rewrites the stored author");

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
fn collect_issue_events(
    reader: &mut std::io::BufReader<std::os::unix::net::UnixStream>,
    event_type: &str,
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
                if frame["kind"] != "event" || frame["event"]["event_type"] != event_type {
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
        "missing {event_type} for {remaining:?}; saw {found:?}"
    );
    found
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

#[tokio::test(flavor = "multi_thread")]
async fn operator_route_names_every_issue_a_save_changes() {
    let h = common::harness();
    let secret = Arc::new(InstallationSecret::from_key(&[53; 32]));
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

    let mentioned = client
        .call_tool(call(
            "save_issue",
            json!({ "project_id": 1, "title": "Mentioned over HTTP" }),
        ))
        .await
        .unwrap();
    assert!(!mentioned.is_error.unwrap_or(false), "{mentioned:?}");
    let mentioned = mentioned.structured_content.unwrap();
    let mentioned_id = mentioned["ticket_id"].as_u64().unwrap();
    let mentioned_key = mentioned["identifier"].as_str().unwrap();
    let saved = client
        .call_tool(call(
            "save_issue",
            json!({
                "project_id": 1,
                "title": "Source over HTTP",
                "description": format!("See {mentioned_key} now"),
            }),
        ))
        .await
        .unwrap();
    assert!(!saved.is_error.unwrap_or(false), "{saved:?}");
    collect_issue_events(&mut events, "ticket.edited", &[mentioned_id]);

    client
        .call_tool(call(
            "update_project_settings",
            json!({ "project_id": 1, "auto_close_sub_issues": true }),
        ))
        .await
        .unwrap();
    let grandparent = client
        .call_tool(call(
            "save_issue",
            json!({ "project_id": 1, "title": "Grandparent over HTTP" }),
        ))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let parent = client
        .call_tool(call(
            "save_issue",
            json!({
                "project_id": 1,
                "title": "Parent over HTTP",
                "parent_id": grandparent["identifier"],
            }),
        ))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let child = client
        .call_tool(call(
            "save_issue",
            json!({
                "project_id": 1,
                "title": "Child over HTTP",
                "parent_id": parent["identifier"],
            }),
        ))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let closed = client
        .call_tool(call(
            "save_issue",
            json!({ "identifier": child["identifier"], "state": "Done" }),
        ))
        .await
        .unwrap();
    assert!(!closed.is_error.unwrap_or(false), "{closed:?}");
    collect_issue_events(
        &mut events,
        "ticket.state.changed",
        &[
            parent["ticket_id"].as_u64().unwrap(),
            grandparent["ticket_id"].as_u64().unwrap(),
        ],
    );

    let target = client
        .call_tool(call(
            "save_issue",
            json!({ "project_id": 1, "title": "Canonical over HTTP" }),
        ))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let duplicate = client
        .call_tool(call(
            "save_issue",
            json!({ "project_id": 1, "title": "Duplicate over HTTP" }),
        ))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let marked = client
        .call_tool(call(
            "save_issue",
            json!({
                "identifier": duplicate["identifier"],
                "duplicate_of": target["identifier"],
            }),
        ))
        .await
        .unwrap();
    assert!(!marked.is_error.unwrap_or(false), "{marked:?}");
    let target_id = target["ticket_id"].as_u64().unwrap();
    collect_issue_events(&mut events, "ticket.edited", &[target_id]);
    let read = client
        .call_tool(call(
            "get_issue",
            json!({ "identifier": target["identifier"] }),
        ))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(
        read["duplicates"],
        json!([duplicate["identifier"]]),
        "the target lists the duplicate"
    );
    assert!(
        read["version"].as_u64().unwrap() > 1,
        "the target membership change is visible over HTTP"
    );

    client.cancel().await.unwrap();
    service.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn operator_route_accepts_linear_shaped_calls() {
    let h = common::harness();
    std::fs::write(
        h._dir.path().join("config.json"),
        r#"{"operator_name":"Ada"}"#,
    )
    .unwrap();
    let secret = Arc::new(InstallationSecret::from_key(&[61; 32]));
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
    let tools = client.list_all_tools().await.unwrap();
    let save_tool = tools
        .iter()
        .find(|tool| tool.name == "save_issue")
        .expect("save_issue is listed");
    let described = serde_json::to_value(&save_tool.input_schema).unwrap()["properties"]["state"]
        ["description"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert!(
        described.contains("Draft") && described.contains("Duplicate"),
        "tools/list describes state with the Kanban names: {described}"
    );
    assert!(
        tools.iter().any(|tool| tool.name == "save_comment"),
        "save_comment is offered"
    );

    let label = client
        .call_tool(call(
            "create_issue_label",
            json!({
                "project_id": 1,
                "name": "Web",
                "color": "#336699",
                "group": "Project",
                "group_single_select": true
            }),
        ))
        .await
        .unwrap();
    assert!(!label.is_error.unwrap_or(false), "{label:?}");
    let milestone = client
        .call_tool(call(
            "save_milestone",
            json!({ "project_id": 1, "name": "M1 Foundations" }),
        ))
        .await
        .unwrap();
    assert!(!milestone.is_error.unwrap_or(false), "{milestone:?}");

    let created = client
        .call_tool(call(
            "save_issue",
            json!({
                "team": "CORE",
                "project": "Web",
                "milestone": "M1 Foundations",
                "title": "Named over HTTP",
                "description": "Full text",
                "assignee": "me"
            }),
        ))
        .await
        .unwrap();
    assert!(!created.is_error.unwrap_or(false), "{created:?}");
    let created = created.structured_content.unwrap();
    let key = created["identifier"].as_str().unwrap().to_owned();
    assert_eq!(key, "CORE-1", "{key}");

    let read = client
        .call_tool(call("get_issue", json!({ "id": key })))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(read["team"], json!("CORE"));
    assert_eq!(read["project"], json!("Web"));
    assert_eq!(read["milestone"], json!("M1 Foundations"));
    assert_eq!(read["creator"], json!("Ada"));
    assert_eq!(read["assignee"], json!("Ada"));

    let listed = client
        .call_tool(call("list_issues", json!({ "project": "Web" })))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert!(listed["issues"][0].get("description").is_none());
    let detailed = client
        .call_tool(call(
            "list_issues",
            json!({ "project": "Web", "fields": ["description"] }),
        ))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(detailed["issues"][0]["description"], json!("Full text"));

    let second = client
        .call_tool(call(
            "save_issue",
            json!({ "project_id": 1, "title": "Second over HTTP" }),
        ))
        .await
        .unwrap();
    assert!(!second.is_error.unwrap_or(false), "{second:?}");
    assert_eq!(
        second.structured_content.unwrap()["identifier"],
        json!("CORE-2")
    );

    let posted = client
        .call_tool(call(
            "save_comment",
            json!({ "issue": "CORE-2", "body": "Ship report" }),
        ))
        .await
        .unwrap();
    assert!(!posted.is_error.unwrap_or(false), "{posted:?}");
    let posted = posted.structured_content.unwrap();
    let comment_id = posted["id"].as_u64().expect("comment id");
    assert_eq!(posted["identifier"], json!("CORE-2"));
    assert_eq!(posted["url"], json!("kanban://issue/CORE-2"));

    let comments = client
        .call_tool(call(
            "list_comments",
            json!({"target": {"kind": "ticket", "id": "CORE-2"}, "limit": 10}),
        ))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(comments["comments"][0]["author"], json!("Ada"));
    assert!(comments["comments"][0].get("parent_comment_id").is_none());

    let reply = client
        .call_tool(call(
            "save_comment",
            json!({
                "issue": "CORE-2",
                "body": "Noted",
                "author": "lin",
                "parentId": comment_id
            }),
        ))
        .await
        .unwrap();
    assert!(!reply.is_error.unwrap_or(false), "{reply:?}");
    let reply = reply.structured_content.unwrap();
    assert_eq!(reply["identifier"], json!("CORE-2"));
    assert_eq!(reply["url"], json!("kanban://issue/CORE-2"));
    let comments = client
        .call_tool(call(
            "list_comments",
            json!({"target": {"kind": "ticket", "id": "CORE-2"}, "limit": 10}),
        ))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(comments["comments"][1]["author"], json!("lin"));
    assert_eq!(
        comments["comments"][1]["parent_comment_id"],
        json!(comment_id)
    );

    let labels = client
        .call_tool(call("list_issue_labels", json!({ "project_id": 1 })))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert!(
        labels["labels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["name"] == "Web"),
        "{labels}"
    );
    let milestones = client
        .call_tool(call("list_milestones", json!({ "project_id": 1 })))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert!(
        milestones["milestones"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["name"] == "M1 Foundations"),
        "{milestones}"
    );

    let blocker = client
        .call_tool(call(
            "save_issue",
            json!({ "project_id": 1, "title": "Blocker over HTTP" }),
        ))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let child = client
        .call_tool(call(
            "save_issue",
            json!({
                "title": "Child over HTTP",
                "parentId": "CORE-1",
                "dueDate": "2026-12-01",
                "blockedBy": [blocker["identifier"]]
            }),
        ))
        .await
        .unwrap();
    assert!(!child.is_error.unwrap_or(false), "{child:?}");
    let child_key = child.structured_content.unwrap()["identifier"]
        .as_str()
        .unwrap()
        .to_owned();
    let child_read = client
        .call_tool(call("get_issue", json!({ "id": child_key })))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(child_read["parent"], json!("CORE-1"));
    assert_eq!(child_read["due_date"], json!("2026-12-01"));
    assert!(
        child_read["relations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| {
                row["kind"] == "blocked-by" && row["identifier"] == blocker["identifier"]
            }),
        "{child_read}"
    );
    let renamed = client
        .call_tool(call(
            "save_issue",
            json!({ "id": child_key, "title": "Child renamed over HTTP" }),
        ))
        .await
        .unwrap();
    assert!(!renamed.is_error.unwrap_or(false), "{renamed:?}");
    let renamed = client
        .call_tool(call("get_issue", json!({ "id": child_key })))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(renamed["title"], json!("Child renamed over HTTP"));
    let children = client
        .call_tool(call("list_issues", json!({ "parentId": "CORE-1" })))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert!(
        children["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["identifier"] == child_key),
        "{children}"
    );

    let refused = client
        .call_tool(call(
            "save_issue",
            json!({ "project_id": 1, "title": "Bad state", "state": "Todo" }),
        ))
        .await
        .unwrap();
    assert!(refused.is_error.unwrap_or(false));
    let message = refused.structured_content.unwrap()["message"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    assert!(
        message.contains("Draft") && message.contains("Todo"),
        "{message}"
    );

    client.cancel().await.unwrap();
    service.shutdown();
}

fn validate_schemas(cases: Value) {
    use std::process::{Command, Stdio};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = Command::new("node")
        .current_dir(root)
        .args(["-e", r#"
            const fs = require('node:fs');
            const assert = require('node:assert/strict');
            const Ajv = require('./node_modules/.pnpm/node_modules/ajv');
            const cases = JSON.parse(fs.readFileSync(0, 'utf8'));
            for (const {name, schema, body, valid} of cases) {
                const check = new Ajv({allErrors:true, unknownFormats:'ignore', logger:false}).compile(schema);
                assert.equal(check(body), valid, name + ': ' + JSON.stringify(check.errors));
            }
            console.log('Validated ' + cases.length + ' schema payloads');
        "#])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("Node and the installed web dependencies are available");
    serde_json::to_writer(child.stdin.take().unwrap(), &cases).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operator_comment_schemas_allow_the_default_author_only_for_operators() {
    let f = Fixture::configured(true, Some("Ada"));
    let operator = operator_client(f.service(), &f.secret).await;
    let operator_tools = operator.list_all_tools().await.unwrap();
    let run = f.client().await;
    let run_tools = run.list_all_tools().await.unwrap();
    let mut cases = Vec::new();
    for name in ["comment_create", "save_comment"] {
        let tool = operator_tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap();
        cases.push(json!({"name":name,"schema":tool.input_schema,"body":{"issue":"CORE-2","body":"Raw body"},"valid":true}));
    }
    let run_schema = &run_tools
        .iter()
        .find(|tool| tool.name == "comment_create")
        .unwrap()
        .input_schema;
    let body =
        json!({"mutation":common::mutation(0,"run-comment"),"issue":"CORE-2","body":"Raw body"});
    cases.push(json!({"name":"run without author","schema":run_schema,"body":body,"valid":false}));
    let mut authored = body;
    authored["author"] = json!("Ada");
    cases.push(json!({"name":"run with author","schema":run_schema,"body":authored,"valid":true}));
    let without_name = Fixture::new(true);
    let unnamed = operator_client(without_name.service(), &without_name.secret).await;
    let unnamed_tools = unnamed.list_all_tools().await.unwrap();
    let schema = &unnamed_tools
        .iter()
        .find(|tool| tool.name == "comment_create")
        .unwrap()
        .input_schema;
    cases.push(json!({"name":"operator without name or author","schema":schema,"body":{"issue":"CORE-2","body":"Raw body"},"valid":false}));
    cases.push(json!({"name":"operator without name and with author","schema":schema,"body":{"issue":"CORE-2","body":"Raw body","author":"Ada"},"valid":true}));
    validate_schemas(json!(cases));
    operator.cancel().await.unwrap();
    run.cancel().await.unwrap();
    unnamed.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_aliases_validate_against_live_and_generated_schemas() {
    let f = Fixture::configured(true, Some("Ada"));
    let client = operator_client(f.service(), &f.secret).await;
    let live = serde_json::to_value(client.list_all_tools().await.unwrap()).unwrap();
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/contracts/src");
    let generated: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("mcp-tools.json")).unwrap())
            .unwrap();
    let mut cases = Vec::new();
    for (name, dto, body, valid) in [
        ("get_issue", "IssueGetQuery", json!({"id":"CORE-2"}), true),
        (
            "get_issue",
            "IssueGetQuery",
            json!({"identifier":"CORE-2"}),
            true,
        ),
        ("get_issue", "IssueGetQuery", json!({}), false),
        (
            "save_issue",
            "IssueSaveRequest",
            json!({"id":"CORE-2","parentId":null,"dueDate":null,"blockedBy":["CORE-1"]}),
            true,
        ),
        (
            "save_issue",
            "IssueSaveRequest",
            json!({"identifier":"CORE-2","parent_id":12,"due_date":"2026-12-01","add_blocked_by":["CORE-1"]}),
            true,
        ),
        (
            "save_issue",
            "IssueSaveRequest",
            json!({"parentId":12,"title":"Child"}),
            true,
        ),
        (
            "save_issue",
            "IssueSaveRequest",
            json!({"parentId":"CORE-2","title":"Child","dueDate":"2026-12-01"}),
            true,
        ),
        (
            "list_issues",
            "IssueListQuery",
            json!({"parentId":12}),
            true,
        ),
        (
            "list_issues",
            "IssueListQuery",
            json!({"parentId":"CORE-2"}),
            true,
        ),
        (
            "list_issues",
            "IssueListQuery",
            json!({"parentId":null}),
            true,
        ),
        (
            "list_issues",
            "IssueListQuery",
            json!({"parent_id":12}),
            true,
        ),
        (
            "list_issues",
            "IssueListQuery",
            json!({"parent_id":null}),
            true,
        ),
        (
            "comment_create",
            "CommentCreateRequest",
            json!({"issue":"CORE-2","body":"Reply","author":"Ada","parentId":1}),
            true,
        ),
        (
            "comment_create",
            "CommentCreateRequest",
            json!({"issue":"CORE-2","body":"Reply","author":"Ada","parent_comment_id":1}),
            true,
        ),
    ] {
        for (surface, tools) in [("live", &live), ("generated", &generated)] {
            let tool = tools
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap();
            cases.push(json!({"name":format!("{surface} {name} {body}"),"schema":tool["inputSchema"],"body":body,"valid":valid}));
        }
        let schema: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join(format!("schemas/{dto}.json"))).unwrap(),
        )
        .unwrap();
        let mut canonical = body;
        if name == "save_issue" || name == "comment_create" {
            canonical["mutation"] = common::mutation(0, "schema-call");
        }
        cases.push(json!({"name":format!("{dto} {canonical}"),"schema":schema,"body":canonical,"valid":valid}));
    }
    let comment = live
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "save_comment")
        .unwrap();
    cases.push(json!({"name":"save_comment parentId","schema":comment["inputSchema"],"body":{"issue":"CORE-2","body":"Reply","parentId":1},"valid":true}));
    validate_schemas(json!(cases));
    client.cancel().await.unwrap();
}
