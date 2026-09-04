//! Desktop shell entry point. The core process owns durability; this
//! process owns the window (ADR-0001).

// Prevent an extra console window on Windows; a no-op on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    kanban_desktop_lib::run().expect("the desktop shell should run");
}
