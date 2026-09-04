//! Durable per-user core process entry point (ADR-0001).

fn main() {
    if let Err(failure) = kanban_service::run_managed() {
        eprintln!("kanban core could not start: {failure}");
        std::process::exit(1);
    }
}
