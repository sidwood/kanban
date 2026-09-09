//! Desktop shell entry point. The core process owns durability; this
//! process owns the window (ADR-0001).

// Prevent an extra console window on Windows; a no-op on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    match arguments.as_slice() {
        [] => kanban_desktop_lib::run().expect("the desktop shell should run"),
        [flag] if flag == "--version" => println!("{}", kanban_dto::build_identity::json()),
        [flag, directory] if flag == "--package-smoke" => {
            match kanban_desktop_lib::package_smoke::run(std::path::Path::new(directory)) {
                Ok(report) => println!("{report}"),
                Err(error) => {
                    eprintln!("installed package probe failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("usage: Kanban [--version | --package-smoke NEW_DATA_DIR]");
            std::process::exit(2);
        }
    }
}
