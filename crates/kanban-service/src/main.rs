//! Durable per-user core process entry point (ADR-0001).

fn main() {
    if std::env::args_os().skip(1).eq(["--version"]) {
        println!("{}", kanban_dto::build_identity::json());
        return;
    }
    if let Err(failure) = kanban_service::run_managed() {
        eprintln!("kanban core could not start: {failure}");
        std::process::exit(1);
    }
}
