//! Walkthrough proof: every Ticket names a click-path or text-path
//! the operator can see, and only a shell-minted run with artifacts
//! can prove it. Files, git, MCP self-reports, and empty “it works”
//! claims cannot.

use std::fmt;

use crate::evidence::{ContentHash, EvidenceKind};

/// Where the operator sees the behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkthroughSurface {
    /// A graphical UI: windows, pages, controls.
    Graphical,
    /// A text UI: TUI or command-line output.
    Textual,
}

/// Who claims to have executed the walkthrough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkthroughOrigin {
    /// The desktop shell, watching the live WebView or TUI.
    Shell,
    /// Any MCP or socket client. Cannot mint proof.
    Mcp,
}

/// One captured artefact of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkthroughArtifactKind {
    /// A screenshot of the graphical surface.
    Screenshot,
    /// A before/after diff of the surface.
    Diff,
    /// A terminal or log transcript of the textual surface.
    Transcript,
}

/// Why a walkthrough definition or run was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkthroughError {
    /// A walkthrough with no steps proves nothing.
    NoSteps,
    /// A step that names no location cannot be executed.
    BlankLocation,
    /// A step that names no action cannot be executed.
    BlankAction,
    /// A step that states no expected result cannot be judged.
    BlankExpected,
    /// A run with no screenshot, diff, or transcript is a story.
    NoArtifacts,
    /// MCP and other non-shell clients cannot mint walkthrough proof.
    NotShellMinted,
    /// A failed run cannot satisfy a criterion.
    FailedRun,
    /// Proof is bound to the source tip under review.
    WrongTip,
    /// Graphical proof cannot satisfy a textual walkthrough, and vice versa.
    SurfaceMismatch,
    /// File and repository evidence cannot satisfy a walkthrough criterion.
    NotWalkthroughEvidence,
    /// A Ticket without a walkthrough cannot be created or landed.
    MissingWalkthrough,
}

impl fmt::Display for WalkthroughError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSteps => write!(f, "a walkthrough names at least one observable step"),
            Self::BlankLocation => write!(f, "a walkthrough step names where to look"),
            Self::BlankAction => write!(f, "a walkthrough step names what to do"),
            Self::BlankExpected => {
                write!(f, "a walkthrough step states the visible result")
            }
            Self::NoArtifacts => write!(
                f,
                "a walkthrough run captures a screenshot, a diff, or a transcript"
            ),
            Self::NotShellMinted => write!(
                f,
                "only the desktop shell can mint walkthrough proof; MCP cannot"
            ),
            Self::FailedRun => write!(f, "a failed walkthrough cannot satisfy a criterion"),
            Self::WrongTip => {
                write!(
                    f,
                    "walkthrough proof is bound to the source tip under review"
                )
            }
            Self::SurfaceMismatch => write!(
                f,
                "walkthrough proof must match the ticket's graphical or textual surface"
            ),
            Self::NotWalkthroughEvidence => write!(
                f,
                "a walkthrough criterion is satisfied only by walkthrough evidence"
            ),
            Self::MissingWalkthrough => {
                write!(f, "every ticket carries a walkthrough that can be proven")
            }
        }
    }
}

impl std::error::Error for WalkthroughError {}

/// One step the operator (or an agent driving the live UI) performs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkthroughStep {
    location: String,
    action: String,
    expected: String,
}

impl WalkthroughStep {
    /// Assemble one step, refusing anything the operator could not
    /// follow or judge.
    pub fn new(
        location: impl Into<String>,
        action: impl Into<String>,
        expected: impl Into<String>,
    ) -> Result<Self, WalkthroughError> {
        let location = location.into();
        let action = action.into();
        let expected = expected.into();
        if location.trim().is_empty() {
            return Err(WalkthroughError::BlankLocation);
        }
        if action.trim().is_empty() {
            return Err(WalkthroughError::BlankAction);
        }
        if expected.trim().is_empty() {
            return Err(WalkthroughError::BlankExpected);
        }
        Ok(Self {
            location,
            action,
            expected,
        })
    }

    /// Where to look: a selector, a route, or a CLI prompt.
    pub fn location(&self) -> &str {
        &self.location
    }

    /// What to do there.
    pub fn action(&self) -> &str {
        &self.action
    }

    /// The visible or textual result that counts as proof.
    pub fn expected(&self) -> &str {
        &self.expected
    }
}

/// The click-path or text-path a Ticket must prove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walkthrough {
    surface: WalkthroughSurface,
    steps: Vec<WalkthroughStep>,
}

impl Walkthrough {
    /// Assemble a walkthrough, refusing an empty path.
    pub fn new(
        surface: WalkthroughSurface,
        steps: Vec<WalkthroughStep>,
    ) -> Result<Self, WalkthroughError> {
        if steps.is_empty() {
            return Err(WalkthroughError::NoSteps);
        }
        Ok(Self { surface, steps })
    }

    /// One graphical observe-step, for tickets whose proof is a
    /// visible control or chip.
    pub fn graphical(
        location: impl Into<String>,
        expected: impl Into<String>,
    ) -> Result<Self, WalkthroughError> {
        Self::new(
            WalkthroughSurface::Graphical,
            vec![WalkthroughStep::new(location, "observe", expected)?],
        )
    }

    /// One textual observe-step, for tickets whose proof is CLI or TUI
    /// output.
    pub fn textual(
        location: impl Into<String>,
        expected: impl Into<String>,
    ) -> Result<Self, WalkthroughError> {
        Self::new(
            WalkthroughSurface::Textual,
            vec![WalkthroughStep::new(location, "observe", expected)?],
        )
    }

    /// Graphical or textual.
    pub fn surface(&self) -> WalkthroughSurface {
        self.surface
    }

    /// The ordered steps.
    pub fn steps(&self) -> &[WalkthroughStep] {
        &self.steps
    }
}

/// One captured artefact: a hashed screenshot, diff, or transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkthroughArtifact {
    kind: WalkthroughArtifactKind,
    content_hash: ContentHash,
}

impl WalkthroughArtifact {
    /// Bind a captured artefact to its content hash.
    pub fn new(kind: WalkthroughArtifactKind, content_hash: ContentHash) -> Self {
        Self { kind, content_hash }
    }

    /// Screenshot, diff, or transcript.
    pub fn kind(&self) -> WalkthroughArtifactKind {
        self.kind
    }

    /// The hash of the captured bytes.
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

/// A finished attempt to execute a walkthrough against a live surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkthroughRun {
    origin: WalkthroughOrigin,
    surface: WalkthroughSurface,
    tip: String,
    artifacts: Vec<WalkthroughArtifact>,
    passed: bool,
}

impl WalkthroughRun {
    /// Record a run. The shell is the only legal origin; a pass still
    /// needs artefacts; a fail never satisfies.
    pub fn mint(
        origin: WalkthroughOrigin,
        surface: WalkthroughSurface,
        tip: impl Into<String>,
        artifacts: Vec<WalkthroughArtifact>,
        passed: bool,
    ) -> Result<Self, WalkthroughError> {
        if origin != WalkthroughOrigin::Shell {
            return Err(WalkthroughError::NotShellMinted);
        }
        if artifacts.is_empty() {
            return Err(WalkthroughError::NoArtifacts);
        }
        if !passed {
            return Err(WalkthroughError::FailedRun);
        }
        let tip = tip.into();
        if tip.trim().is_empty() {
            return Err(WalkthroughError::WrongTip);
        }
        Ok(Self {
            origin,
            surface,
            tip,
            artifacts,
            passed,
        })
    }

    /// The desktop shell.
    pub fn origin(&self) -> WalkthroughOrigin {
        self.origin
    }

    /// The surface that was exercised.
    pub fn surface(&self) -> WalkthroughSurface {
        self.surface
    }

    /// The source tip this run proves.
    pub fn tip(&self) -> &str {
        &self.tip
    }

    /// Captured screenshots, diffs, or transcripts.
    pub fn artifacts(&self) -> &[WalkthroughArtifact] {
        &self.artifacts
    }

    /// Whether every step matched.
    pub fn passed(&self) -> bool {
        self.passed
    }
}

/// A walkthrough criterion is satisfied only by walkthrough evidence.
pub fn walkthrough_evidence_fits(kind: EvidenceKind) -> Result<(), WalkthroughError> {
    match kind {
        EvidenceKind::Walkthrough => Ok(()),
        EvidenceKind::ManagedFile | EvidenceKind::Repository => {
            Err(WalkthroughError::NotWalkthroughEvidence)
        }
    }
}

/// A Ticket without a walkthrough is not ready to exist or land.
pub fn require_walkthrough(walkthrough: Option<&Walkthrough>) -> Result<(), WalkthroughError> {
    match walkthrough {
        Some(_) => Ok(()),
        None => Err(WalkthroughError::MissingWalkthrough),
    }
}

/// Landing and satisfaction require a shell-minted pass at this tip,
/// on the ticket's own surface, with artefacts.
pub fn require_proven_walkthrough(
    walkthrough: &Walkthrough,
    run: &WalkthroughRun,
    source_tip: &str,
) -> Result<(), WalkthroughError> {
    if run.surface() != walkthrough.surface() {
        return Err(WalkthroughError::SurfaceMismatch);
    }
    if run.tip() != source_tip {
        return Err(WalkthroughError::WrongTip);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Walkthrough, WalkthroughArtifact, WalkthroughArtifactKind, WalkthroughError,
        WalkthroughOrigin, WalkthroughRun, WalkthroughStep, WalkthroughSurface,
        require_proven_walkthrough, require_walkthrough, walkthrough_evidence_fits,
    };
    use crate::evidence::{ContentHash, EvidenceKind};

    fn hash() -> ContentHash {
        ContentHash::new(&"a".repeat(64)).expect("the digest validates")
    }

    fn screenshot() -> WalkthroughArtifact {
        WalkthroughArtifact::new(WalkthroughArtifactKind::Screenshot, hash())
    }

    fn ui() -> Walkthrough {
        Walkthrough::graphical("[data-testid=connection-chip]", "Service running")
            .expect("the fixture walkthrough validates")
    }

    fn passed_run(surface: WalkthroughSurface, tip: &str) -> WalkthroughRun {
        WalkthroughRun::mint(
            WalkthroughOrigin::Shell,
            surface,
            tip,
            vec![screenshot()],
            true,
        )
        .expect("a shell pass with a screenshot mints")
    }

    #[test]
    fn a_walkthrough_refuses_empty_steps_and_blank_fields() {
        assert_eq!(
            Walkthrough::new(WalkthroughSurface::Graphical, vec![]).unwrap_err(),
            WalkthroughError::NoSteps
        );
        assert_eq!(
            WalkthroughStep::new(" ", "click", "open").unwrap_err(),
            WalkthroughError::BlankLocation
        );
        assert_eq!(
            WalkthroughStep::new("#chip", "  ", "open").unwrap_err(),
            WalkthroughError::BlankAction
        );
        assert_eq!(
            WalkthroughStep::new("#chip", "click", "\n").unwrap_err(),
            WalkthroughError::BlankExpected
        );
    }

    #[test]
    fn mcp_cannot_mint_walkthrough_proof() {
        let refused = WalkthroughRun::mint(
            WalkthroughOrigin::Mcp,
            WalkthroughSurface::Graphical,
            "a".repeat(40),
            vec![screenshot()],
            true,
        );
        assert_eq!(refused.unwrap_err(), WalkthroughError::NotShellMinted);
    }

    #[test]
    fn a_pass_without_artifacts_is_not_proof() {
        let refused = WalkthroughRun::mint(
            WalkthroughOrigin::Shell,
            WalkthroughSurface::Graphical,
            "a".repeat(40),
            vec![],
            true,
        );
        assert_eq!(refused.unwrap_err(), WalkthroughError::NoArtifacts);
    }

    #[test]
    fn a_failed_run_cannot_satisfy() {
        let refused = WalkthroughRun::mint(
            WalkthroughOrigin::Shell,
            WalkthroughSurface::Graphical,
            "a".repeat(40),
            vec![screenshot()],
            false,
        );
        assert_eq!(refused.unwrap_err(), WalkthroughError::FailedRun);
    }

    #[test]
    fn file_and_repository_evidence_cannot_satisfy_a_walkthrough() {
        assert_eq!(
            walkthrough_evidence_fits(EvidenceKind::ManagedFile).unwrap_err(),
            WalkthroughError::NotWalkthroughEvidence
        );
        assert_eq!(
            walkthrough_evidence_fits(EvidenceKind::Repository).unwrap_err(),
            WalkthroughError::NotWalkthroughEvidence
        );
        walkthrough_evidence_fits(EvidenceKind::Walkthrough)
            .expect("walkthrough evidence fits a walkthrough criterion");
    }

    #[test]
    fn a_ticket_without_a_walkthrough_is_refused() {
        assert_eq!(
            require_walkthrough(None).unwrap_err(),
            WalkthroughError::MissingWalkthrough
        );
        require_walkthrough(Some(&ui())).expect("a named walkthrough is present");
    }

    #[test]
    fn landing_requires_a_shell_pass_at_the_source_tip_on_the_same_surface() {
        let tip = "b".repeat(40);
        let walkthrough = ui();
        require_proven_walkthrough(
            &walkthrough,
            &passed_run(WalkthroughSurface::Graphical, &tip),
            &tip,
        )
        .expect("matching graphical proof at the tip lands");

        assert_eq!(
            require_proven_walkthrough(
                &walkthrough,
                &passed_run(WalkthroughSurface::Textual, &tip),
                &tip,
            )
            .unwrap_err(),
            WalkthroughError::SurfaceMismatch
        );
        assert_eq!(
            require_proven_walkthrough(
                &walkthrough,
                &passed_run(WalkthroughSurface::Graphical, &"c".repeat(40)),
                &tip,
            )
            .unwrap_err(),
            WalkthroughError::WrongTip
        );
    }

    #[test]
    fn textual_walkthroughs_are_first_class() {
        let walkthrough = Walkthrough::textual("$ kanban health", "connected: true")
            .expect("a CLI walkthrough validates");
        assert_eq!(walkthrough.surface(), WalkthroughSurface::Textual);
        assert_eq!(walkthrough.steps()[0].expected(), "connected: true");
    }
}
