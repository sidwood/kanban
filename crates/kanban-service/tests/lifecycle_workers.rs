use kanban_service::{ServiceRuntime, serve_with_http};
use kanban_storage::Database;
use kanban_storage::backup::SnapshotStepGate;
use kanban_storage::migrations::AllowAllMigrations;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

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
        let gate = (cycle == 0 || cycle == 3).then(|| {
            std::fs::create_dir_all(&data).unwrap();
            let path = data.join("kanban.sqlite");
            let mut database = Database::open(&path).unwrap();
            database.migrate(&AllowAllMigrations).unwrap();
            drop(database);
            SnapshotStepGate::arm_for_database(&path)
        });
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
        if let Some((events, release, gate)) = gate {
            assert_eq!(
                events.recv_timeout(Duration::from_secs(15)).unwrap(),
                (1, false),
                "startup backup must be held at an incomplete real snapshot step"
            );
            std::thread::scope(|scope| {
                let (idle, readiness) = std::sync::mpsc::channel();
                let data = &data;
                let waiter = scope.spawn(move || {
                    wait_for_startup_backup(data);
                    idle.send(()).unwrap();
                });
                let premature = readiness.recv_timeout(Duration::from_millis(300));
                let held_success = startup_backup_success(data);
                drop(release);
                drop(gate);
                waiter.join().unwrap();
                assert_eq!(held_success, None);
                assert_eq!(
                    premature,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout),
                    "idle readiness must not pass while the startup snapshot is held"
                );
            });
            println!("cycle={cycle} held_startup_backup_rejected_as_idle");
        } else {
            wait_for_startup_backup(&data);
        }
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
        assert!(
            startup_backup_success(&data).is_some(),
            "timed idle stop requires a persisted successful startup backup; state={:?}",
            std::fs::read_to_string(data.join(".backup-scheduler.json"))
        );
        if address.is_some() {
            request_stop(core.socket_path());
        }
        let shutdown_started = Instant::now();
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
        let shutdown_elapsed = shutdown_started.elapsed();
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
            "cycle={cycle} baseline_threads={baseline} post_shutdown_threads={remaining} database_descriptors={descriptors} idle_shutdown={shutdown_elapsed:?}"
        );
        assert!(!data.join("core.sock").exists());
        assert_eq!(remaining, baseline, "shutdown must join every owned worker");
        assert_eq!(
            descriptors, 0,
            "shutdown must close all database connections"
        );
    }
}

fn wait_for_startup_backup(data: &Path) {
    // Startup owes a real backup; its copy time is not part of the idle shutdown budget.
    let deadline = Instant::now() + Duration::from_secs(15);
    while startup_backup_success(data).is_none() {
        assert!(
            Instant::now() < deadline,
            "startup backup did not complete; state={:?}; logs={:?}",
            std::fs::read_to_string(data.join(".backup-scheduler.json")),
            std::fs::read_to_string(data.join("logs/core.log"))
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn startup_backup_success(data: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(data.join(".backup-scheduler.json")).ok()?;
    let state: Value = serde_json::from_str(&text).ok()?;
    state["last_success_unix_secs"].as_u64()
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
