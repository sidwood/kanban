//! The on-demand start: a missing core is spawned and serves; a
//! serving core is reused; a spawned core outlives the shell.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use kanban_desktop_lib::ensure_core_running;
use tempfile::TempDir;

/// Env-var tests share one lock: std::env::set_var races otherwise.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// The isolated fake-core tool package builds this binary in the root
/// workspace target directory.
fn fake_core_binary() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_fake-core") {
        return PathBuf::from(path);
    }
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_owned());
    let built = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("target")
        .join(profile)
        .join("fake-core");
    assert!(
        built.is_file(),
        "build the fake core with `cargo build -p kanban-fake-core --bin fake-core`"
    );
    built
}

/// Wait until `socket_path` is connectable, or panic.
fn await_socket(socket_path: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if UnixStream::connect(socket_path).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the core never served its socket");
}

/// Stop the fake core serving `data_dir`, by the pid it reported.
fn stop_fake_core(data_dir: &Path) {
    let pid_path = data_dir.join("fake-core.pid");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let pid = loop {
        if let Ok(text) = std::fs::read_to_string(&pid_path) {
            break text
                .trim()
                .parse::<i32>()
                .expect("the fake core reported a pid");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the fake core never reported its pid"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status();
}

#[test]
fn a_missing_core_is_started_on_demand_and_serves() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = TempDir::new().expect("a scratch directory is available");
    let socket_path = dir.path().join("core.sock");
    // Safety: the lock above serialises every env-writing test in
    // this binary.
    unsafe { std::env::set_var("KANBAN_CORE_BIN", fake_core_binary()) };

    let spawned = ensure_core_running(&socket_path).expect("the core starts on demand");

    assert!(spawned.is_some(), "the missing core was spawned");
    assert!(
        UnixStream::connect(&socket_path).is_ok(),
        "the spawned core serves its socket"
    );
    // The shell dropping the child is the UI-quit stand-in: the core
    // must keep serving (DR-RB-02).
    drop(spawned);
    assert!(
        UnixStream::connect(&socket_path).is_ok(),
        "the core outlives the shell that started it"
    );

    stop_fake_core(dir.path());
    // Safety: still under the lock.
    unsafe { std::env::remove_var("KANBAN_CORE_BIN") };
}

#[test]
fn a_serving_core_is_reused_without_a_spawn() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = TempDir::new().expect("a scratch directory is available");
    let socket_path = dir.path().join("core.sock");
    // A core that could not possibly start: had the shell tried to
    // spawn it, the start would fail loudly.
    // Safety: the lock above serialises every env-writing test in
    // this binary.
    unsafe { std::env::set_var("KANBAN_CORE_BIN", "/nonexistent/kanban-service") };
    let mut core = std::process::Command::new(fake_core_binary())
        .arg(dir.path())
        .spawn()
        .expect("the fake core spawns");
    await_socket(&socket_path);

    let spawned = ensure_core_running(&socket_path)
        .expect("a serving core satisfies the demand without a spawn");

    assert!(
        spawned.is_none(),
        "the serving core was reused, not replaced"
    );

    let _ = core.kill();
    let _ = core.wait();
    // Safety: still under the lock.
    unsafe { std::env::remove_var("KANBAN_CORE_BIN") };
}

#[test]
fn a_core_that_never_serves_fails_the_start_loudly() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = TempDir::new().expect("a scratch directory is available");
    let socket_path = dir.path().join("core.sock");
    // Safety: the lock above serialises every env-writing test in
    // this binary.
    unsafe { std::env::set_var("KANBAN_CORE_BIN", "/usr/bin/true") };

    let refused = ensure_core_running(&socket_path);

    assert!(
        refused.is_err(),
        "a silent binary must fail the start, not hang it"
    );

    // Safety: still under the lock.
    unsafe { std::env::remove_var("KANBAN_CORE_BIN") };
}
