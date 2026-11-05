//! On Linux the shell makes itself the `kanban` scheme's handler as it
//! starts, with nothing announced to the WebView: it writes its own
//! desktop entry and makes it the desktop's default for the scheme.
//! The one test here points the desktop's data and configuration
//! directories at a scratch home first, so it never touches the
//! developer's own.
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Command;

use kanban_desktop_lib::issue_link;

#[test]
fn the_shell_makes_itself_the_kanban_scheme_handler() {
    let home = tempfile::TempDir::new().expect("a scratch home");
    let data = home.path().join("data");
    let config = home.path().join("config");
    // SAFETY: this binary holds this one test, which sets the variables
    // before it starts any thread or child process.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("XDG_DATA_HOME", &data);
        std::env::set_var("XDG_CONFIG_HOME", &config);
    }
    let applications = data.join("applications");

    issue_link::write_scheme_handler(
        &applications,
        Path::new("/opt/kanban/kanban-desktop"),
        Path::new("/opt/kanban/Kanban.AppImage"),
        Some("Kanban"),
    )
    .expect("the scheme is registered");

    let entry = std::fs::read_to_string(applications.join("kanban-desktop-handler.desktop"))
        .expect("the desktop entry is written");
    assert_eq!(
        entry,
        "[Desktop Entry]\nType=Application\nName=Kanban\nExec=\"/opt/kanban/Kanban.AppImage\" %u\nTerminal=false\nMimeType=x-scheme-handler/kanban;\nNoDisplay=true\n"
    );
    let handler = Command::new("xdg-mime")
        .args(["query", "default", "x-scheme-handler/kanban"])
        .output()
        .expect("xdg-mime answers");
    assert_eq!(
        String::from_utf8_lossy(&handler.stdout).trim(),
        "kanban-desktop-handler.desktop",
        "the desktop opens kanban links with the shell"
    );
}
