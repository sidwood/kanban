use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use kanban_app::contracts_gen::{default_output_root, generate};

fn main() -> ExitCode {
    let output_root = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_output_root);

    if let Err(error) = generate(&output_root) {
        eprintln!("kanban-contracts-gen: {error}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
