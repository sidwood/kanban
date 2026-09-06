//! Run commands and queries: acknowledge the run of a claimed Dispatch
//! Request with its requested and effective profile snapshots, and
//! list a Project's runs (KAN-S9-US3, DR-EP-04). The snapshots freeze
//! at the mint; a later catalogue change never rewrites them
//! (DR-EP-05). The commands land beside this port in the application
//! slice; this file owns the storage contract they share.

use kanban_domain::{DispatchRequest, DispatchRequestId, ProfileSnapshot, ProjectId, Run, RunId};
use kanban_dto::ApiError;

use crate::timeline::TimelineEnvelope;

/// The facts one mint writes: the claimed request the run executes and
/// the snapshots the effective resolution produced.
#[derive(Debug, Clone)]
pub struct RunMint {
    /// The claimed Dispatch Request the run executes. Storage calls
    /// the domain mint, which refuses an unclaimed request.
    pub request: DispatchRequest,
    /// The requested profile snapshot: what the assignment named.
    pub requested: ProfileSnapshot,
    /// The effective profile snapshot: what actually runs.
    pub effective: ProfileSnapshot,
    /// The names the fallback walk touched, requested first.
    pub fallback_path: Vec<String>,
    /// When the run mints, as unix seconds.
    pub created_at: u64,
}

/// The storage port Run operations call through. `mint` lands the run
/// row and the timeline envelope inside one write, and the partial
/// unique index keeps one executing run per claimed request.
pub trait RunStore: Send + Sync {
    /// Insert a fresh executing run. Storage assigns the identity and
    /// asks `envelope` for the timeline row that identity belongs in.
    /// A request already holding an executing run is refused.
    fn mint(
        &self,
        draft: &RunMint,
        envelope: &dyn Fn(RunId) -> TimelineEnvelope,
    ) -> Result<Run, ApiError>;
    /// Every run of one Project, oldest first.
    fn list_for_project(&self, project: ProjectId) -> Result<Vec<Run>, ApiError>;
    /// The executing run of one claimed Dispatch Request, if it has
    /// one.
    fn executing_for_request(&self, request: DispatchRequestId) -> Result<Option<Run>, ApiError>;
}
