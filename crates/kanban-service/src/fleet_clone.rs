//! The fleet branch-clone skill, wrapped (KAN-S6-US4). The `git
//! bc-add` family is the only sanctioned clone mechanism; this
//! adapter invokes it and nothing else, because the guards — the
//! conflict refusal, the ordering, the timeline rows — all live in
//! the application layer, which calls this port only after every
//! precondition has held. Every invocation runs under a bounded
//! deadline: an overdue skill is terminated together with its whole
//! subprocess tree, so a stalled fetch can never hold the Core
//! command gate until process death (KAN-T120).
//!
//! A failed invocation's stderr is diagnostics, not payload: the
//! skill's own output can carry ANSI colour, a runaway postadd dump,
//! or a secret a fetch picked up, and every byte of it would otherwise
//! reach the operator's error and the durable timeline unchanged. The
//! report is therefore stripped, scrubbed, and bounded here — at the
//! one boundary where skill output enters — so both surfaces that copy
//! it inherit clean text and no second redaction policy exists
//! anywhere else.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use kanban_app::FleetCloneTool;
use kanban_dto::ApiError;

use crate::redaction::Redactor;

/// How long one fleet clone skill may run while the Core command gate
/// is held: long enough for a hardlinked local clone plus the
/// Operator's configured post-add command, and short enough that a
/// stalled fetch can never pin every Core command until process death
/// (KAN-T120).
const SKILL_DEADLINE: Duration = Duration::from_secs(300);

/// How often a running skill is checked for completion. The check is
/// one cheap syscall, so a short poll keeps the deadline honest
/// without costing anything measurable.
const COMPLETION_POLL: Duration = Duration::from_millis(50);

/// How long the refusal text is still collected once the skill has
/// exited: a grandchild the skill detached can hold the error pipe
/// open after its parent is gone, so this drain is bounded like
/// everything else.
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// How many characters of a skill's report any surface may carry. The
/// fleet's own refusals are one line; everything past this bound is a
/// dump nobody can read anyway.
const REPORT_CHAR_BOUND: usize = 2_000;

/// What a capped report says in place of the rest of itself.
const TRUNCATED: &str = "... [truncated]";

/// What a withheld report says, in fixed words, when the managed
/// configuration cannot feed redaction: knowing nothing is not the
/// same as having nothing to scrub, so a report nobody can vouch for
/// must not be repeated — only named.
const UNREDACTABLE_REPORT: &str =
    "the skill's report is withheld: redaction knowledge is unavailable";

/// The signature the fleet skills' own `bc_abort` writes when they
/// refuse deliberately — an `Error:` line naming the refusing skill —
/// as opposed to the unformatted stderr a death by `set -e` leaves
/// behind. Colour is stripped before the match, so this is the human
/// text of a refusal, and it is the only signal separating a refusal
/// the caller caused from a skill that simply died.
const REFUSAL_MARKER: &str = "Error: git-bc-";

/// Run the fleet's `git bc-add` and `git bc-rm` skills locally, every
/// invocation bounded by a deadline (KAN-T120).
#[derive(Debug)]
pub struct LocalFleetCloneTool {
    /// The managed data directory whose configuration feeds redaction,
    /// re-read on every failed invocation so a secret added or rotated
    /// since the last one is still scrubbed.
    data_dir: PathBuf,
    /// The binary the skill is invoked through. Production always
    /// runs `git`; tests substitute a fixture binary, because the
    /// skill itself lives in the Operator's dotfiles rather than in a
    /// clean checkout.
    program: String,
    /// The deadline one invocation runs under.
    deadline: Duration,
}

impl Default for LocalFleetCloneTool {
    /// A tool with no managed configuration to scrub against: the
    /// fleet's own `git` skills under SKILL_DEADLINE. Production
    /// wires `new` with the managed data directory; this default is
    /// the test shape, for callers with no reports to scrub.
    fn default() -> Self {
        Self {
            data_dir: PathBuf::new(),
            program: "git".to_owned(),
            deadline: SKILL_DEADLINE,
        }
    }
}

impl LocalFleetCloneTool {
    /// A tool whose reports are scrubbed against the managed
    /// configuration under `data_dir`, with every invocation bounded
    /// by SKILL_DEADLINE.
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            program: "git".to_owned(),
            deadline: SKILL_DEADLINE,
        }
    }

    /// `git bc-add <source> <branch> <target>`: the target is always
    /// explicit, so the guarded path is the path that lands.
    fn add_arguments(source: &str, branch: &str, target: &str) -> Vec<String> {
        vec![source.to_owned(), branch.to_owned(), target.to_owned()]
    }

    /// `git bc-rm <clone-dir> -y`: the confirmation prompt is skipped
    /// because the guarded command already carries the operator's
    /// authority, while `-f` is never passed — the fleet's own
    /// refusal rules against base clones, dirty trees, and unique
    /// commits stay armed.
    fn remove_arguments(target: &str) -> Vec<String> {
        vec![target.to_owned(), "-y".to_owned()]
    }

    /// Run one fleet skill under the tool's deadline and report its
    /// refusal, failure, or overrun.
    fn run_skill(&self, skill: &str, arguments: Vec<String>) -> Result<(), ApiError> {
        let mut command = Command::new(&self.program);
        command.arg(skill).args(&arguments);
        run_to_deadline(command, skill, self.deadline, |skill, stderr| {
            self.report_failure(skill, stderr)
        })
    }

    /// Report one finished-but-failed skill invocation from its stderr:
    /// classified by the fleet's own refusal signature, colour
    /// stripped, secrets scrubbed through the one redaction policy the
    /// core already owns, and the result bounded. A configuration that
    /// cannot feed redaction withholds the report wholesale, because
    /// repeating text nobody can vouch for is how a secret reaches the
    /// error surface.
    fn report_failure(&self, skill: &str, stderr: &[u8]) -> ApiError {
        let report = strip_ansi(&String::from_utf8_lossy(stderr))
            .trim()
            .to_owned();
        // Classified before redaction: whether the caller caused the
        // outcome is a fact about the report, not its content, so even
        // a withheld report still names its class.
        let refusal = report.contains(REFUSAL_MARKER);
        let report = match Redactor::from_config(&self.data_dir) {
            Ok(redactor) => redactor.redact_text(&report).into_owned(),
            Err(source) => format!("{UNREDACTABLE_REPORT} ({})", source.reason()),
        };
        let report = cap_report(&report);
        if refusal {
            ApiError::invalid_request(&format!(
                "the fleet clone skill `git {skill}` refused: {report}"
            ))
        } else {
            ApiError::internal(&format!(
                "the fleet clone skill `git {skill}` failed: {report}"
            ))
        }
    }
}

impl FleetCloneTool for LocalFleetCloneTool {
    fn add_clone(&self, source: &str, path: &str, branch: &str) -> Result<(), ApiError> {
        self.run_skill("bc-add", Self::add_arguments(source, branch, path))
    }

    fn remove_clone(&self, path: &str) -> Result<(), ApiError> {
        self.run_skill("bc-rm", Self::remove_arguments(path))
    }
}

/// Run one skill invocation to completion under `deadline`, in a
/// process group of its own. A skill that overruns is killed together
/// with every subprocess it spawned, and its direct child reaped by
/// the wait that follows; the overrun is reported as an ordinary
/// refusal instead of a hang holding the Core command gate
/// (KAN-T120-AC1). A skill that finishes failing is reported through
/// `report_failure`, so its stderr meets the one stripping, redaction,
/// and bounding policy every other failure meets.
fn run_to_deadline(
    mut command: Command,
    skill: &str,
    deadline: Duration,
    report_failure: impl FnOnce(&str, &[u8]) -> ApiError,
) -> Result<(), ApiError> {
    let mut child = command
        // The skill reads no input from the core: a prompt must fail
        // fast, not wait on a terminal that never answers.
        .stdin(Stdio::null())
        // The wrapper reports only the refusal text; stdout is
        // discarded so a verbose skill cannot stall on a pipe nobody
        // drains.
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        // The skill leads a process group of its own, so the deadline
        // kill reaches every subprocess it spawned, not only the
        // binary itself.
        .process_group(0)
        .spawn()
        .map_err(|source| {
            ApiError::internal(&format!(
                "the fleet clone skill `git {skill}` could not run: {source}"
            ))
        })?;
    let refusal = drain_refusals(&mut child);
    let deadline_at = Instant::now() + deadline;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline_at => {
                thread::sleep(COMPLETION_POLL);
            }
            Ok(None) => {
                terminate_tree(&mut child);
                return Err(ApiError::internal(&format!(
                    "the fleet clone skill `git {skill}` overran its {deadline:?} deadline \
                     and was terminated"
                )));
            }
            Err(source) => {
                terminate_tree(&mut child);
                return Err(ApiError::internal(&format!(
                    "the fleet clone skill `git {skill}` could not be observed: {source}"
                )));
            }
        }
    };
    let refusal = refusal();
    if status.success() {
        return Ok(());
    }
    Err(report_failure(skill, refusal.as_bytes()))
}

/// Drain the skill's error pipe in the background and hand back a
/// collector for the refusal text. The collector waits no longer than
/// DRAIN_GRACE for the pipe to close: the skill has already exited by
/// then, but a grandchild it detached can keep the pipe open, and the
/// refusal is what landed, never a reason to hang.
fn drain_refusals(child: &mut Child) -> impl FnOnce() -> String + use<> {
    let stderr = child
        .stderr
        .take()
        .expect("stderr is piped for the refusal text");
    let drained = Arc::new(Mutex::new(Vec::new()));
    let sink = drained.clone();
    let reader = thread::spawn(move || {
        let mut stderr = stderr;
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        *sink.lock().expect("the drain lock is sound") = bytes;
    });
    move || {
        let closed_by = Instant::now() + DRAIN_GRACE;
        while !reader.is_finished() && Instant::now() < closed_by {
            thread::sleep(COMPLETION_POLL);
        }
        let bytes = drained.lock().expect("the drain lock is sound").clone();
        String::from_utf8_lossy(&bytes).trim().to_owned()
    }
}

/// Kill the skill's whole process group and reap its direct child.
/// The group, not the child alone, is the target: the skill spawns
/// its own subprocesses, and a fetch that survived the kill would
/// outlive the deadline that terminated it. SIGKILL, because a skill
/// that already ignored its whole deadline has nothing left to
/// negotiate.
fn terminate_tree(child: &mut Child) {
    // process_group(0) at spawn made the child its own group leader,
    // so its pid names the group.
    let group = child.id() as libc::pid_t;
    // A refusal here means the group is already gone; the wait reaps
    // whatever remains either way.
    unsafe {
        libc::killpg(group, libc::SIGKILL);
    }
    let _ = child.wait();
}

/// Removes ANSI escape sequences — the colour and cursor codes the
/// fleet skills wrap their output in — leaving the human text they
/// coloured. An escape that never terminates consumes the rest of the
/// report, because dropping diagnostics is survivable and emitting
/// raw escapes is not.
fn strip_ansi(report: &str) -> String {
    let mut stripped = String::with_capacity(report.len());
    let mut characters = report.chars();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            stripped.push(character);
            continue;
        }
        match characters.next() {
            // CSI: parameters and intermediates until the final byte.
            Some('[') => {
                for next in characters.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
            }
            // OSC: until BEL, or the ESC \ of string termination.
            Some(']') => {
                for next in characters.by_ref() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' {
                        characters.next();
                        break;
                    }
                }
            }
            // Every other escape is two characters wide.
            Some(_) => {}
            None => break,
        }
    }
    stripped
}

/// Bounds a report to [`REPORT_CHAR_BOUND`] characters, saying so when
/// it cuts. Applied after redaction so a secret straddling the bound
/// is scrubbed whole, never split into unrecognisable halves.
fn cap_report(report: &str) -> String {
    if report.chars().count() <= REPORT_CHAR_BOUND {
        return report.to_owned();
    }
    let bounded: String = report.chars().take(REPORT_CHAR_BOUND).collect();
    format!("{bounded}{TRUNCATED}")
}

#[cfg(test)]
mod reports {
    use kanban_dto::ErrorCode;
    use tempfile::TempDir;

    use super::LocalFleetCloneTool;
    use crate::redaction::{REDACTED, serialized_forms};

    const PLANTED_SECRET: &str = "kct_t121_planted_skill_secret";
    const PLANTED_QUOTED: &str = r#"t121 "quoted" skill secret"#;

    /// The refusal `bc_abort` writes: red-wrapped, deliberate, on
    /// stderr, and the shape every planted refusal below borrows.
    const REFUSAL_STDERR: &str = "\n \u{1b}[31mError: git-bc-rm: refusing to remove the clone holding unpushed work: /workspaces/kanban.fleet-t121\u{1b}[0m\n\n";

    /// A tool bound to a scratch data directory holding `config`.
    fn tool_with_config(config: Option<serde_json::Value>) -> (TempDir, LocalFleetCloneTool) {
        let dir = TempDir::new().expect("a scratch directory is available");
        if let Some(config) = config {
            std::fs::write(
                dir.path().join("config.json"),
                serde_json::to_string(&config).expect("the configuration serialises"),
            )
            .expect("the configuration is written");
        }
        let tool = LocalFleetCloneTool::new(dir.path().to_path_buf());
        (dir, tool)
    }

    #[test]
    fn ansi_escapes_never_reach_the_reported_failure() {
        let (_dir, tool) = tool_with_config(None);
        let stderr = "\u{1b}]0;window title\u{7}\u{1b}[31mError:\u{1b}[0m git died: \
                      \u{1b}[1;31mFatal:\u{1b}[0m could not read \u{1b}[2mFrom\u{1b}[22m the remote";

        let error = tool.report_failure("bc-add", stderr.as_bytes());

        assert!(
            !error.message.contains('\u{1b}'),
            "no escape byte may survive into the operator-visible report: {}",
            error.message
        );
        assert_eq!(
            error.message,
            "the fleet clone skill `git bc-add` failed: \
             Error: git died: Fatal: could not read From the remote",
            "the report carries exactly the human text, nothing the escapes wrapped"
        );
    }

    #[test]
    fn the_report_is_capped_to_a_bounded_length() {
        let (_dir, tool) = tool_with_config(None);
        let runaway: String = "x".repeat(50_000);
        let stderr = format!("{REFUSAL_STDERR}{runaway}");

        let error = tool.report_failure("bc-add", stderr.as_bytes());

        assert!(
            error.message.chars().count() <= 2_200,
            "a runaway skill report must stay bounded, found {} characters",
            error.message.chars().count()
        );
        assert!(
            error.message.contains("... [truncated]"),
            "the cap must say it cut the report: {}",
            error.message
        );
        assert!(
            error.message.contains("refusing to remove the clone"),
            "the cap keeps the head of the report, where the refusal lives: {}",
            error.message
        );
    }

    #[test]
    fn a_planted_secret_never_survives_the_report() {
        let (_dir, tool) = tool_with_config(Some(serde_json::json!({
            "mcp": { "install_token": PLANTED_SECRET },
            "backup_passphrase": PLANTED_QUOTED,
        })));
        let stderr = format!("{REFUSAL_STDERR}unlocked with {PLANTED_SECRET} and {PLANTED_QUOTED}");

        let error = tool.report_failure("bc-rm", stderr.as_bytes());

        for secret in [PLANTED_SECRET, PLANTED_QUOTED] {
            for form in serialized_forms(secret) {
                assert!(
                    !error.message.contains(&form),
                    "the planted form `{form}` must never reach the report: {}",
                    error.message
                );
            }
        }
        assert!(
            error.message.contains(REDACTED),
            "redaction leaves its marker behind: {}",
            error.message
        );
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn a_configuration_that_cannot_feed_redaction_withholds_the_report() {
        for (label, reason) in [
            ("a malformed configuration", "configuration_malformed"),
            ("an unreadable configuration", "configuration_unreadable"),
        ] {
            let dir = TempDir::new().expect("a scratch directory is available");
            let config_path = dir.path().join("config.json");
            match reason {
                "configuration_malformed" => {
                    std::fs::write(&config_path, "{ not json")
                        .expect("the malformed configuration is written");
                }
                _ => {
                    std::fs::create_dir(&config_path)
                        .expect("the unreadable configuration is planted");
                }
            }
            let tool = LocalFleetCloneTool::new(dir.path().to_path_buf());
            let stderr = format!("{REFUSAL_STDERR}unlocked with {PLANTED_SECRET} meanwhile");

            let error = tool.report_failure("bc-add", stderr.as_bytes());

            assert!(
                !error.message.contains(PLANTED_SECRET),
                "with {label}, the planted secret must never reach the report: {}",
                error.message
            );
            for withheld in [
                "refusing to remove the clone",
                "unlocked with",
                "meanwhile",
                "\u{1b}",
            ] {
                assert!(
                    !error.message.contains(withheld),
                    "with {label}, no report content may survive the withhold: {}",
                    error.message
                );
            }
            assert!(
                error.message.contains("redaction knowledge is unavailable"),
                "with {label}, the withhold names why in fixed words: {}",
                error.message
            );
            assert!(
                error.message.contains(reason),
                "with {label}, the withhold carries the reason `{reason}`: {}",
                error.message
            );
        }
    }

    #[test]
    fn an_absent_configuration_reports_the_skill_text_untouched() {
        let (_dir, tool) = tool_with_config(None);

        let error = tool.report_failure("bc-rm", REFUSAL_STDERR.as_bytes());

        assert!(
            error.message.contains("refusing to remove the clone"),
            "an absent configuration is not a failure and withholds nothing: {}",
            error.message
        );
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    /// The fleet skills refuse through `bc_abort`, whose deliberate
    /// `Error:` line is the only signal separating a refusal the
    /// caller caused from a skill that simply died.
    #[test]
    fn a_deliberate_refusal_is_classified_as_the_callers() {
        let (_dir, tool) = tool_with_config(None);

        let error = tool.report_failure("bc-rm", REFUSAL_STDERR.as_bytes());

        assert_eq!(
            error.code,
            ErrorCode::InvalidRequest,
            "the skill's own refusal is the caller's refusal, not a core fault: {}",
            error.message
        );
        assert!(
            error.message.contains("refused:"),
            "the report names a refusal: {}",
            error.message
        );
    }

    /// A death by `set -e` — a failed fetch, a broken postadd —
    /// leaves git's own stderr and no deliberate report, so it must
    /// stay a tool failure and never pose as an invalid request.
    #[test]
    fn a_death_without_the_deliberate_report_stays_a_tool_failure() {
        let (_dir, tool) = tool_with_config(None);
        let stderr = "fatal: could not read from remote repository";

        let error = tool.report_failure("bc-add", stderr.as_bytes());

        assert_eq!(
            error.code,
            ErrorCode::Internal,
            "a skill that died is a tool failure, never an invalid request: {}",
            error.message
        );
        assert!(
            error.message.contains("failed:"),
            "the report names a failure: {}",
            error.message
        );
        assert!(
            error
                .message
                .contains("could not read from remote repository"),
            "the human text still travels: {}",
            error.message
        );
    }

    /// A withheld report still classifies: the operator learns the
    /// invocation was refused, without learning what it said.
    #[test]
    fn a_withheld_refusal_classifies_without_content() {
        let dir = TempDir::new().expect("a scratch directory is available");
        std::fs::write(dir.path().join("config.json"), "{ not json")
            .expect("the malformed configuration is written");
        let tool = LocalFleetCloneTool::new(dir.path().to_path_buf());

        let error = tool.report_failure("bc-add", REFUSAL_STDERR.as_bytes());

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("refused:"),
            "the withhold sits inside a refusal, not behind it: {}",
            error.message
        );
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};

    use kanban_dto::{ApiError, ErrorCode};
    use tempfile::TempDir;

    use super::{LocalFleetCloneTool, run_to_deadline};

    /// The plain reporter the deadline machinery is proven under:
    /// stripping, redaction, and bounding are the tool's own policy,
    /// proven by the `reports` module, so these tests report the
    /// skill's text untouched.
    fn plain_failure(skill: &str, stderr: &[u8]) -> ApiError {
        ApiError::internal(&format!(
            "the fleet clone skill `git {skill}` refused: {}",
            String::from_utf8_lossy(stderr).trim()
        ))
    }

    /// The argument lists are asserted without running the skill: it
    /// lives in the Operator's dotfiles, not in a clean checkout, so
    /// the wrapper's contract is the arguments and never a live
    /// invocation here.
    #[test]
    fn add_names_the_source_the_branch_and_the_explicit_target() {
        assert_eq!(
            LocalFleetCloneTool::add_arguments(
                "/workspaces/kanban.seed",
                "fleet/kan-t34",
                "/workspaces/kanban.fleet-kan-t34",
            ),
            vec![
                "/workspaces/kanban.seed".to_owned(),
                "fleet/kan-t34".to_owned(),
                "/workspaces/kanban.fleet-kan-t34".to_owned(),
            ],
            "the guarded path is the target git bc-add lands"
        );
    }

    #[test]
    fn remove_skips_the_prompt_but_never_forces() {
        let arguments = LocalFleetCloneTool::remove_arguments("/workspaces/kanban.fleet-kan-t34");

        assert_eq!(
            arguments,
            vec![
                "/workspaces/kanban.fleet-kan-t34".to_owned(),
                "-y".to_owned()
            ]
        );
        assert!(
            !arguments
                .iter()
                .any(|argument| argument == "-f" || argument == "--force"),
            "the fleet's own refusal rules must stay armed"
        );
    }

    /// Write an executable fixture that behaves like a stalled fleet
    /// skill: it records its own pid and one background child's, then
    /// waits on a sleep that outlives any deadline a test would set.
    /// The recorded pids let a test prove the whole owned tree was
    /// reaped, not only the binary the runner spawned.
    fn stalled_skill(dir: &Path) -> String {
        let script = dir.join("stalled-skill");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nsleep 600 &\necho $! > '{}'\nwait\n",
                dir.join("skill.pid").display(),
                dir.join("skill-child.pid").display(),
            ),
        )
        .expect("the stalled skill fixture is written");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .expect("the stalled skill fixture is executable");
        script.to_str().expect("the path is UTF-8").to_owned()
    }

    fn read_pid(dir: &Path, name: &str) -> u32 {
        fs::read_to_string(dir.join(name))
            .expect("the fixture recorded a pid")
            .trim()
            .parse()
            .expect("the recorded pid is a number")
    }

    /// Whether `pid` no longer exists, waiting briefly for the
    /// operating system to reap what the kill terminated. Signal 0
    /// still reaches a zombie, so success here proves the process was
    /// reaped, not merely killed.
    fn reaped(pid: u32) -> bool {
        let by = Instant::now() + Duration::from_secs(5);
        loop {
            let gone = unsafe { libc::kill(pid as libc::pid_t, 0) } == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            if gone {
                return true;
            }
            assert!(
                Instant::now() < by,
                "process {pid} must be terminated and reaped"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// KAN-T120-AC1: an overdue skill is terminated and its owned
    /// subprocess tree reaped — the direct child by the runner's own
    /// wait, the orphaned grandchild by the operating system — while
    /// the runner answers with a refusal instead of hanging.
    #[test]
    fn an_overdue_skill_is_terminated_and_its_tree_reaped() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let stalled = stalled_skill(dir.path());
        let mut command = Command::new(&stalled);
        command.args([
            "bc-add",
            "/repositories/kanban",
            "fleet/kan-t120",
            "/workspaces/kanban.fleet-t120",
        ]);
        let started = Instant::now();

        let error = run_to_deadline(command, "bc-add", Duration::from_millis(300), plain_failure)
            .expect_err("the overdue skill is refused");

        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "the runner answers in bounded time, took {elapsed:?}"
        );
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(
            error.message.contains("bc-add"),
            "the refusal names the skill: {}",
            error.message
        );
        assert!(
            error.message.contains("overran"),
            "the refusal names the overrun: {}",
            error.message
        );
        let skill_pid = read_pid(dir.path(), "skill.pid");
        let child_pid = read_pid(dir.path(), "skill-child.pid");
        assert!(
            reaped(skill_pid),
            "the skill the runner spawned is terminated and reaped"
        );
        assert!(
            reaped(child_pid),
            "the subprocess the skill spawned is terminated and reaped"
        );
    }

    #[test]
    fn a_skill_that_finishes_inside_the_deadline_succeeds() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 0"]);

        run_to_deadline(command, "bc-add", Duration::from_secs(5), plain_failure)
            .expect("the completed skill succeeds");
    }

    #[test]
    fn a_failing_skill_reports_its_refusal_text() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo 'the clone holds unique commits' >&2; exit 1"]);

        let error = run_to_deadline(command, "bc-rm", Duration::from_secs(5), plain_failure)
            .expect_err("the failing skill is refused");

        assert!(
            error.message.contains("refused"),
            "the refusal keeps its shape: {}",
            error.message
        );
        assert!(
            error.message.contains("the clone holds unique commits"),
            "the fleet's own refusal text survives the wrapping: {}",
            error.message
        );
    }

    #[test]
    fn the_production_tool_runs_git_under_the_bounded_deadline() {
        let tool = LocalFleetCloneTool::default();

        assert_eq!(tool.program, "git");
        assert_eq!(tool.deadline, super::SKILL_DEADLINE);
    }
}
