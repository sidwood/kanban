use kanban_service::{ServiceRuntime, serve_with_http};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[cfg(target_os = "macos")]
fn thread_count() -> usize {
    let output = Command::new("/bin/ps")
        .args(["-M", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .skip(1)
        .count()
}

#[cfg(not(target_os = "macos"))]
fn thread_count() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().count()
}

fn database_descriptors(data: &Path) -> usize {
    let output = Command::new("lsof")
        .args(["-a", "-p", &std::process::id().to_string(), "+D"])
        .arg(data)
        .output()
        .unwrap();
    assert!(output.status.success() || output.status.code() == Some(1));
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter(|line| line.contains("kanban.sqlite"))
        .count()
}

fn main() {
    let dir = tempfile::TempDir::new().unwrap();
    let baseline = thread_count();
    for cycle in 0..6 {
        let data = dir.path().join(format!("cycle-{cycle}"));
        let core = serve_with_http(
            &data,
            ServiceRuntime {
                mcp_executable: "/usr/bin/false".into(),
                herdr_socket_root: data.join("isolated-herdr"),
                installation_secret: Some(std::sync::Arc::new(
                    kanban_app::secrets::InstallationSecret::from_key(&[43; 32]),
                )),
            },
            kanban_transport::loopback::LoopbackHttpConfig {
                bind: (cycle >= 3).then(|| "127.0.0.1:0".parse().unwrap()),
            },
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert!(thread_count() > baseline);
        assert!(database_descriptors(&data) > 0);
        let address = core.http_address();
        let pending = address.map(|address| {
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream.write_all(b"POST /mcp HTTP/1.1\r\n").unwrap();
            stream
        });
        if address.is_some() {
            request_stop(core.socket_path());
        }
        let (done, finished) = std::sync::mpsc::channel();
        let shutdown = std::thread::spawn(move || {
            if address.is_some() {
                core.wait_for_stop();
            } else {
                core.shutdown();
            }
            done.send(()).unwrap();
        });
        finished
            .recv_timeout(Duration::from_secs(3))
            .expect("shutdown must wake idle schedulers");
        shutdown.join().unwrap();
        if let Some(address) = address {
            assert!(TcpStream::connect(address).is_err());
        }
        if let Some(mut pending) = pending {
            let result = pending.read(&mut [0; 1]);
            assert!(
                matches!(result, Ok(0))
                    || result
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::ConnectionReset)
            );
        }
        let remaining = thread_count();
        let descriptors = database_descriptors(&data);
        println!(
            "cycle={cycle} baseline_threads={baseline} post_shutdown_threads={remaining} database_descriptors={descriptors}"
        );
        assert!(!data.join("core.sock").exists());
        assert_eq!(remaining, baseline, "shutdown must join every owned worker");
        assert_eq!(
            descriptors, 0,
            "shutdown must close all database connections"
        );
    }
}

fn request_stop(socket: &Path) {
    let request = |kind: &str, operation: &str, payload: Value| {
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
    };
    let warning = request("query", "service.stop_warning", json!({}));
    let response = request(
        "command",
        "service.stop",
        json!({
            "mutation":{"optimistic_version":warning["version"],"idempotency_key":"http-worker-stop"},
            "instance_id":warning["instance_id"],"warning_id":warning["warning_id"],"confirmed":true
        }),
    );
    assert_eq!(response["status"], "stop_requested");
}
