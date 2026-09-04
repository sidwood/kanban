//! Thin Tauri shell: window, lifecycle, and the core socket client
//! (ADR-0001). It starts the core on demand, exposes only typed
//! commands, and forwards the core's ordered events; quitting the
//! window never takes the core down with it. Domain rules never live
//! here.

use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kanban_dto::{ApiError, HealthResponse};
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};

pub mod core_link;

use core_link::CoreLink;

/// The shell emits the core's ordered events under this name.
pub const CORE_EVENT: &str = "core://event";

/// The shell announces its connection to the core under this name.
/// This is shell-level plumbing, not a domain contract: the WebView
/// confirms real state through the generated client's health query.
pub const CONNECTION_EVENT: &str = "core://connection";

/// How long the on-demand start waits for the spawned core to serve.
const CORE_START_TIMEOUT: Duration = Duration::from_secs(15);

/// How often the on-demand start polls for the socket.
const CORE_START_POLL: Duration = Duration::from_millis(100);

/// The shell's connection to the core, when it has one.
#[derive(Default)]
struct Shell {
    link: Mutex<Option<CoreLink>>,
}

/// The connection the shell announces to the WebView.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectionState {
    /// The socket is serving and the event stream is attached.
    Connected,
    /// No core is reachable.
    Disconnected,
}

/// The one query the catalog exposes today, served through the
/// generated contract's DTOs.
#[tauri::command]
async fn health_get(shell: State<'_, Arc<Shell>>) -> Result<HealthResponse, ApiError> {
    let shell = shell.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = shell
            .link
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let link = guard.as_ref().ok_or_else(|| {
            ApiError::internal("the core connection is not up; retry once it connects")
        })?;
        let payload = link.query("health.get", &json!({}))?;
        serde_json::from_value(payload)
            .map_err(|_| ApiError::internal("the health answer did not match its contract"))
    })
    .await
    .map_err(|_| ApiError::internal("the health task did not finish"))?
}

/// Build the window, start the core on demand, and supervise the
/// connection for as long as this shell process lives.
pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .setup(|app| {
            let socket_path = managed_socket_path()?;
            let shell = Arc::new(Shell::default());
            app.manage(shell.clone());
            let supervisor = app.handle().clone();
            let spawn = std::thread::Builder::new()
                .name("kanban-shell-supervisor".to_owned())
                .spawn(move || supervise(socket_path, shell, supervisor));
            match spawn {
                Ok(_) => Ok(()),
                Err(failure) => Err(Box::new(failure) as Box<dyn std::error::Error>),
            }
        })
        .invoke_handler(tauri::generate_handler![health_get])
        .run(tauri::generate_context!())
}

/// Keep the shell's view of the core honest: start it if it is not
/// serving, connect, forward events, and announce loss.
fn supervise(socket_path: PathBuf, shell: Arc<Shell>, app: AppHandle) {
    let spawned = match ensure_core_running(&socket_path) {
        Ok(spawned) => spawned,
        Err(failure) => {
            eprintln!("kanban shell: {failure}");
            let _ = app.emit(CONNECTION_EVENT, ConnectionState::Disconnected);
            return;
        }
    };
    let link = match CoreLink::connect(&socket_path) {
        Ok(link) => link,
        Err(failure) => {
            eprintln!("kanban shell: the core socket is unreachable: {failure}");
            let _ = app.emit(CONNECTION_EVENT, ConnectionState::Disconnected);
            return;
        }
    };
    eprintln!("kanban shell: connected to {}", socket_path.display());
    *shell
        .link
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(link);
    let _ = app.emit(CONNECTION_EVENT, ConnectionState::Connected);

    // Blocks until the core closes the socket or the connection
    // dies; one reader thread keeps event order intact.
    let event_app = app.clone();
    let forward = core_link::forward_events(&socket_path, move |envelope| {
        let _ = event_app.emit(CORE_EVENT, envelope);
    });
    if let Err(failure) = forward {
        eprintln!("kanban shell: the event stream ended: {failure}");
    }
    // The socket is gone: drop the request link, say so, and reap
    // the core we spawned if it was ours and has exited. The shell
    // never kills a live core; reconnecting is KAN-S13 hardening.
    *shell
        .link
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let _ = app.emit(CONNECTION_EVENT, ConnectionState::Disconnected);
    if let Some(mut child) = spawned {
        let _ = child.try_wait();
    }
}

/// The socket the core serves inside managed application data.
fn managed_socket_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let data_dir = kanban_storage::paths::managed_data_dir()?;
    Ok(data_dir.join(kanban_transport::SOCKET_FILE_NAME))
}

/// True when a core is already serving `socket_path`.
pub fn socket_serving(socket_path: &Path) -> bool {
    UnixStream::connect(socket_path).is_ok()
}

/// Make sure exactly one core is serving `socket_path`: reuse a live
/// core, otherwise spawn one detached and wait for it to serve. The
/// child is returned for reaping, never for killing — quitting the
/// UI must leave the core running (DR-RB-02).
pub fn ensure_core_running(socket_path: &Path) -> Result<Option<Child>, String> {
    if socket_serving(socket_path) {
        return Ok(None);
    }
    let binary = locate_core_binary()?;
    let data_dir = socket_path.parent().unwrap_or_else(|| Path::new("."));
    let child = Command::new(&binary)
        .arg(data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Own process group: signals aimed at the shell's group (a
        // Ctrl-C on `tauri dev`) must not reach the durable core.
        .process_group(0)
        .spawn()
        .map_err(|failure| {
            format!(
                "could not start the core at {}: {failure}",
                binary.display()
            )
        })?;
    let deadline = std::time::Instant::now() + CORE_START_TIMEOUT;
    while std::time::Instant::now() < deadline {
        if socket_serving(socket_path) {
            return Ok(Some(child));
        }
        std::thread::sleep(CORE_START_POLL);
    }
    Err(format!(
        "the core at {} did not serve its socket within {} seconds",
        binary.display(),
        CORE_START_TIMEOUT.as_secs()
    ))
}

/// Where the core binary is: an explicit override first, the copy
/// packaged beside the shell second, the workspace build `just dev`
/// produces third.
fn locate_core_binary() -> Result<PathBuf, String> {
    if let Some(override_path) = std::env::var_os("KANBAN_CORE_BIN") {
        return Ok(PathBuf::from(override_path));
    }
    if let Ok(exe) = std::env::current_exe() {
        let beside = exe
            .parent()
            .map(|dir| dir.join("kanban-service"))
            .filter(|path| path.is_file());
        if let Some(beside) = beside {
            return Ok(beside);
        }
    }
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/debug/kanban-service");
    if dev.is_file() {
        return Ok(dev);
    }
    Err(
        "no kanban-service binary found; set KANBAN_CORE_BIN or build it with `cargo build -p kanban-service`"
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    /// Env-var tests share one lock: std::env::set_var races
    /// otherwise.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn an_override_names_the_core_binary() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // Safety: the lock above serialises every env-writing test.
        unsafe { std::env::set_var("KANBAN_CORE_BIN", "/explicit/kanban-service") };
        let located = super::locate_core_binary().expect("the override resolves");
        assert_eq!(
            located,
            std::path::PathBuf::from("/explicit/kanban-service")
        );
        // Safety: still under the lock.
        unsafe { std::env::remove_var("KANBAN_CORE_BIN") };
    }
}
