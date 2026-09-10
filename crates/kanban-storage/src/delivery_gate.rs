//! The authorisation a resume delivery holds while it prompts.
//!
//! A resume delivery reads current custody, then speaks to Herdr over
//! a socket. The database write that read custody cannot be held
//! across that socket call, so without a second boundary a
//! cancellation or an archival could commit in the gap and the prompt
//! would go out under authority the operator had already withdrawn
//! (KAN-T142-AC2).
//!
//! This gate is that boundary. A delivery holds it from the custody
//! check through the prompt to the acknowledgement, and every
//! mutation acquires it before opening its own write span, so the two
//! are totally ordered: a mutation that wins commits before the
//! custody check reads it, and a delivery that wins prompts under
//! state no mutation has changed. It is not a database lock — the
//! connection is released before the socket call, and readers are
//! never blocked.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::{Mutex, MutexGuard};

/// Mutual exclusion between one resume delivery and every mutation
/// that could invalidate the custody it was authorised under.
pub(crate) struct DeliveryGate {
    held: Mutex<()>,
}

impl DeliveryGate {
    /// Hold the gate. A delivery holds it across its prompt, so a
    /// caller must never run a mutation inside the window it opens:
    /// the wait is bounded by the session client's I/O deadline, and
    /// a mutation raised inside it would wait on a gate its own
    /// thread already holds.
    pub(crate) fn enter(&self) -> MutexGuard<'_, ()> {
        self.held.lock()
    }
}

/// The gate belonging to the database file at `path`.
///
/// The product opens one authoritative database (ADR-0002), but a
/// process may hold more than one handle on that file, and a delivery
/// must exclude every writer that reaches it — so the gate belongs to
/// the file rather than to the handle.
pub(crate) fn gate_for(path: &Path) -> Arc<DeliveryGate> {
    static GATES: OnceLock<Mutex<HashMap<PathBuf, Arc<DeliveryGate>>>> = OnceLock::new();
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    Arc::clone(
        GATES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .entry(key)
            .or_insert_with(|| {
                Arc::new(DeliveryGate {
                    held: Mutex::new(()),
                })
            }),
    )
}

#[cfg(test)]
mod delivery_authorisation {
    use super::gate_for;

    #[test]
    fn one_database_file_has_one_gate() {
        let dir = tempfile::tempdir().expect("a scratch directory is available");
        let path = dir.path().join("kanban.sqlite");
        std::fs::write(&path, b"").expect("the file exists to be canonicalised");

        let first = gate_for(&path);
        let second = gate_for(&path);
        let other = gate_for(&dir.path().join("other.sqlite"));

        assert!(std::sync::Arc::ptr_eq(&first, &second));
        assert!(!std::sync::Arc::ptr_eq(&first, &other));
    }

    #[test]
    fn a_held_gate_excludes_another_thread_until_it_is_released() {
        let dir = tempfile::tempdir().expect("a scratch directory is available");
        let path = dir.path().join("kanban.sqlite");
        let gate = gate_for(&path);
        let (entered, waiting) = std::sync::mpsc::channel();

        std::thread::scope(|scope| {
            let held = gate.enter();
            let contender = gate_for(&path);
            scope.spawn(move || {
                let _held = contender.enter();
                entered.send(()).expect("the second holder reports back");
            });
            assert!(
                waiting
                    .recv_timeout(std::time::Duration::from_millis(250))
                    .is_err(),
                "a held gate admits no second holder"
            );
            drop(held);
            waiting
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("the released gate admits the waiting holder");
        });
    }
}
