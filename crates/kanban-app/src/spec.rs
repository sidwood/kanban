//! The storage port Spec commands call through, and the shared
//! Spec-module surface. The commands and queries that drive this port
//! arrive with the core serving slice.

use kanban_domain::{Project, ProjectId, Spec, SpecContent, SpecId, SpecNumber};
use kanban_dto::ApiError;

use crate::timeline::TimelineEnvelope;

/// The storage port Spec commands call through. Implementations land
/// the row changes and the timeline envelope unchanged inside one
/// write, so a Spec, its content versions, and the Project counter a
/// mint moves never split across a crash boundary.
pub trait SpecStore: Send + Sync {
    /// Insert a fresh Spec. `project` carries the minted Spec number
    /// and the counter move that minted it; both land in the same
    /// write as the Spec row and its opening draft version. Storage
    /// assigns the Spec's identity and asks `envelope` for the
    /// timeline row that identity belongs in.
    fn create(
        &self,
        project: &Project,
        number: SpecNumber,
        content: &SpecContent,
        envelope: &dyn Fn(SpecId) -> TimelineEnvelope,
    ) -> Result<Spec, ApiError>;
    /// Load one Spec, if it exists.
    fn find(&self, id: SpecId) -> Result<Option<Spec>, ApiError>;
    /// Load one Project's Spec by its minted number, if it exists.
    fn find_by_number(
        &self,
        project: ProjectId,
        number: SpecNumber,
    ) -> Result<Option<Spec>, ApiError>;
    /// Persist the applied Spec — its execution, Plan binding, and
    /// every content version — with the timeline envelope, all in one
    /// write.
    fn save(&self, spec: &Spec, envelope: TimelineEnvelope) -> Result<(), ApiError>;
    /// Every Spec of one Project in id order, terminal execution
    /// states included.
    fn list(&self, project: ProjectId) -> Result<Vec<Spec>, ApiError>;
}
