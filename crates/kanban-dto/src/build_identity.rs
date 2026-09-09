//! Public build provenance shared by the installed shell and service.

include!(concat!(env!("OUT_DIR"), "/build_identity.rs"));

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A non-secret diagnostic available without a data directory or toolchain.
pub fn json() -> serde_json::Value {
    serde_json::json!({
        "version": VERSION,
        "source_revision": SOURCE_REVISION,
        "source_epoch": SOURCE_EPOCH,
    })
}
