//! The activation scheduler the core service owns (KAN-S11
//! implementation decisions; DR-SA-06): one thread that fires every
//! due one-time activation through the application pass, running its
//! first pass at startup so an activation whose moment passed while
//! the core was down fires promptly after the restart. The scheduler
//! owns the clock; the pass and the domain rules never do.

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kanban_app::{ActivationPass, EventSink};

use crate::logs::{LogLevel, LogRecord, LogWriter};

/// How often the production scheduler wakes to look for a due
/// activation. One-time activations are operator-scale moments, so a
/// one-second tick keeps a due activation firing within a second of
/// its moment for the cost of one indexed scan.
const ACTIVATION_TICK: Duration = Duration::from_secs(1);

/// The activation scheduler owned by a running core.
pub(crate) struct ActivationScheduler {
    _handle: JoinHandle<()>,
}

impl ActivationScheduler {
    /// Spawn the activation loop for `pass`, announcing every Ticket
    /// it makes ready on `events` and recording outcomes in `log`.
    pub fn spawn(pass: ActivationPass, events: Arc<dyn EventSink>, log: Arc<LogWriter>) -> Self {
        Self::spawn_with_interval(pass, events, log, ACTIVATION_TICK)
    }

    /// Spawn a scheduler with a testable interval.
    pub fn spawn_with_interval(
        pass: ActivationPass,
        events: Arc<dyn EventSink>,
        log: Arc<LogWriter>,
        interval: Duration,
    ) -> Self {
        let handle = thread::spawn(move || scheduler_loop(&pass, &*events, &log, interval));
        Self { _handle: handle }
    }
}

/// Run one pass, sleep, and repeat: the first pass runs before the
/// first sleep, so an overdue activation fires promptly at startup
/// (DR-SA-06).
fn scheduler_loop(
    pass: &ActivationPass,
    events: &dyn EventSink,
    log: &LogWriter,
    interval: Duration,
) {
    loop {
        run_pass(pass, events, log);
        thread::sleep(interval);
    }
}

/// Fire every due activation at the clock's current reading, logging
/// the outcomes the operator's health view wants: what fired, what
/// stayed held back, and any refusal the pass could not apply.
fn run_pass(pass: &ActivationPass, events: &dyn EventSink, log: &LogWriter) {
    match pass.fire_due(&now_stored(), events) {
        Ok(report) if report.fired > 0 || report.skipped > 0 => {
            let _ = log.append(&LogRecord::new(
                LogLevel::Info,
                "scheduler",
                format!(
                    "activation pass fired {} and left {} waiting",
                    report.fired, report.skipped
                ),
            ));
        }
        Ok(_) => {}
        Err(error) => {
            let _ = log.append(&LogRecord::new(
                LogLevel::Error,
                "scheduler",
                format!("activation pass failed: {}", error.message),
            ));
        }
    }
}

/// The clock's current reading in the stored instant shape every
/// schedule compares through. A reading that cannot render matches no
/// stored activation, so a failing render skips the pass instead of
/// firing the wrong moment.
fn now_stored() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the system clock reads after the epoch")
        .as_nanos() as i128;
    kanban_domain::stored_instant_of(nanos).unwrap_or_default()
}

#[cfg(test)]
mod scheduler_restart {
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::now_stored;
    use crate::test_client::{Client, boot};

    /// The stored-shape instant `secs` from now.
    fn now_plus(secs: i64) -> String {
        let nanos = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the system clock reads after the epoch")
            .as_nanos() as i128)
            + i128::from(secs) * 1_000_000_000;
        kanban_domain::stored_instant_of(nanos).expect("the offset instant renders")
    }

    /// A scratch directory standing in for a Git repository the
    /// service's own observation accepts.
    fn scratch_repository(dir: &TempDir, name: &str) -> String {
        let repository = dir.path().join(name);
        std::fs::create_dir_all(repository.join(".git"))
            .expect("the scratch repository is created");
        repository
            .canonicalize()
            .expect("the repository path canonicalises")
            .to_str()
            .expect("the path is UTF-8")
            .to_owned()
    }

    /// Register one Project and create one Task Ticket, returning the
    /// client's connection and the Ticket's identity.
    fn project_and_task(core_socket: &std::path::Path, repository: &str) -> (Client, u64) {
        let mut client = Client::connect(core_socket);
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-core" },
                "code": "CORE",
                "name": "Control plane",
                "repository": repository,
                "seed_workspace": "/workspaces/kanban.seed",
                "default_branch": "main",
                "herdr_workspace": "kanban.seed",
            }),
        );
        let created = client.command(
            "ticket.create",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "task-create" },
                "project_id": 1,
                "kind": "task",
                "priority": "normal",
                "title": "Archive the old register",
                "subtype": "operational",
                "mode": "human",
                "completion": ["The register is archived."],
            }),
        );
        (
            client,
            created["id"].as_u64().expect("the identity is a number"),
        )
    }

    /// Schedule `ticket` with a one-time Schedule activating at
    /// `activation`, returning the scheduled record.
    fn schedule_with(client: &mut Client, ticket: u64, activation: &str, key: &str) -> Value {
        client.command(
            "ticket.schedule",
            json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": key },
                "ticket_id": ticket,
                "activation": activation,
                "timezone": "Europe/Amsterdam",
                "profile": "standard",
            }),
        )
    }

    /// Poll `client` until the Ticket reaches `state`, failing at the
    /// deadline.
    fn await_state(client: &mut Client, ticket: u64, state: &str) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let read = client.query_with("ticket.get", json!({ "ticket_id": ticket }));
            if read["state"] == json!(state) {
                return read;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "ticket {ticket} never reached {state}, last read: {read}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Backdate the stored Schedule row so its moment passed while
    /// the core was down.
    fn backdate(dir: &TempDir, past: &str) {
        let conn = rusqlite::Connection::open(dir.path().join("kanban.sqlite"))
            .expect("the database reopens for the backdate");
        conn.execute(
            "UPDATE schedules SET activation_at = ?1, next_activation = ?1 WHERE id = 1",
            rusqlite::params![past],
        )
        .expect("the schedule row backdates");
    }

    /// The number of activated transition rows on the Project
    /// timeline.
    fn activated_rows(client: &mut Client) -> usize {
        let timeline = client.query_with(
            "timeline.query",
            json!({ "scope": { "project": 1 }, "kinds": ["transition"] }),
        );
        timeline["events"]
            .as_array()
            .expect("the timeline answers with events")
            .iter()
            .filter(|event| event["detail"]["action"] == json!("activated"))
            .count()
    }

    /// The stored state of the first Schedule row.
    fn schedule_state(dir: &TempDir) -> String {
        let conn = rusqlite::Connection::open(dir.path().join("kanban.sqlite"))
            .expect("the database reopens for the read");
        conn.query_row("SELECT state FROM schedules WHERE id = 1", [], |row| {
            row.get(0)
        })
        .expect("the schedule row reads")
    }

    #[test]
    fn an_overdue_one_time_schedule_activates_after_restart() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = scratch_repository(&dir, "kanban");
        let core = boot(&dir);
        let (mut client, ticket) = project_and_task(core.socket_path(), &repository);

        let scheduled = schedule_with(&mut client, ticket, &now_plus(3_600), "schedule-future");
        assert_eq!(scheduled["state"], json!("scheduled"));
        core.shutdown();

        // The moment passes while no core is running.
        backdate(&dir, &now_plus(-3_600));
        let rebooted = boot(&dir);
        let mut second = Client::connect(rebooted.socket_path());

        let ready = await_state(&mut second, ticket, "ready");

        assert_eq!(ready["version"], json!(3), "one applied activation");
        assert_eq!(schedule_state(&dir), "fired", "the Schedule spent");
        assert_eq!(
            activated_rows(&mut second),
            1,
            "the restart pass appended exactly one activation row (DR-SA-06)"
        );

        rebooted.shutdown();
    }

    #[test]
    fn a_future_one_time_schedule_survives_a_restart_without_firing() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = scratch_repository(&dir, "kanban");
        let core = boot(&dir);
        let (mut client, ticket) = project_and_task(core.socket_path(), &repository);
        schedule_with(&mut client, ticket, &now_plus(3_600), "schedule-future");
        core.shutdown();

        let rebooted = boot(&dir);
        let mut second = Client::connect(rebooted.socket_path());
        // The restart pass has run; the future activation stays
        // waiting.
        thread::sleep(Duration::from_millis(200));
        let read = second.query_with("ticket.get", json!({ "ticket_id": ticket }));
        assert_eq!(read["state"], json!("scheduled"), "not yet the moment");
        assert_eq!(read["version"], json!(2));
        assert_eq!(schedule_state(&dir), "waiting");
        assert_eq!(activated_rows(&mut second), 0);

        rebooted.shutdown();
    }

    #[test]
    fn a_fired_schedule_does_not_fire_twice_across_restarts() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = scratch_repository(&dir, "kanban");
        let core = boot(&dir);
        let (mut client, ticket) = project_and_task(core.socket_path(), &repository);
        schedule_with(&mut client, ticket, &now_plus(1), "schedule-soon");
        let _ = await_state(&mut client, ticket, "ready");
        core.shutdown();

        let rebooted = boot(&dir);
        let mut second = Client::connect(rebooted.socket_path());
        thread::sleep(Duration::from_millis(200));
        let read = second.query_with("ticket.get", json!({ "ticket_id": ticket }));

        assert_eq!(read["state"], json!("ready"));
        assert_eq!(
            read["version"],
            json!(3),
            "the spent activation fired once, never again"
        );
        assert_eq!(activated_rows(&mut second), 1);
        assert_eq!(schedule_state(&dir), "fired");

        rebooted.shutdown();
    }

    #[test]
    fn a_schedule_fires_on_the_live_tick_once_its_moment_arrives() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let repository = scratch_repository(&dir, "kanban");
        let core = boot(&dir);
        let (mut client, ticket) = project_and_task(core.socket_path(), &repository);

        schedule_with(&mut client, ticket, &now_plus(1), "schedule-soon");
        let ready = await_state(&mut client, ticket, "ready");

        assert_eq!(ready["version"], json!(3));
        assert_eq!(activated_rows(&mut client), 1);
        let _ = now_stored();

        core.shutdown();
    }
}
