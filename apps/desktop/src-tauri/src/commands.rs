//! Tauri command forwarding: one generated request DTO per operation.

use std::sync::Arc;

use kanban_dto::ApiError;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::Shell;
use crate::core_link::CoreLink;

/// The IPC envelope every shell command accepts from the WebView.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellInvokeArgs<T> {
    pub request: T,
}

/// Encode a typed request into its JSON payload for the core.
pub fn encode_request<T: Serialize>(request: T) -> Result<Value, ApiError> {
    serde_json::to_value(request)
        .map_err(|_| ApiError::internal("the typed request could not be encoded"))
}

/// Decode the IPC envelope the WebView hands to a shell command.
pub fn decode_invoke_args<T: DeserializeOwned>(payload: Value) -> Result<T, ApiError> {
    let envelope: ShellInvokeArgs<T> = serde_json::from_value(payload).map_err(|_| {
        ApiError::invalid_request("the shell request envelope could not be decoded")
    })?;
    Ok(envelope.request)
}

/// Run one named query on the shell's link.
pub fn forward_query<T: Serialize, R: DeserializeOwned>(
    shell: &Arc<Shell>,
    operation: &str,
    subject: &str,
    request: T,
) -> Result<R, ApiError> {
    let payload = encode_request(request)?;
    forward_query_value(shell, operation, subject, payload)
}

/// Run one named command on the shell's link.
pub fn forward_command<T: Serialize, R: DeserializeOwned>(
    shell: &Arc<Shell>,
    operation: &str,
    subject: &str,
    request: T,
) -> Result<R, ApiError> {
    let payload = encode_request(request)?;
    forward_command_value(shell, operation, subject, payload)
}

/// Run one named query with an already-encoded payload.
pub fn forward_query_value<R: DeserializeOwned>(
    shell: &Arc<Shell>,
    operation: &str,
    subject: &str,
    payload: Value,
) -> Result<R, ApiError> {
    over_link(shell, subject, |link| link.query(operation, &payload))
}

/// Run one named command with an already-encoded payload.
pub fn forward_command_value<R: DeserializeOwned>(
    shell: &Arc<Shell>,
    operation: &str,
    subject: &str,
    payload: Value,
) -> Result<R, ApiError> {
    over_link(shell, subject, |link| link.command(operation, &payload))
}

/// Install a core link for tests that exercise the forwarding path.
pub fn install_link(shell: &Arc<Shell>, link: CoreLink) {
    *shell
        .link
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(link);
}

fn over_link<T, F>(shell: &Arc<Shell>, subject: &str, call: F) -> Result<T, ApiError>
where
    T: DeserializeOwned,
    F: FnOnce(&CoreLink) -> Result<Value, ApiError>,
{
    let guard = shell
        .link
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let link = guard.as_ref().ok_or_else(|| {
        ApiError::internal("the core connection is not up; retry once it connects")
    })?;
    let payload = call(link)?;
    serde_json::from_value(payload).map_err(|_| {
        ApiError::internal(&format!("the {subject} answer did not match its contract"))
    })
}

#[cfg(test)]
mod tests {
    use kanban_dto::{HealthQuery, InitiativeCreateRequest, MutationContext};
    use serde_json::json;

    use super::{ShellInvokeArgs, decode_invoke_args};

    fn mutation() -> MutationContext {
        MutationContext {
            optimistic_version: 0,
            idempotency_key: "key-1".to_owned(),
        }
    }

    #[test]
    fn the_invoke_envelope_rejects_unknown_top_level_fields() {
        let refused: Result<ShellInvokeArgs<HealthQuery>, _> =
            serde_json::from_value(json!({ "request": {}, "extra": true }));
        assert!(refused.is_err(), "unknown top-level fields are rejected");
    }

    #[test]
    fn the_invoke_envelope_rejects_unknown_request_fields() {
        let refused: Result<ShellInvokeArgs<InitiativeCreateRequest>, _> =
            serde_json::from_value(json!({
                "request": {
                    "mutation": mutation(),
                    "name": "Alpha",
                    "delete": true,
                },
            }));
        assert!(refused.is_err(), "unknown request fields are rejected");
    }

    #[test]
    fn decode_invoke_args_returns_the_inner_request() {
        let request = InitiativeCreateRequest {
            mutation: mutation(),
            name: "Alpha".to_owned(),
        };
        let decoded = decode_invoke_args::<InitiativeCreateRequest>(json!({
            "request": {
                "mutation": mutation(),
                "name": "Alpha",
            },
        }))
        .expect("the envelope decodes");
        assert_eq!(decoded, request);
    }

    #[test]
    fn decode_invoke_args_refuses_unknown_request_fields() {
        let refused = decode_invoke_args::<InitiativeCreateRequest>(json!({
            "request": {
                "mutation": mutation(),
                "name": "Alpha",
                "extra": true,
            },
        }));
        assert!(refused.is_err(), "unknown request fields are rejected");
    }
}
