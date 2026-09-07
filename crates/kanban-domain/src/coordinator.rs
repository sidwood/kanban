//! Coordinator loop domain rules (KAN-S9-US2, DR-HB-15): workspace
//! selection under the reuse rules and the branch naming a Ticket's
//! execution checkout carries.

use crate::{Workspace, WorkspaceId};

/// The branch name one Ticket's execution checkout uses.
pub fn execution_branch(ticket_number: u64) -> String {
    format!("kan-t{}", ticket_number)
}

/// The Workspace path one Ticket's execution checkout lands in.
pub fn execution_workspace_path(ticket_number: u64) -> String {
    format!("/workspaces/kanban.{}", execution_branch(ticket_number))
}

/// Select the first reusable Workspace, in stable id order. A
/// reusable Workspace is clean, unassigned, and free of unique
/// unlanded commits (DR-LW-06).
pub fn select_reusable_workspace(workspaces: &[Workspace]) -> Option<WorkspaceId> {
    workspaces
        .iter()
        .filter(|workspace| workspace.reuse_evaluation().reusable())
        .min_by_key(|workspace| workspace.id().value())
        .map(|workspace| workspace.id())
}

#[cfg(test)]
mod reuse_rules {
    use crate::project::ProjectId;
    use crate::workspace::{Workspace, WorkspaceCheckout, WorkspaceId, WorkspaceRegistration};

    use super::{execution_branch, execution_workspace_path, select_reusable_workspace};

    fn registration(path: &str) -> WorkspaceRegistration {
        WorkspaceRegistration::new(ProjectId::new(1), path, false)
            .expect("the registration validates")
    }

    fn observed_clean(workspace: &mut Workspace) {
        workspace
            .observe(
                true,
                Some("identity".to_owned()),
                Some(WorkspaceCheckout::Branch("feature".to_owned())),
                Some("abc123".to_owned()),
                Some(true),
                Some(false),
            )
            .expect("the observation transitions");
    }

    #[test]
    fn select_reusable_workspace_picks_the_lowest_id() {
        let mut first = Workspace::new(WorkspaceId::new(2), registration("/workspaces/kanban.one"));
        observed_clean(&mut first);
        let mut second =
            Workspace::new(WorkspaceId::new(5), registration("/workspaces/kanban.two"));
        observed_clean(&mut second);

        let selected = select_reusable_workspace(&[second, first]).expect("one is reusable");

        assert_eq!(selected.value(), 2);
    }

    #[test]
    fn select_reusable_workspace_skips_dirty_workspaces() {
        let mut reusable = Workspace::new(
            WorkspaceId::new(3),
            registration("/workspaces/kanban.clean"),
        );
        observed_clean(&mut reusable);
        let mut dirty = Workspace::new(
            WorkspaceId::new(1),
            registration("/workspaces/kanban.dirty"),
        );
        dirty
            .observe(
                true,
                Some("identity".to_owned()),
                Some(WorkspaceCheckout::Branch("feature".to_owned())),
                Some("abc123".to_owned()),
                Some(false),
                Some(false),
            )
            .expect("the observation transitions");

        let selected = select_reusable_workspace(&[dirty, reusable]).expect("one is reusable");

        assert_eq!(selected.value(), 3);
    }

    #[test]
    fn select_reusable_workspace_returns_none_when_every_workspace_refuses() {
        let unobserved = Workspace::new(
            WorkspaceId::new(1),
            registration("/workspaces/kanban.blind"),
        );

        assert!(select_reusable_workspace(&[unobserved]).is_none());
    }

    #[test]
    fn execution_branch_and_workspace_path_follow_the_ticket_number() {
        assert_eq!(execution_branch(46), "kan-t46");
        assert_eq!(execution_workspace_path(46), "/workspaces/kanban.kan-t46");
    }
}
