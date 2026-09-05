//! One Project's Herdr binding: the effective session, the product
//! workspace it maps, and the target Herdr workspace inside that
//! session (DR-HB-01, DR-HB-02, DR-HB-19).

use kanban_domain::HerdrSession;

use crate::error::HerdrError;
use crate::protocol::Snapshot;

/// The mapping a Project's Herdr observation is grounded in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMapping {
    session: HerdrSession,
    product_workspace: String,
    herdr_workspace: String,
}

impl SessionMapping {
    /// Bind one effective session to one product workspace and the
    /// Herdr workspace it targets inside that session.
    pub fn new(session: HerdrSession, product_workspace: &str, herdr_workspace: &str) -> Self {
        Self {
            session,
            product_workspace: product_workspace.to_owned(),
            herdr_workspace: herdr_workspace.to_owned(),
        }
    }

    /// The session this mapping resolves inside: named, or Herdr's
    /// default session.
    pub fn session(&self) -> &HerdrSession {
        &self.session
    }

    /// The one product Seed Workspace this session maps to.
    pub fn product_workspace(&self) -> &str {
        &self.product_workspace
    }

    /// The one target Herdr workspace, resolved inside the effective
    /// session: the same identifier in a different session is a
    /// different workspace (DR-HB-19).
    pub fn herdr_workspace(&self) -> &str {
        &self.herdr_workspace
    }

    /// Confirm a snapshot serves this mapping: the session a name
    /// selected, the product workspace, and the target Herdr
    /// workspace, all resolved inside the session this connection
    /// reached.
    pub fn verify_snapshot(&self, snapshot: &Snapshot) -> Result<(), HerdrError> {
        if let Some(name) = self.session.as_name()
            && snapshot.session != name
        {
            return Err(HerdrError::Remote {
                message: format!(
                    "snapshot named session `{}`, expected `{}`",
                    snapshot.session, name
                ),
            });
        }
        if snapshot.product_workspace != self.product_workspace {
            return Err(HerdrError::WorkspaceMismatch {
                expected: self.product_workspace.clone(),
                observed: snapshot.product_workspace.clone(),
            });
        }
        if snapshot.herdr_workspace != self.herdr_workspace {
            return Err(HerdrError::WorkspaceMismatch {
                expected: self.herdr_workspace.clone(),
                observed: snapshot.herdr_workspace.clone(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod session_mapping {
    use kanban_domain::HerdrSession;

    use super::SessionMapping;
    use crate::error::HerdrError;
    use crate::protocol::Snapshot;

    fn named_mapping() -> SessionMapping {
        SessionMapping::new(
            HerdrSession::named("kanban-main").expect("the name validates"),
            "/workspaces/kanban.seed",
            "kanban.seed",
        )
    }

    fn snapshot(session: &str, product_workspace: &str, herdr_workspace: &str) -> Snapshot {
        Snapshot {
            session: session.to_owned(),
            product_workspace: product_workspace.to_owned(),
            herdr_workspace: herdr_workspace.to_owned(),
            state: serde_json::json!({}),
            captured_at: "2026-09-05T04:46:00Z".to_owned(),
        }
    }

    #[test]
    fn a_named_mapping_accepts_a_matching_session_and_workspaces() {
        named_mapping()
            .verify_snapshot(&snapshot(
                "kanban-main",
                "/workspaces/kanban.seed",
                "kanban.seed",
            ))
            .expect("the mapping accepts a matching snapshot");
    }

    #[test]
    fn verify_snapshot_refuses_the_session_a_name_selected() {
        let refusal = named_mapping().verify_snapshot(&snapshot(
            "other-main",
            "/workspaces/kanban.seed",
            "kanban.seed",
        ));

        assert_eq!(
            refusal,
            Err(HerdrError::Remote {
                message: "snapshot named session `other-main`, expected `kanban-main`".to_owned(),
            })
        );
    }

    #[test]
    fn verify_snapshot_refuses_a_different_product_workspace() {
        let mapping = SessionMapping::new(
            HerdrSession::named("kanban-main").expect("the name validates"),
            "/workspaces/kanban.seed",
            "kanban.seed",
        );
        let refusal = mapping.verify_snapshot(&snapshot(
            "kanban-main",
            "/workspaces/other.seed",
            "kanban.seed",
        ));

        assert_eq!(
            refusal,
            Err(HerdrError::WorkspaceMismatch {
                expected: "/workspaces/kanban.seed".to_owned(),
                observed: "/workspaces/other.seed".to_owned(),
            })
        );
    }

    #[test]
    fn verify_snapshot_refuses_a_different_herdr_workspace() {
        let refusal = named_mapping().verify_snapshot(&snapshot(
            "kanban-main",
            "/workspaces/kanban.seed",
            "other.seed",
        ));

        assert_eq!(
            refusal,
            Err(HerdrError::WorkspaceMismatch {
                expected: "kanban.seed".to_owned(),
                observed: "other.seed".to_owned(),
            })
        );
    }

    #[test]
    fn the_default_session_accepts_whatever_name_herdr_reports() {
        let mapping = SessionMapping::new(
            HerdrSession::Default,
            "/workspaces/kanban.seed",
            "kanban.seed",
        );

        mapping
            .verify_snapshot(&snapshot(
                "default",
                "/workspaces/kanban.seed",
                "kanban.seed",
            ))
            .expect("the default session is not named, so its reported name is accepted");
        mapping
            .verify_snapshot(&snapshot(
                "kanban-main",
                "/workspaces/kanban.seed",
                "kanban.seed",
            ))
            .expect("the workspace identities, not the reported name, ground the mapping");
    }
}
