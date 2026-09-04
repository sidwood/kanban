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
        pub fn catalogue_invoke_handler(
        ) -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
            tauri::generate_handler![$($handler),*]
        }
    };
}

pub(crate) use shell_handler_catalogue;
