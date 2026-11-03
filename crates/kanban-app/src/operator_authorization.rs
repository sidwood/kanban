//! Installation-authenticated operator MCP session (Linear issue tools).
use crate::Core;
use crate::catalog::{OperationKind, exposed_operations};
use crate::issue_operator::OPERATOR_MCP_OPERATIONS;
use kanban_dto::ApiError;
use serde_json::Value;
use std::sync::Arc;

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
            OperationKind::Command => self.core.command(name, payload),
        }
    }
}

fn denied() -> ApiError {
    ApiError::invalid_request("operation is outside the installation operator surface")
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
