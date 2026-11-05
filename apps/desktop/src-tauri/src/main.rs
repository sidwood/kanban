//! Desktop shell entry point. The core process owns durability; this
//! process owns the window (ADR-0001).

// Prevent an extra console window on Windows; a no-op on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use kanban_desktop_lib::launch::{self, Launch};

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    match launch::read(&arguments) {
        Launch::Shell { link } => {
            kanban_desktop_lib::run(link).expect("the desktop shell should run")
        }
        Launch::Version => println!("{}", kanban_dto::build_identity::json()),
        Launch::PackageSmoke(directory) => {
            match kanban_desktop_lib::package_smoke::run(&directory) {
                Ok(report) => println!("{report}"),
                Err(error) => {
                    eprintln!("installed package probe failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        Launch::Usage => {
            eprintln!(
                "usage: Kanban [--version | --package-smoke NEW_DATA_DIR | kanban://issue/KEY]"
            );
            std::process::exit(2);
        }
    }
}
