//! Per-session Herdr socket client: snapshots, subscriptions,
//! reconciliation, and polling fallback. Emits telemetry, never
//! verdicts.

mod client;
mod commands;
mod error;
mod paths;
mod protocol;
mod reconcile;
mod session;

pub use client::{PromptRequest, SessionClient, WaitRequest};
pub use commands::session_arguments;
pub use error::HerdrError;
pub use paths::{herdr_sessions_dir, session_socket_in, session_socket_path};
pub use protocol::{HerdrRequest, HerdrResponse, Snapshot};
pub use reconcile::{
    DEFAULT_RECONCILIATION_INTERVAL, Reconciler, ReconciliationPlan, SnapshotDifference,
    StateDifference, diff_state,
};
pub use session::SessionMapping;

/// Scripted Herdr session socket for tests. Available only behind the
/// `test-support` feature, which production builds never enable and
/// consumers reach only through dev-dependencies.
#[cfg(feature = "test-support")]
pub mod fixture;
