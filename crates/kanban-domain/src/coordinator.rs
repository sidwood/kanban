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

/// Select the first reusable Workspace for `ticket_number`, in stable
/// id order. A reusable Workspace is clean, unassigned, free of unique
/// unlanded commits (DR-LW-06), not the Project Seed (DR-LW-07), and
/// registered at the Ticket's execution path.
pub fn select_reusable_workspace(
    workspaces: &[Workspace],
    ticket_number: u64,
) -> Option<WorkspaceId> {
    let execution_path = execution_workspace_path(ticket_number);
    workspaces
        .iter()
        .filter(|workspace| {
            !workspace.registration().is_seed()
                && workspace.registration().path() == execution_path
                && workspace.reuse_evaluation().reusable()
        })
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
        let mut first = Workspace::new(
            WorkspaceId::new(2),
            registration("/workspaces/kanban.kan-t1"),
        );
        observed_clean(&mut first);
        let mut second = Workspace::new(
            WorkspaceId::new(5),
            registration("/workspaces/kanban.kan-t1"),
        );
        observed_clean(&mut second);

        let selected = select_reusable_workspace(&[second, first], 1).expect("one is reusable");

        assert_eq!(selected.value(), 2);
    }

    #[test]
    fn select_reusable_workspace_skips_dirty_workspaces() {
        let mut reusable = Workspace::new(
            WorkspaceId::new(3),
            registration("/workspaces/kanban.kan-t1"),
        );
        observed_clean(&mut reusable);
        let mut dirty = Workspace::new(
            WorkspaceId::new(1),
            registration("/workspaces/kanban.kan-t1"),
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

        let selected = select_reusable_workspace(&[dirty, reusable], 1).expect("one is reusable");

        assert_eq!(selected.value(), 3);
    }

    #[test]
    fn select_reusable_workspace_returns_none_when_every_workspace_refuses() {
        let unobserved = Workspace::new(
            WorkspaceId::new(1),
            registration("/workspaces/kanban.kan-t1"),
        );

        assert!(select_reusable_workspace(&[unobserved], 1).is_none());
    }

    #[test]
    fn select_reusable_workspace_skips_seed_workspaces() {
        let mut seed = Workspace::new(
            WorkspaceId::new(1),
            WorkspaceRegistration::new(ProjectId::new(1), "/workspaces/kanban.seed", true)
                .expect("the registration validates"),
        );
        observed_clean(&mut seed);
        let mut reusable = Workspace::new(
            WorkspaceId::new(2),
            registration("/workspaces/kanban.kan-t1"),
        );
        observed_clean(&mut reusable);

        let selected = select_reusable_workspace(&[seed, reusable], 1).expect("one is reusable");

        assert_eq!(selected.value(), 2);
    }

    #[test]
    fn select_reusable_workspace_only_selects_the_execution_path() {
        let mut wrong_path = Workspace::new(
            WorkspaceId::new(1),
            registration("/workspaces/kanban.feature"),
        );
        observed_clean(&mut wrong_path);
        let mut right_path = Workspace::new(
            WorkspaceId::new(2),
            registration("/workspaces/kanban.kan-t1"),
        );
        observed_clean(&mut right_path);

        let selected =
            select_reusable_workspace(&[wrong_path, right_path], 1).expect("one is reusable");

        assert_eq!(selected.value(), 2);
    }

    #[test]
    fn execution_branch_and_workspace_path_follow_the_ticket_number() {
        assert_eq!(execution_branch(46), "kan-t46");
        assert_eq!(execution_workspace_path(46), "/workspaces/kanban.kan-t46");
    }
}
