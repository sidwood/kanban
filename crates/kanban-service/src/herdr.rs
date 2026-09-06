//! Herdr session observation: connect through each Project's
//! effective session, snapshot on startup and reconnect, and append
//! telemetry events (KAN-S8-US1, DR-HB-08, DR-HB-19). Push events
//! drive normal operation and land through the telemetry
//! projection (KAN-S8-US2, DR-HB-07), while whole-session
//! reconciliation compares full state on the Project's cadence and
//! appends the differences push events may have missed (KAN-S8-US3,
//! DR-HB-09, DR-HB-10). The same observed events feed the Project's
//! stall and missing-result deadlines, evaluated on a bounded
//! cadence and emitted as attention signals (KAN-S8-US4).

use std::collections::{HashMap, VecDeque};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use kanban_app::deadlines::{DeadlineConfig, DeadlineMonitor};
use kanban_app::telemetry::{AttentionSignal, TelemetryProjection, project_herdr_event};
use kanban_app::{
    CoordinatorWake, CoordinatorWakeRequest, HerdrDiagnostics, HerdrProjectObserver,
    HerdrSettingsStore, TimelineEnvelope,
};
use kanban_domain::{HerdrSession, Project};
use kanban_dto::{HerdrConnectionDiagnostics, TimelineEventKind};
use kanban_herdr::{
    HerdrError, PollingFallback, Reconciler, ReconciliationPlan, SESSION_IO_TIMEOUT, SessionClient,
    SessionMapping, SnapshotDifference, WakeRequest,
};
use kanban_storage::{Database, SqliteHerdrSettingsStore};
use serde_json::{Value, json};

/// Why observation could not start for one Project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationError {
    Connect(String),
    Snapshot(String),
    Timeline(String),
}

impl std::fmt::Display for ObservationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(message) | Self::Snapshot(message) | Self::Timeline(message) => {
                write!(f, "{message}")
            }
        }
    }
}

/// Live diagnostics for one Project's Herdr binding: the session the
/// Project selected (absent for Herdr's default), the product
/// workspace, and the target Herdr workspace resolved inside that
/// session.
#[derive(Debug, Clone, Default)]
struct SessionDiagnostics {
    session_name: Option<String>,
    product_workspace: String,
    herdr_workspace: String,
    connected: bool,
    last_snapshot_at: Option<String>,
    last_error: Option<String>,
    consecutive_failures: u32,
}

/// How failed attempts space out: the base delay doubles with each
/// consecutive failure, capped at the maximum (KAN-T78-AC2).
#[derive(Debug, Clone, Copy)]
pub struct BackoffPolicy {
    base: Duration,
    max: Duration,
}

impl BackoffPolicy {
    /// The production curve: one second doubling up to one minute.
    pub const PRODUCTION: Self = Self {
        base: Duration::from_secs(1),
        max: Duration::from_secs(60),
    };

    /// A custom curve, for tests that need quick redials.
    pub const fn new(base: Duration, max: Duration) -> Self {
        Self { base, max }
    }
}

/// How one observer redials and when a live subscription counts as
/// settled: only a subscription that holds for the settle window
/// resets the backoff and appends its snapshot, so a session that
/// flaps — subscribing and dropping at once — is another failed
/// attempt, never a cycle of resets and reconnect snapshots
/// (KAN-T78-AC2).
#[derive(Debug, Clone, Copy)]
pub struct ObservationTuning {
    /// The backoff failed attempts follow.
    pub backoff: BackoffPolicy,
    /// How long a live subscription must hold before it is settled.
    pub settle: Duration,
    /// How long each Herdr request round-trip may block.
    pub io_timeout: Duration,
}

impl ObservationTuning {
    /// The production tuning: one-second backoff doubling to a minute,
    /// settled after five seconds of a held subscription, with a
    /// five-second request I/O deadline.
    pub const PRODUCTION: Self = Self {
        backoff: BackoffPolicy::PRODUCTION,
        settle: Duration::from_secs(5),
        io_timeout: SESSION_IO_TIMEOUT,
    };
}

/// Cap the doubling shift so it cannot overflow the base.
const MAX_BACKOFF_SHIFT: u32 = 16;

/// How often the steady read re-reads the Project's Herdr settings
/// while nothing is due: a quiet session retunes its whole-session
/// cadence — reconciliation interval or an opted-in polling
/// fallback — and re-evaluates its deadlines within this window,
/// without waiting for a reconnect.
const SETTINGS_REFRESH_WINDOW: Duration = Duration::from_secs(1);

/// How many attention signals one Project retains: a breach is
/// re-reported on every evaluation while it holds, so retention —
/// not deduplication, which belongs to the KAN-S11 consumer —
/// bounds what an unconsumed producer keeps.
const RETAINED_SIGNALS_PER_PROJECT: usize = 256;

/// How many Coordinator wakes one Project's observation retains
/// before new ones are refused: the Dispatch Request behind every
/// wake is already durable and waits for the Coordinator's next
/// loop regardless, so the bound caps memory without losing work.
const WAKE_INBOX_CAPACITY: usize = 16;

/// One Coordinator wake waiting for its Project's observation
/// thread: the mapping the wake speaks through, and the Dispatch
/// Request it announces.
#[derive(Debug, Clone)]
struct WakeDelivery {
    mapping: SessionMapping,
    dispatch_request_id: u64,
}

/// The delay after `failures` consecutive failed attempts: the base
/// doubling per failure, bounded by the policy maximum.
fn backoff_delay(policy: BackoffPolicy, failures: u32) -> Duration {
    let shift = failures.saturating_sub(1).min(MAX_BACKOFF_SHIFT);
    policy.base.saturating_mul(1 << shift).min(policy.max)
}

/// One owned observation: the stop flag its thread polls, the socket
/// duplicate that wakes a blocked read, the bounded inbox of
/// Coordinator wakes the thread delivers, and the thread itself.
struct Observation {
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<UnixStream>>>,
    wakes: Arc<Mutex<VecDeque<WakeDelivery>>>,
    handle: Option<JoinHandle<()>>,
}

impl Observation {
    /// Enqueue one Coordinator wake for the observation thread,
    /// refusing it when the bounded inbox is full. The Dispatch
    /// Request is already durable, so a refused wake loses only the
    /// advisory nudge, never the work.
    fn offer_wake(&self, delivery: WakeDelivery) -> bool {
        let mut pending = self.wakes.lock().unwrap();
        if pending.len() >= WAKE_INBOX_CAPACITY {
            return false;
        }
        pending.push_back(delivery);
        true
    }

    /// Rouse the observation thread from its backoff park so a
    /// queued Coordinator wake is delivered promptly, not at the
    /// next redial the backoff was spacing out.
    fn nudge(&self) {
        if let Some(handle) = &self.handle {
            handle.thread().unpark();
        }
    }
}

/// End one observation: signal its thread, wake any blocked read
/// through the socket duplicate, then join the thread so its socket
/// and database references are released before the call returns.
fn end_observation(observation: Observation) {
    observation.stop.store(true, Ordering::Relaxed);
    if let Some(socket) = observation.socket.lock().unwrap().take() {
        let _ = socket.shutdown(Shutdown::Both);
    }
    if let Some(handle) = observation.handle {
        handle.thread().unpark();
        let _ = handle.join();
    }
}

/// The service-side Herdr observer.
pub struct HerdrObserver {
    socket_root: PathBuf,
    database: Arc<Database>,
    diagnostics: Arc<Mutex<HashMap<u64, SessionDiagnostics>>>,
    signals: Arc<Mutex<HashMap<u64, Vec<AttentionSignal>>>>,
    sessions: Mutex<HashMap<u64, Observation>>,
    backoff: BackoffPolicy,
    settle: Duration,
    io_timeout: Duration,
}

impl HerdrObserver {
    /// Create an observer rooted at `socket_root`.
    pub fn new(database: Arc<Database>, socket_root: PathBuf) -> Arc<Self> {
        Self::with_observation(database, socket_root, ObservationTuning::PRODUCTION)
    }

    /// Create an observer whose failed attempts follow `backoff`.
    pub fn with_backoff(
        database: Arc<Database>,
        socket_root: PathBuf,
        backoff: BackoffPolicy,
    ) -> Arc<Self> {
        Self::with_observation(
            database,
            socket_root,
            ObservationTuning {
                backoff,
                ..ObservationTuning::PRODUCTION
            },
        )
    }

    /// Create an observer following one `tuning`: failed attempts
    /// space out by its backoff, and a live subscription counts as
    /// settled only after its settle window.
    pub fn with_observation(
        database: Arc<Database>,
        socket_root: PathBuf,
        tuning: ObservationTuning,
    ) -> Arc<Self> {
        Arc::new(Self {
            socket_root,
            database,
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            signals: Arc::new(Mutex::new(HashMap::new())),
            sessions: Mutex::new(HashMap::new()),
            backoff: tuning.backoff,
            settle: tuning.settle,
            io_timeout: tuning.io_timeout,
        })
    }

    /// Start observing every active Project in `projects`.
    pub fn observe_projects(&self, projects: &[Project]) {
        for project in projects {
            if project.is_archived() {
                continue;
            }
            self.observe_project(project);
        }
    }

    /// Start observing one Project's Herdr binding through its
    /// effective session.
    pub fn observe_project(&self, project: &Project) {
        if project.is_archived() {
            return;
        }
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.contains_key(&project.id().value()) {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let socket = Arc::new(Mutex::new(None));
        let wakes: Arc<Mutex<VecDeque<WakeDelivery>>> = Arc::new(Mutex::new(VecDeque::new()));
        let handle = self.observation_handle(project, stop.clone(), socket.clone(), wakes.clone());
        let thread_name = project
            .registration()
            .effective_herdr_session()
            .as_name()
            .map(|name| format!("herdr-{name}"))
            .unwrap_or_else(|| "herdr-default".to_owned());
        let thread = thread::Builder::new()
            .name(thread_name)
            .spawn(move || handle.run())
            .expect("a Herdr observation thread starts");
        sessions.insert(
            project.id().value(),
            Observation {
                stop,
                socket,
                wakes,
                handle: Some(thread),
            },
        );
    }

    /// Register the Project's diagnostics entry and build the handle
    /// its observation thread runs.
    fn observation_handle(
        &self,
        project: &Project,
        stop: Arc<AtomicBool>,
        socket: Arc<Mutex<Option<UnixStream>>>,
        wakes: Arc<Mutex<VecDeque<WakeDelivery>>>,
    ) -> HerdrObserverHandle {
        let registration = project.registration();
        let session = registration.effective_herdr_session();
        self.diagnostics.lock().unwrap().insert(
            project.id().value(),
            SessionDiagnostics {
                session_name: registration.herdr_session().map(str::to_owned),
                product_workspace: registration.seed_workspace().to_owned(),
                herdr_workspace: registration.herdr_workspace().to_owned(),
                ..SessionDiagnostics::default()
            },
        );
        HerdrObserverHandle {
            project_id: project.id().value(),
            mapping: SessionMapping::new(
                session,
                registration.seed_workspace(),
                registration.herdr_workspace(),
            ),
            socket_root: self.socket_root.clone(),
            database: self.database.clone(),
            diagnostics: self.diagnostics.clone(),
            signals: self.signals.clone(),
            stop,
            socket,
            wakes,
            backoff: self.backoff,
            settle: self.settle,
            io_timeout: self.io_timeout,
        }
    }

    /// Build one Project's handle together with its stop flag, without
    /// spawning its thread: a test driving the whole loop through
    /// [`HerdrObserverHandle::run_parking`] stops it from its own park
    /// hook — exactly where production's parked thread would notice a
    /// stop (KAN-T130-AC1).
    #[cfg(test)]
    fn test_driven_loop(&self, project: &Project) -> (HerdrObserverHandle, Arc<AtomicBool>) {
        let stop = Arc::new(AtomicBool::new(false));
        let handle = self.observation_handle(
            project,
            stop.clone(),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(VecDeque::new())),
        );
        (handle, stop)
    }

    /// Build one Project's handle without spawning its thread: tests
    /// drive the reconnect lifecycle cycle by cycle on their own
    /// thread, so every transition is asserted after the cycle that
    /// caused it, with no sleeps (KAN-T94-AC2).
    #[cfg(test)]
    fn test_driven_handle(&self, project: &Project) -> HerdrObserverHandle {
        self.observation_handle(
            project,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(VecDeque::new())),
        )
    }

    /// Whether this observer still owns one Project's session.
    pub fn is_observing(&self, project_id: u64) -> bool {
        self.sessions.lock().unwrap().contains_key(&project_id)
    }

    /// Consecutive failed attempts one Project's observation is
    /// backing off, for diagnostics (KAN-T78-AC2).
    pub fn consecutive_failures(&self, project_id: u64) -> u32 {
        self.diagnostics
            .lock()
            .unwrap()
            .get(&project_id)
            .map(|entry| entry.consecutive_failures)
            .unwrap_or(0)
    }

    /// The attention signals observation has emitted for one
    /// Project, oldest first (KAN-T41-AC3): the producer boundary
    /// the KAN-S11 attention inbox consumes. A breach holds on every
    /// evaluation while it lasts, so one breach appears repeatedly;
    /// collapsing those re-reports into Attention items belongs to
    /// the consumer.
    pub fn attention_signals(&self, project_id: u64) -> Vec<AttentionSignal> {
        self.signals
            .lock()
            .unwrap()
            .get(&project_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Stop observing one Project, release its socket and database
    /// references, and join its thread (KAN-T78-AC1).
    pub fn stop_observing(&self, project_id: u64) {
        let Some(observation) = self.sessions.lock().unwrap().remove(&project_id) else {
            return;
        };
        end_observation(observation);
        self.diagnostics.lock().unwrap().remove(&project_id);
        self.signals.lock().unwrap().remove(&project_id);
    }

    /// The Herdr socket root this observer dials.
    #[cfg(test)]
    pub(crate) fn socket_root(&self) -> &Path {
        &self.socket_root
    }

    /// The Coordinator wakes queued for one Project's observation
    /// thread, oldest first, for tests that fill and inspect the
    /// bounded inbox.
    #[cfg(test)]
    fn pending_wakes(&self, project_id: u64) -> Vec<WakeDelivery> {
        self.sessions
            .lock()
            .unwrap()
            .get(&project_id)
            .map(|observation| observation.wakes.lock().unwrap().iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Stop every owned observer and join every thread: shutdown
    /// leaves no session observed and no resource held (KAN-T78-AC1).
    pub fn shutdown(&self) {
        let observations: Vec<Observation> = self
            .sessions
            .lock()
            .unwrap()
            .drain()
            .map(|(_, observation)| observation)
            .collect();
        for observation in observations {
            end_observation(observation);
        }
        self.diagnostics.lock().unwrap().clear();
        self.signals.lock().unwrap().clear();
    }
}

impl HerdrProjectObserver for HerdrObserver {
    fn observe(&self, project: &Project) {
        self.observe_project(project);
    }

    fn stop_observing(&self, project_id: u64) {
        HerdrObserver::stop_observing(self, project_id);
    }
}

impl CoordinatorWake for HerdrObserver {
    fn wake(&self, request: CoordinatorWakeRequest) {
        // The caller runs inside the global command gate, so this
        // performs no socket I/O: it resolves the mapping, hands the
        // Project's observation worker a bounded inbox entry, and
        // rouses its thread. The worker delivers on its own thread
        // (see [`HerdrObserverHandle::deliver_wakes`]); a full inbox,
        // or a Project nothing observes, drops the wake — the
        // Dispatch Request is already durable and stays queued for
        // the Coordinator's next loop.
        let session = match request.herdr_session.as_deref() {
            Some(name) => match HerdrSession::named(name) {
                Ok(session) => session,
                Err(_) => return,
            },
            None => HerdrSession::Default,
        };
        let mapping =
            SessionMapping::new(session, &request.seed_workspace, &request.herdr_workspace);
        let sessions = self.sessions.lock().unwrap();
        let Some(observation) = sessions.get(&request.project_id) else {
            return;
        };
        if observation.offer_wake(WakeDelivery {
            mapping,
            dispatch_request_id: request.dispatch_request_id,
        }) {
            observation.nudge();
        }
    }
}

struct HerdrObserverHandle {
    project_id: u64,
    mapping: SessionMapping,
    socket_root: PathBuf,
    database: Arc<Database>,
    diagnostics: Arc<Mutex<HashMap<u64, SessionDiagnostics>>>,
    signals: Arc<Mutex<HashMap<u64, Vec<AttentionSignal>>>>,
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<UnixStream>>>,
    wakes: Arc<Mutex<VecDeque<WakeDelivery>>>,
    backoff: BackoffPolicy,
    settle: Duration,
    io_timeout: Duration,
}

impl HerdrObserverHandle {
    fn run(self) {
        self.run_parking(thread::park_timeout);
    }

    /// The observer loop with its backoff park injected: production
    /// parks the thread for the bounded delay, while a driven test
    /// records each cycle's delay — and stops the loop from the hook —
    /// so the loop's own failure accounting and bounded delay are
    /// proven cycle by cycle without a real sleep (KAN-T130-AC1).
    fn run_parking(self, mut park: impl FnMut(Duration)) {
        let mut failures = 0u32;
        let mut live_once = false;
        // The monitor outlives every redial — roles observed before a
        // disconnect stay watched once the session returns — and every
        // settled capture reconciles it, so roles that finished,
        // exited, or disappeared inside the gap retire instead of
        // breaching forever.
        let mut deadlines = DeadlineMonitor::new(self.session_tuning().1);
        while !self.stopped() {
            // Between cycles — parked in backoff or redialling — the
            // thread owns no live subscription, so queued wakes go
            // out over their own bounded connections.
            self.deliver_wakes(None);
            if self.observe_live(&mut live_once, &mut deadlines) {
                // A settled session that ended is the first failure of
                // the next cycle, not a reset.
                failures = 1;
            } else {
                failures = failures.saturating_add(1);
            }
            self.note_failures(failures);
            if self.stopped() {
                break;
            }
            park(backoff_delay(self.backoff, failures));
        }
    }

    /// Publish the cycle's failure count, so the settled-live rule is
    /// observable per Project (KAN-T78-AC2).
    fn note_failures(&self, failures: u32) {
        if let Some(entry) = self.diagnostics.lock().unwrap().get_mut(&self.project_id) {
            entry.consecutive_failures = failures;
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Deliver every queued Coordinator wake, oldest first, on this
    /// observation thread — never the command gate, which only
    /// enqueues. A session that cannot be reached leaves the request
    /// queued for the next Coordinator loop rather than rolling
    /// dispatch back.
    fn deliver_wakes(&self, live: Option<&mut SessionClient>) {
        let mut live = live;
        loop {
            if self.stopped() {
                return;
            }
            let delivery = self.wakes.lock().unwrap().pop_front();
            let Some(delivery) = delivery else {
                return;
            };
            self.deliver_wake(&delivery, live.as_deref_mut());
        }
    }

    /// Deliver one Coordinator wake. With `live`, the settled
    /// subscription's own connection carries the request — its
    /// handshake already verified the session's identity. Without
    /// one, the wake opens its own connection under this handle's
    /// I/O deadline and verifies the mapping before it speaks, so a
    /// dead or lying session can neither stall the worker
    /// indefinitely nor be woken under the wrong identity.
    fn deliver_wake(&self, delivery: &WakeDelivery, live: Option<&mut SessionClient>) {
        let wake = WakeRequest {
            dispatch_request_id: delivery.dispatch_request_id,
        };
        let delivered = match live {
            Some(client) => client.wake_coordinator(wake).map(|_| ()),
            None => SessionClient::open_with_io_timeout(
                delivery.mapping.clone(),
                &self.socket_root,
                self.io_timeout,
            )
            .and_then(|mut client| {
                let snapshot = client.snapshot()?;
                client.mapping().verify_snapshot(&snapshot)?;
                client.wake_coordinator(wake).map(|_| ())
            }),
        };
        let _ = delivered;
    }

    /// Connect, subscribe, and observe the live subscription. Returns
    /// whether the subscription settled and then ended: a failed
    /// connection, subscription, or settle window reports through
    /// diagnostics alone, and the captured snapshot is appended only
    /// once the subscription has held past the settle window
    /// (KAN-T78-AC2, KAN-T78-AC3). Every push event read on the way
    /// feeds `deadlines` and lands through the telemetry projection
    /// (DR-HB-07).
    fn observe_live(&self, live_once: &mut bool, deadlines: &mut DeadlineMonitor) -> bool {
        // Open carries no request traffic: the socket duplicate — the
        // only handle a stop can shut down — is registered before the
        // snapshot handshake blocks, so stopping this thread is always
        // possible, even mid-handshake.
        let mut client = match SessionClient::open_with_io_timeout(
            self.mapping.clone(),
            &self.socket_root,
            self.io_timeout,
        ) {
            Ok(client) => client,
            Err(error) => {
                self.mark_connected(false, Some(error.to_string()));
                return false;
            }
        };
        // Register the duplicate before anything can block, so a stop
        // can always wake this thread.
        match client.duplicate_socket() {
            Ok(duplicate) => *self.socket.lock().unwrap() = Some(duplicate),
            Err(error) => {
                self.mark_connected(false, Some(error.to_string()));
                return false;
            }
        }
        if self.stopped() {
            return false;
        }
        // Capture before subscribing so no push event can overtake
        // the capture; the append below is gated on the settle window.
        let snapshot = match client.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.mark_connected(false, Some(error.to_string()));
                return false;
            }
        };
        if let Err(error) = client.mapping().verify_snapshot(&snapshot) {
            self.mark_connected(false, Some(error.to_string()));
            return false;
        }
        if let Err(error) = client.subscribe() {
            *self.socket.lock().unwrap() = None;
            self.mark_connected(false, Some(error.to_string()));
            return false;
        }
        self.mark_connected(true, None);
        // Only a subscription that holds for the settle window counts
        // as the live session: one that drops inside the window is
        // another failed attempt, so it neither resets the backoff nor
        // lands its snapshot (KAN-T78-AC2).
        let mut windowed = Vec::new();
        if self.stopped() || !self.settles(&mut client, &mut windowed) {
            self.mark_connected(false, Some("disconnected".to_owned()));
            return false;
        }
        let reason = if *live_once { "reconnect" } else { "startup" };
        *live_once = true;
        // The settled snapshot is also reconciliation's first
        // baseline: the first whole-session comparison waits a full
        // cadence beyond it (DR-HB-09).
        let now = SystemTime::now();
        let (plan, deadline_config) = self.session_tuning();
        deadlines.retune(deadline_config);
        let mut reconciler = Reconciler::seeded_with(plan, &snapshot, now);
        // The settled capture is as authoritative for the deadlines
        // as it is for the baseline: the push events a disconnected
        // gap swallowed cannot be replayed, so this is the path that
        // retires a role that finished, exited, or disappeared while
        // the subscription was down — before the first post-reconnect
        // evaluation can report a phantom breach.
        deadlines.observe_snapshot(now, &snapshot.state);
        match self.append_snapshot(&snapshot, reason) {
            Ok(()) => self.record_snapshot(snapshot.captured_at),
            Err(error) => self.mark_error(error),
        }
        // The capture precedes every event it raced, so the events
        // buffered inside the window append after the snapshot row.
        for event in windowed {
            self.append_push_event(deadlines, &event);
        }
        while !self.stopped() {
            if self
                .observe_once(&mut reconciler, &mut client, deadlines)
                .is_none()
            {
                break;
            }
        }
        true
    }

    /// One step of the steady observation: wait for the next push
    /// event or the next whole-session capture, whichever comes
    /// first, bounded by the settings refresh window so a quiet
    /// session still retunes its cadence and re-evaluates its
    /// deadlines. Returns `None` when the subscription ended and the
    /// observer must redial.
    fn observe_once(
        &self,
        reconciler: &mut Reconciler,
        client: &mut SessionClient,
        deadlines: &mut DeadlineMonitor,
    ) -> Option<()> {
        // Queued Coordinator wakes ride the live subscription this
        // step already holds; each step is bounded by its read
        // window, so wake delivery stays prompt while connected.
        self.deliver_wakes(Some(&mut *client));
        self.evaluate_deadlines(deadlines);
        let window = reconciler
            .remaining_until(SystemTime::now())
            .min(SETTINGS_REFRESH_WINDOW);
        if window.is_zero() {
            return self.capture_session(reconciler, client, deadlines);
        }
        match client.read_event_within(window) {
            Ok(event) => {
                self.append_push_event(deadlines, &event);
                Some(())
            }
            // The window closed without a frame: the cadence is
            // re-checked above, and the plan is retuned so settings
            // changes apply without a reconnect (DR-HB-11).
            Err(HerdrError::TimedOut) => {
                let (plan, deadline_config) = self.session_tuning();
                reconciler.replan(plan);
                deadlines.retune(deadline_config);
                Some(())
            }
            Err(_) => {
                self.mark_connected(false, Some("disconnected".to_owned()));
                None
            }
        }
    }

    /// Capture full session state through the live subscription,
    /// append the difference it reports, if any (DR-HB-09, DR-HB-10),
    /// and reconcile the deadlines with the authoritative capture the
    /// same way a settled reconnect does. Returns `None` when the
    /// capture failed and the subscription cycle must end so the
    /// observer redials.
    fn capture_session(
        &self,
        reconciler: &mut Reconciler,
        client: &mut SessionClient,
        deadlines: &mut DeadlineMonitor,
    ) -> Option<()> {
        let snapshot = match client.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.mark_connected(false, Some(error.to_string()));
                return None;
            }
        };
        let now = SystemTime::now();
        deadlines.observe_snapshot(now, &snapshot.state);
        if let Some(difference) = reconciler.adopt(now, snapshot) {
            self.append_reconciliation(&difference);
        }
        // Each capture retunes the cadence and the deadlines, so
        // interval, fallback, and deadline changes land at the next
        // capture at the latest.
        let (plan, deadline_config) = self.session_tuning();
        reconciler.replan(plan);
        deadlines.retune(deadline_config);
        Some(())
    }

    /// The whole-session cadence and the deadlines the Project's
    /// Herdr settings call for, read together: their reconciliation
    /// interval tightened by the polling fallback while the Project
    /// has one enabled (DR-HB-09, DR-HB-10), and their stall and
    /// missing-result deadlines (KAN-S8-US4). A Project without
    /// settings yet observes on the defaults.
    fn session_tuning(&self) -> (ReconciliationPlan, DeadlineConfig) {
        let Ok(settings) =
            SqliteHerdrSettingsStore::new(&self.database).project_settings(self.project_id)
        else {
            return (ReconciliationPlan::default(), DeadlineConfig::default());
        };
        let fallback = if settings.polling_fallback_enabled {
            PollingFallback::every(Duration::from_secs(settings.polling_fallback_interval_secs))
        } else {
            PollingFallback::off()
        };
        let plan =
            ReconciliationPlan::new(Duration::from_secs(settings.reconciliation_interval_secs))
                .with_fallback(fallback);
        (plan, DeadlineConfig::from(&settings))
    }

    /// Evaluate the Project's deadlines now and record every breach
    /// as an emitted attention signal (KAN-T41-AC3): the producer
    /// boundary the KAN-S11 attention inbox consumes. Deduplicating
    /// the per-evaluation re-reports into Attention items belongs to
    /// that consumer, so retention — not silence — bounds the
    /// record.
    fn evaluate_deadlines(&self, deadlines: &mut DeadlineMonitor) {
        let signals = deadlines.evaluate(self.project_id, SystemTime::now());
        if signals.is_empty() {
            return;
        }
        let mut emitted = self.signals.lock().unwrap();
        let entry = emitted.entry(self.project_id).or_default();
        entry.extend(signals);
        let surplus = entry.len().saturating_sub(RETAINED_SIGNALS_PER_PROJECT);
        entry.drain(..surplus);
    }

    /// Append one whole-session difference as a telemetry event:
    /// what reconciliation found, as observation, never a verdict.
    fn append_reconciliation(&self, difference: &SnapshotDifference) {
        let changes = serde_json::to_value(&difference.changes).unwrap_or(Value::Null);
        let detail = json!({
            "source": "herdr",
            "event": "reconciliation",
            "previous_captured_at": difference.previous_captured_at,
            "captured_at": difference.captured_at,
            "changes": changes,
        });
        let envelope =
            TimelineEnvelope::project(self.project_id, TimelineEventKind::Telemetry, None, detail);
        if let Err(error) = self.database.append_timeline_event(&envelope) {
            self.mark_error(ObservationError::Timeline(error.to_string()));
        }
    }

    /// Wait out the settle window against the live subscription,
    /// collecting the events pushed inside it: a subscription still
    /// answering — pushing events or sitting quiet — when the window
    /// closes is settled, while one whose connection ends inside it is
    /// not (KAN-T78-AC2). A failed window drops its events: that
    /// session never settled, and reconciliation (KAN-T41) is the
    /// recovery path for what a dropped subscription lost.
    fn settles(&self, client: &mut SessionClient, windowed: &mut Vec<Value>) -> bool {
        let deadline = Instant::now() + self.settle;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return true;
            }
            match client.read_event_within(remaining) {
                // A pushed event proves the subscription live; the
                // window still has to be waited out.
                Ok(event) => windowed.push(event),
                // Silence held to the deadline is as settled as
                // traffic.
                Err(HerdrError::TimedOut) => return true,
                // The connection ended inside the window: a flap, not
                // a live session.
                Err(_) => return false,
            }
        }
    }

    /// Append one captured snapshot as a telemetry event.
    fn append_snapshot(
        &self,
        snapshot: &kanban_herdr::Snapshot,
        reason: &str,
    ) -> Result<(), ObservationError> {
        let detail = json!({
            "source": "herdr",
            "event": "snapshot",
            "reason": reason,
            "session": snapshot.session,
            "product_workspace": snapshot.product_workspace,
            "herdr_workspace": snapshot.herdr_workspace,
            "captured_at": snapshot.captured_at,
            "state": snapshot.state,
        });
        let envelope =
            TimelineEnvelope::project(self.project_id, TimelineEventKind::Telemetry, None, detail);
        self.database
            .append_timeline_event(&envelope)
            .map_err(|error| ObservationError::Timeline(error.to_string()))?;
        Ok(())
    }

    /// Feed one observed push event to the Project's deadlines —
    /// every kind of role event is activity, so no unseen Herdr
    /// event can fake a stall (KAN-S8-US4) — and append it through
    /// the telemetry projection (DR-HB-07). The projection can only
    /// mint timeline rows and attention signals; push events never
    /// raise a signal, and the signals' consumer is the KAN-S11
    /// attention inbox.
    fn append_push_event(&self, deadlines: &mut DeadlineMonitor, event: &Value) {
        deadlines.observe_event(SystemTime::now(), event);
        for projection in project_herdr_event(self.project_id, event) {
            match projection {
                TelemetryProjection::Timeline(envelope) => {
                    if let Err(error) = self.database.append_timeline_event(&envelope) {
                        self.mark_error(ObservationError::Timeline(error.to_string()));
                    }
                }
                TelemetryProjection::Attention(_) => {}
            }
        }
    }

    fn mark_connected(&self, connected: bool, last_error: Option<String>) {
        if let Some(entry) = self.diagnostics.lock().unwrap().get_mut(&self.project_id) {
            entry.connected = connected;
            entry.last_error = last_error;
        }
    }

    fn mark_error(&self, error: ObservationError) {
        self.mark_connected(false, Some(error.to_string()));
    }

    fn record_snapshot(&self, captured_at: String) {
        if let Some(entry) = self.diagnostics.lock().unwrap().get_mut(&self.project_id) {
            entry.last_snapshot_at = Some(captured_at);
            entry.connected = true;
            entry.last_error = None;
        }
    }
}

/// Diagnostics served through `herdr.settings.get`.
pub struct LiveHerdrDiagnostics {
    inner: Arc<Mutex<HashMap<u64, SessionDiagnostics>>>,
}

impl LiveHerdrDiagnostics {
    /// Wrap the observer's live diagnostics map.
    pub fn new(observer: &HerdrObserver) -> Self {
        Self {
            inner: observer.diagnostics.clone(),
        }
    }

    /// Seed diagnostics before the observer starts, for tests.
    pub fn empty() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl HerdrDiagnostics for LiveHerdrDiagnostics {
    fn for_project(
        &self,
        project_id: u64,
        session: Option<&str>,
        product_workspace: &str,
        herdr_workspace: &str,
    ) -> HerdrConnectionDiagnostics {
        self.inner
            .lock()
            .unwrap()
            .get(&project_id)
            .cloned()
            .unwrap_or(SessionDiagnostics {
                session_name: session.map(str::to_owned),
                product_workspace: product_workspace.to_owned(),
                herdr_workspace: herdr_workspace.to_owned(),
                ..SessionDiagnostics::default()
            })
            .into()
    }
}

impl From<SessionDiagnostics> for HerdrConnectionDiagnostics {
    fn from(value: SessionDiagnostics) -> Self {
        Self {
            session_name: value.session_name,
            product_workspace: value.product_workspace,
            herdr_workspace: value.herdr_workspace,
            connected: value.connected,
            last_snapshot_at: value.last_snapshot_at,
            last_error: value.last_error,
        }
    }
}

/// Resolve Herdr's config root for default and named session observation.
pub fn production_socket_root() -> PathBuf {
    kanban_herdr::herdr_sessions_dir().unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/herdr")
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use kanban_app::{
        CoordinatorWake, CoordinatorWakeRequest, HerdrDiagnostics, HerdrSettingsStore,
        ProjectStore, TimelineStore,
    };
    use kanban_dto::{
        HerdrConnectionDiagnostics, HerdrSettingsUpdateRequest, MutationContext, TimelineEventKind,
        TimelineQuery, TimelineScope,
    };
    use kanban_herdr::fixture::{ScriptedSession, SessionScript};
    use kanban_herdr::{COORDINATOR_ROLE, HerdrRequest};
    use serde_json::json;
    use tempfile::TempDir;

    use super::{
        BackoffPolicy, HerdrObserver, LiveHerdrDiagnostics, Observation, ObservationTuning,
        WAKE_INBOX_CAPACITY, backoff_delay, production_socket_root,
    };
    use crate::timeline::StorageTimelineStore;
    use kanban_app::deadlines::{
        DeadlineMonitor, MISSING_RESULT_DEADLINE_REASON, STALL_DEADLINE_REASON,
    };
    use kanban_domain::{Project, ProjectId, ProjectRegistration};
    use kanban_storage::{AllowAllMigrations, Database, SqliteHerdrSettingsStore};

    fn project(session: Option<&str>, workspace: &str) -> Project {
        let registration = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            workspace,
            "main",
            "kanban.seed",
            session,
            None,
        )
        .expect("the registration validates");
        Project::new(ProjectId::new(1), registration)
    }

    fn migrated_database(dir: &TempDir) -> Arc<Database> {
        let mut database = Database::open(&dir.path().join("kanban.sqlite"))
            .expect("opening a fresh database succeeds");
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        Arc::new(database)
    }

    /// A fast curve so lifecycle tests redial within milliseconds.
    fn fast_backoff() -> BackoffPolicy {
        BackoffPolicy::new(Duration::from_millis(10), Duration::from_millis(40))
    }

    /// A fast tuning so lifecycle tests settle within milliseconds.
    fn fast_observation() -> ObservationTuning {
        ObservationTuning {
            backoff: fast_backoff(),
            settle: Duration::from_millis(100),
            io_timeout: Duration::from_millis(100),
        }
    }

    /// A tuning whose settle window is short enough to drive a whole
    /// observation cycle synchronously while a held connection still
    /// outlasts it by a wide margin.
    fn driven_observation() -> ObservationTuning {
        ObservationTuning {
            backoff: fast_backoff(),
            settle: Duration::from_millis(50),
            io_timeout: Duration::from_millis(500),
        }
    }

    /// The served diagnostics for Project 1's binding.
    fn binding_diagnostics(observer: &HerdrObserver) -> HerdrConnectionDiagnostics {
        LiveHerdrDiagnostics::new(observer).for_project(
            1,
            Some("kanban-main"),
            "/workspaces/kanban.seed",
            "kanban.seed",
        )
    }

    fn telemetry_details(database: &Arc<Database>) -> Vec<serde_json::Value> {
        let timeline = StorageTimelineStore::new(database.clone());
        timeline
            .query(&TimelineQuery {
                scope: TimelineScope::Project(1),
                entity: None,
                kinds: Some(vec![TimelineEventKind::Telemetry]),
                since: None,
                until: None,
            })
            .expect("telemetry is queryable")
            .into_iter()
            .map(|event| event.detail)
            .collect()
    }

    /// Seed Project 1's Herdr settings from the global defaults,
    /// after registering the Project the settings attach to.
    fn seed_herdr_settings(database: &Arc<Database>) {
        let registration = ProjectRegistration::new(
            "CORE",
            "Control plane",
            "/repositories/kanban",
            "/workspaces/kanban.seed",
            "main",
            "kanban.seed",
            Some("kanban-main"),
            None,
        )
        .expect("the fixture registration validates");
        let projects = kanban_storage::SqliteProjectStore::new(database);
        let project = projects
            .create(&registration, &|id| {
                kanban_app::TimelineEnvelope::project(
                    id.value(),
                    kanban_dto::TimelineEventKind::Transition,
                    None,
                    json!({ "action": "registered" }),
                )
            })
            .expect("the fixture project registers");
        SqliteHerdrSettingsStore::new(database)
            .seed_project_settings(project.id().value())
            .expect("the settings seed from the defaults");
    }

    /// Retune Project 1's Herdr settings through the store the
    /// settings command writes through.
    fn retune_herdr_settings(
        database: &Arc<Database>,
        retune: impl FnOnce(&mut HerdrSettingsUpdateRequest),
    ) {
        let store = SqliteHerdrSettingsStore::new(database);
        let current = store.project_settings(1).expect("the seeded settings load");
        let mut request = HerdrSettingsUpdateRequest {
            mutation: MutationContext {
                optimistic_version: current.version,
                idempotency_key: "observer-tuning".to_owned(),
            },
            project_id: 1,
            reconciliation_interval_secs: current.reconciliation_interval_secs,
            polling_fallback_enabled: current.polling_fallback_enabled,
            polling_fallback_interval_secs: current.polling_fallback_interval_secs,
            stall_deadline_secs: current.stall_deadline_secs,
            missing_result_deadline_secs: current.missing_result_deadline_secs,
        };
        retune(&mut request);
        store
            .update_project_settings(&request)
            .expect("the retuned settings land");
    }

    #[test]
    fn startup_snapshot_appends_a_telemetry_event() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default(),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(300));
        let details = telemetry_details(&database);
        assert_eq!(details.len(), 1);
        assert_eq!(details[0]["event"], json!("snapshot"));
        assert_eq!(details[0]["reason"], json!("startup"));
        observer.shutdown();
    }

    /// KAN-T40-AC1: push events inside the settle window append
    /// after the snapshot they raced, carrying the payload whole.
    #[test]
    fn push_events_inside_the_settle_window_land_after_the_snapshot() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let role_opened = json!({
            "kind": "role.opened",
            "role": "implementer",
            "project": "CORE",
            "ticket": "KAN-T40",
            "lane": "in_progress",
            "reviewer_slot": "primary",
            "run": "run-1",
            "harness": "claude-code",
            "model": "opus-5"
        });
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_events(vec![
                role_opened.clone(),
                json!({ "kind": "role.output", "role": "implementer", "text": "working" }),
            ]),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(300));

        let details = telemetry_details(&database);
        assert_eq!(details.len(), 3, "the snapshot and both push events land");
        assert_eq!(details[0]["event"], json!("snapshot"));
        assert_eq!(details[0]["reason"], json!("startup"));
        assert_eq!(details[1]["event"], json!("role.opened"));
        assert_eq!(details[1]["payload"], role_opened);
        assert_eq!(
            details[1]["tab"],
            json!({
                "project": "CORE",
                "ticket": "KAN-T40",
                "lane": "in_progress",
                "reviewer_slot": "primary",
                "run": "run-1",
                "harness": "claude-code",
                "model": "opus-5"
            }),
            "the role tab metadata rides the row (DR-HB-03)"
        );
        assert_eq!(details[2]["event"], json!("role.output"));

        observer.shutdown();
    }

    /// KAN-T40-AC1: events pushed once the subscription has settled
    /// keep landing through the steady read loop.
    #[test]
    fn push_events_after_the_settle_window_still_land() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default()
                .with_events(vec![json!({ "kind": "role.output", "text": "later" })])
                // Past the fixture's 100ms settle window, so the row
                // must arrive through the steady read, not the
                // settle buffer.
                .with_delayed_events(Duration::from_millis(250)),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(600));

        let details = telemetry_details(&database);
        assert_eq!(details.len(), 2);
        assert_eq!(details[0]["event"], json!("snapshot"));
        assert_eq!(details[1]["event"], json!("role.output"));
        assert_eq!(details[1]["payload"]["text"], json!("later"));

        observer.shutdown();
    }

    /// KAN-T100-AC4: a Project without a session is observed through
    /// Herdr's default session socket, with no session selection.
    #[test]
    fn a_sessionless_project_is_observed_through_the_default_session_socket() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind_default(
            &socket_root,
            "/workspaces/kanban.seed",
            SessionScript::default(),
        );
        let mut database = Database::open(&dir.path().join("kanban.sqlite"))
            .expect("opening a fresh database succeeds");
        database
            .migrate(&AllowAllMigrations)
            .expect("the migrations apply");
        let database = Arc::new(database);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(None, "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(200));
        let timeline = StorageTimelineStore::new(database);
        let events = timeline
            .query(&TimelineQuery {
                scope: TimelineScope::Project(1),
                entity: None,
                kinds: Some(vec![TimelineEventKind::Telemetry]),
                since: None,
                until: None,
            })
            .expect("telemetry is queryable");
        assert_eq!(
            events.len(),
            1,
            "the default session serves the sessionless Project"
        );
        assert_eq!(events[0].detail["event"], json!("snapshot"));
        observer.shutdown();
    }

    #[test]
    fn production_socket_root_matches_the_installed_cli_config_root() {
        let root = production_socket_root();
        assert_eq!(
            root,
            std::path::PathBuf::from(std::env::var_os("HOME").expect("home is known"))
                .join(".config/herdr")
        );
    }

    #[test]
    fn observer_backoff_doubles_until_bounded() {
        let policy = BackoffPolicy::PRODUCTION;
        assert_eq!(backoff_delay(policy, 1), Duration::from_secs(1));
        assert_eq!(backoff_delay(policy, 2), Duration::from_secs(2));
        assert_eq!(backoff_delay(policy, 3), Duration::from_secs(4));
        assert_eq!(backoff_delay(policy, 6), Duration::from_secs(32));
        assert_eq!(
            backoff_delay(policy, 7),
            Duration::from_secs(60),
            "the doubling is bounded at the maximum"
        );
        assert_eq!(backoff_delay(policy, 60), Duration::from_secs(60));
    }

    #[test]
    fn observer_shutdown_stops_the_owned_session() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default(),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(300));
        assert!(observer.is_observing(1));
        assert_eq!(telemetry_details(&database).len(), 1);

        observer.shutdown();

        assert!(
            !observer.is_observing(1),
            "shutdown releases the owned session"
        );
        thread::sleep(Duration::from_millis(150));
        assert_eq!(
            telemetry_details(&database).len(),
            1,
            "no thread survives shutdown to append again"
        );
    }

    /// KAN-T130-AC1: the production loop's own accounting, driven
    /// cycle by cycle on the test thread. Every failed cycle parks for
    /// the bounded delay its failure count calls for — one, two, four,
    /// then the policy maximum — with the count published per cycle,
    /// and no real sleep anywhere in the loop: the park hook records
    /// each delay and stops the loop in place of sleeping it out.
    #[test]
    fn the_loop_parks_the_bounded_backoff_its_failures_call_for() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_flapping_subscriptions(),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, driven_observation());
        let (handle, stop) =
            observer.test_driven_loop(&project(Some("kanban-main"), "/workspaces/kanban.seed"));
        let parked: Arc<Mutex<Vec<(Duration, u32)>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = parked.clone();

        handle.run_parking(|delay| {
            let mut seen = recorded.lock().unwrap();
            seen.push((delay, observer.consecutive_failures(1)));
            if seen.len() >= 4 {
                stop.store(true, Ordering::Relaxed);
            }
        });

        assert_eq!(
            parked.lock().unwrap().as_slice(),
            &[
                (Duration::from_millis(10), 1),
                (Duration::from_millis(20), 2),
                (Duration::from_millis(40), 3),
                (Duration::from_millis(40), 4),
            ],
            "each failed cycle parks the doubling delay its count calls for, bounded at the maximum"
        );
        let state = binding_diagnostics(&observer);
        assert!(
            !state.connected,
            "the flapping run ends with no connection claimed"
        );
        assert_eq!(
            state.last_error,
            Some("disconnected".to_owned()),
            "the flapping drop is the reported failure"
        );
        assert_eq!(
            telemetry_details(&database).len(),
            0,
            "a run that never settles appends no telemetry"
        );
    }

    /// KAN-T94-AC2: stopping one Project's observation is
    /// synchronous — it joins the thread — and leaves the diagnostics
    /// back at the unobserved defaults rather than a stale connected
    /// or error state. The registration happens on the caller's
    /// thread, so no wait is needed on either side of the stop.
    #[test]
    fn stopping_observation_clears_the_live_diagnostics() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default(),
        );
        let database = migrated_database(&dir);
        let observer = HerdrObserver::with_observation(database, socket_root, driven_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        assert_eq!(
            binding_diagnostics(&observer).session_name.as_deref(),
            Some("kanban-main"),
            "registration reports the binding before any cycle finishes"
        );

        observer.stop_observing(1);

        assert!(
            !observer.is_observing(1),
            "the stop joins and releases the session"
        );
        let stopped = binding_diagnostics(&observer);
        assert!(!stopped.connected);
        assert_eq!(
            stopped.last_error, None,
            "a stopped observation leaves no stale error behind"
        );
        assert_eq!(
            stopped.last_snapshot_at, None,
            "a stopped observation claims no capture"
        );
    }

    /// KAN-T94-AC2: while a subscription holds, the diagnostics
    /// report the connected state with its capture clock and no
    /// error. The wait is on the settled capture's own telemetry row —
    /// the cause of the connected transition — never on a fixed
    /// budget, and the live state is inherently concurrent so this is
    /// the one transition observed while the observer thread runs.
    #[test]
    fn the_live_steady_state_reports_connected_with_its_snapshot_clock() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default(),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, driven_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);

        assert!(
            soon_enough(Duration::from_secs(2), || {
                !telemetry_details(&database).is_empty() && {
                    let state = binding_diagnostics(&observer);
                    state.connected
                        && state.last_error.is_none()
                        && state.last_snapshot_at == Some("2026-09-05T04:46:00Z".to_owned())
                }
            }),
            "the live steady state reports connected, error-free, with its capture clock"
        );

        observer.shutdown();
    }

    #[test]
    fn observer_backs_off_failed_connections_without_appending_rows() {
        let dir = TempDir::new().expect("a scratch directory is available");
        // No fixture: every connect fails on the missing socket.
        let socket_root = dir.path().join("sessions");
        let database = migrated_database(&dir);
        let observer = HerdrObserver::with_backoff(database.clone(), socket_root, fast_backoff());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(250));

        assert!(
            observer.is_observing(1),
            "failed connects keep observing under backoff"
        );
        assert_eq!(
            telemetry_details(&database).len(),
            0,
            "a failed connect appends no timeline row"
        );
        let diagnostics = LiveHerdrDiagnostics::new(&observer);
        let state = diagnostics.for_project(
            1,
            Some("kanban-main"),
            "/workspaces/kanban.seed",
            "kanban.seed",
        );
        assert!(!state.connected);
        assert!(
            state
                .last_error
                .expect("the failed connection is reported")
                .contains("not available")
        );

        observer.shutdown();
    }

    #[test]
    fn unresponsive_server_timeout_updates_diagnostics_and_reconnects() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default()
                .with_silent_handshake()
                .with_reconnect_script(SessionScript::default()),
        );
        let database = migrated_database(&dir);
        let tuning = ObservationTuning {
            backoff: BackoffPolicy::new(Duration::from_millis(500), Duration::from_millis(500)),
            settle: Duration::from_millis(100),
            io_timeout: Duration::from_millis(100),
        };
        let observer = HerdrObserver::with_observation(database.clone(), socket_root, tuning);
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        let diagnostics = LiveHerdrDiagnostics::new(&observer);

        assert!(
            soon_enough(Duration::from_secs(2), || {
                let state = diagnostics.for_project(
                    1,
                    Some("kanban-main"),
                    "/workspaces/kanban.seed",
                    "kanban.seed",
                );
                !state.connected
                    && state
                        .last_error
                        .as_deref()
                        .is_some_and(|error| error.contains("window"))
            }),
            "the timeout failure is reported through diagnostics before reconnect"
        );
        assert!(
            soon_enough(Duration::from_secs(3), || {
                let state = diagnostics.for_project(
                    1,
                    Some("kanban-main"),
                    "/workspaces/kanban.seed",
                    "kanban.seed",
                );
                state.connected && state.last_error.is_none()
            }),
            "the bounded reconnect path restores a live subscription"
        );
        assert!(
            soon_enough(Duration::from_secs(2), || telemetry_details(&database)
                .len()
                == 1),
            "only the reconnect snapshot lands after the timeout cycle"
        );

        observer.shutdown();
    }

    #[test]
    fn observer_appends_no_rows_when_subscription_never_succeeds() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_subscribe_error("session is sealed"),
        );
        let database = migrated_database(&dir);
        let observer = HerdrObserver::with_backoff(database.clone(), socket_root, fast_backoff());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(250));

        assert!(
            observer.is_observing(1),
            "failed subscriptions keep observing under backoff"
        );
        assert_eq!(
            telemetry_details(&database).len(),
            0,
            "a connection without a live subscription appends no snapshot row"
        );
        let diagnostics = LiveHerdrDiagnostics::new(&observer);
        let state = diagnostics.for_project(
            1,
            Some("kanban-main"),
            "/workspaces/kanban.seed",
            "kanban.seed",
        );
        assert!(!state.connected);
        assert!(
            state
                .last_error
                .expect("the refused subscription is reported")
                .contains("sealed")
        );

        observer.shutdown();
    }

    /// KAN-T94-AC2: the diagnostics report the Project's binding
    /// before any cycle runs — its named session and both
    /// workspaces, with no connection claimed and no error invented —
    /// and a Project nothing observes falls back to the same quiet
    /// defaults.
    #[test]
    fn diagnostics_report_the_binding_before_any_connection() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let database = migrated_database(&dir);
        let observer = HerdrObserver::with_observation(
            database,
            dir.path().join("sessions"),
            driven_observation(),
        );
        let _handle =
            observer.test_driven_handle(&project(Some("kanban-main"), "/workspaces/kanban.seed"));
        let diagnostics = LiveHerdrDiagnostics::new(&observer);

        let state = binding_diagnostics(&observer);
        assert_eq!(state.session_name.as_deref(), Some("kanban-main"));
        assert_eq!(state.product_workspace, "/workspaces/kanban.seed");
        assert_eq!(state.herdr_workspace, "kanban.seed");
        assert!(
            !state.connected,
            "no connection is claimed before one settles"
        );
        assert_eq!(state.last_error, None, "a quiet binding invents no error");
        assert_eq!(
            state.last_snapshot_at, None,
            "no capture is claimed before one settles"
        );

        let unobserved = diagnostics.for_project(7, None, "/workspaces/other.seed", "other.seed");
        assert!(!unobserved.connected);
        assert_eq!(unobserved.last_error, None);
        assert_eq!(unobserved.last_snapshot_at, None);
    }

    /// KAN-T94-AC2: a connect that cannot reach the session socket
    /// reports the socket's absence. Driven on the test thread, the
    /// cycle returns the moment the refusal is decided: no sleep, no
    /// spawned observer.
    #[test]
    fn connect_refusals_report_the_missing_socket() {
        let dir = TempDir::new().expect("a scratch directory is available");
        // No fixture: the session socket is simply not there.
        let database = migrated_database(&dir);
        let observer = HerdrObserver::with_observation(
            database.clone(),
            dir.path().join("sessions"),
            driven_observation(),
        );
        let handle =
            observer.test_driven_handle(&project(Some("kanban-main"), "/workspaces/kanban.seed"));
        let mut live_once = false;
        let mut deadlines = DeadlineMonitor::new(handle.session_tuning().1);

        assert!(
            !handle.observe_live(&mut live_once, &mut deadlines),
            "a connect to a missing socket never settles"
        );

        let state = binding_diagnostics(&observer);
        assert!(!state.connected);
        assert!(
            state
                .last_error
                .expect("the refusal is reported")
                .contains("not available"),
            "the missing socket is the reported failure"
        );
        assert_eq!(
            telemetry_details(&database).len(),
            0,
            "a refused connect appends no telemetry"
        );
    }

    /// KAN-T94-AC2: a refused subscription reports the session's own
    /// refusal word, synchronously through one driven cycle.
    #[test]
    fn refused_subscriptions_report_the_remote_refusal() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_subscribe_error("session is sealed"),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, driven_observation());
        let handle =
            observer.test_driven_handle(&project(Some("kanban-main"), "/workspaces/kanban.seed"));
        let mut live_once = false;
        let mut deadlines = DeadlineMonitor::new(handle.session_tuning().1);

        assert!(
            !handle.observe_live(&mut live_once, &mut deadlines),
            "a refused subscription never settles"
        );

        let state = binding_diagnostics(&observer);
        assert!(!state.connected);
        assert!(
            state
                .last_error
                .expect("the refusal is reported")
                .contains("sealed"),
            "the session's own refusal word is the reported failure"
        );
        assert_eq!(
            telemetry_details(&database).len(),
            0,
            "a refused subscription appends no telemetry"
        );
    }

    /// KAN-T94-AC2: a stream dropped inside the settle window reports
    /// the drop itself — the exact wording the operator surface
    /// serves, not a stale connected claim.
    #[test]
    fn streams_dropped_inside_the_settle_window_report_disconnected() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_flapping_subscriptions(),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, driven_observation());
        let handle =
            observer.test_driven_handle(&project(Some("kanban-main"), "/workspaces/kanban.seed"));
        let mut live_once = false;
        let mut deadlines = DeadlineMonitor::new(handle.session_tuning().1);

        assert!(
            !handle.observe_live(&mut live_once, &mut deadlines),
            "a subscription that drops inside the window never settles"
        );

        let state = binding_diagnostics(&observer);
        assert!(!state.connected);
        assert_eq!(
            state.last_error,
            Some("disconnected".to_owned()),
            "the dropped stream is reported as a disconnection"
        );
        assert_eq!(
            telemetry_details(&database).len(),
            0,
            "a subscription that never settles appends no telemetry"
        );
    }

    /// KAN-T94-AC2: a session that never answers reports the bounded
    /// I/O window, decided by the read deadline the cycle itself
    /// armed.
    #[test]
    fn silent_handshakes_report_the_bounded_window() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_silent_handshake(),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, driven_observation());
        let handle =
            observer.test_driven_handle(&project(Some("kanban-main"), "/workspaces/kanban.seed"));
        let mut live_once = false;
        let mut deadlines = DeadlineMonitor::new(handle.session_tuning().1);

        assert!(
            !handle.observe_live(&mut live_once, &mut deadlines),
            "a session that never answers never settles"
        );

        let state = binding_diagnostics(&observer);
        assert!(!state.connected);
        assert!(
            state
                .last_error
                .expect("the silent handshake is reported")
                .contains("window"),
            "the bounded I/O window is the reported failure"
        );
        assert_eq!(
            telemetry_details(&database).len(),
            0,
            "a silent handshake appends no telemetry"
        );
    }

    /// KAN-T94-AC2: a subscription that settles records its capture —
    /// the snapshot clock the diagnostics serve — and the stream's
    /// later drop is reported as a disconnection.
    #[test]
    fn a_settled_subscription_keeps_its_clock_and_reports_the_drop() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().close_after_hold(Duration::from_millis(500)),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, driven_observation());
        let handle =
            observer.test_driven_handle(&project(Some("kanban-main"), "/workspaces/kanban.seed"));
        let mut live_once = false;
        let mut deadlines = DeadlineMonitor::new(handle.session_tuning().1);

        assert!(
            handle.observe_live(&mut live_once, &mut deadlines),
            "a subscription held past the settle window settles before the scripted drop"
        );

        let details = telemetry_details(&database);
        assert_eq!(details.len(), 1);
        assert_eq!(details[0]["event"], json!("snapshot"));
        assert_eq!(details[0]["reason"], json!("startup"));
        let state = binding_diagnostics(&observer);
        assert_eq!(
            state.last_snapshot_at.as_deref(),
            Some("2026-09-05T04:46:00Z"),
            "the settled capture sets the snapshot clock"
        );
        assert!(
            !state.connected,
            "the drop is reported, never left stale-connected"
        );
        assert_eq!(
            state.last_error,
            Some("disconnected".to_owned()),
            "the steady stream's end is reported as a disconnection"
        );
    }

    /// KAN-T94-AC1, KAN-T94-AC2: after the drop the next cycle
    /// redials, lands its reconnect snapshot, and the new capture
    /// replaces the clock — the reconnect arc proven end to end, one
    /// synchronous cycle at a time.
    #[test]
    fn the_reconnect_cycle_lands_its_snapshot_and_advances_the_clock() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default()
                .close_after_hold(Duration::from_millis(500))
                .with_reconnect_script(
                    SessionScript::default()
                        .with_captured_at("2026-09-05T04:47:00Z")
                        .close_after_hold_every(Duration::from_millis(500)),
                ),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, driven_observation());
        let handle =
            observer.test_driven_handle(&project(Some("kanban-main"), "/workspaces/kanban.seed"));
        let mut live_once = false;
        let mut deadlines = DeadlineMonitor::new(handle.session_tuning().1);

        assert!(
            handle.observe_live(&mut live_once, &mut deadlines),
            "the first held subscription settles before its scripted drop"
        );
        assert!(
            handle.observe_live(&mut live_once, &mut deadlines),
            "the redial settles in its turn"
        );

        let reasons: Vec<_> = telemetry_details(&database)
            .iter()
            .filter(|detail| detail["event"] == json!("snapshot"))
            .map(|detail| detail["reason"].clone())
            .collect();
        assert_eq!(
            reasons,
            vec![json!("startup"), json!("reconnect")],
            "each settled cycle lands exactly its own snapshot"
        );
        let state = binding_diagnostics(&observer);
        assert_eq!(
            state.last_snapshot_at.as_deref(),
            Some("2026-09-05T04:47:00Z"),
            "the reconnect capture replaces the snapshot clock"
        );
        assert_eq!(
            state.last_error,
            Some("disconnected".to_owned()),
            "the second scripted drop is reported the same way"
        );
    }

    #[test]
    fn observer_reconnect_snapshot_lands_only_after_a_live_subscription() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            // The first connection settles — held past the settle
            // window — before it closes, so the second connection is a
            // genuine reconnect of a live session.
            SessionScript::default()
                .with_events(vec![json!({ "kind": "role.output", "text": "working" })])
                .close_after_hold(Duration::from_millis(300)),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(800));

        let reasons: Vec<_> = telemetry_details(&database)
            .iter()
            .filter(|detail| detail["event"] == json!("snapshot"))
            .map(|detail| detail["reason"].clone())
            .collect();
        assert_eq!(
            reasons,
            vec![json!("startup"), json!("reconnect")],
            "the reconnect snapshot lands after the second settled subscription, and only after it"
        );

        observer.shutdown();
    }

    #[test]
    fn flapping_subscriptions_neither_reset_backoff_nor_append() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_flapping_subscriptions(),
        );
        let database = migrated_database(&dir);
        // A settle window far beyond any flap, so no cycle can settle.
        let observer = HerdrObserver::with_observation(
            database.clone(),
            socket_root,
            ObservationTuning {
                backoff: fast_backoff(),
                settle: Duration::from_millis(500),
                io_timeout: Duration::from_millis(100),
            },
        );
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(1200));

        assert!(
            observer.is_observing(1),
            "flapping subscriptions keep observing under backoff"
        );
        assert!(
            telemetry_details(&database).len() <= 1,
            "a subscription that never settles appends no snapshot per cycle"
        );
        assert!(
            observer.consecutive_failures(1) >= 3,
            "flapping is a run of failures, not a reset per cycle"
        );

        observer.shutdown();
    }

    #[test]
    fn observer_threads_stop_on_core_shutdown() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let repository = dir.path().join("wave");
        std::fs::create_dir_all(repository.join(".git")).expect("the scratch repository exists");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "wave-main",
            "/workspaces/wave.seed",
            SessionScript::default(),
        );
        let core = crate::serve_with_herdr_sessions(dir.path(), socket_root)
            .expect("the core boots for the test");
        let mut client = crate::test_client::Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-wave" },
                "code": "WAVE",
                "name": "Wave pool",
                "repository": repository.to_str().expect("the path is UTF-8"),
                "seed_workspace": "/workspaces/wave.seed",
                "default_branch": "main",
                "herdr_session": "wave-main",
                "herdr_workspace": "wave.seed",
            }),
        );
        thread::sleep(Duration::from_millis(200));
        let observer = core.herdr.clone();
        assert!(
            observer.is_observing(1),
            "registration starts observation without a restart"
        );

        core.shutdown();

        assert!(
            !observer.is_observing(1),
            "core shutdown stops the owned observer and joins its thread"
        );
    }

    #[test]
    fn observer_stops_when_the_project_archives() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let repository = dir.path().join("wave");
        std::fs::create_dir_all(repository.join(".git")).expect("the scratch repository exists");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "wave-main",
            "/workspaces/wave.seed",
            SessionScript::default(),
        );
        let core = crate::serve_with_herdr_sessions(dir.path(), socket_root)
            .expect("the core boots for the test");
        let mut client = crate::test_client::Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-wave" },
                "code": "WAVE",
                "name": "Wave pool",
                "repository": repository.to_str().expect("the path is UTF-8"),
                "seed_workspace": "/workspaces/wave.seed",
                "default_branch": "main",
                "herdr_session": "wave-main",
                "herdr_workspace": "wave.seed",
            }),
        );
        // The startup snapshot row lands in its own time, not in a
        // fixed budget a loaded host can miss.
        assert!(
            soon_enough(Duration::from_secs(2), || {
                client.query_with(
                    "timeline.query",
                    json!({
                        "scope": { "project": 1 },
                        "kinds": ["telemetry"],
                    }),
                )["events"]
                    .as_array()
                    .is_some_and(|events| !events.is_empty())
            }),
            "the startup snapshot lands before the archive"
        );
        let observer = core.herdr.clone();
        assert!(observer.is_observing(1));

        client.command(
            "project.archive",
            json!({
                "mutation": { "optimistic_version": 1, "idempotency_key": "archive-wave" },
                "project_id": 1,
            }),
        );

        assert!(
            !observer.is_observing(1),
            "the landed archive releases the owned observer"
        );
        // The archive's post-commit release joins the observer thread
        // before the command returns, so no row can land after it;
        // no fixed wait is needed to prove one absent.
        let answer = client.query_with(
            "timeline.query",
            json!({
                "scope": { "project": 1 },
                "kinds": ["telemetry"],
            }),
        );
        assert_eq!(
            answer["events"].as_array().map(Vec::len),
            Some(1),
            "no snapshot lands after the archive stops observation"
        );

        core.shutdown();
    }

    /// Sleep in small steps until `condition` holds or `limit`
    /// elapses, reporting whether the condition was met. The step is
    /// coarse enough that several tests waiting in parallel do not
    /// busy-spin the shared host against each other.
    fn soon_enough(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(25));
        }
        condition()
    }

    /// Run `action` on a helper thread and report whether it finished
    /// within `limit`: a stop that could hang forever fails the test
    /// instead of hanging it.
    fn bounded_within(limit: Duration, action: impl FnOnce() + Send + 'static) -> bool {
        let finished = Arc::new(AtomicBool::new(false));
        let flag = finished.clone();
        thread::spawn(move || {
            action();
            flag.store(true, Ordering::SeqCst);
        });
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if finished.load(Ordering::SeqCst) {
                return true;
            }
            thread::sleep(Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn shutdown_during_the_snapshot_handshake_is_bounded() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_silent_handshake(),
        );
        let database = migrated_database(&dir);
        let observer = HerdrObserver::with_backoff(database, socket_root, fast_backoff());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        assert!(
            soon_enough(Duration::from_secs(2), || fixture.requests_seen() >= 1),
            "the observer reaches the handshake it blocks on"
        );

        assert!(
            bounded_within(Duration::from_secs(5), move || observer.shutdown()),
            "shutdown interrupts the blocked handshake instead of hanging on its join"
        );
    }

    #[test]
    fn archiving_through_dispatch_cannot_deadlock() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let repository = dir.path().join("wave");
        std::fs::create_dir_all(repository.join(".git")).expect("the scratch repository exists");
        let fixture = ScriptedSession::bind(
            &socket_root,
            "wave-main",
            "/workspaces/wave.seed",
            SessionScript::default().with_silent_handshake(),
        );
        let core = crate::serve_with_herdr_sessions(dir.path(), socket_root)
            .expect("the core boots for the test");
        let mut client = crate::test_client::Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-wave" },
                "code": "WAVE",
                "name": "Wave pool",
                "repository": repository.to_str().expect("the path is UTF-8"),
                "seed_workspace": "/workspaces/wave.seed",
                "default_branch": "main",
                "herdr_session": "wave-main",
                "herdr_workspace": "wave.seed",
            }),
        );
        assert!(
            soon_enough(Duration::from_secs(2), || fixture.requests_seen() >= 1),
            "the observer blocks on the handshake before the archive dispatches"
        );

        // The archive must return even though its observer is blocked
        // on the handshake read: the release joins outside the write
        // span and the registered duplicate interrupts the read.
        assert!(
            bounded_within(Duration::from_secs(5), move || {
                client.command(
                    "project.archive",
                    json!({
                        "mutation": { "optimistic_version": 1, "idempotency_key": "archive-wave" },
                        "project_id": 1,
                    }),
                );
            }),
            "the archive returns instead of deadlocking on its observer"
        );

        let mut after = crate::test_client::Client::connect(core.socket_path());
        let listed = after.query("project.list");
        assert_eq!(
            listed["projects"][0]["archived"],
            json!(true),
            "the archive that returned also landed"
        );

        core.shutdown();
    }

    /// KAN-T42 review: Coordinator wakes enqueue from the command
    /// gate without socket I/O and deliver through the observation
    /// worker, in commit order, over the settled session connection.
    #[test]
    fn coordinator_wakes_deliver_through_the_worker_in_commit_order() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_wake_accepted(true),
        );
        let database = migrated_database(&dir);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        // The wakes ride the settled subscription, so the startup
        // snapshot — their carrier's proof — comes first.
        assert!(
            soon_enough(Duration::from_secs(5), || !telemetry_details(&database)
                .is_empty()),
            "the observation settles before the wakes enqueue"
        );

        for id in [7u64, 8, 9] {
            CoordinatorWake::wake(
                observer.as_ref(),
                CoordinatorWakeRequest {
                    project_id: 1,
                    dispatch_request_id: id,
                    seed_workspace: "/workspaces/kanban.seed".to_owned(),
                    herdr_workspace: "kanban.seed".to_owned(),
                    herdr_session: Some("kanban-main".to_owned()),
                },
            );
        }

        assert!(
            soon_enough(Duration::from_secs(5), || delivered_wake_ids(&fixture)
                == vec![7, 8, 9]),
            "the worker delivers every wake, in commit order, over the session socket"
        );

        observer.shutdown();
    }

    /// KAN-T42 review: the wake inbox is bounded — a full inbox
    /// refuses new wakes without ever blocking the caller — and a
    /// Project nothing observes drops its wake the same way, because
    /// the Dispatch Request behind it is durable either way.
    #[test]
    fn a_full_wake_inbox_refuses_new_wakes_without_blocking() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let database = migrated_database(&dir);
        let observer = HerdrObserver::with_observation(
            database,
            dir.path().join("sessions"),
            fast_observation(),
        );
        // A still observation with no thread: nothing drains the
        // inbox while the test fills it.
        observer.sessions.lock().unwrap().insert(
            1,
            Observation {
                stop: Arc::new(AtomicBool::new(true)),
                socket: Arc::new(Mutex::new(None)),
                wakes: Arc::new(Mutex::new(VecDeque::new())),
                handle: None,
            },
        );

        let sender = observer.clone();
        assert!(
            bounded_within(Duration::from_secs(2), move || {
                for id in 1..=(WAKE_INBOX_CAPACITY as u64 + 4) {
                    CoordinatorWake::wake(
                        sender.as_ref(),
                        CoordinatorWakeRequest {
                            project_id: 1,
                            dispatch_request_id: id,
                            seed_workspace: "/workspaces/kanban.seed".to_owned(),
                            herdr_workspace: "kanban.seed".to_owned(),
                            herdr_session: Some("kanban-main".to_owned()),
                        },
                    );
                }
                // A Project nothing observes drops its wake the same
                // bounded way.
                CoordinatorWake::wake(
                    sender.as_ref(),
                    CoordinatorWakeRequest {
                        project_id: 9,
                        dispatch_request_id: 99,
                        seed_workspace: "/workspaces/kanban.seed".to_owned(),
                        herdr_workspace: "kanban.seed".to_owned(),
                        herdr_session: None,
                    },
                );
            }),
            "no wake enqueue blocks, however full the inbox"
        );

        let queued: Vec<u64> = observer
            .pending_wakes(1)
            .iter()
            .map(|delivery| delivery.dispatch_request_id)
            .collect();
        assert_eq!(
            queued,
            (1..=(WAKE_INBOX_CAPACITY as u64)).collect::<Vec<_>>(),
            "the inbox keeps the oldest wakes and refuses the new"
        );
        assert!(
            observer.pending_wakes(9).is_empty(),
            "an unobserved Project queues nothing"
        );
    }

    /// The Dispatch Request identities of every wake the session
    /// recorded, in arrival order.
    fn delivered_wake_ids(fixture: &ScriptedSession) -> Vec<u64> {
        fixture
            .recorded_requests()
            .into_iter()
            .filter_map(|request| match request {
                HerdrRequest::Wake {
                    role,
                    dispatch_request_id,
                } if role == COORDINATOR_ROLE => Some(dispatch_request_id),
                _ => None,
            })
            .collect()
    }

    /// Seed the execution Profile and one open Ticket for
    /// Project 1 straight into the core's database, returning the
    /// Ticket's identity.
    fn seed_dispatch_ticket(dir: &TempDir) -> u64 {
        let conn = rusqlite::Connection::open(dir.path().join("kanban.sqlite"))
            .expect("the core database reopens");
        conn.execute(
            "INSERT INTO execution_profiles
                 (name, harness, model, effort, usage_pool, fallback, retired, version)
             VALUES ('standard', 'claude-code', 'opus', 'high', 'operator', NULL, 0, 1)",
            rusqlite::params![],
        )
        .expect("the fixture Profile lands");
        conn.execute(
            "INSERT INTO tickets
                 (project_id, number, kind, priority, state, title, criteria,
                  subtype, mode, completion, profile, version)
             VALUES (1, 1, 'task', 'normal', 'draft', 'One slice', '[]',
                     'operational', 'human', '[\"done\"]', 'standard', 1)",
            rusqlite::params![],
        )
        .expect("the fixture Ticket lands");
        conn.last_insert_rowid()
            .try_into()
            .expect("the Ticket identity fits")
    }

    /// KAN-T42 review: through the serving core, a landed Dispatch
    /// Request still wakes the Coordinator over the session socket —
    /// delivered by the observation worker, off the command gate.
    #[test]
    fn dispatch_wakes_the_coordinator_over_the_session_socket() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let repository = dir.path().join("wave");
        std::fs::create_dir_all(repository.join(".git")).expect("the scratch repository exists");
        let fixture = ScriptedSession::bind(
            &socket_root,
            "wave-main",
            "/workspaces/wave.seed",
            SessionScript::default().with_wake_accepted(true),
        );
        let core = crate::serve_with_herdr_sessions(dir.path(), socket_root)
            .expect("the core boots for the test");
        let mut client = crate::test_client::Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-wave" },
                "code": "WAVE",
                "name": "Wave pool",
                "repository": repository.to_str().expect("the path is UTF-8"),
                "seed_workspace": "/workspaces/wave.seed",
                "default_branch": "main",
                "herdr_session": "wave-main",
                "herdr_workspace": "wave.seed",
            }),
        );
        // The wake rides the settled subscription, so the startup
        // snapshot comes first.
        assert!(
            soon_enough(Duration::from_secs(5), || {
                client.query_with(
                    "timeline.query",
                    json!({
                        "scope": { "project": 1 },
                        "kinds": ["telemetry"],
                    }),
                )["events"]
                    .as_array()
                    .is_some_and(|events| !events.is_empty())
            }),
            "the observation settles before dispatch"
        );
        let ticket = seed_dispatch_ticket(&dir);

        let created = client.command(
            "dispatch.request",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "dispatch-wave" },
                "ticket_id": ticket,
            }),
        );
        let id = created["id"]
            .as_u64()
            .expect("the Dispatch Request identity is a number");

        assert!(
            soon_enough(Duration::from_secs(5), || {
                delivered_wake_ids(&fixture) == vec![id]
            }),
            "the worker delivers the Coordinator wake over the session socket"
        );
        let queue = client.query_with("dispatch.queue", json!({ "project_id": 1 }));
        assert_eq!(
            queue["requests"].as_array().map(Vec::len),
            Some(1),
            "the Dispatch Request is durable and queued behind its wake"
        );

        core.shutdown();
    }

    /// KAN-T42 review: a Coordinator wake the session never answers
    /// cannot stall the global command gate — the dispatch that
    /// queued it, and every unrelated command after it, return while
    /// the observation worker alone carries the blocked wake.
    #[test]
    fn an_unresponsive_coordinator_wake_cannot_stall_unrelated_commands() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let repository = dir.path().join("wave");
        std::fs::create_dir_all(repository.join(".git")).expect("the scratch repository exists");
        // The session answers nothing: every handshake, and every
        // wake, blocks until its I/O window closes.
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "wave-main",
            "/workspaces/wave.seed",
            SessionScript::default().with_silent_handshake(),
        );
        let core = crate::serve_with_herdr_sessions(dir.path(), socket_root)
            .expect("the core boots for the test");
        let mut registration = crate::test_client::Client::connect(core.socket_path());
        registration.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-wave" },
                "code": "WAVE",
                "name": "Wave pool",
                "repository": repository.to_str().expect("the path is UTF-8"),
                "seed_workspace": "/workspaces/wave.seed",
                "default_branch": "main",
                "herdr_session": "wave-main",
                "herdr_workspace": "wave.seed",
            }),
        );
        let socket = core.socket_path().to_path_buf();
        let ticket = seed_dispatch_ticket(&dir);

        // Before the fix this held the global gate for the session's
        // whole handshake window: the wake used to dial from the
        // command path. Now the command only enqueues.
        let dispatch_socket = socket.clone();
        assert!(
            bounded_within(Duration::from_secs(2), move || {
                let mut client = crate::test_client::Client::connect(&dispatch_socket);
                client.command(
                    "dispatch.request",
                    json!({
                        "mutation": { "optimistic_version": 0, "idempotency_key": "dispatch-wave" },
                        "ticket_id": ticket,
                    }),
                );
            }),
            "the dispatch returns even though its wake can never be answered"
        );

        assert!(
            bounded_within(Duration::from_secs(2), move || {
                let mut client = crate::test_client::Client::connect(&socket);
                client.command(
                    "comment.create",
                    json!({
                        "mutation": { "optimistic_version": 0, "idempotency_key": "comment-wave" },
                        "project_id": 1,
                        "target": { "kind": "ticket", "id": "wave-1" },
                        "text": "unrelated to any Coordinator wake",
                    }),
                );
            }),
            "an unrelated command lands while the wake is still pending"
        );

        // The request is durable with or without its wake: nothing
        // about the stalled wake rolled the dispatch back.
        let mut reader = crate::test_client::Client::connect(core.socket_path());
        let queue = reader.query_with("dispatch.queue", json!({ "project_id": 1 }));
        assert_eq!(
            queue["requests"].as_array().map(Vec::len),
            Some(1),
            "the Dispatch Request stays queued while its wake goes unanswered"
        );

        core.shutdown();
    }

    /// KAN-T41-AC1: the observer compares full session state on the
    /// Project's reconciliation interval and appends the difference it
    /// finds as telemetry.
    #[test]
    fn reconciliation_appends_difference_events_on_its_interval() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_snapshot_states(vec![
                json!({ "roles": [] }),
                json!({ "roles": [{ "name": "implementer" }] }),
            ]),
        );
        let database = migrated_database(&dir);
        seed_herdr_settings(&database);
        retune_herdr_settings(&database, |request| {
            request.reconciliation_interval_secs = 1;
        });
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);

        assert!(
            soon_enough(Duration::from_secs(6), || {
                telemetry_details(&database)
                    .iter()
                    .any(|detail| detail["event"] == json!("reconciliation"))
            }),
            "the interval elapses and the difference lands"
        );

        let differences: Vec<_> = telemetry_details(&database)
            .into_iter()
            .filter(|detail| detail["event"] == json!("reconciliation"))
            .collect();
        assert_eq!(
            differences.len(),
            1,
            "the changed state is reported once per difference"
        );
        assert_eq!(differences[0]["source"], json!("herdr"));
        assert_eq!(
            differences[0]["changes"],
            json!([
                { "op": "changed", "key": "roles", "from": [], "to": [{ "name": "implementer" }] }
            ]),
            "the row carries what the whole-session comparison found"
        );
        assert!(
            fixture.requests_seen() >= 3,
            "the comparison captured through the session socket"
        );

        observer.shutdown();
    }

    /// KAN-T41-AC1: a comparison that finds nothing appends nothing.
    #[test]
    fn reconciliation_appends_nothing_while_state_is_unchanged() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default(),
        );
        let database = migrated_database(&dir);
        seed_herdr_settings(&database);
        retune_herdr_settings(&database, |request| {
            request.reconciliation_interval_secs = 1;
        });
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(2_500));

        assert!(
            fixture.requests_seen() >= 3,
            "whole-session captures happened on the interval"
        );
        assert_eq!(
            telemetry_details(&database).len(),
            1,
            "unchanged state appends no difference row, however often it is compared"
        );

        observer.shutdown();
    }

    /// KAN-T41-AC2: with the settings a Project starts with, the
    /// whole-session polling fallback stays off: nothing polls the
    /// session before the five-minute reconciliation interval.
    #[test]
    fn polling_fallback_stays_off_for_default_settings() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default(),
        );
        let database = migrated_database(&dir);
        seed_herdr_settings(&database);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(600));

        assert_eq!(
            fixture.requests_seen(),
            2,
            "only the handshake's snapshot and subscribe reached the session"
        );
        assert_eq!(telemetry_details(&database).len(), 1);

        observer.shutdown();
    }

    /// KAN-T41-AC2: a Project can opt into the fallback while its
    /// session is under observation, and the faster whole-session
    /// cadence starts without a reconnect.
    #[test]
    fn polling_fallback_opts_in_live_without_a_reconnect() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_snapshot_states(vec![
                json!({ "roles": [] }),
                json!({ "roles": [{ "name": "reviewer" }] }),
            ]),
        );
        let database = migrated_database(&dir);
        seed_herdr_settings(&database);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        thread::sleep(Duration::from_millis(300));
        retune_herdr_settings(&database, |request| {
            request.polling_fallback_enabled = true;
            request.polling_fallback_interval_secs = 1;
        });

        assert!(
            soon_enough(Duration::from_secs(6), || {
                telemetry_details(&database)
                    .iter()
                    .any(|detail| detail["event"] == json!("reconciliation"))
            }),
            "the opted-in fallback polls the whole session and reports the difference"
        );
        let snapshots: Vec<_> = telemetry_details(&database)
            .into_iter()
            .filter(|detail| detail["event"] == json!("snapshot"))
            .collect();
        assert_eq!(
            snapshots.len(),
            1,
            "the fallback cadence took effect on the live connection, without a reconnect"
        );

        observer.shutdown();
    }

    /// KAN-T41-AC3: a role whose observed output goes quiet past the
    /// Project's stall deadline raises an attention signal from the
    /// production observation path, under the deadlines the Project's
    /// settings call for.
    #[test]
    fn a_stalled_role_emits_its_attention_signal_from_observation() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_events(vec![json!({
                "kind": "role.output",
                "role": "implementer",
                "text": "working"
            })]),
        );
        let database = migrated_database(&dir);
        seed_herdr_settings(&database);
        retune_herdr_settings(&database, |request| {
            request.stall_deadline_secs = 1;
        });
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);

        assert!(
            soon_enough(Duration::from_secs(6), || {
                observer
                    .attention_signals(1)
                    .iter()
                    .any(|signal| signal.reason == STALL_DEADLINE_REASON)
            }),
            "the quiet role breaches the stall deadline and observation emits the signal"
        );

        let signal = observer
            .attention_signals(1)
            .into_iter()
            .find(|signal| signal.reason == STALL_DEADLINE_REASON)
            .expect("the emitted stall signal is retained");
        assert_eq!(signal.project_id, 1);
        assert_eq!(signal.detail["deadline"], json!("stall"));
        assert_eq!(signal.detail["role"], json!("implementer"));
        assert_eq!(signal.detail["deadline_secs"], json!(1));

        observer.shutdown();
    }

    /// KAN-T41-AC3: a role that settled without its result breaches
    /// the missing-result deadline, not the stall deadline.
    #[test]
    fn a_settled_role_missing_its_result_emits_the_missing_result_signal() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_events(vec![json!({
                "kind": "role.settled",
                "role": "implementer"
            })]),
        );
        let database = migrated_database(&dir);
        seed_herdr_settings(&database);
        retune_herdr_settings(&database, |request| {
            request.missing_result_deadline_secs = 1;
        });
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);

        assert!(
            soon_enough(Duration::from_secs(6), || {
                observer
                    .attention_signals(1)
                    .iter()
                    .any(|signal| signal.reason == MISSING_RESULT_DEADLINE_REASON)
            }),
            "a settled role without a result breaches the missing-result deadline"
        );

        let reasons: Vec<_> = observer
            .attention_signals(1)
            .into_iter()
            .map(|signal| signal.reason)
            .collect();
        assert!(
            reasons
                .iter()
                .all(|reason| reason == MISSING_RESULT_DEADLINE_REASON),
            "a settled role faces the missing-result deadline alone: {reasons:?}"
        );

        observer.shutdown();
    }

    /// KAN-T41-AC3: a role whose result was observed faces no
    /// deadline, so a quiet session with a finished role emits
    /// nothing.
    #[test]
    fn a_role_whose_result_was_observed_emits_no_deadline_signal() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            // The captures keep listing the role, so only the observed
            // result can retire its deadlines.
            SessionScript::default()
                .with_snapshot_states(vec![json!({ "roles": [{ "name": "implementer" }] })])
                .with_events(vec![
                    json!({ "kind": "role.output", "role": "implementer", "text": "working" }),
                    json!({ "kind": "role.settled", "role": "implementer" }),
                    json!({ "kind": "role.result", "role": "implementer", "outcome": "done" }),
                ]),
        );
        let database = migrated_database(&dir);
        seed_herdr_settings(&database);
        retune_herdr_settings(&database, |request| {
            request.reconciliation_interval_secs = 1;
            request.stall_deadline_secs = 1;
            request.missing_result_deadline_secs = 1;
        });
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);

        // The observed result, not a fixed wait, is the premise: the
        // signal cannot be checked before the monitor has seen it.
        assert!(
            soon_enough(Duration::from_secs(6), || {
                telemetry_details(&database)
                    .iter()
                    .any(|detail| detail["event"] == json!("role.result"))
            }),
            "the role's result was observed"
        );
        // Two whole-session captures past the result prove the quiet
        // session was re-evaluated across a full stall window — every
        // capture follows at least one evaluation — with nothing to
        // report.
        let captured = fixture.requests_seen();
        assert!(
            soon_enough(Duration::from_secs(6), || {
                fixture.requests_seen() >= captured + 2
            }),
            "the session was captured and re-evaluated past the deadline"
        );
        assert!(
            observer.attention_signals(1).is_empty(),
            "an observed result retires both deadlines"
        );

        observer.shutdown();
    }

    /// KAN-T41-AC3: tightened deadline settings apply to the live
    /// observation without a reconnect, like the reconciliation
    /// cadence does.
    #[test]
    fn deadline_settings_tighten_live_without_a_reconnect() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default().with_events(vec![json!({
                "kind": "role.output",
                "role": "implementer",
                "text": "working"
            })]),
        );
        let database = migrated_database(&dir);
        seed_herdr_settings(&database);
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);
        // The role's output must already be observed under the seeded
        // stall deadline of an hour — only the tightened one can ever
        // breach — so the retune proves the live path, not a fresh
        // connect.
        assert!(
            soon_enough(Duration::from_secs(2), || {
                telemetry_details(&database)
                    .iter()
                    .any(|detail| detail["payload"]["text"] == json!("working"))
            }),
            "the role's output is observed under the seeded deadline before the retune"
        );
        retune_herdr_settings(&database, |request| {
            request.stall_deadline_secs = 1;
        });

        assert!(
            soon_enough(Duration::from_secs(6), || {
                observer
                    .attention_signals(1)
                    .iter()
                    .any(|signal| signal.reason == STALL_DEADLINE_REASON)
            }),
            "the tightened stall deadline applies to the live observation"
        );

        observer.shutdown();
    }

    /// KAN-T41-AC3, reconnect recovery: the settled capture a
    /// reconnect takes is authoritative, so a role that disappeared
    /// inside the disconnected gap retires its deadline instead of
    /// phantom-breaching forever, while a role the capture still
    /// lists keeps its genuine breach.
    #[test]
    fn roles_that_disappear_across_a_reconnect_retire_their_deadlines() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default()
                .with_events(vec![
                    json!({ "kind": "role.output", "role": "reviewer", "text": "working" }),
                    json!({ "kind": "role.output", "role": "ghost", "text": "working" }),
                ])
                .close_after_hold(Duration::from_millis(300))
                .with_reconnect_script(
                    SessionScript::default()
                        .with_snapshot_states(vec![json!({ "roles": [{ "name": "reviewer" }] })]),
                ),
        );
        let database = migrated_database(&dir);
        seed_herdr_settings(&database);
        retune_herdr_settings(&database, |request| {
            request.stall_deadline_secs = 1;
        });
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);

        // The listed role's stall is genuine — quiet since before the
        // gap — so its signal must survive the reconnect that
        // retired the disappeared role.
        assert!(
            soon_enough(Duration::from_secs(6), || {
                observer.attention_signals(1).iter().any(|signal| {
                    signal.reason == STALL_DEADLINE_REASON
                        && signal.detail["role"] == json!("reviewer")
                })
            }),
            "the role the reconnect capture still lists keeps its genuine stall signal"
        );
        let roles: Vec<_> = observer
            .attention_signals(1)
            .into_iter()
            .map(|signal| signal.detail["role"].clone())
            .collect();
        assert!(
            !roles.contains(&json!("ghost")),
            "a role that disappeared inside the gap never breaches: {roles:?}"
        );

        observer.shutdown();
    }

    /// Reconnect recovery keeps retention honest: a retired phantom
    /// breach stops re-reporting, so it cannot churn the
    /// retained-signal buffer and evict a genuine signal. The
    /// reconnect's delayed result retires the listed role mid-breach,
    /// then a burst of role-less events drives one evaluation each —
    /// enough to overflow the whole buffer were the disappeared role
    /// still breaching.
    #[test]
    fn retired_phantom_breaches_cannot_evict_genuine_signals() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "kanban-main",
            "/workspaces/kanban.seed",
            SessionScript::default()
                .with_events(vec![
                    json!({ "kind": "role.output", "role": "reviewer", "text": "working" }),
                    json!({ "kind": "role.output", "role": "ghost", "text": "working" }),
                ])
                .close_after_hold(Duration::from_millis(300))
                .with_reconnect_script(
                    SessionScript::default()
                        .with_snapshot_states(vec![json!({ "roles": [{ "name": "reviewer" }] })])
                        .with_delayed_events(Duration::from_millis(1_500))
                        .with_events({
                            let mut events = vec![json!({
                                "kind": "role.result",
                                "role": "reviewer",
                                "outcome": "done"
                            })];
                            events.extend(
                                (0..300).map(|i| json!({ "kind": "session.heartbeat", "i": i })),
                            );
                            events
                        }),
                ),
        );
        let database = migrated_database(&dir);
        seed_herdr_settings(&database);
        retune_herdr_settings(&database, |request| {
            request.stall_deadline_secs = 1;
        });
        let observer =
            HerdrObserver::with_observation(database.clone(), socket_root, fast_observation());
        observer.observe_projects(&[project(Some("kanban-main"), "/workspaces/kanban.seed")]);

        assert!(
            soon_enough(Duration::from_secs(10), || {
                telemetry_details(&database)
                    .iter()
                    .filter(|detail| detail["event"] == json!("session.heartbeat"))
                    .count()
                    == 300
            }),
            "every post-reconnect event was observed, driving an evaluation per event"
        );

        let roles: Vec<_> = observer
            .attention_signals(1)
            .into_iter()
            .map(|signal| signal.detail["role"].clone())
            .collect();
        assert!(
            !roles.contains(&json!("ghost")),
            "the disappeared role retired instead of re-breaching per event: {roles:?}"
        );
        assert!(
            roles.contains(&json!("reviewer")),
            "the genuine breach stays retained past the flood that would have evicted it"
        );

        observer.shutdown();
    }

    /// KAN-T41-AC3: the serving core — booted the way production
    /// boots it, registered and retuned through the served commands —
    /// emits the attention signal a breached deadline raises.
    #[test]
    fn the_serving_core_emits_attention_signals_on_deadline_breaches() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let socket_root = dir.path().join("sessions");
        let repository = dir.path().join("wave");
        std::fs::create_dir_all(repository.join(".git")).expect("the scratch repository exists");
        let _fixture = ScriptedSession::bind(
            &socket_root,
            "wave-main",
            "/workspaces/wave.seed",
            SessionScript::default().with_events(vec![json!({
                "kind": "role.output",
                "role": "implementer",
                "text": "working"
            })]),
        );
        let core = crate::serve_with_herdr_sessions(dir.path(), socket_root)
            .expect("the core boots for the test");
        let mut client = crate::test_client::Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation": { "optimistic_version": 0, "idempotency_key": "register-wave" },
                "code": "WAVE",
                "name": "Wave pool",
                "repository": repository.to_str().expect("the path is UTF-8"),
                "seed_workspace": "/workspaces/wave.seed",
                "default_branch": "main",
                "herdr_session": "wave-main",
                "herdr_workspace": "wave.seed",
            }),
        );
        let current = client.query_with("herdr.settings.get", json!({ "project_id": 1 }));
        let current = &current["settings"];
        client.command(
            "herdr.settings.update",
            json!({
                "mutation": {
                    "optimistic_version": current["version"],
                    "idempotency_key": "tighten-wave-stall"
                },
                "project_id": 1,
                "reconciliation_interval_secs": current["reconciliation_interval_secs"],
                "polling_fallback_enabled": current["polling_fallback_enabled"],
                "polling_fallback_interval_secs": current["polling_fallback_interval_secs"],
                "stall_deadline_secs": 1,
                "missing_result_deadline_secs": current["missing_result_deadline_secs"],
            }),
        );

        let observer = core.herdr.clone();
        assert!(
            soon_enough(Duration::from_secs(8), || {
                observer
                    .attention_signals(1)
                    .iter()
                    .any(|signal| signal.reason == STALL_DEADLINE_REASON)
            }),
            "the serving core's observation path emits the breach signal"
        );
        let signal = observer
            .attention_signals(1)
            .into_iter()
            .find(|signal| signal.reason == STALL_DEADLINE_REASON)
            .expect("the emitted stall signal is retained");
        assert_eq!(signal.detail["role"], json!("implementer"));
        assert_eq!(signal.detail["deadline_secs"], json!(1));

        core.shutdown();
    }
}
