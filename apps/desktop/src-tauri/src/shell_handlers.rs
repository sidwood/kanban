//! Mechanically checked Tauri command surface. Handler symbols are
//! the single shell-side list consumed by both Tauri registration
//! and the catalogue parity test.

/// Declare the shell's typed Tauri commands and derive the checked
/// representation from the same symbols `generate_handler!` uses.
macro_rules! shell_handler_catalogue {
    ($($handler:ident),* $(,)?) => {
        /// Every typed Tauri command the shell exposes.
        pub const REGISTERED_TAURI_COMMANDS: &[&str] = &[$(stringify!($handler)),*];

        /// Register every catalogue operation with one typed handler.
        /// Generic over the runtime so the tests drive this same
        /// registration on Tauri's mock runtime.
        pub fn catalogue_invoke_handler<R: tauri::Runtime>(
        ) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
            tauri::generate_handler![$($handler),*]
        }
    };
}

pub(crate) use shell_handler_catalogue;
