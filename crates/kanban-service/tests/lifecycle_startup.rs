// cspell:ignore nonblocking
use kanban_service::{ServiceRuntime, run_with_runtime};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Children {
    processes: Vec<Child>,
    data: PathBuf,
}

impl Children {
    fn require_time(&mut self, deadline: Instant, phase: &str) {
        if Instant::now() < deadline {
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(child) = self
            .processes
            .iter_mut()
            .find_map(|child| matches!(child.try_wait(), Ok(None)).then_some(child))
        {
            sample_owned_child(child, &self.data);
        }
        let statuses: Vec<_> = self
            .processes
            .iter_mut()
            .enumerate()
            .map(|(index, child)| (index, child.id(), child.try_wait()))
            .collect();
        let logs = std::fs::read_to_string(self.data.join("logs/core.log"));
        let backup = std::fs::read_to_string(self.data.join(".backup-scheduler.json"));
        panic!(
            "{phase}; children (index, pid, status)={statuses:?}; socket_exists={}; backup={backup:?}; logs={logs:?}",
            self.data.join("core.sock").exists()
        );
    }
}

#[cfg(target_os = "macos")]
fn sample_owned_child(child: &mut Child, data: &Path) {
    let path = data.join("owned-child-sample.txt");
    let started = Instant::now();
    let result = Command::new("/usr/bin/sample")
        .arg(child.id().to_string())
        .args(["1", "10", "-file"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let status = match result {
        Ok(mut sampler) => loop {
            match sampler.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if started.elapsed() < Duration::from_secs(3) => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                result => {
                    let _ = sampler.kill();
                    let _ = sampler.wait();
                    break Err(format!("sample deadline or wait failure: {result:?}"));
                }
            }
        },
        Err(error) => Err(error.to_string()),
    };
    let mut trace = Vec::new();
    let read =
        std::fs::File::open(&path).and_then(|file| file.take(24 * 1024).read_to_end(&mut trace));
    eprintln!(
        "owned_child_sample pid={} sample_status={status:?} sample_seconds={} read={read:?}\n{}",
        child.id(),
        started.elapsed().as_secs_f64(),
        String::from_utf8_lossy(&trace)
    );
}

impl Drop for Children {
    fn drop(&mut self) {
        for (index, child) in self.processes.iter_mut().enumerate() {
            if child.try_wait().unwrap().is_none() {
                child.kill().unwrap();
            }
            child.wait().unwrap();
            if std::thread::panicking() {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                eprintln!("child={index} pid={} stderr={stderr}", child.id());
            }
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
        let backup_gate = std::env::args_os()
            .nth(2)
            .map(|gate_socket| coordinate_backup(data, Path::new(&gate_socket)));
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
        if let Some(backup_gate) = backup_gate {
            assert!(
                data.join(".backup-scheduler.json").exists(),
                "shutdown returned before publishing the gated backup"
            );
            backup_gate.join().unwrap();
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
        let mut children = Children {
            data: data.clone(),
            processes: (0..8)
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
        };
        for child in &mut children.processes {
            child.stdin.take().unwrap().write_all(&[1]).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        let health = loop {
            if let Some(health) = request(&socket, "query", "health.get", json!({})) {
                break health;
            }
            children.require_time(deadline, "no starter became healthy");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(health["connected"], true);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let exited = children
                .processes
                .iter_mut()
                .map(|child| child.try_wait().unwrap())
                .filter(Option::is_some)
                .count();
            if exited == 7 {
                break;
            }
            children.require_time(deadline, "followers did not reuse the healthy winner");
            std::thread::sleep(Duration::from_millis(20));
        }
        // The exit deadline measures idle teardown, not a throttled SQLite copy.
        // Busy shutdown has its own real-backup case below; no backup is disabled.
        let idle_health = wait_for_startup_backup(&socket, &mut children);
        assert!(
            idle_health["scheduler"]["last_backup_success_at"].is_string(),
            "timed idle stop requires a completed startup backup: {idle_health}"
        );
        let warning = request(&socket, "query", "service.stop_warning", json!({})).unwrap();
        let response = request(&socket, "command", "service.stop", json!({
            "mutation":{"optimistic_version":warning["version"],"idempotency_key":format!("trial-{trial}-stop")},
            "instance_id":warning["instance_id"],"warning_id":warning["warning_id"],"confirmed":true,
        })).unwrap();
        assert_eq!(response["status"], "stop_requested");
        let deadline = Instant::now() + Duration::from_secs(5);
        while children
            .processes
            .iter_mut()
            .any(|child| child.try_wait().unwrap().is_none())
        {
            children.require_time(deadline, "winner did not stop after its startup backup");
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut failures = Vec::new();
        for (index, child) in children.processes.iter_mut().enumerate() {
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
    stop_joins_an_in_flight_backup(dir.path());
}

fn wait_for_startup_backup(socket: &Path, children: &mut Children) -> Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(health) = request(socket, "query", "health.get", json!({}))
            && health["scheduler"]["last_backup_success_at"].is_string()
        {
            return health;
        }
        children.require_time(deadline, "startup backup did not complete");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stop_joins_an_in_flight_backup(root: &Path) {
    use kanban_storage::migrations::{AllowAllMigrations, LATEST_SCHEMA_VERSION};
    use kanban_storage::{BackupStore, Database};

    let data = root.join("busy-backup");
    std::fs::create_dir(&data).unwrap();
    let database_path = data.join("kanban.sqlite");
    let mut database = Database::open(&database_path).unwrap();
    database.migrate(&AllowAllMigrations).unwrap();
    drop(database);
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let payload: Vec<u8> = (0..4096).map(|index| (index % 251) as u8).collect();
    connection
        .execute_batch("CREATE TABLE lifecycle_backup_fixture (payload BLOB NOT NULL);")
        .unwrap();
    connection
        .execute(
            "INSERT INTO lifecycle_backup_fixture VALUES (?1)",
            [&payload],
        )
        .unwrap();
    let pages: i64 = connection
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .unwrap();
    drop(connection);
    let socket = data.join("core.sock");
    let gate_socket = data.join("gate.sock");
    let gate_listener = UnixListener::bind(&gate_socket).unwrap();
    gate_listener.set_nonblocking(true).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .arg(&data)
        .arg(&gate_socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut children = Children {
        processes: vec![child],
        data: data.clone(),
    };
    children.processes[0]
        .stdin
        .take()
        .unwrap()
        .write_all(&[1])
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut gate = loop {
        match gate_listener.accept() {
            Ok((gate, _)) => break gate,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("backup gate accept failed: {error}"),
        }
        children.require_time(deadline, "busy-backup child did not connect its gate");
        std::thread::sleep(Duration::from_millis(20));
    };
    gate.set_nonblocking(true).unwrap();
    let mut event = [255];
    loop {
        match gate.read(&mut event) {
            Ok(1) => break,
            Ok(_) => panic!("backup gate closed before a real snapshot step"),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("backup gate read failed: {error}"),
        }
        children.require_time(deadline, "backup never reached its first real step");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(event, [0], "the real snapshot must still be incomplete");
    let bundles: Vec<_> = std::fs::read_dir(data.join("backups"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(bundles.len(), 1);
    let in_flight_bundle = &bundles[0];
    let deadline = Instant::now() + Duration::from_secs(15);
    let warning = loop {
        if let Some(warning) = request(&socket, "query", "service.stop_warning", json!({})) {
            break warning;
        }
        children.require_time(deadline, "busy-backup service did not become ready");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(!data.join(".backup-scheduler.json").exists());
    let response = request(&socket, "command", "service.stop", json!({
        "mutation":{"optimistic_version":warning["version"],"idempotency_key":"busy-stop"},
        "instance_id":warning["instance_id"],"warning_id":warning["warning_id"],"confirmed":true,
    })).unwrap();
    assert_eq!(response["status"], "stop_requested");
    let started = Instant::now();
    let deadline = started + Duration::from_secs(5);
    while socket.exists() {
        children.require_time(deadline, "busy shutdown did not close transport");
        std::thread::sleep(Duration::from_millis(20));
    }
    let store = BackupStore::new(data.clone());
    // Hold the real copy across the idle-exit budget; overlap no longer depends on disk speed.
    let held_until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < held_until {
        assert!(
            children.processes[0].try_wait().unwrap().is_none(),
            "owner exited while its real backup was held"
        );
        assert!(!data.join(".backup-scheduler.json").exists());
        assert!(!in_flight_bundle.join("manifest.json").exists());
        assert!(!in_flight_bundle.join("verified.json").exists());
        assert!(
            store
                .verified_record_for(LATEST_SCHEMA_VERSION)
                .unwrap()
                .is_none()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    gate.write_all(&[1]).unwrap();
    let released = Instant::now();
    let deadline = started + Duration::from_secs(30);
    while children.processes[0].try_wait().unwrap().is_none() {
        children.require_time(deadline, "busy shutdown did not finish its backup");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(children.processes[0].wait().unwrap().success());
    assert!(data.join(".backup-scheduler.json").exists());
    let record = store
        .verified_record_for(LATEST_SCHEMA_VERSION)
        .unwrap()
        .expect("shutdown must publish the in-flight backup before exiting");
    let bundle = data.join("backups").join(record.bundle_id);
    assert_eq!(&bundle, in_flight_bundle);
    store.validate(&bundle, None).unwrap();
    let snapshot = rusqlite::Connection::open(bundle.join("kanban.sqlite")).unwrap();
    let copied: Vec<u8> = snapshot
        .query_row("SELECT payload FROM lifecycle_backup_fixture", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(copied, payload);
    println!(
        "busy_backup: joined and verified {} bytes, {pages} source pages; release-to-exit {:?}, total {:?}",
        copied.len(),
        released.elapsed(),
        started.elapsed()
    );
}

fn coordinate_backup(data: &Path, gate_socket: &Path) -> std::thread::JoinHandle<()> {
    let (events, release, gate) =
        kanban_storage::backup::SnapshotStepGate::arm_for_database(&data.join("kanban.sqlite"));
    let mut control = UnixStream::connect(gate_socket).unwrap();
    control
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    control
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    std::thread::spawn(move || {
        let (step, done) = events
            .recv_timeout(Duration::from_secs(15))
            .expect("real snapshot step");
        assert_eq!(step, 1);
        assert!(!done);
        control.write_all(&[u8::from(done)]).unwrap();
        let mut command = [0];
        control.read_exact(&mut command).unwrap();
        assert_eq!(command, [1]);
        drop(release);
        drop(gate);
    })
}
