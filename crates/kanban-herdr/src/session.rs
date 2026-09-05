//! One Project's exclusive Herdr session mapped to one product
//! workspace (DR-HB-01, DR-HB-02).

use crate::error::HerdrError;
use crate::protocol::Snapshot;

/// The mapping a Project's Herdr observation is grounded in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMapping {
    session_name: String,
    product_workspace: String,
}

impl SessionMapping {
    /// Bind one named session to one product workspace.
    pub fn new(session_name: &str, product_workspace: &str) -> Self {
        Self {
            session_name: session_name.to_owned(),
            product_workspace: product_workspace.to_owned(),
        }
    }

    /// The exclusive named Herdr session.
    pub fn session_name(&self) -> &str {
        &self.session_name
    }

    /// The one product Seed Workspace this session serves.
    pub fn product_workspace(&self) -> &str {
        &self.product_workspace
    }

    /// Confirm a snapshot maps this session to the expected workspace.
    pub fn verify_snapshot(&self, snapshot: &Snapshot) -> Result<(), HerdrError> {
        if snapshot.session != self.session_name {
            return Err(HerdrError::Remote {
                message: format!(
                    "snapshot named session `{}`, expected `{}`",
                    snapshot.session, self.session_name
                ),
            });
        }
        if snapshot.product_workspace != self.product_workspace {
            return Err(HerdrError::WorkspaceMismatch {
                expected: self.product_workspace.clone(),
                observed: snapshot.product_workspace.clone(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::SessionMapping;
    use crate::error::HerdrError;
    use crate::protocol::Snapshot;

    fn snapshot(product_workspace: &str) -> Snapshot {
        Snapshot {
            session: "kanban-main".to_owned(),
            product_workspace: product_workspace.to_owned(),
            herdr_workspace: "kanban.seed".to_owned(),
            state: json!({}),
            captured_at: "2026-09-05T04:46:00Z".to_owned(),
        }
    }

    #[test]
    fn verify_snapshot_accepts_a_matching_workspace() {
        let mapping = SessionMapping::new("kanban-main", "/workspaces/kanban.seed");
        mapping
            .verify_snapshot(&snapshot("/workspaces/kanban.seed"))
            .expect("the mapping accepts a matching snapshot");
    }

    #[test]
    fn verify_snapshot_refuses_a_different_workspace() {
        let mapping = SessionMapping::new("kanban-main", "/workspaces/kanban.seed");
        let refusal = mapping.verify_snapshot(&snapshot("/workspaces/other.seed"));
        assert_eq!(
            refusal,
            Err(HerdrError::WorkspaceMismatch {
                expected: "/workspaces/kanban.seed".to_owned(),
                observed: "/workspaces/other.seed".to_owned(),
            })
        );
    }
}
