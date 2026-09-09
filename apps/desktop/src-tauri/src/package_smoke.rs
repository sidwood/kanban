//! A bounded installed-artifact probe using the shell's real discovery and IPC.
//! It exclusively owns a newly created data directory, never an existing Core.
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

use kanban_dto::{HealthResponse, build_identity};
use serde_json::{Value, json};

use crate::{core_link::CoreLink, ensure_core_running, locate_core_binary};

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// Validate a copied bundle without needing a checkout, tools, or a WebView.
/// A successful result means graceful process exit, not merely a stop response.
pub fn run(data_dir: &Path) -> Result<Value, String> {
    if !data_dir.is_absolute() {
        return Err("the package smoke data directory must be absolute".to_owned());
    }
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(data_dir)
        .map_err(|error| format!("package smoke data directory must not exist: {error}"))?;
    let service_binary = locate_core_binary()?;
    if !service_binary.with_file_name("kanban-mcp").is_file() {
        return Err("the required packaged MCP adapter is missing; reinstall Kanban".to_owned());
    }
    let socket = data_dir.join("core.sock");
    let child = ensure_core_running(&socket)?
        .ok_or_else(|| "package smoke refuses to reuse an existing service".to_owned())?;
    let mut child = OwnedChild(child);
    let link = CoreLink::connect(&socket).map_err(|error| error.to_string())?;
    let health: HealthResponse = serde_json::from_value(
        link.query("health.get", &json!({}))
            .map_err(|error| error.message)?,
    )
    .map_err(|error| error.to_string())?;
    if !health.connected
        || health.service_version != build_identity::VERSION
        || health.service.source_revision != build_identity::SOURCE_REVISION
        || health.service.source_epoch != build_identity::SOURCE_EPOCH
    {
        return Err(
            "the bundled shell and running service have different build identities".to_owned(),
        );
    }
    if health
        .herdr
        .connection_diagnostic
        .as_deref()
        .is_none_or(str::is_empty)
    {
        return Err(
            "fresh installed health did not explain the external Herdr prerequisite".to_owned(),
        );
    }
    let warning = link
        .query("service.stop_warning", &json!({}))
        .map_err(|error| error.message)?;
    let response = link
        .command(
            "service.stop",
            &json!({
                "mutation": {
                    "optimistic_version": warning["version"],
                    "idempotency_key": format!("package-smoke-stop-{}", warning["instance_id"]),
                },
                "instance_id": warning["instance_id"],
                "warning_id": warning["warning_id"],
                "confirmed": true,
            }),
        )
        .map_err(|error| error.message)?;
    if response["status"] != "stop_requested" || response["instance_id"] != warning["instance_id"] {
        return Err("the service did not acknowledge this instance's stop request".to_owned());
    }
    drop(link);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.0.try_wait().map_err(|error| error.to_string())? {
            if !status.success() || socket.exists() {
                return Err("the service did not complete a clean shutdown".to_owned());
            }
            return Ok(json!({
                "identity": build_identity::json(),
                "health": health,
                "service_binary": service_binary,
                "stopped": true,
            }));
        }
        if Instant::now() >= deadline {
            return Err("the service did not stop before the installed probe deadline".to_owned());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
