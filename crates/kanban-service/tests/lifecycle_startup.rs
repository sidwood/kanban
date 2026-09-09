use kanban_service::{ServiceRuntime, run_with_runtime};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
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
    // A real multi-step snapshot keeps this case independent of idle teardown.
    // cspell:ignore zeroblob
    connection
        .execute_batch(
            "CREATE TABLE lifecycle_backup_fixture (payload BLOB NOT NULL);
             INSERT INTO lifecycle_backup_fixture VALUES (zeroblob(4194304));",
        )
        .unwrap();
    drop(connection);
    let socket = data.join("core.sock");
    let child = Command::new(std::env::current_exe().unwrap())
        .arg(&data)
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
    let warning = loop {
        if data.join("backups").exists()
            && let Some(warning) = request(&socket, "query", "service.stop_warning", json!({}))
        {
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
    assert!(children.processes[0].try_wait().unwrap().is_none());
    // This bound contains the fixture workload, not the product's OS work.
    let deadline = started + Duration::from_secs(30);
    while children.processes[0].try_wait().unwrap().is_none() {
        children.require_time(deadline, "busy shutdown did not finish its backup");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(children.processes[0].wait().unwrap().success());
    assert!(data.join(".backup-scheduler.json").exists());
    let store = BackupStore::new(data.clone());
    let record = store
        .verified_record_for(LATEST_SCHEMA_VERSION)
        .unwrap()
        .expect("shutdown must publish the in-flight backup before exiting");
    let bundle = data.join("backups").join(record.bundle_id);
    store.validate(&bundle, None).unwrap();
    let snapshot = rusqlite::Connection::open(bundle.join("kanban.sqlite")).unwrap();
    let bytes: i64 = snapshot
        .query_row(
            "SELECT length(payload) FROM lifecycle_backup_fixture",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(bytes, 4_194_304);
    println!(
        "busy_backup: joined and verified {bytes} bytes in {:?}",
        started.elapsed()
    );
}
