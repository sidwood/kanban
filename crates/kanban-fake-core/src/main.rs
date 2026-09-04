//! A real core binary for the on-demand start tests: it serves a
//! scratch data directory and parks, exactly like the managed core.

fn main() {
    let data_dir = std::env::args()
        .nth(1)
        .expect("usage: fake-core <data-dir>");
    // Held for the process's life so the served core never drops.
    let _core =
        kanban_service::serve(std::path::Path::new(&data_dir)).expect("the fake core boots");
    std::fs::write(
        std::path::Path::new(&data_dir).join("fake-core.pid"),
        std::process::id().to_string(),
    )
    .expect("the fake core reports its pid");
    loop {
        std::thread::park();
    }
}
