//! Release-profile probe: prints the resolved core binary path so
//! integration tests can prove release builds ignore `KANBAN_CORE_BIN`.

use std::io::Write;

fn main() {
    let resolved = kanban_desktop_lib::locate_core_binary().unwrap_or_else(|failure| {
        eprintln!("{failure}");
        std::process::exit(1);
    });
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{}", resolved.display()).expect("the resolved path prints");
}
