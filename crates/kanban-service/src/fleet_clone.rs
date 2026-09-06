//! The fleet branch-clone skill, wrapped (KAN-S6-US4). The `git
//! bc-add` family is the only sanctioned clone mechanism; this
//! adapter invokes it and nothing else, because the guards — the
//! conflict refusal, the ordering, the timeline rows — all live in
//! the application layer, which calls this port only after every
//! precondition has held.
//!
//! A failed invocation's stderr is diagnostics, not payload: the
//! skill's own output can carry ANSI colour, a runaway postadd dump,
//! or a secret a fetch picked up, and every byte of it would otherwise
//! reach the operator's error and the durable timeline unchanged. The
//! report is therefore stripped, scrubbed, and bounded here — at the
//! one boundary where skill output enters — so both surfaces that copy
//! it inherit clean text and no second redaction policy exists
//! anywhere else.

use std::path::PathBuf;
use std::process::Command;

use kanban_app::FleetCloneTool;
use kanban_dto::ApiError;

use crate::redaction::Redactor;

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

/// Run the fleet's `git bc-add` and `git bc-rm` skills locally.
#[derive(Debug)]
pub struct LocalFleetCloneTool {
    /// The managed data directory whose configuration feeds redaction,
    /// re-read on every failed invocation so a secret added or rotated
    /// since the last one is still scrubbed.
    data_dir: PathBuf,
}

impl LocalFleetCloneTool {
    /// A tool whose reports are scrubbed against the managed
    /// configuration under `data_dir`.
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
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

    /// Run one fleet skill and report its refusal or failure.
    fn run_skill(&self, skill: &str, arguments: Vec<String>) -> Result<(), ApiError> {
        let output = Command::new("git")
            .arg(skill)
            .args(&arguments)
            .output()
            .map_err(|source| {
                ApiError::internal(&format!(
                    "the fleet clone skill `git {skill}` could not run: {source}"
                ))
            })?;
        if output.status.success() {
            return Ok(());
        }
        Err(self.report_failure(skill, &output.stderr))
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
    use super::LocalFleetCloneTool;

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
        let arguments = LocalFleetCloneTool::remove_arguments("/workspaces/kanban.fleet-t34");

        assert_eq!(
            arguments,
            vec!["/workspaces/kanban.fleet-t34".to_owned(), "-y".to_owned()]
        );
        assert!(
            !arguments
                .iter()
                .any(|argument| argument == "-f" || argument == "--force"),
            "the fleet's own refusal rules must stay armed"
        );
    }
}
