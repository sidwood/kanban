//! Execution admission (KAN-T138, KAN-S9-US2, DR-EP-08): the one
//! eligibility decision every ordinary admission path — the claim,
//! the acknowledgement, and the Coordinator loop that drives them —
//! answers with the Ticket's current authoritative state before a
//! Run, a Lane, a capability, or capacity changes. Queue order is a
//! snapshot taken at enqueue (DR-EP-08); admission never is, so a
//! stale queue record admits nothing the Ticket no longer allows. A
//! refusal is the whole outcome: nothing here infers an emergency
//! override (DR-LC-10).

use std::fmt;

use crate::dependency::Readiness;
use crate::ticket::{TaskMode, Ticket, TicketKind, TicketState};

/// The role a Dispatch Request executes as, which decides the states
/// execution may proceed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdmissionRole {
    /// An implementer run starts or continues the Ticket's work.
    Implementer,
    /// A reviewer run evaluates a submitted code tip.
    Reviewer,
}

impl AdmissionRole {
    fn noun(self) -> &'static str {
        match self {
            Self::Implementer => "an implementer run",
            Self::Reviewer => "a reviewer run",
        }
    }
}

/// The indefinite article a state name takes in a refusal message.
fn article(word: &str) -> &'static str {
    if word.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    }
}

/// Why admission refused. Every refusal leaves the Ticket, the
/// Dispatch Request, the Run, the Lane, and capacity exactly as they
/// stood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionRefusal {
    /// The owning Project is archived, and archived is terminal.
    ArchivedProject,
    /// Cancelled and superseded are terminal (DR-LC-02); neither
    /// executes again.
    TerminalTicket {
        /// The terminal state the Ticket holds.
        state: TicketState,
    },
    /// A Bug executes only once its qualification is complete
    /// (DR-TK-09).
    UnqualifiedBug,
    /// An Implementation executes only through the approved Ticket
    /// graph that pinned it (DR-PS-17, DR-DE-06).
    GraphUnapproved,
    /// A Ticket of any kind that a proposed Ticket graph names waits
    /// for the same human gate: approval is what installs the
    /// proposal's ordering and pins its members (DR-PS-17).
    AwaitingGraphApproval,
    /// A human-mode Task is Sid's own work (KAN-S4-US4); dispatch
    /// mints no implementer authority over it.
    HumanTask,
    /// The lifecycle state admits no run of this role: scheduled work
    /// waits for its activation (DR-SA-02), and draft, parked,
    /// blocked, approved, landing, and done work is not executing.
    NotExecutable {
        /// The state the Ticket holds.
        state: TicketState,
        /// The role that asked.
        role: AdmissionRole,
    },
    /// A computed readiness still holds the Ticket back (DR-DE-03).
    NotReady {
        /// How many unresolved dependencies or blockers hold it.
        waiting_on: usize,
    },
}

impl fmt::Display for AdmissionRefusal {
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
            Self::UnqualifiedBug => write!(
                f,
                "the Bug is unqualified; a Bug executes only once its qualification is complete"
            ),
            Self::GraphUnapproved => write!(
                f,
                "the Implementation Ticket belongs to no approved Ticket graph; \
                 a human approves the graph before execution"
            ),
            Self::AwaitingGraphApproval => write!(
                f,
                "the Ticket is named by a Ticket graph still awaiting approval; \
                 a human approves the graph before execution"
            ),
            Self::HumanTask => write!(
                f,
                "the Task is human-mode; Sid executes it, so no implementer run is dispatched"
            ),
            Self::NotExecutable {
                state: TicketState::Scheduled,
                ..
            } => write!(
                f,
                "the Ticket is scheduled; it is unavailable until its activation"
            ),
            Self::NotExecutable { state, role } => {
                let admitted = executable_states(*role)
                    .iter()
                    .map(|state| state.wire_name())
                    .collect::<Vec<_>>();
                let (last, rest) = admitted
                    .split_last()
                    .expect("every role admits at least one state");
                let quality = if role == &AdmissionRole::Reviewer {
                    "reviewable"
                } else {
                    "executable"
                };
                write!(
                    f,
                    "{} {} Ticket is not {quality}; {} admits a {}{}{last} Ticket",
                    article(state.wire_name()),
                    state.wire_name(),
                    role.noun(),
                    rest.join(", "),
                    if rest.len() > 1 { ", or " } else { " or " },
                )
            }
            Self::NotReady { waiting_on } => write!(
                f,
                "the Ticket is held back by {waiting_on} unresolved dependencies or external blockers"
            ),
        }
    }
}

impl std::error::Error for AdmissionRefusal {}

/// The facts one admission decision reads, all current: the Ticket
/// as stored, whether its Project is archived, and the readiness
/// projection computed from its dependencies and blockers now.
#[derive(Debug, Clone, Copy)]
pub struct AdmissionInputs<'a> {
    /// The Ticket the Dispatch Request executes.
    pub ticket: &'a Ticket,
    /// Whether the owning Project is archived.
    pub project_archived: bool,
    /// Whether a Ticket graph that names this Ticket is still
    /// awaiting the human approval gate.
    pub awaits_graph_approval: bool,
    /// The Ticket's readiness, computed fresh.
    pub readiness: &'a Readiness,
    /// The role the request executes as.
    pub role: AdmissionRole,
}

/// The lifecycle states from which a run of `role` may proceed. An
/// implementer starts from ready and continues from active; a
/// reviewer evaluates work that is in review, and — until execution
/// outcomes move the Ticket themselves — work still marked ready or
/// active.
pub fn executable_states(role: AdmissionRole) -> &'static [TicketState] {
    match role {
        AdmissionRole::Implementer => &[TicketState::Ready, TicketState::Active],
        AdmissionRole::Reviewer => &[
            TicketState::Ready,
            TicketState::Active,
            TicketState::InReview,
        ],
    }
}

/// Decide whether execution may be admitted now. The checks run from
/// the most final fact to the most transient, so the reason names
/// what the operator must change first.
pub fn admit_execution(inputs: AdmissionInputs<'_>) -> Result<(), AdmissionRefusal> {
    if inputs.project_archived {
        return Err(AdmissionRefusal::ArchivedProject);
    }
    let ticket = inputs.ticket;
    if ticket.state().is_terminal() {
        return Err(AdmissionRefusal::TerminalTicket {
            state: ticket.state(),
        });
    }
    if !ticket.is_qualified() {
        return Err(AdmissionRefusal::UnqualifiedBug);
    }
    if ticket.pinned_version().is_none() {
        // An Implementation exists only to deliver a Spec version
        // through a graph, so an unpinned one has passed no gate at
        // all. Every other kind may stand alone, and it is graph
        // participation — not kind — that puts one behind the gate.
        if ticket.kind() == TicketKind::Implementation {
            return Err(AdmissionRefusal::GraphUnapproved);
        }
        if inputs.awaits_graph_approval {
            return Err(AdmissionRefusal::AwaitingGraphApproval);
        }
    }
    if inputs.role == AdmissionRole::Implementer && ticket.task_mode() == Some(TaskMode::Human) {
        return Err(AdmissionRefusal::HumanTask);
    }
    if !executable_states(inputs.role).contains(&ticket.state()) {
        return Err(AdmissionRefusal::NotExecutable {
            state: ticket.state(),
            role: inputs.role,
        });
    }
    if inputs.role == AdmissionRole::Implementer {
        let waiting_on = inputs.readiness.blocked_by().len();
        if waiting_on > 0 {
            return Err(AdmissionRefusal::NotReady { waiting_on });
        }
    }
    Ok(())
}

#[cfg(test)]
mod admission_rules {
    use super::{AdmissionInputs, AdmissionRefusal, AdmissionRole, admit_execution};
    use crate::coverage::{AcceptanceCriterion, UserStoryRef, VerificationStep};
    use crate::dependency::{
        BlockerDescription, ExternalBlocker, ExternalBlockerId, Readiness, ReadinessInputs,
        compute_readiness,
    };
    use crate::plan::SpecNumber;
    use crate::project::ProjectId;
    use crate::spec::SpecId;
    use crate::ticket::{
        BugQualification, Priority, Severity, TaskMode, TaskSubtype, TaskTiming, Ticket,
        TicketBody, TicketId, TicketNumber, TicketState,
    };

    fn number(value: u64) -> TicketNumber {
        TicketNumber::new(value).expect("the fixture number is positive")
    }

    fn criterion() -> AcceptanceCriterion {
        let story = UserStoryRef::new(
            SpecNumber::new(1).expect("the fixture number is positive"),
            1,
        )
        .expect("the fixture ordinal is positive");
        AcceptanceCriterion::new("The claim fails closed.", vec![story])
            .expect("the fixture criterion links")
    }

    fn qualification() -> BugQualification {
        BugQualification::new(
            "Ordinary admission refuses an ineligible Ticket.",
            "Enqueue an unqualified Bug and claim it.",
            "macOS 26, disposable SQLite.",
            Severity::Critical,
            "Every claim.",
            "Every dispatch path.",
            "Unauthorised execution.",
            vec![criterion()],
            vec![
                VerificationStep::new("cargo test -p kanban-domain admission")
                    .expect("the fixture step carries its command"),
            ],
        )
        .expect("the fixture qualification is complete")
    }

    fn bug(qualified: bool) -> Ticket {
        let body = TicketBody::bug(
            "Claim admits an unqualified Bug",
            None,
            "The claim succeeded.",
            "The recovery audit probe transcript.",
        )
        .expect("the fixture body validates");
        let TicketBody::Bug(mut boxed) = body else {
            unreachable!("the fixture body is a Bug's");
        };
        if qualified {
            boxed = Box::new(crate::ticket::BugTicket::restore(
                boxed.title(),
                None,
                boxed.actual_behaviour(),
                boxed.reporter_evidence(),
                Some(qualification()),
                boxed.facts().clone(),
            ));
        }
        Ticket::new(
            TicketId::new(1),
            ProjectId::new(1),
            number(1),
            Priority::Normal,
            TicketBody::Bug(boxed),
        )
    }

    fn task() -> Ticket {
        task_in(TaskMode::Agent)
    }

    fn task_in(mode: TaskMode) -> Ticket {
        Ticket::new(
            TicketId::new(2),
            ProjectId::new(1),
            number(2),
            Priority::Normal,
            TicketBody::task(
                "Bounded operational work",
                None,
                Some(TaskSubtype::Operational),
                Some(mode),
                vec![
                    crate::ticket::CompletionCriterion::new("The work is done.")
                        .expect("the fixture criterion names an outcome"),
                ],
                TaskTiming::none(),
            )
            .expect("the fixture body validates"),
        )
    }

    fn implementation(pinned: bool) -> Ticket {
        let mut ticket = Ticket::new(
            TicketId::new(3),
            ProjectId::new(1),
            number(3),
            Priority::Normal,
            TicketBody::implementation(
                Some(SpecId::new(1)),
                SpecNumber::new(1).expect("the fixture number is positive"),
                "Refuse unsafe admission",
                vec![criterion()],
            )
            .expect("the fixture body validates"),
        );
        if pinned {
            ticket.pin_to(1).expect("the fixture pins once");
        }
        ticket
    }

    fn at(mut ticket: Ticket, state: TicketState) -> Ticket {
        ticket.transition_state(state);
        ticket
    }

    fn ready() -> Readiness {
        compute_readiness(ReadinessInputs {
            dependencies: &[],
            blockers: &[],
        })
    }

    fn blocked() -> Readiness {
        let blocker = ExternalBlocker::restore(
            ExternalBlockerId::new(1),
            TicketId::new(2),
            BlockerDescription::new("waiting on an unregistered vendor")
                .expect("the fixture description validates"),
        );
        compute_readiness(ReadinessInputs {
            dependencies: &[],
            blockers: &[blocker],
        })
    }

    fn admit(
        ticket: &Ticket,
        role: AdmissionRole,
        readiness: &Readiness,
    ) -> Result<(), AdmissionRefusal> {
        admit_execution(AdmissionInputs {
            ticket,
            project_archived: false,
            awaits_graph_approval: false,
            readiness,
            role,
        })
    }

    fn admit_in_proposed_graph(
        ticket: &Ticket,
        role: AdmissionRole,
    ) -> Result<(), AdmissionRefusal> {
        admit_execution(AdmissionInputs {
            ticket,
            project_archived: false,
            awaits_graph_approval: true,
            readiness: &ready(),
            role,
        })
    }

    #[test]
    fn an_archived_project_refuses_before_anything_else() {
        let ticket = at(task(), TicketState::Ready);
        assert_eq!(
            admit_execution(AdmissionInputs {
                ticket: &ticket,
                project_archived: true,
                awaits_graph_approval: false,
                readiness: &ready(),
                role: AdmissionRole::Implementer,
            }),
            Err(AdmissionRefusal::ArchivedProject)
        );
    }

    #[test]
    fn a_terminal_ticket_never_executes_again() {
        for state in [TicketState::Cancelled, TicketState::Superseded] {
            let ticket = at(task(), state);
            for role in [AdmissionRole::Implementer, AdmissionRole::Reviewer] {
                assert_eq!(
                    admit(&ticket, role, &ready()),
                    Err(AdmissionRefusal::TerminalTicket { state })
                );
            }
        }
    }

    #[test]
    fn an_unqualified_bug_is_refused_and_a_qualified_ready_bug_admits() {
        assert_eq!(
            admit(&bug(false), AdmissionRole::Implementer, &ready()),
            Err(AdmissionRefusal::UnqualifiedBug)
        );
        let qualified = at(bug(true), TicketState::Ready);
        assert_eq!(
            admit(&qualified, AdmissionRole::Implementer, &ready()),
            Ok(())
        );
    }

    #[test]
    fn an_implementation_admits_only_through_its_approved_graph_pin() {
        let unpinned = at(implementation(false), TicketState::Ready);
        assert_eq!(
            admit(&unpinned, AdmissionRole::Implementer, &ready()),
            Err(AdmissionRefusal::GraphUnapproved)
        );
        let pinned = at(implementation(true), TicketState::Ready);
        assert_eq!(admit(&pinned, AdmissionRole::Implementer, &ready()), Ok(()));
    }

    #[test]
    fn a_ticket_a_proposed_graph_names_waits_for_the_human_gate_whatever_its_kind() {
        for ticket in [
            at(task(), TicketState::Ready),
            at(bug(true), TicketState::Ready),
        ] {
            assert_eq!(
                admit_in_proposed_graph(&ticket, AdmissionRole::Implementer),
                Err(AdmissionRefusal::AwaitingGraphApproval),
                "{:?}",
                ticket.kind()
            );
            assert_eq!(
                admit(&ticket, AdmissionRole::Implementer, &ready()),
                Ok(()),
                "{:?}: no graph names it, so it stands alone",
                ticket.kind()
            );
        }
        let mut pinned = at(task(), TicketState::Ready);
        pinned.pin_to(1).expect("the fixture pins once");
        assert_eq!(
            admit_in_proposed_graph(&pinned, AdmissionRole::Implementer),
            Ok(()),
            "a member an approved graph already pinned executes through that pin"
        );
    }

    #[test]
    fn a_human_mode_task_never_admits_an_implementer_run() {
        let human = at(task_in(TaskMode::Human), TicketState::Ready);
        assert_eq!(
            admit(&human, AdmissionRole::Implementer, &ready()),
            Err(AdmissionRefusal::HumanTask)
        );
        assert_eq!(
            admit(&human, AdmissionRole::Reviewer, &ready()),
            Ok(()),
            "a reviewer evaluates the tip Sid submitted (KAN-S10)"
        );
    }

    #[test]
    fn an_implementer_run_admits_ready_or_active_work_only() {
        for state in TicketState::ALL.iter().copied() {
            let outcome = admit(&at(task(), state), AdmissionRole::Implementer, &ready());
            match state {
                TicketState::Ready | TicketState::Active => {
                    assert_eq!(outcome, Ok(()), "{state:?}")
                }
                TicketState::Cancelled | TicketState::Superseded => {
                    assert_eq!(outcome, Err(AdmissionRefusal::TerminalTicket { state }))
                }
                other => assert_eq!(
                    outcome,
                    Err(AdmissionRefusal::NotExecutable {
                        state: other,
                        role: AdmissionRole::Implementer
                    })
                ),
            }
        }
    }

    #[test]
    fn a_reviewer_run_admits_in_review_work_as_well() {
        for state in TicketState::ALL.iter().copied() {
            let outcome = admit(&at(task(), state), AdmissionRole::Reviewer, &blocked());
            match state {
                TicketState::Ready | TicketState::Active | TicketState::InReview => {
                    assert_eq!(
                        outcome,
                        Ok(()),
                        "{state:?}: a reviewer answers no readiness gate"
                    )
                }
                TicketState::Cancelled | TicketState::Superseded => {
                    assert_eq!(outcome, Err(AdmissionRefusal::TerminalTicket { state }))
                }
                other => assert_eq!(
                    outcome,
                    Err(AdmissionRefusal::NotExecutable {
                        state: other,
                        role: AdmissionRole::Reviewer
                    })
                ),
            }
        }
    }

    #[test]
    fn an_implementer_run_answers_the_current_readiness() {
        let ticket = at(task(), TicketState::Ready);
        assert_eq!(
            admit(&ticket, AdmissionRole::Implementer, &blocked()),
            Err(AdmissionRefusal::NotReady { waiting_on: 1 })
        );
    }

    #[test]
    fn every_refusal_names_its_reason() {
        assert_eq!(
            AdmissionRefusal::NotExecutable {
                state: TicketState::Scheduled,
                role: AdmissionRole::Implementer,
            }
            .to_string(),
            "the Ticket is scheduled; it is unavailable until its activation"
        );
        assert_eq!(
            AdmissionRefusal::NotExecutable {
                state: TicketState::Draft,
                role: AdmissionRole::Implementer,
            }
            .to_string(),
            "a draft Ticket is not executable; an implementer run admits a ready or active Ticket"
        );
        assert_eq!(
            AdmissionRefusal::NotExecutable {
                state: TicketState::Approved,
                role: AdmissionRole::Reviewer,
            }
            .to_string(),
            "an approved Ticket is not reviewable; a reviewer run admits a ready, active, or in_review Ticket"
        );
        assert_eq!(
            AdmissionRefusal::NotReady { waiting_on: 2 }.to_string(),
            "the Ticket is held back by 2 unresolved dependencies or external blockers"
        );
        assert_eq!(
            AdmissionRefusal::AwaitingGraphApproval.to_string(),
            "the Ticket is named by a Ticket graph still awaiting approval; \
             a human approves the graph before execution"
        );
        assert_eq!(
            AdmissionRefusal::HumanTask.to_string(),
            "the Task is human-mode; Sid executes it, so no implementer run is dispatched"
        );
    }
}
