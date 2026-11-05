//! Installation-authenticated operator MCP session (Linear issue tools).
use crate::Core;
use crate::catalog::{OperationKind, exposed_operations};
use crate::issue_operator::OPERATOR_MCP_OPERATIONS;
use kanban_dto::{ApiError, ErrorCode};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct OperatorSession {
    core: Arc<Core>,
}

impl OperatorSession {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }

    pub fn operations(&self) -> Result<Vec<String>, ApiError> {
        Ok(OPERATOR_MCP_OPERATIONS
            .iter()
            .map(|name| (*name).to_owned())
            .collect())
    }

    pub fn call(&self, name: &str, payload: &Value) -> Result<Value, ApiError> {
        if !OPERATOR_MCP_OPERATIONS.contains(&name) {
            return Err(denied());
        }
        let kind = exposed_operations()
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.kind)
            .ok_or_else(denied)?;
        match kind {
            OperationKind::Query => self.core.query(name, payload),
            OperationKind::Command => command_with_optional_mutation(&self.core, name, payload),
        }
    }
}

fn denied() -> ApiError {
    ApiError::invalid_request("operation is outside the installation operator surface")
}

fn command_with_optional_mutation(
    core: &Core,
    name: &str,
    payload: &Value,
) -> Result<Value, ApiError> {
    if payload
        .get("mutation")
        .is_some_and(|value| !value.is_null())
    {
        return core.command(name, payload);
    }
    let mut version = 0_u64;
    let mut last_stale = None;
    for _ in 0..8 {
        let mut attempt = payload.clone();
        let object = attempt.as_object_mut().ok_or_else(|| {
            ApiError::invalid_request("operator command payload must be an object")
        })?;
        object.insert(
            "mutation".to_owned(),
            json!({
                "optimistic_version": version,
                "idempotency_key": fresh_operator_key(),
            }),
        );
        match core.command(name, &attempt) {
            Err(error) if error.code == ErrorCode::StaleVersion => {
                let Some(current) = error.current_version else {
                    return Err(error);
                };
                version = current;
                last_stale = Some(error);
            }
            other => return other,
        }
    }
    Err(last_stale.unwrap_or_else(|| ApiError::internal("operator write did not settle")))
}

fn fresh_operator_key() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let ticks = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "operator-{}-{}-{}",
        std::process::id(),
        ticks,
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::exposed_operations;

    #[test]
    fn operator_surface_matches_catalogue() {
        for name in OPERATOR_MCP_OPERATIONS {
            assert!(
                exposed_operations().iter().any(|op| op.name == *name),
                "operator operation `{name}` is missing from the catalogue"
            );
        }
    }
}
