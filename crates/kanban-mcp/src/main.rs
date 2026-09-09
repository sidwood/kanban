//! Only the durable service may supply fd 3; stdin/stdout are MCP alone.
use rmcp::ServiceExt;
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixStream;
fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if let [socket_flag, socket, capability_flag, capability] = args.as_slice()
        && socket_flag == "--socket"
        && capability_flag == "--capability"
    {
        let capability = capability.to_str().and_then(|value| {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            value.parse::<u64>().ok().filter(|value| *value != 0)
        });
        let Some(capability) = capability else {
            std::process::exit(1)
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("MCP launcher runtime starts");
        let result = runtime.block_on(bridge(std::path::Path::new(socket), capability));
        // Tokio's stdin reader is blocking and cannot be cancelled. Do not wait
        // for it when the service disconnected but the client kept stdin open.
        runtime.shutdown_background();
        if result.is_err() {
            eprintln!("Kanban agent connection is unavailable");
            std::process::exit(1);
        }
        return;
    }
    if args != ["--application-channel"] {
        eprintln!("start this adapter through the Kanban core");
        std::process::exit(1);
    }
    // SAFETY: the core installed an owned, open socket at fd 3 before exec.
    // Reject direct starts without the descriptor before taking ownership.
    if unsafe { libc::fcntl(3, libc::F_GETFD) } < 0 {
        std::process::exit(1);
    }
    let channel = unsafe { UnixStream::from_raw_fd(3) };
    if channel.peer_addr().is_err() {
        std::process::exit(1);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("MCP runtime starts");
    let result = runtime.block_on(async {
        let running = kanban_mcp::Adapter::from_channel(channel)
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|_| ())?;
        running.waiting().await.map_err(|_| ())
    });
    if result.is_err() {
        std::process::exit(1);
    }
}

/// The local client launches this relay; the core launches the actual adapter.
async fn bridge(socket: &std::path::Path, capability_id: u64) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    let mut channel = tokio::net::UnixStream::connect(socket).await?;
    let attachment = kanban_transport::RequestFrame {
        kind: kanban_transport::FrameKind::Agent,
        operation: None,
        payload: Some(serde_json::to_value(
            kanban_transport::agent::AgentAttachment { capability_id },
        )?),
    };
    channel.write_all(&serde_json::to_vec(&attachment)?).await?;
    channel.write_all(b"\n").await?;
    let mut reader = tokio::io::BufReader::new(channel);
    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        (&mut reader).take(4097).read_until(b'\n', &mut response),
    )
    .await??;
    if response.len() > 4096
        || !matches!(
            serde_json::from_slice(&response),
            Ok(kanban_transport::ResponseFrame::Response { .. })
        )
    {
        return Err(std::io::Error::other("agent attachment refused"));
    }
    let (mut read_half, mut write_half) = tokio::io::split(reader);
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    tokio::select! {
        result = tokio::io::copy(&mut stdin, &mut write_half) => { result?; }
        result = tokio::io::copy(&mut read_half, &mut stdout) => { result?; }
    }
    Ok(())
}
