//! The shell's launch arguments. A `kanban:` link starts the shell
//! holding the link's text exactly as the operating system passed it;
//! a flag the shell knows does its one job; anything else is refused
//! with usage, before any window or core starts. A link launch is read
//! here the way `main` reads it rather than run, because running it
//! starts the real shell, which would hand its link to a shell the
//! developer already has open; `issue_links.rs` follows the link it
//! yields through the shell to the WebView.

use std::ffi::OsString;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use kanban_desktop_lib::launch::{self, Launch};

fn read(arguments: &[&str]) -> Launch {
    let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
    launch::read(&arguments)
}

#[test]
fn a_link_argument_starts_the_shell_with_the_link_as_it_arrived() {
    for link in [
        "kanban://issue/CORE-12",
        "kanban://issue/x/../CORE-12",
        "kanban://issue/CORE-12 ",
        "KANBAN://issue/CORE-12",
        "kanban://board",
    ] {
        assert_eq!(
            read(&[link]),
            Launch::Shell {
                link: Some(link.to_owned())
            },
            "{link:?} starts the shell holding exactly that text"
        );
    }
}

#[test]
fn no_argument_starts_the_shell_without_a_link() {
    assert_eq!(read(&[]), Launch::Shell { link: None });
}

#[test]
fn the_shell_flags_do_their_own_jobs() {
    assert_eq!(read(&["--version"]), Launch::Version);
    assert_eq!(
        read(&["--package-smoke", "/tmp/new-data"]),
        Launch::PackageSmoke("/tmp/new-data".into())
    );
}

#[test]
fn an_argument_that_is_neither_a_flag_nor_a_link_is_read_as_a_refusal() {
    for arguments in [
        &["--bogus"][..],
        &["issue/CORE-12"],
        &["kanban://issue/CORE-12", "kanban://issue/EDGE-3"],
        &["--version", "kanban://issue/CORE-12"],
    ] {
        assert_eq!(read(arguments), Launch::Usage, "{arguments:?}");
    }
}

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
