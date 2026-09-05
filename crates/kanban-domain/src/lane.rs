//! The Lane entity: a durable execution slot, distinct from a
//! Workspace (CONTEXT.md, DR-LW-01). A Lane holds at most one active
//! Ticket (DR-LW-02); a non-seed Workspace belongs to at most one
//! active Lane (DR-LW-03); the Seed Workspace is the Project's
//! landing area and never an execution Lane (DR-LW-07). Occupancy is
//! explicit: assignment claims a slot, release frees it, and nothing
//! here mutates a Workspace's observed git facts — the application
//! layer mirrors an applied claim onto the Workspace record inside
//! the same write.

use std::fmt;

use crate::project::ProjectId;
use crate::ticket::TicketId;
use crate::workspace::{Workspace, WorkspaceId};

/// The identity of one Lane. Assigned once by storage and immutable
/// afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LaneId(u64);

impl LaneId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for LaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a Lane assignment or release was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneError {
    /// The Lane already holds another active Ticket (DR-LW-02).
    LaneHoldsTicket { held: TicketId },
    /// The Lane holds no Ticket to release.
    LaneHoldsNoTicket,
    /// The Lane already holds another Workspace.
    LaneHoldsWorkspace { held: WorkspaceId },
    /// The Lane holds no Workspace to release.
    LaneHoldsNoWorkspace,
    /// The Workspace is the Project's Seed, which never executes in
    /// a Lane (DR-LW-07).
    SeedWorkspace { path: String },
    /// A retired Workspace cannot host execution.
    RetiredWorkspace { path: String },
}

impl fmt::Display for LaneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LaneHoldsTicket { held } => {
                write!(f, "the Lane already holds Ticket {held}")
            }
            Self::LaneHoldsNoTicket => write!(f, "the Lane holds no Ticket to release"),
            Self::LaneHoldsWorkspace { held } => {
                write!(f, "the Lane already holds Workspace {held}")
            }
            Self::LaneHoldsNoWorkspace => write!(f, "the Lane holds no Workspace to release"),
            Self::SeedWorkspace { path } => write!(
                f,
                "the Seed Workspace can never be an execution Lane: {path}"
            ),
            Self::RetiredWorkspace { path } => {
                write!(f, "a retired Workspace cannot host a Lane: {path}")
            }
        }
    }
}

impl std::error::Error for LaneError {}

/// One Lane aggregate: a durable execution slot of one Project, at
/// most one held Ticket, at most one held Workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lane {
    id: LaneId,
    project_id: ProjectId,
    workspace_id: Option<WorkspaceId>,
    ticket_id: Option<TicketId>,
    version: u64,
}

impl Lane {
    /// A freshly created Lane at version 1 holding nothing.
    pub fn new(id: LaneId, project_id: ProjectId) -> Self {
        Self {
            id,
            project_id,
            workspace_id: None,
            ticket_id: None,
            version: 1,
        }
    }

    /// Rehydrate a stored Lane exactly as recorded.
    pub fn restore(
        id: LaneId,
        project_id: ProjectId,
        workspace_id: Option<WorkspaceId>,
        ticket_id: Option<TicketId>,
        version: u64,
    ) -> Self {
        Self {
            id,
            project_id,
            workspace_id,
            ticket_id,
            version,
        }
    }

    /// The immutable identity.
    pub fn id(&self) -> LaneId {
        self.id
    }

    /// The owning Project.
    pub fn project(&self) -> ProjectId {
        self.project_id
    }

    /// The Workspace this Lane runs in, when claimed (DR-LW-03).
    pub fn workspace_id(&self) -> Option<WorkspaceId> {
        self.workspace_id
    }

    /// The Ticket this Lane holds, when one is active (DR-LW-02).
    pub fn ticket_id(&self) -> Option<TicketId> {
        self.ticket_id
    }

    /// The aggregate version for optimistic checks.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Claim `workspace` as this Lane's execution Workspace. The Seed
    /// is refused outright (DR-LW-07), a retired record cannot host
    /// execution, and a Lane runs in at most one Workspace. Claiming
    /// the Workspace already held is a no-op.
    pub fn assign_workspace(&mut self, workspace: &Workspace) -> Result<(), LaneError> {
        if workspace.registration().is_seed() {
            return Err(LaneError::SeedWorkspace {
                path: workspace.registration().path().to_owned(),
            });
        }
        if workspace.is_retired() {
            return Err(LaneError::RetiredWorkspace {
                path: workspace.registration().path().to_owned(),
            });
        }
        match self.workspace_id {
            Some(held) if held != workspace.id() => Err(LaneError::LaneHoldsWorkspace { held }),
            Some(_) => Ok(()),
            None => {
                self.workspace_id = Some(workspace.id());
                self.version += 1;
                Ok(())
            }
        }
    }

    /// Free this Lane's Workspace claim. Refused when nothing is
    /// held, so an explicit release never silently absorbs a mistake.
    pub fn release_workspace(&mut self) -> Result<(), LaneError> {
        if self.workspace_id.is_none() {
            return Err(LaneError::LaneHoldsNoWorkspace);
        }
        self.workspace_id = None;
        self.version += 1;
        Ok(())
    }

    /// Hold `ticket` as this Lane's active Ticket. A second, different
    /// Ticket is refused (DR-LW-02); holding the Ticket already held
    /// is a no-op.
    pub fn assign_ticket(&mut self, ticket: TicketId) -> Result<(), LaneError> {
        match self.ticket_id {
            Some(held) if held != ticket => Err(LaneError::LaneHoldsTicket { held }),
            Some(_) => Ok(()),
            None => {
                self.ticket_id = Some(ticket);
                self.version += 1;
                Ok(())
            }
        }
    }

    /// Free this Lane's Ticket slot. Refused when no Ticket is held.
    pub fn release_ticket(&mut self) -> Result<(), LaneError> {
        if self.ticket_id.is_none() {
            return Err(LaneError::LaneHoldsNoTicket);
        }
        self.ticket_id = None;
        self.version += 1;
        Ok(())
    }
}

/// The Lane, if any, whose claim on a Workspace blocks `lane` from
/// claiming it (DR-LW-03): a Workspace belongs to at most one active
/// Lane, so any different current holder refuses the assignment.
pub fn workspace_lane_conflict(holder: Option<LaneId>, lane: LaneId) -> Option<LaneId> {
    match holder {
        Some(current) if current != lane => Some(current),
        _ => None,
    }
}

#[cfg(test)]
mod lane_rules {
    use crate::project::ProjectId;
    use crate::workspace::{Workspace, WorkspaceId, WorkspaceRegistration};

    use super::{Lane, LaneError, LaneId, workspace_lane_conflict};

    fn registration(path: &str, is_seed: bool) -> WorkspaceRegistration {
        WorkspaceRegistration::new(ProjectId::new(1), path, is_seed)
            .expect("the fixture registration validates")
    }

    fn workspace(id: u64, path: &str, is_seed: bool) -> Workspace {
        Workspace::new(WorkspaceId::new(id), registration(path, is_seed))
    }

    fn lane(id: u64) -> Lane {
        Lane::new(LaneId::new(id), ProjectId::new(1))
    }

    #[test]
    fn a_fresh_lane_is_an_empty_slot_of_its_project() {
        let lane = lane(1);

        assert_eq!(lane.project(), ProjectId::new(1));
        assert_eq!(lane.workspace_id(), None);
        assert_eq!(lane.ticket_id(), None);
        assert_eq!(lane.version(), 1);
    }

    #[test]
    fn a_lane_holds_at_most_one_active_ticket() {
        let mut lane = lane(1);

        lane.assign_ticket(crate::ticket::TicketId::new(5))
            .expect("the first Ticket holds the slot");

        assert_eq!(
            lane.assign_ticket(crate::ticket::TicketId::new(6)),
            Err(LaneError::LaneHoldsTicket {
                held: crate::ticket::TicketId::new(5)
            }),
            "a second Ticket must be refused while one is active"
        );
        assert_eq!(lane.ticket_id(), Some(crate::ticket::TicketId::new(5)));
        assert_eq!(lane.version(), 2, "the refusal changed nothing");
    }

    #[test]
    fn releasing_a_ticket_frees_the_slot_for_the_next_one() {
        let mut lane = lane(1);
        lane.assign_ticket(crate::ticket::TicketId::new(5))
            .expect("the first Ticket holds the slot");

        lane.release_ticket().expect("a held Ticket releases");

        lane.assign_ticket(crate::ticket::TicketId::new(6))
            .expect("the freed slot holds the next Ticket");
        assert_eq!(lane.ticket_id(), Some(crate::ticket::TicketId::new(6)));
    }

    #[test]
    fn releasing_an_empty_ticket_slot_is_refused() {
        let mut lane = lane(1);

        assert_eq!(lane.release_ticket(), Err(LaneError::LaneHoldsNoTicket));
        assert_eq!(lane.version(), 1, "the refusal changed nothing");
    }

    #[test]
    fn holding_the_same_ticket_again_changes_nothing() {
        let mut lane = lane(1);
        lane.assign_ticket(crate::ticket::TicketId::new(5))
            .expect("the Ticket holds the slot");

        lane.assign_ticket(crate::ticket::TicketId::new(5))
            .expect("holding the Ticket already held is a no-op");

        assert_eq!(lane.version(), 2, "a no-op claim costs no version");
    }

    #[test]
    fn the_seed_workspace_is_never_an_execution_lane() {
        let mut lane = lane(1);
        let seed = workspace(9, "/workspaces/kanban.seed", true);

        assert_eq!(
            lane.assign_workspace(&seed),
            Err(LaneError::SeedWorkspace {
                path: "/workspaces/kanban.seed".to_owned()
            }),
            "the Seed is the landing area, never an execution Lane"
        );
        assert_eq!(lane.workspace_id(), None);
        assert_eq!(lane.version(), 1, "the refusal changed nothing");
    }

    #[test]
    fn a_retired_workspace_cannot_host_a_lane() {
        let mut lane = lane(1);
        let mut retired = workspace(4, "/workspaces/kanban.old", false);
        retired.retire().expect("the Workspace retires");

        assert_eq!(
            lane.assign_workspace(&retired),
            Err(LaneError::RetiredWorkspace {
                path: "/workspaces/kanban.old".to_owned()
            })
        );
        assert_eq!(lane.workspace_id(), None);
    }

    #[test]
    fn a_lane_runs_in_at_most_one_workspace() {
        let mut lane = lane(1);

        lane.assign_workspace(&workspace(2, "/workspaces/kanban.feature", false))
            .expect("the first Workspace is claimed");

        assert_eq!(
            lane.assign_workspace(&workspace(3, "/workspaces/kanban.other", false)),
            Err(LaneError::LaneHoldsWorkspace {
                held: WorkspaceId::new(2)
            })
        );
        assert_eq!(lane.workspace_id(), Some(WorkspaceId::new(2)));
        assert_eq!(lane.version(), 2, "the refusal changed nothing");
    }

    #[test]
    fn releasing_a_workspace_frees_the_claim_for_the_next_one() {
        let mut lane = lane(1);
        lane.assign_workspace(&workspace(2, "/workspaces/kanban.feature", false))
            .expect("the first Workspace is claimed");

        lane.release_workspace().expect("a held Workspace releases");

        lane.assign_workspace(&workspace(3, "/workspaces/kanban.other", false))
            .expect("the freed claim holds the next Workspace");
        assert_eq!(lane.workspace_id(), Some(WorkspaceId::new(3)));
    }

    #[test]
    fn releasing_an_empty_workspace_claim_is_refused() {
        let mut lane = lane(1);

        assert_eq!(
            lane.release_workspace(),
            Err(LaneError::LaneHoldsNoWorkspace)
        );
        assert_eq!(lane.version(), 1, "the refusal changed nothing");
    }

    #[test]
    fn claiming_the_workspace_already_held_changes_nothing() {
        let mut lane = lane(1);
        lane.assign_workspace(&workspace(2, "/workspaces/kanban.feature", false))
            .expect("the Workspace is claimed");

        lane.assign_workspace(&workspace(2, "/workspaces/kanban.feature", false))
            .expect("claiming the Workspace already held is a no-op");

        assert_eq!(lane.version(), 2, "a no-op claim costs no version");
    }

    #[test]
    fn a_workspace_belongs_to_at_most_one_active_lane() {
        let lane_one = LaneId::new(1);
        let lane_two = LaneId::new(2);

        assert_eq!(
            workspace_lane_conflict(Some(lane_two), lane_one),
            Some(lane_two),
            "a different Lane's claim refuses the assignment"
        );
        assert_eq!(
            workspace_lane_conflict(Some(lane_one), lane_one),
            None,
            "the Lane already holding the Workspace is no conflict"
        );
        assert_eq!(
            workspace_lane_conflict(None, lane_one),
            None,
            "an unclaimed Workspace conflicts with nothing"
        );
    }

    #[test]
    fn a_lane_records_only_assignment_facts_never_git_state() {
        // DR-LW-01: a Lane is a slot, not a Workspace. It holds
        // identities, so restoring one carries no path, branch, or
        // health — only its claims and version.
        let mut lane = lane(1);
        lane.assign_workspace(&workspace(2, "/workspaces/kanban.feature", false))
            .expect("the Workspace is claimed");
        lane.assign_ticket(crate::ticket::TicketId::new(5))
            .expect("the Ticket holds the slot");

        let restored = Lane::restore(
            LaneId::new(1),
            ProjectId::new(1),
            Some(WorkspaceId::new(2)),
            Some(crate::ticket::TicketId::new(5)),
            3,
        );

        assert_eq!(lane, restored, "a Lane restores exactly its claims");
    }
}
