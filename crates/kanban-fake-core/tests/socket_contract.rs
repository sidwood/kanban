//! Socket-contract smoke for the fake core binary.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use tempfile::TempDir;

#[test]
fn fake_core_serves_the_socket_contract() {
    let dir = TempDir::new().expect("a scratch directory is available");
    let socket_path = dir.path().join("core.sock");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_fake-core"))
        .arg(dir.path())
        .spawn()
        .expect("the fake core spawns");

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if UnixStream::connect(&socket_path).is_ok() {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = child.kill();
    let _ = child.wait();
    panic!("the fake core never served its socket");
}
