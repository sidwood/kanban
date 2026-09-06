//! Runs: one execution attempt of a Ticket in a Lane by an implementer
//! or reviewer (CONTEXT.md, DR-EP-04). A run belongs to exactly one
//! claimed Dispatch Request and freezes two profile snapshots — the
//! requested profile the assignment names and the effective profile
//! actually used after the fallback policy — so a later catalogue
//! change never rewrites what ran (DR-EP-05). Settlement vocabulary
//! arrives with authoritative submissions; this module owns the mint
//! and its snapshots alone.

use std::fmt;

use crate::dispatch::{DispatchRequest, DispatchRequestId, DispatchStatus};
use crate::profile::{ExecutionProfile, ProfileName};
use crate::project::ProjectId;
use crate::ticket::TicketId;

/// The identity of one run. Assigned once by storage and immutable
/// afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(u64);

impl RunId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a run rule was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// A blank text field. The value names the field.
    Blank(&'static str),
    /// A run belongs to exactly one claimed Dispatch Request; a queued
    /// request has no run to mint.
    UnclaimedRequest,
    /// The requested profile names no entry, or no fallback chain from
    /// it reaches an active entry, so no effective profile exists to
    /// run.
    UnresolvedEffective { name: String },
    /// The fallback chain returns to an entry it already walked.
    FallbackLoop { name: String },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blank(field) => write!(f, "a run snapshot {field} cannot be blank"),
            Self::UnclaimedRequest => {
                write!(f, "a run belongs to exactly one claimed Dispatch Request")
            }
            Self::UnresolvedEffective { name } => write!(
                f,
                "the profile `{name}` resolves to no effective profile to run"
            ),
            Self::FallbackLoop { name } => {
                write!(f, "the fallback chain from `{name}` loops")
            }
        }
    }
}

impl std::error::Error for RunError {}

/// The closed run status vocabulary. A run mints executing and stays
/// executing for this slice; settlement vocabulary arrives with the
/// authoritative submissions that own it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunStatus {
    /// Minted from a claimed request and occupying its execution.
    Executing,
}

impl RunStatus {
    /// The stored and wire name of this status.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Executing => "executing",
        }
    }

    /// The status a stored row names, or `None` outside the
    /// vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        match stored {
            "executing" => Some(Self::Executing),
            _ => None,
        }
    }
}

/// The frozen values of one profile a run records: the entry's name
/// and its five decisions as they stood at the mint, so a later
/// catalogue change cannot rewrite what ran (DR-EP-04, DR-EP-05).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSnapshot {
    name: String,
    harness: String,
    model: String,
    effort: String,
    usage_pool: String,
}

impl ProfileSnapshot {
    /// Assemble a snapshot, refusing any blank decision.
    pub fn new(
        name: impl Into<String>,
        harness: impl Into<String>,
        model: impl Into<String>,
        effort: impl Into<String>,
        usage_pool: impl Into<String>,
    ) -> Result<Self, RunError> {
        let fields = [
            ("name", name.into()),
            ("harness", harness.into()),
            ("model", model.into()),
            ("effort", effort.into()),
            ("usage pool", usage_pool.into()),
        ];
        for (field, value) in &fields {
            if value.trim().is_empty() {
                return Err(RunError::Blank(match *field {
                    "name" => "name",
                    "harness" => "harness",
                    "model" => "model",
                    "effort" => "effort",
                    _ => "usage pool",
                }));
            }
        }
        let [name, harness, model, effort, usage_pool] = fields.map(|(_, value)| value);
        Ok(Self {
            name,
            harness,
            model,
            effort,
            usage_pool,
        })
    }

    /// Rehydrate a stored snapshot exactly as it was recorded.
    pub fn restore(
        name: String,
        harness: String,
        model: String,
        effort: String,
        usage_pool: String,
    ) -> Self {
        Self {
            name,
            harness,
            model,
            effort,
            usage_pool,
        }
    }

    /// The snapshotted entry name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The snapshotted harness family.
    pub fn harness(&self) -> &str {
        &self.harness
    }

    /// The snapshotted model family.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The snapshotted effort.
    pub fn effort(&self) -> &str {
        &self.effort
    }

    /// The snapshotted usage pool.
    pub fn usage_pool(&self) -> &str {
        &self.usage_pool
    }
}

/// Resolve the effective profile a run of `requested` would use: walk
/// the fallback policy from the requested entry, skipping entries the
/// catalogue no longer assigns, until an active entry answers. The
/// returned path names every entry the walk touched, requested first,
/// so the snapshot records the fallback transitions themselves.
pub fn resolve_effective<'a>(
    entries: &'a [ExecutionProfile],
    requested: &ProfileName,
) -> Result<(&'a ExecutionProfile, Vec<ProfileName>), RunError> {
    let mut path = vec![requested.clone()];
    for _ in 0..=entries.len() {
        let cursor = path
            .last()
            .expect("the path starts with the requested name")
            .clone();
        let entry = entries.iter().find(|entry| entry.name() == &cursor).ok_or(
            RunError::UnresolvedEffective {
                name: cursor.as_str().to_owned(),
            },
        )?;
        if !entry.is_retired() {
            return Ok((entry, path));
        }
        let Some(next) = entry.fallback().cloned() else {
            return Err(RunError::UnresolvedEffective {
                name: cursor.as_str().to_owned(),
            });
        };
        if path.contains(&next) {
            return Err(RunError::FallbackLoop {
                name: requested.as_str().to_owned(),
            });
        }
        path.push(next);
    }
    Err(RunError::FallbackLoop {
        name: requested.as_str().to_owned(),
    })
}

/// One run, frozen at its mint (CONTEXT.md, DR-EP-04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    id: RunId,
    dispatch_request: DispatchRequestId,
    project: ProjectId,
    ticket: TicketId,
    status: RunStatus,
    requested: ProfileSnapshot,
    effective: ProfileSnapshot,
    fallback: bool,
    fallback_path: Vec<String>,
    created_at: u64,
    version: u64,
}

impl Run {
    /// Mint the run of one claimed Dispatch Request. The snapshots and
    /// the fallback path arrive from the effective resolution; the
    /// fallback flag itself is derived from the snapshot names, never
    /// trusted from the caller.
    pub fn acknowledge(
        id: RunId,
        request: &DispatchRequest,
        requested: ProfileSnapshot,
        effective: ProfileSnapshot,
        fallback_path: Vec<String>,
        created_at: u64,
    ) -> Result<Self, RunError> {
        if request.status() != DispatchStatus::Claimed {
            return Err(RunError::UnclaimedRequest);
        }
        let fallback = effective.name() != requested.name();
        Ok(Self {
            id,
            dispatch_request: request.id(),
            project: request.project(),
            ticket: request.ticket(),
            status: RunStatus::Executing,
            requested,
            effective,
            fallback,
            fallback_path: if fallback { fallback_path } else { Vec::new() },
            created_at,
            version: 1,
        })
    }

    /// Rehydrate a stored run exactly as it was recorded.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        id: RunId,
        dispatch_request: DispatchRequestId,
        project: ProjectId,
        ticket: TicketId,
        status: RunStatus,
        requested: ProfileSnapshot,
        effective: ProfileSnapshot,
        fallback: bool,
        fallback_path: Vec<String>,
        created_at: u64,
        version: u64,
    ) -> Self {
        Self {
            id,
            dispatch_request,
            project,
            ticket,
            status,
            requested,
            effective,
            fallback,
            fallback_path,
            created_at,
            version,
        }
    }

    /// The immutable identity.
    pub fn id(&self) -> RunId {
        self.id
    }

    /// The claimed Dispatch Request this run executes.
    pub fn dispatch_request(&self) -> DispatchRequestId {
        self.dispatch_request
    }

    /// The Project the run belongs to.
    pub fn project(&self) -> ProjectId {
        self.project
    }

    /// The Ticket the run executes.
    pub fn ticket(&self) -> TicketId {
        self.ticket
    }

    /// The run's status.
    pub fn status(&self) -> RunStatus {
        self.status
    }

    /// The requested profile snapshot.
    pub fn requested(&self) -> &ProfileSnapshot {
        &self.requested
    }

    /// The effective profile snapshot.
    pub fn effective(&self) -> &ProfileSnapshot {
        &self.effective
    }

    /// Whether the effective profile is not the requested one.
    pub fn fell_back(&self) -> bool {
        self.fallback
    }

    /// The names the fallback walk touched, requested first; empty when
    /// no fallback happened.
    pub fn fallback_path(&self) -> &[String] {
        &self.fallback_path
    }

    /// When the run minted, as unix seconds.
    pub fn created_at(&self) -> u64 {
        self.created_at
    }

    /// The aggregate version, for optimistic mutation checks.
    pub fn version(&self) -> u64 {
        self.version
    }
}

#[cfg(test)]
mod run_rules {
    use super::{ProfileSnapshot, Run, RunError, RunId, RunStatus, resolve_effective};
    use crate::dispatch::{DispatchRequest, DispatchRequestId, DispatchStatus};
    use crate::profile::{ExecutionProfile, ProfileDefinition, ProfileName, ProfileState};
    use crate::project::ProjectId;
    use crate::ticket::{Priority, TicketId};

    fn named(raw: &str) -> ProfileName {
        ProfileName::new(raw).expect("a non-blank name is accepted")
    }

    fn definition(fallback: Option<&str>, model: &str) -> ProfileDefinition {
        ProfileDefinition::new(
            "claude-code",
            model,
            "high",
            "operator",
            fallback.map(named),
        )
        .expect("the definition validates")
    }

    /// One catalogue entry rehydrated straight from stored columns, so a
    /// test can stage a retired entry or a retired fallback hop the
    /// collection rules would refuse defining afresh.
    fn stored(name: &str, fallback: Option<&str>, model: &str, retired: bool) -> ExecutionProfile {
        ExecutionProfile::restore(
            named(name),
            definition(fallback, model),
            if retired {
                ProfileState::Retired
            } else {
                ProfileState::Active
            },
            1,
        )
    }

    fn claimed_request() -> DispatchRequest {
        DispatchRequest::restore(
            DispatchRequestId::new(4),
            ProjectId::new(1),
            TicketId::new(9),
            DispatchStatus::Claimed,
            Priority::Normal,
            true,
            "claude-code".to_owned(),
            "opus".to_owned(),
            "operator".to_owned(),
            10,
            2,
        )
    }

    fn snapshot(name: &str, model: &str) -> ProfileSnapshot {
        ProfileSnapshot::new(name, "claude-code", model, "high", "operator")
            .expect("a complete snapshot is accepted")
    }

    fn acknowledge(
        requested: ProfileSnapshot,
        effective: ProfileSnapshot,
        fallback_path: Vec<String>,
    ) -> Result<Run, RunError> {
        Run::acknowledge(
            RunId::new(3),
            &claimed_request(),
            requested,
            effective,
            fallback_path,
            20,
        )
    }

    #[test]
    fn a_run_snapshots_the_requested_profile_unchanged() {
        let run = acknowledge(
            snapshot("standard", "opus"),
            snapshot("standard", "opus"),
            vec![],
        )
        .expect("a claimed request mints its run");

        assert_eq!(run.id().value(), 3);
        assert_eq!(run.dispatch_request().value(), 4);
        assert_eq!(run.ticket(), TicketId::new(9));
        assert_eq!(run.project(), ProjectId::new(1));
        assert_eq!(run.status(), RunStatus::Executing);
        assert_eq!(run.requested().name(), "standard");
        assert_eq!(run.requested().model(), "opus");
        assert_eq!(run.effective().name(), "standard");
        assert!(!run.fell_back());
        assert!(run.fallback_path().is_empty());
        assert_eq!(run.created_at(), 20);
        assert_eq!(run.version(), 1);
    }

    #[test]
    fn an_active_requested_profile_resolves_to_itself() {
        let entries = vec![stored("standard", None, "opus", false)];

        let (effective, path) =
            resolve_effective(&entries, &named("standard")).expect("the entry resolves");

        assert_eq!(effective.name().as_str(), "standard");
        assert_eq!(path, vec![named("standard")]);
    }

    #[test]
    fn a_retired_requested_profile_runs_its_fallback() {
        let entries = vec![
            stored("nightly", Some("standard"), "opus", true),
            stored("standard", None, "sonnet", false),
        ];

        let (effective, path) =
            resolve_effective(&entries, &named("nightly")).expect("the fallback resolves");

        assert_eq!(effective.name().as_str(), "standard");
        assert_eq!(effective.model(), "sonnet");
        assert_eq!(path, vec![named("nightly"), named("standard")]);

        let run = acknowledge(
            snapshot("nightly", "opus"),
            snapshot("standard", "sonnet"),
            vec!["nightly".to_owned(), "standard".to_owned()],
        )
        .expect("the run mints");
        assert!(run.fell_back());
        assert_eq!(
            run.fallback_path(),
            &["nightly".to_owned(), "standard".to_owned()]
        );
    }

    #[test]
    fn the_fallback_walk_crosses_every_retired_hop() {
        // A store restored from an older catalogue can hold a retired
        // entry whose fallback is itself retired: the walk keeps going
        // until an active entry answers.
        let entries = vec![
            stored("alpha", Some("beta"), "opus", true),
            stored("beta", Some("gamma"), "sonnet", true),
            stored("gamma", None, "haiku", false),
        ];

        let (effective, path) =
            resolve_effective(&entries, &named("alpha")).expect("the chain resolves");

        assert_eq!(effective.name().as_str(), "gamma");
        assert_eq!(
            path,
            vec![named("alpha"), named("beta"), named("gamma")],
            "the snapshot records every transition the walk took"
        );
    }

    #[test]
    fn a_requested_profile_with_no_resolvable_effective_is_refused() {
        let retired_bare = vec![stored("nightly", None, "opus", true)];
        assert_eq!(
            resolve_effective(&retired_bare, &named("nightly")),
            Err(RunError::UnresolvedEffective {
                name: "nightly".to_owned()
            })
        );

        let unknown = vec![stored("standard", None, "opus", false)];
        assert_eq!(
            resolve_effective(&unknown, &named("ghost")),
            Err(RunError::UnresolvedEffective {
                name: "ghost".to_owned()
            })
        );
    }

    #[test]
    fn a_fallback_chain_that_loops_is_refused() {
        let entries = vec![
            stored("alpha", Some("beta"), "opus", true),
            stored("beta", Some("alpha"), "sonnet", true),
        ];

        assert_eq!(
            resolve_effective(&entries, &named("alpha")),
            Err(RunError::FallbackLoop {
                name: "alpha".to_owned()
            })
        );
    }

    #[test]
    fn a_queued_request_mints_no_run() {
        // A request that never won its claim: restored straight from
        // the queue, before any claim applied.
        let queued = DispatchRequest::restore(
            DispatchRequestId::new(4),
            ProjectId::new(1),
            TicketId::new(9),
            DispatchStatus::Queued,
            Priority::Normal,
            true,
            "claude-code".to_owned(),
            "opus".to_owned(),
            "operator".to_owned(),
            10,
            1,
        );

        let outcome = Run::acknowledge(
            RunId::new(3),
            &queued,
            snapshot("standard", "opus"),
            snapshot("standard", "opus"),
            vec![],
            20,
        );

        assert_eq!(outcome, Err(RunError::UnclaimedRequest));
    }

    #[test]
    fn a_snapshot_without_a_fallback_forgets_any_path() {
        // The fallback flag is derived, never trusted: an effective
        // profile under the requested name is no fallback, whatever
        // path the caller carried in.
        let run = acknowledge(
            snapshot("standard", "opus"),
            snapshot("standard", "opus"),
            vec!["standard".to_owned()],
        )
        .expect("the run mints");

        assert!(!run.fell_back());
        assert!(run.fallback_path().is_empty());
    }

    #[test]
    fn every_blank_snapshot_decision_is_refused() {
        for blank in ["", " "] {
            for field in ["name", "harness", "model", "effort", "usage pool"] {
                let error = ProfileSnapshot::new(
                    if field == "name" { blank } else { "standard" },
                    if field == "harness" {
                        blank
                    } else {
                        "claude-code"
                    },
                    if field == "model" { blank } else { "opus" },
                    if field == "effort" { blank } else { "high" },
                    if field == "usage pool" {
                        blank
                    } else {
                        "operator"
                    },
                )
                .expect_err("a blank decision is refused");
                assert_eq!(error, RunError::Blank(field), "{field} must be named");
            }
        }
    }

    #[test]
    fn restore_rehydrates_every_recorded_fact() {
        let run = Run::restore(
            RunId::new(5),
            DispatchRequestId::new(4),
            ProjectId::new(1),
            TicketId::new(9),
            RunStatus::Executing,
            snapshot("nightly", "opus"),
            snapshot("standard", "sonnet"),
            true,
            vec!["nightly".to_owned(), "standard".to_owned()],
            20,
            1,
        );

        assert_eq!(run.requested().name(), "nightly");
        assert_eq!(run.effective().name(), "standard");
        assert!(run.fell_back());
        assert_eq!(run.status(), RunStatus::Executing);
    }
}
