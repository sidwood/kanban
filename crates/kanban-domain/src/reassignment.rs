//! The reassignment rules (DR-DE-07, CONTEXT.md): reassignment
//! replaces a Ticket by creating a replacement Ticket that supersedes
//! the original, so that history survives changed plans. The two
//! halves belong to one act — the replacement is created into draft
//! referencing its predecessor, a one-directional reference the
//! reassignment act alone sets, and the original moves to superseded,
//! the terminal state no drag and no named command reaches
//! (DR-LC-01); only the audited emergency override may pass through
//! supersession, as recovery (DR-LC-10). A superseded Ticket keeps
//! every recorded fact — its row is never deleted, its number was
//! minted from the Project's monotonic counter and is never reused
//! (KAN-T8) — and its timeline history stays exactly as it stands.

use std::fmt;

use crate::ticket::{Ticket, TicketId, TicketState};

/// Why a reassignment was refused. Every refusal leaves both Tickets
/// exactly as they stood.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReassignmentError {
    /// Cancelled and superseded are terminal (DR-LC-02); a terminal
    /// Ticket is replaced by nothing.
    Terminal,
    /// Done is final; landed work is not reassigned.
    Complete,
    /// The replacement names another predecessor. A replacement
    /// references the Ticket it replaces (DR-DE-07), so a pair that
    /// does not agree replaces nothing.
    Detached,
}

impl fmt::Display for ReassignmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Terminal => write!(
                f,
                "cancelled and superseded are terminal; the Ticket accepts no further changes"
            ),
            Self::Complete => write!(f, "done is final; landed work is not reassigned"),
            Self::Detached => write!(
                f,
                "a reassignment's replacement references the Ticket it replaces"
            ),
        }
    }
}

impl std::error::Error for ReassignmentError {}

/// Apply one reassignment (DR-DE-07): the replacement being minted
/// names `predecessor` — the identity a `Ticket::replacement` lands
/// referencing — and `original`, the Ticket that predecessor must be,
/// moves to the terminal superseded state from any open state. The
/// replacement's own identity is storage's to assign; this rule owns
/// the pairing and the supersession alone, and the caller lands the
/// replacement beside the superseded original in one write.
pub fn apply_reassignment(
    original: &mut Ticket,
    predecessor: TicketId,
) -> Result<(), ReassignmentError> {
    if original.state().is_terminal() {
        return Err(ReassignmentError::Terminal);
    }
    if original.state() == TicketState::Done {
        return Err(ReassignmentError::Complete);
    }
    if predecessor != original.id() {
        return Err(ReassignmentError::Detached);
    }
    original.transition_state(TicketState::Superseded);
    Ok(())
}

#[cfg(test)]
mod reassignment_rules {
    use super::{ReassignmentError, apply_reassignment};
    use crate::project::ProjectId;
    use crate::ticket::{
        Priority, TaskMode, TaskSubtype, TaskTiming, Ticket, TicketBody, TicketId, TicketNumber,
        TicketState,
    };

    fn number(value: u64) -> TicketNumber {
        TicketNumber::new(value).expect("the fixture number is positive")
    }

    /// One Task body, the kind whose facts every assertion shares.
    fn body() -> TicketBody {
        TicketBody::task(
            "Archive the old register",
            None,
            Some(TaskSubtype::Operational),
            Some(TaskMode::Human),
            vec![
                crate::ticket::CompletionCriterion::new("The register is archived.")
                    .expect("the fixture outcome binds"),
            ],
            TaskTiming::none(),
        )
        .expect("the fixture body validates")
    }

    /// One Ticket in the state a test chooses, at version 1.
    fn ticket(id: u64, state: TicketState) -> Ticket {
        Ticket::restore(
            TicketId::new(id),
            ProjectId::new(1),
            number(4),
            Priority::Normal,
            state,
            body(),
            None,
            None,
            1,
        )
    }

    /// One replacement for `original`, minted as the reassignment act
    /// mints it, at the identity a test chooses.
    fn replacement_for(original: &Ticket, id: u64) -> Ticket {
        Ticket::replacement(
            TicketId::new(id),
            original.project(),
            number(5),
            Priority::High,
            original.id(),
            body(),
        )
    }

    #[test]
    fn a_replacement_is_a_draft_referencing_its_predecessor() {
        let original = ticket(1, TicketState::Draft);
        let replacement = replacement_for(&original, 9);

        assert_eq!(replacement.id(), TicketId::new(9));
        assert_eq!(replacement.project(), original.project());
        assert_eq!(replacement.number().value(), 5);
        assert_eq!(replacement.priority(), Priority::High);
        assert_eq!(replacement.state(), TicketState::Draft);
        assert_eq!(
            replacement.predecessor(),
            Some(original.id()),
            "the replacement references its predecessor (DR-DE-07)"
        );
        assert_eq!(replacement.version(), 1);
        assert_eq!(replacement.kind(), original.kind());

        // An ordinary Ticket and a rehydrated one without a
        // predecessor reference nothing.
        assert_eq!(
            Ticket::new(
                TicketId::new(2),
                ProjectId::new(1),
                number(1),
                Priority::Normal,
                body(),
            )
            .predecessor(),
            None,
            "a fresh Ticket replaces nothing"
        );
    }

    #[test]
    fn reassignment_supersedes_the_original_from_any_open_state() {
        for from in [
            TicketState::Draft,
            TicketState::Parked,
            TicketState::Blocked,
            TicketState::Scheduled,
            TicketState::Ready,
            TicketState::Active,
            TicketState::InReview,
            TicketState::Approved,
            TicketState::Landing,
        ] {
            let mut original = ticket(1, from);

            apply_reassignment(&mut original, TicketId::new(1))
                .unwrap_or_else(|error| panic!("reassignment serves {from:?}: {error}"));

            assert_eq!(original.state(), TicketState::Superseded, "{from:?}");
            assert_eq!(
                original.version(),
                2,
                "{from:?}: the supersession counts as one applied change"
            );
            // The superseded Ticket keeps every recorded fact: the
            // number, the body, and the identity all stand.
            assert_eq!(original.number().value(), 4);
        }
    }

    #[test]
    fn terminal_and_landed_originals_are_never_reassigned() {
        for state in [TicketState::Cancelled, TicketState::Superseded] {
            let mut original = ticket(1, state);
            assert_eq!(
                apply_reassignment(&mut original, TicketId::new(1)),
                Err(ReassignmentError::Terminal),
                "{state:?} is terminal"
            );
            assert_eq!(original.state(), state, "the refusal moved nothing");
            assert_eq!(original.version(), 1, "the refusal changed nothing");
        }
        assert_eq!(
            ReassignmentError::Terminal.to_string(),
            "cancelled and superseded are terminal; the Ticket accepts no further changes"
        );

        let mut landed = ticket(1, TicketState::Done);
        assert_eq!(
            apply_reassignment(&mut landed, TicketId::new(1)),
            Err(ReassignmentError::Complete),
            "done is final; landed work is not reassigned"
        );
        assert_eq!(landed.state(), TicketState::Done);
        assert_eq!(
            ReassignmentError::Complete.to_string(),
            "done is final; landed work is not reassigned"
        );
    }

    #[test]
    fn a_replacement_that_names_another_predecessor_is_refused() {
        let mut original = ticket(1, TicketState::Active);
        let elsewhere = ticket(2, TicketState::Draft);

        assert_eq!(
            apply_reassignment(&mut original, elsewhere.id()),
            Err(ReassignmentError::Detached),
            "a replacement references the Ticket it replaces"
        );
        assert_eq!(original.state(), TicketState::Active, "nothing moved");
        assert_eq!(original.version(), 1);
        assert_eq!(
            ReassignmentError::Detached.to_string(),
            "a reassignment's replacement references the Ticket it replaces"
        );
    }

    #[test]
    fn no_drag_or_named_command_reaches_superseded() {
        // Supersession is the reassignment act alone: the transition
        // table holds no edge into superseded from any state, so no
        // drag and no named command lands there (DR-LC-01).
        use crate::lifecycle::{HumanCommand, legal_targets};
        for state in TicketState::ALL {
            assert!(
                !legal_targets(*state).contains(&TicketState::Superseded),
                "{state:?} holds no edge into superseded"
            );
        }
        // Every open state can still be superseded by reassignment,
        // which is the one act that reaches the state.
        let mut original = ticket(1, TicketState::Ready);
        apply_reassignment(&mut original, TicketId::new(1))
            .expect("reassignment reaches superseded");
        assert_eq!(original.state(), TicketState::Superseded);
        let _ = HumanCommand::Park;
    }
}
