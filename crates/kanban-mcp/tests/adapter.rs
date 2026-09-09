//! A real MCP client traverses generated tools into the SQLite-backed Core.
use kanban_domain::CapabilityId;
use kanban_storage::{
    SqliteCapabilityStore, SqliteEvidenceStore, SqliteProjectStore, SqliteRunStore,
    SqliteSpecStore, SqliteTicketStore,
};
use rmcp::{ServiceExt, model::CallToolRequestParams};
use serde_json::{Value, json};
use std::sync::Arc;
#[path = "../../kanban-app/tests/common/mod.rs"]
mod common;

#[tokio::test(flavor = "multi_thread")]
async fn adapter_capability_enforcement_uses_generated_tools_and_real_core() {
    exercise(AdapterExercise::InProcess).await;
}
#[tokio::test(flavor = "multi_thread")]
async fn adapter_stdio_child_uses_parent_bound_authority() {
    exercise(AdapterExercise::Child).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn adapter_connects_through_the_running_service() {
    exercise_service(ServiceExercise::Attach).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn adapter_stdio_launcher_reaches_the_running_service() {
    exercise_service(ServiceExercise::Stdio).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn adapter_stdio_launcher_exits_when_the_service_stops() {
    exercise_service(ServiceExercise::Stop).await;
}

enum ServiceExercise {
    Attach,
    Stdio,
    Stop,
    SecretEnvelope,
    SecretRequestId,
}

#[tokio::test(flavor = "multi_thread")]
async fn adapter_secret_exclusion_covers_protocol_envelopes() {
    exercise_service(ServiceExercise::SecretEnvelope).await;
    exercise_service(ServiceExercise::SecretRequestId).await;
}

async fn exercise_service(exercise: ServiceExercise) {
    let h = common::harness();
    let own = common::insert_ticket(&h.database_path, 1, "normal");
    common::assign_lane(&h.database_path, own);
    let request = h
        .core
        .command(
            "dispatch.request",
            &json!({
                "mutation": common::mutation(0, "request"), "ticket_id": own
            }),
        )
        .unwrap();
    let claim = h
        .core
        .command(
            "dispatch.claim",
            &json!({
                "mutation": common::mutation(1, "claim"), "dispatch_request_id": request["id"]
            }),
        )
        .unwrap();
    h.core
        .command(
            "run.acknowledge",
            &json!({
                "mutation": common::mutation(2, "acknowledge"), "dispatch_request_id": request["id"]
            }),
        )
        .unwrap();
    let secret = Arc::new(kanban_app::secrets::InstallationSecret::from_key(&[23; 32]));
    let service = kanban_service::serve_with_runtime(
        h._dir.path(),
        kanban_service::ServiceRuntime {
            mcp_executable: env!("CARGO_BIN_EXE_kanban-mcp").into(),
            herdr_socket_root: h._dir.path().join("herdr"),
            installation_secret: Some(secret.clone()),
        },
    )
    .unwrap();
    if matches!(
        exercise,
        ServiceExercise::Stop | ServiceExercise::SecretEnvelope | ServiceExercise::SecretRequestId
    ) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_kanban-mcp"))
            .arg("--socket")
            .arg(service.socket_path())
            .arg("--capability")
            .arg(claim["capability"]["id"].as_u64().unwrap().to_string())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = tokio::io::BufReader::new(child.stdout.take().unwrap());
        input.write_all(format!("{}\n", json!({
            "jsonrpc":"2.0", "id":1, "method":"initialize",
            "params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}
        })).as_bytes()).await.unwrap();
        let mut response = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            output.read_line(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            serde_json::from_str::<Value>(&response)
                .unwrap()
                .get("result")
                .is_some()
        );
        let mut excluded = true;
        if matches!(
            exercise,
            ServiceExercise::SecretEnvelope | ServiceExercise::SecretRequestId
        ) {
            let id = if matches!(exercise, ServiceExercise::SecretRequestId) {
                json!(secret.expose())
            } else {
                json!(3)
            };
            let version = if matches!(exercise, ServiceExercise::SecretEnvelope) {
                secret.expose()
            } else {
                "2025-03-26"
            };
            input
                .write_all(
                    format!(
                        "{}\n{}\n",
                        json!({
                            "jsonrpc":"2.0", "method":"notifications/initialized"
                        }),
                        json!({
                            "jsonrpc":"2.0", "id":id, "method":"tools/list",
                            "params":{"_meta":{"io.modelcontextprotocol/protocolVersion":version}}
                        })
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            response.clear();
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                output.read_line(&mut response),
            )
            .await;
            excluded = matches!(result, Ok(Ok(_))) && !response.contains(secret.expose());
        }
        service.shutdown();
        let exited = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
        if exited.is_err() {
            child.kill().await.unwrap();
        }
        assert!(
            exited.is_ok(),
            "a live stdin must not strand the relay after service shutdown"
        );
        drop(input);
        assert!(
            excluded,
            "request IDs and protocol metadata must not reflect installation credentials"
        );
        return;
    }
    if matches!(exercise, ServiceExercise::Stdio) {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_kanban-mcp"));
        command
            .arg("--socket")
            .arg(service.socket_path())
            .arg("--capability")
            .arg(claim["capability"]["id"].as_u64().unwrap().to_string());
        let transport = rmcp::transport::TokioChildProcess::new(command).unwrap();
        let connection =
            tokio::time::timeout(std::time::Duration::from_secs(5), ().serve(transport)).await;
        match connection {
            Ok(Ok(client)) => {
                let result = client
                    .call_tool(
                        CallToolRequestParams::new("ticket_get")
                            .with_arguments(json!({"ticket_id": own}).as_object().unwrap().clone()),
                    )
                    .await
                    .unwrap();
                assert_eq!(result.structured_content.unwrap()["id"], own);
                client.cancel().await.unwrap();
                service.shutdown();
                return;
            }
            failure => {
                service.shutdown();
                panic!("the stdio launcher must reach the managed service: {failure:?}");
            }
        }
    }
    let mut channel = std::os::unix::net::UnixStream::connect(service.socket_path()).unwrap();
    channel
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    use std::io::{BufRead, Write};
    writeln!(
        channel,
        "{}",
        json!({
            "kind": "agent", "payload": {"capability_id": claim["capability"]["id"]}
        })
    )
    .unwrap();
    let mut reader = std::io::BufReader::new(channel);
    let mut response = String::new();
    reader.read_line(&mut response).unwrap();
    let response: Value = serde_json::from_str(&response).unwrap();
    service.shutdown();
    assert_eq!(
        response["kind"], "response",
        "the live core must offer a bound agent connection: {response}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn adapter_oversized_response_closes_private_channel() {
    exercise(AdapterExercise::LargeResponse).await;
}

enum AdapterExercise {
    InProcess,
    Child,
    LargeResponse,
}

async fn exercise(mode: AdapterExercise) {
    let child = !matches!(mode, AdapterExercise::InProcess);
    let mut h = common::harness();
    h.core
        .register_tickets(
            Arc::new(SqliteTicketStore::new(&h.database)),
            Arc::new(SqliteProjectStore::new(&h.database)),
            Arc::new(SqliteSpecStore::new(&h.database)),
            Arc::new(SqliteEvidenceStore::new(
                &h.database,
                h._dir.path().join("attachments"),
            )),
        )
        .unwrap();
    h.core.register_agent_authority(Arc::new(
        kanban_app::agent_authorization::RunAuthority::new(
            Arc::new(SqliteCapabilityStore::new(&h.database)),
            Arc::new(SqliteTicketStore::new(&h.database)),
            Arc::new(SqliteRunStore::new(&h.database)),
        ),
    ));
    let own = common::insert_ticket(&h.database_path, 1, "normal");
    let other = common::insert_ticket(&h.database_path, 2, "normal");
    common::assign_lane(&h.database_path, own);
    let request = h
        .core
        .command(
            "dispatch.request",
            &json!({"mutation":common::mutation(0,"request"),"ticket_id":own}),
        )
        .unwrap();
    let claim = h
        .core
        .command(
            "dispatch.claim",
            &json!({"mutation":common::mutation(1,"claim"),"dispatch_request_id":request["id"]}),
        )
        .unwrap();
    h.core.command("run.acknowledge",&json!({"mutation":common::mutation(2,"acknowledge"),"dispatch_request_id":request["id"]})).unwrap();
    let cap = CapabilityId::new(claim["capability"]["id"].as_u64().unwrap());
    let core = Arc::new(h.core);
    if child {
        let disposable = kanban_service::mcp::spawn_adapter(
            core.clone(),
            cap,
            std::path::Path::new(env!("CARGO_BIN_EXE_kanban-mcp")),
        )
        .unwrap();
        let pid = disposable.id() as libc::pid_t;
        drop(disposable);
        // Probe only the exact child this test just created; clean up on RED.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        if alive {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
                libc::waitpid(pid, std::ptr::null_mut(), 0);
            }
        }
        assert!(!alive, "dropping a core-owned adapter must reap its child");
    }
    let adapter = kanban_mcp::Adapter::new(core.clone(), cap);
    let (server_io, client_io) = tokio::io::duplex(1024 * 1024);
    let (mut server_reader, mut server_writer) = tokio::io::split(server_io);
    let mut child_process = if child {
        Some(
            kanban_service::mcp::spawn_adapter(
                core.clone(),
                cap,
                std::path::Path::new(env!("CARGO_BIN_EXE_kanban-mcp")),
            )
            .unwrap(),
        )
    } else {
        None
    };
    let child_input = child_process
        .as_mut()
        .map(|child| child.stdin.take().unwrap());
    let child_output = child_process
        .as_mut()
        .map(|child| child.stdout.take().unwrap());
    let serving = tokio::spawn(async move {
        if child {
            let mut input = tokio::fs::File::from_std(std::fs::File::from(
                std::os::fd::OwnedFd::from(child_input.unwrap()),
            ));
            let mut output = tokio::fs::File::from_std(std::fs::File::from(
                std::os::fd::OwnedFd::from(child_output.unwrap()),
            ));
            tokio::join!(
                async {
                    let _ = tokio::io::copy(&mut server_reader, &mut input).await;
                },
                async {
                    let _ = tokio::io::copy(&mut output, &mut server_writer).await;
                }
            );
        } else {
            adapter
                .serve((server_reader, server_writer))
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        }
    });
    let client = ().serve(client_io).await.unwrap();
    let secret = kanban_app::secrets::InstallationSecret::from_key(&[19; 32]);
    let custom = client
        .peer()
        .send_request(rmcp::model::CustomRequest::new(secret.expose(), None).into())
        .await
        .expect_err("unlisted protocol methods must be refused");
    let reflected_secret = format!("{custom:?}").contains(secret.expose());
    let tools = client.list_all_tools().await.unwrap();
    let generated: Vec<Value> = serde_json::from_str(include_str!(
        "../../../packages/contracts/src/mcp-tools.json"
    ))
    .unwrap();
    assert!(!tools.is_empty());
    for tool in &tools {
        let source = generated
            .iter()
            .find(|source| source["name"] == tool.name.as_ref())
            .unwrap();
        let actual = serde_json::to_value(tool).unwrap();
        assert_eq!(source["inputSchema"], actual["inputSchema"]);
        assert_eq!(source["outputSchema"], actual["outputSchema"]);
    }
    assert!(tools.iter().any(|tool| tool.name == "ticket_get"));
    assert!(!tools.iter().any(|tool| tool.name == "dispatch_claim"));
    let call = |ticket| {
        CallToolRequestParams::new("ticket_get")
            .with_arguments(json!({"ticket_id":ticket}).as_object().unwrap().clone())
    };
    let result = client.call_tool(call(own)).await.unwrap();
    assert_eq!(result.structured_content.as_ref().unwrap()["id"], own);
    assert_eq!(
        result.structured_content.unwrap(),
        core.query("ticket.get", &json!({"ticket_id":own})).unwrap()
    );
    assert_eq!(
        client.call_tool(call(other)).await.unwrap().is_error,
        Some(true)
    );
    assert_eq!(
        client
            .call_tool(CallToolRequestParams::new("dispatch_claim"))
            .await
            .unwrap()
            .is_error,
        Some(true)
    );
    let response_closed = if matches!(mode, AdapterExercise::LargeResponse) {
        rusqlite::Connection::open(&h.database_path)
            .unwrap()
            .execute(
                "UPDATE tickets SET title = ?1 WHERE id = ?2",
                rusqlite::params!["large".repeat(1024 * 1024), own as i64],
            )
            .unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.call_tool(call(own)),
        )
        .await;
        let closed = matches!(result, Ok(Ok(ref response)) if response.is_error == Some(true));
        if !closed {
            child_process.as_mut().unwrap().kill().unwrap();
        }
        closed
    } else {
        true
    };
    let _ = client.cancel().await;
    if let Some(mut child) = child_process {
        child.kill().unwrap();
        assert!(child.wait().unwrap().code() != Some(0));
    }
    serving.await.unwrap();
    assert!(
        !reflected_secret,
        "MCP protocol errors must not echo caller-controlled methods"
    );
    assert!(
        response_closed,
        "a failed private response must return an error, not hang"
    );
}
