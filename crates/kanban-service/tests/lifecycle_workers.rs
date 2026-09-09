use kanban_service::{ServiceRuntime, serve_with_runtime};
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
    for cycle in 0..3 {
        let data = dir.path().join(format!("cycle-{cycle}"));
        let core = serve_with_runtime(
            &data,
            ServiceRuntime {
                mcp_executable: "/usr/bin/false".into(),
                herdr_socket_root: data.join("isolated-herdr"),
                installation_secret: None,
            },
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert!(thread_count() > baseline);
        assert!(database_descriptors(&data) > 0);
        let (done, finished) = std::sync::mpsc::channel();
        let shutdown = std::thread::spawn(move || {
            core.shutdown();
            done.send(()).unwrap();
        });
        finished
            .recv_timeout(Duration::from_secs(3))
            .expect("shutdown must wake idle schedulers");
        shutdown.join().unwrap();
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
