//! Mechanically checked Tauri command surface. Handler symbols are
//! the single shell-side list consumed by both Tauri registration
//! and the catalogue parity test.

/// Declare the shell's typed Tauri commands and derive the checked
/// representation from the same symbols `generate_handler!` uses:
/// one command per catalogue operation, then, after `; shell:`, the
/// shell's own commands, which forward nothing to the core.
macro_rules! shell_handler_catalogue {
    ($($handler:ident),+ ; shell: $($shell:ident),* $(,)?) => {
        /// Every typed Tauri command forwarding a catalogue operation.
        pub const REGISTERED_TAURI_COMMANDS: &[&str] = &[$(stringify!($handler)),*];

        /// The shell's own typed commands: plumbing the WebView needs
        /// from the shell itself, never a catalogue operation.
        pub const SHELL_TAURI_COMMANDS: &[&str] = &[$(stringify!($shell)),*];

        /// Register every catalogue operation, and the shell's own
        /// commands, with one typed handler. Generic over the runtime
        /// so the tests drive this same registration on Tauri's mock
        /// runtime.
        pub fn catalogue_invoke_handler<R: tauri::Runtime>(
        ) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
            tauri::generate_handler![$($handler,)* $($shell),*]
        }
    };
}

pub(crate) use shell_handler_catalogue;
