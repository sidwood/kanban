#[path = "../../kanban-transport/tests/common/http_response.rs"]
mod http_response;

use http_response::read_response;
use kanban_app::secrets::InstallationSecret;
use kanban_service::{ServiceRuntime, run_with_args};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct StopOnDrop(std::path::PathBuf);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        if UnixStream::connect(&self.0).is_ok() {
            let _ = std::panic::catch_unwind(|| stop(&self.0));
            let deadline = Instant::now() + Duration::from_secs(3);
            while self.0.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn request(socket: &Path, kind: &str, operation: &str, payload: Value) -> Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    writeln!(
        stream,
        "{}",
        json!({"kind":kind,"operation":operation,"payload":payload})
    )
    .unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    let frame: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(frame["kind"], "response");
    frame["payload"].clone()
}

fn stop(socket: &Path) {
    let warning = request(socket, "query", "service.stop_warning", json!({}));
    let response = request(
        socket,
        "command",
        "service.stop",
        json!({
            "mutation":{"optimistic_version":warning["version"],"idempotency_key":"http-stop"},
            "instance_id":warning["instance_id"],"warning_id":warning["warning_id"],"confirmed":true
        }),
    );
    assert_eq!(response["status"], "stop_requested");
}

fn wait_ready(socket: &Path, child: &mut Child, detached: bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while UnixStream::connect(socket).is_err() {
        assert!(
            detached || child.try_wait().unwrap().is_none(),
            "runtime exited before readiness"
        );
        assert!(Instant::now() < deadline, "runtime did not become ready");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        request(socket, "query", "health.get", json!({}))["connected"],
        true
    );
}

fn wait_exit(child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "explicit stop must end the process"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn assert_auth(address: SocketAddr, authorized: bool) {
    let mut stream = TcpStream::connect(address)
        .expect("explicit HTTP opt-in must bind in production orchestration");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let credential = if authorized {
        format!(
            "Authorization: Bearer {}\r\n",
            InstallationSecret::from_key(&[41; 32]).expose()
        )
    } else {
        String::new()
    };
    write!(stream, "POST /mcp HTTP/1.1\r\nHost: {address}\r\n{credential}X-Kanban-Capability: 1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
    let response = read_response(&mut stream).unwrap();
    let status = if authorized {
        "HTTP/1.1 403"
    } else {
        "HTTP/1.1 401"
    };
    assert!(
        response.starts_with(status),
        "authentication or capability boundary changed"
    );
    assert!(!response.contains(InstallationSecret::from_key(&[41; 32]).expose()));
}

fn process_start_and_stop(enabled: bool, detached: bool, explicit_data: bool) {
    let dir = tempfile::TempDir::new_in("/tmp").unwrap();
    let data = dir.path().join("selected data");
    let socket = data.join("core.sock");
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = held.local_addr().unwrap();
    drop(held);
    let mut command = Command::new(std::env::current_exe().unwrap());
    if detached {
        command.arg("--launch-once");
    }
    command
        .env_clear()
        .env("HOME", dir.path().join("unused-home"));
    if enabled {
        command.arg("--loopback-http").arg(address.to_string());
    } else {
        command
            .env("KANBAN_LOOPBACK_HTTP", address.to_string())
            .env("KANBAN_HTTP_BIND", address.to_string())
            .env("KANBAN_HTTP_SECRET", "planted-environment-secret");
    }
    if explicit_data {
        command.arg("--data-dir");
    }
    let mut child = ChildGuard(command.arg(&data).spawn().unwrap());
    let _cleanup = StopOnDrop(socket.clone());
    if detached {
        wait_exit(&mut child.0);
    }
    wait_ready(&socket, &mut child.0, detached);
    assert!(data.join("kanban.sqlite").exists());
    assert!(!dir.path().join("unused-home").exists());
    let mut pending = None;
    if enabled {
        assert_auth(address, false);
        assert_auth(address, true);
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream.write_all(b"POST /mcp HTTP/1.1\r\n").unwrap();
        pending = Some(stream);
    } else {
        assert!(
            TcpStream::connect(address).is_err(),
            "environment must not enable HTTP"
        );
    }
    stop(&socket);
    wait_exit(&mut child.0);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !data.join("fixture-finished").exists() {
        assert!(
            Instant::now() < deadline,
            "detached runtime did not return after stop"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(UnixStream::connect(&socket).is_err());
    assert!(TcpStream::connect(address).is_err());
    if let Some(mut pending) = pending {
        let result = pending.read(&mut [0; 1]);
        assert!(
            matches!(result, Ok(0))
                || result.is_err_and(|error| error.kind() == std::io::ErrorKind::ConnectionReset)
        );
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(!socket.exists(), "explicit stop must not silently restart");
    assert!(TcpListener::bind(address).is_ok());
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(data.join("core.lock"))
        .unwrap();
    assert!(
        lock.try_lock().is_ok(),
        "process shutdown must release startup ownership"
    );
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if !args.is_empty() {
        let mut owned_data = None;
        let result = run_with_args(args, &std::env::current_exe().unwrap(), |data| {
            owned_data = Some(data.to_path_buf());
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(data.join("core.lock"))
                .unwrap();
            assert!(
                matches!(lock.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
                "runtime must be constructed only under ownership"
            );
            assert!(!data.join("kanban.sqlite").exists());
            Ok(ServiceRuntime {
                mcp_executable: "/usr/bin/false".into(),
                herdr_socket_root: data.join("isolated-herdr"),
                installation_secret: Some(Arc::new(InstallationSecret::from_key(&[41; 32]))),
            })
        });
        if result.is_err() {
            std::process::exit(1);
        }
        if let Some(data) = owned_data {
            std::fs::write(data.join("fixture-finished"), b"stopped").unwrap();
        }
        return;
    }
    process_start_and_stop(false, false, false);
    process_start_and_stop(false, false, true);
    process_start_and_stop(true, false, false);
    process_start_and_stop(true, false, true);
    process_start_and_stop(true, true, false);
    process_start_and_stop(true, true, true);
    println!(
        "loopback_process: production argv, owned fixture runtime, default off, authentication, capability refusal, bounded explicit HTTP stop passed"
    );
}
