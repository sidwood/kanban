//! Recovery resume custody (KAN-T142, KAN-S13-US1): whether one
//! existing Run attempt may still be resumed under the authority it
//! already holds. Resume continues a frozen attempt rather than
//! starting a new one, so it is not ordinary execution admission
//! (KAN-S9) and answers none of admission's queue, readiness, or
//! graph questions — a Ticket that is merely blocked, parked, or
//! unready still has an attempt worth reconnecting to. What it does
//! answer is whether the work it would continue is still the work
//! the operator wants: the terminal Ticket states and the archived
//! Project are the same ones every other command refuses further
//! change from (DR-LC-02, DR-AE-08), and an attempt that was
//! superseded, already submitted, closed, or stripped of its
//! authority has nothing left to resume.
//!
//! A refusal is the whole outcome. Nothing here moves a Ticket, and
//! no resume summary stands in for the audited emergency override
//! that alone reaches past a terminal state (DR-LC-10).

use std::fmt;

use crate::ticket::TicketState;

/// The current facts one resume decision reads, all of them read
/// fresh: the attempt's own custody and the authoritative lifecycle
/// state of the work it would continue.
#[derive(Debug, Clone, Copy)]
pub struct ResumeCustody {
    pub project_archived: bool,
    /// The state the Ticket holds now, not the state it held when
    /// the attempt started.
    pub ticket_state: TicketState,
    /// Whether recovery already replaced the attempt with a fresh
    /// request, rather than the Ticket itself being superseded.
    pub superseded: bool,
    pub has_submission: bool,
    /// Whether the Dispatch Request behind the attempt is still open
    /// and, for a reviewer, still holds its required slot.
    pub request_active: bool,
    pub authority_active: bool,
}

/// Why a resume was refused. Every refusal leaves the recovery
/// history, its Rulings, any accepted delivery intent, the Run, and
/// the Ticket exactly as they stood.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeRefusal {
    ArchivedProject,
    /// Cancelled and superseded are terminal (DR-LC-02); neither
    /// resumes.
    TerminalTicket {
        state: TicketState,
    },
    /// Recovery already replaced the attempt with a fresh request.
    Superseded,
    ResultSubmitted,
    /// The Dispatch Request is closed, or its required review slot
    /// no longer accepts the attempt.
    RequestClosed,
    AuthoritySettled,
}

impl fmt::Display for ResumeRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ArchivedProject => write!(
                f,
                "archived is terminal; the Project accepts no further changes"
            ),
            Self::TerminalTicket { .. } => write!(
                f,
                "cancelled and superseded are terminal; the Ticket accepts no further changes"
            ),
            Self::Superseded => write!(
                f,
                "recovery already superseded this attempt; its replacement request carries the work"
            ),
            Self::ResultSubmitted => write!(
                f,
                "this attempt already recorded a result; resuming it would re-execute settled work"
            ),
            Self::RequestClosed => write!(
                f,
                "the Dispatch Request behind this attempt is no longer open; resume has nothing to continue"
            ),
            Self::AuthoritySettled => write!(
                f,
                "this attempt cannot resume with its original authority; request a new attempt instead"
            ),
        }
    }
}

impl std::error::Error for ResumeRefusal {}

/// Decide whether the attempt may resume now. The checks run from
/// the most final fact to the most transient, so the reason names
/// what the operator must change first — and, for the terminal
/// states, that change is the audited emergency override, never this
/// command.
pub fn admit_run_resume(custody: ResumeCustody) -> Result<(), ResumeRefusal> {
    if custody.project_archived {
        return Err(ResumeRefusal::ArchivedProject);
    }
    if custody.ticket_state.is_terminal() {
        return Err(ResumeRefusal::TerminalTicket {
            state: custody.ticket_state,
        });
    }
    if custody.superseded {
        return Err(ResumeRefusal::Superseded);
    }
    if custody.has_submission {
        return Err(ResumeRefusal::ResultSubmitted);
    }
    if !custody.request_active {
        return Err(ResumeRefusal::RequestClosed);
    }
    if !custody.authority_active {
        return Err(ResumeRefusal::AuthoritySettled);
    }
    Ok(())
}

#[cfg(test)]
mod resume_rules {
    use super::{ResumeCustody, ResumeRefusal, admit_run_resume};
    use crate::ticket::TicketState;

    /// An attempt nothing has invalidated.
    fn eligible() -> ResumeCustody {
        ResumeCustody {
            project_archived: false,
            ticket_state: TicketState::Active,
            superseded: false,
            has_submission: false,
            request_active: true,
            authority_active: true,
        }
    }

    #[test]
    fn an_attempt_nothing_invalidated_resumes() {
        assert_eq!(admit_run_resume(eligible()), Ok(()));
    }

    #[test]
    fn every_terminal_ticket_state_refuses_by_name() {
        for state in [TicketState::Cancelled, TicketState::Superseded] {
            assert_eq!(
                admit_run_resume(ResumeCustody {
                    ticket_state: state,
                    ..eligible()
                }),
                Err(ResumeRefusal::TerminalTicket { state })
            );
        }
    }

    #[test]
    fn an_archived_project_refuses_before_the_ticket_is_read() {
        assert_eq!(
            admit_run_resume(ResumeCustody {
                project_archived: true,
                ticket_state: TicketState::Cancelled,
                ..eligible()
            }),
            Err(ResumeRefusal::ArchivedProject)
        );
    }

    /// Resume continues an attempt rather than starting one, so the
    /// states ordinary admission refuses — and the readiness it
    /// computes — are none of this rule's business.
    #[test]
    fn a_non_terminal_ticket_resumes_from_any_open_state() {
        for state in TicketState::ALL
            .iter()
            .copied()
            .filter(|state| !state.is_terminal())
        {
            assert_eq!(
                admit_run_resume(ResumeCustody {
                    ticket_state: state,
                    ..eligible()
                }),
                Ok(()),
                "{} is not terminal",
                state.wire_name()
            );
        }
    }

    #[test]
    fn each_spent_attempt_names_its_own_reason() {
        assert_eq!(
            admit_run_resume(ResumeCustody {
                superseded: true,
                ..eligible()
            }),
            Err(ResumeRefusal::Superseded)
        );
        assert_eq!(
            admit_run_resume(ResumeCustody {
                has_submission: true,
                ..eligible()
            }),
            Err(ResumeRefusal::ResultSubmitted)
        );
        assert_eq!(
            admit_run_resume(ResumeCustody {
                request_active: false,
                ..eligible()
            }),
            Err(ResumeRefusal::RequestClosed)
        );
        assert_eq!(
            admit_run_resume(ResumeCustody {
                authority_active: false,
                ..eligible()
            }),
            Err(ResumeRefusal::AuthoritySettled)
        );
    }

    #[test]
    fn every_refusal_explains_itself() {
        for refusal in [
            ResumeRefusal::ArchivedProject,
            ResumeRefusal::TerminalTicket {
                state: TicketState::Cancelled,
            },
            ResumeRefusal::Superseded,
            ResumeRefusal::ResultSubmitted,
            ResumeRefusal::RequestClosed,
            ResumeRefusal::AuthoritySettled,
        ] {
            assert!(!refusal.to_string().is_empty());
        }
    }
}
