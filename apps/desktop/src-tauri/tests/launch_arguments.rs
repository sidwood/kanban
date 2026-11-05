//! The shell's launch arguments: one that is neither a flag the shell
//! knows nor a `kanban:` link is refused with usage, before any window
//! or core starts. A link launch itself cannot be driven here: it
//! starts the real shell, which would hand its link to a shell the
//! developer already has open.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn an_argument_that_is_neither_a_flag_nor_a_link_is_refused() {
    for argument in [
        "--bogus",
        "https://example.com/issue/CORE-12",
        "kanban-dev://issue/CORE-12",
        "issue/CORE-12",
    ] {
        let home = tempfile::TempDir::new().expect("isolated home");
        let mut child = Command::new(env!("CARGO_BIN_EXE_kanban-desktop"))
            .arg(argument)
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin")
            .env("KANBAN_CORE_BIN", "/nonexistent/isolated-core")
            .current_dir(home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the built shell starts");
        let deadline = Instant::now() + Duration::from_secs(3);
        while child.try_wait().expect("child status").is_none() {
            if Instant::now() >= deadline {
                child.kill().expect("stop an unexpectedly started shell");
                child.wait().expect("shell reaped");
                panic!("`{argument}` must be refused without starting the shell");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().expect("refusal output");
        assert_eq!(output.status.code(), Some(2), "`{argument}` is refused");
        let usage = String::from_utf8_lossy(&output.stderr);
        assert!(
            usage.contains("usage: Kanban") && usage.contains("kanban://issue/KEY"),
            "the refusal names every accepted form: {usage}"
        );
        assert_eq!(
            std::fs::read_dir(home.path()).unwrap().count(),
            0,
            "a refused launch writes nothing"
        );
    }
}
