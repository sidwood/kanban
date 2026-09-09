use kanban_service::{ServiceRuntime, run_with_runtime};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Children(Vec<Child>);
impl Drop for Children {
    fn drop(&mut self) {
        for child in &mut self.0 {
            if child.try_wait().unwrap().is_none() {
                child.kill().unwrap();
            }
            child.wait().unwrap();
        }
    }
}

fn request(socket: &Path, kind: &str, operation: &str, payload: Value) -> Option<Value> {
    let mut stream = UnixStream::connect(socket).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(200)))
        .ok()?;
    writeln!(
        stream,
        "{}",
        json!({"kind":kind,"operation":operation,"payload":payload})
    )
    .ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let response: Value = serde_json::from_str(&line).ok()?;
    (response["kind"] == "response").then(|| response["payload"].clone())
}

fn main() {
    if let Some(data) = std::env::args_os().nth(1) {
        let data = Path::new(&data);
        std::io::stdin().read_exact(&mut [0]).unwrap();
        let result = run_with_runtime(
            data,
            ServiceRuntime {
                mcp_executable: "/usr/bin/false".into(),
                herdr_socket_root: data.join("isolated-herdr"),
                installation_secret: None,
            },
        );
        if let Err(error) = result {
            eprintln!("startup refused: {error:?}");
            std::process::exit(1);
        }
        return;
    }

    let dir = tempfile::TempDir::new().unwrap();
    for trial in 0..3 {
        let data = dir.path().join(format!("trial-{trial}"));
        std::fs::create_dir(&data).unwrap();
        let alias = dir.path().join(format!("alias-{trial}"));
        std::os::unix::fs::symlink(&data, &alias).unwrap();
        let socket = data.join("core.sock");
        let mut children = Children(
            (0..8)
                .map(|index| {
                    Command::new(std::env::current_exe().unwrap())
                        .arg(if index % 2 == 0 { &data } else { &alias })
                        .stdin(Stdio::piped())
                        .stdout(Stdio::null())
                        .stderr(Stdio::piped())
                        .spawn()
                        .unwrap()
                })
                .collect(),
        );
        for child in &mut children.0 {
            child.stdin.take().unwrap().write_all(&[1]).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        let health = loop {
            if let Some(health) = request(&socket, "query", "health.get", json!({})) {
                break health;
            }
            assert!(Instant::now() < deadline, "no starter became healthy");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(health["connected"], true);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let exited = children
                .0
                .iter_mut()
                .map(|child| child.try_wait().unwrap())
                .filter(Option::is_some)
                .count();
            if exited == 7 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "followers did not reuse the healthy winner"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let warning = request(&socket, "query", "service.stop_warning", json!({})).unwrap();
        let response = request(&socket, "command", "service.stop", json!({
            "mutation":{"optimistic_version":warning["version"],"idempotency_key":format!("trial-{trial}-stop")},
            "instance_id":warning["instance_id"],"warning_id":warning["warning_id"],"confirmed":true,
        })).unwrap();
        assert_eq!(response["status"], "stop_requested");
        let deadline = Instant::now() + Duration::from_secs(5);
        while children
            .0
            .iter_mut()
            .any(|child| child.try_wait().unwrap().is_none())
        {
            assert!(Instant::now() < deadline, "winner did not stop");
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut failures = Vec::new();
        for (index, child) in children.0.iter_mut().enumerate() {
            let status = child.wait().unwrap();
            let mut stderr = String::new();
            child
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut stderr)
                .unwrap();
            if !status.success() {
                failures.push((index, status, stderr));
            }
        }
        println!("trial={trial} starters=8 failures={failures:?}");
        assert!(!socket.exists());
        assert!(
            failures.is_empty(),
            "concurrent starters must reuse, not race migrations: {failures:?}"
        );
    }
}
