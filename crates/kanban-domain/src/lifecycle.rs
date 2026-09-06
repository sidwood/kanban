//! The Ticket lifecycle rules (CONTEXT.md, DR-LC-01 to DR-LC-10): the
//! canonical state order is enforced as a closed transition table, the
//! kinds carry their own gates — a Bug is sealed into draft until its
//! qualification is complete (DR-TK-09), and no Ticket becomes ready
//! or starts work while a computed readiness still holds it back
//! (DR-DE-03) — ownership separates the actors (humans drag Task
//! Tickets only, DR-LC-07; Implementation and Bug transitions are
//! agent-owned, DR-LC-08), the human actions park, unpark, schedule,
//! cancel, and review decisions are named commands rather than drags
//! (DR-LC-09), and recovery moves a Ticket through one audited
//! emergency override that names its operator and its reason, never an
//! unrestricted drag (DR-LC-10). Readiness arrives as a value: this
//! module reads the projection KAN-T20 computes and never mutates it.

use std::fmt;

use crate::dependency::Readiness;
use crate::ticket::{Ticket, TicketKind, TicketState};

/// Who is moving a Ticket through its lifecycle. The transport is the
/// operator's own surface, so a drag arriving there is a human's; the
/// dispatch and MCP surfaces speak for agents (DR-LC-07, DR-LC-08).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// The operator, through the desktop or an operator command.
    Human,
    /// A dispatched agent, through its run-scoped surface.
    Agent,
}

/// One explicit human review decision (DR-LC-09): the review flows
/// that stage findings are KAN-S10's; this is the decision the
/// lifecycle records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    /// The review approves; the Ticket waits to land.
    Approve,
    /// The review rejects; the Ticket returns to work.
    Reject,
}

impl ReviewDecision {
    /// The stored and wire name of this decision.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }
}

/// The named human lifecycle commands (DR-LC-09). Each names one
/// lifecycle movement; none is a drag, so none answers the ownership
/// rule a drag answers — a human may park, unpark, schedule, cancel,
/// and review-decide every kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumanCommand {
    /// Set aside work that has not started executing.
    Park,
    /// Return parked work to circulation.
    Unpark,
    /// Hold qualified work until its activation.
    Schedule,
    /// End the Ticket. Cancelled is terminal (DR-LC-02).
    Cancel,
    /// Record one explicit review decision.
    Review(ReviewDecision),
}

/// The audited justification one emergency override carries (DR-LC-10):
/// who ran it and why. Assembled whole and refused blank, because the
/// audit row is only an audit when both parts name something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideJustification {
    who: String,
    why: String,
}

impl OverrideJustification {
    /// Assemble one justification, refusing a blank operator or a
    /// blank reason.
    pub fn new(who: &str, why: &str) -> Result<Self, LifecycleError> {
        let who = who.trim();
        if who.is_empty() {
            return Err(LifecycleError::Blank("operator"));
        }
        let why = why.trim();
        if why.is_empty() {
            return Err(LifecycleError::Blank("reason"));
        }
        Ok(Self {
            who: who.to_owned(),
            why: why.to_owned(),
        })
    }

    /// Who ran the override, as recorded.
    pub fn who(&self) -> &str {
        &self.who
    }

    /// Why the override ran, as recorded.
    pub fn why(&self) -> &str {
        &self.why
    }
}

/// Why a lifecycle move was refused. Every refusal leaves the Ticket
/// exactly as it stood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleError {
    /// The canonical lifecycle holds no edge between these states
    /// (DR-LC-01).
    Illegal {
        /// The state the Ticket holds.
        from: TicketState,
        /// The state the move named.
        to: TicketState,
    },
    /// Cancelled and superseded are terminal (DR-LC-02); neither
    /// accepts a further move.
    Terminal,
    /// Done is final; landed work accepts no further movement.
    Complete,
    /// A Bug is sealed into draft until its qualification is complete
    /// (DR-TK-09).
    UnqualifiedBug,
    /// The move enters ready or starts work while a computed readiness
    /// still holds the Ticket back (DR-DE-03). The value counts the
    /// dependencies and external blockers holding it.
    NotReady {
        /// How many unresolved dependencies or blockers hold the
        /// Ticket.
        waiting_on: usize,
    },
    /// A human dragged a kind whose transitions are agent-owned
    /// (DR-LC-08).
    AgentOwned {
        /// The kind that owns its own transitions.
        kind: TicketKind,
    },
    /// An emergency override field held nothing. The value names the
    /// field.
    Blank(&'static str),
    /// The Ticket already holds the named state.
    Unchanged,
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Illegal { from, to } => write!(
                f,
                "a Ticket moves along the canonical lifecycle; {} to {} is not one of its moves",
                from.wire_name(),
                to.wire_name()
            ),
            Self::Terminal => write!(
                f,
                "cancelled and superseded are terminal; the Ticket accepts no further changes"
            ),
            Self::Complete => write!(f, "done is final; the Ticket accepts no further movement"),
            Self::UnqualifiedBug => {
                write!(
                    f,
                    "a Bug stays draft until it carries a complete qualification"
                )
            }
            Self::NotReady { waiting_on } => write!(
                f,
                "the Ticket is held back by {waiting_on} unresolved dependencies or external blockers"
            ),
            Self::AgentOwned { kind } => write!(
                f,
                "{} transitions are agent-owned; a human may drag only Task Tickets",
                kind.wire_name()
            ),
            Self::Blank(field) => {
                write!(f, "an emergency override {field} cannot be blank")
            }
            Self::Unchanged => write!(f, "the Ticket already holds that state"),
        }
    }
}

impl std::error::Error for LifecycleError {}

/// Every state a Ticket may move to from `state` along the canonical
/// lifecycle (DR-LC-01). Terminal states and done hold no edges, and
/// cancelled and superseded are reached only by their own acts — the
/// cancel command and reassignment (DR-DE-07) — never by a drag.
pub fn legal_targets(state: TicketState) -> &'static [TicketState] {
    use TicketState as State;
    match state {
        State::Draft => &[
            State::Parked,
            State::Blocked,
            State::Scheduled,
            State::Ready,
        ],
        State::Parked => &[State::Ready],
        State::Blocked => &[State::Parked, State::Ready],
        State::Scheduled => &[State::Parked, State::Ready],
        State::Ready => &[State::Parked, State::Active],
        State::Active => &[State::InReview],
        State::InReview => &[State::Active, State::Approved],
        State::Approved => &[State::Landing],
        State::Landing => &[State::Done],
        State::Done | State::Cancelled | State::Superseded => &[],
    }
}

/// Whether a human may drag this kind through its transitions
/// (DR-LC-07, DR-LC-08): Task Tickets answer a drag; Implementation
/// and Bug transitions belong to the agents that execute them.
pub fn human_may_drag(kind: TicketKind) -> bool {
    matches!(kind, TicketKind::Task)
}

/// Move a Ticket to `to` as `actor` dragged it (DR-LC-06 to
/// DR-LC-08): a human drag answers the ownership rule first, then
/// every actor answers the same transition table and the same
/// kind-specific gates.
pub fn apply_drag(
    ticket: &mut Ticket,
    to: TicketState,
    actor: Actor,
    readiness: &Readiness,
) -> Result<(), LifecycleError> {
    if ticket.state().is_terminal() {
        return Err(LifecycleError::Terminal);
    }
    if matches!(actor, Actor::Human) && !human_may_drag(ticket.kind()) {
        return Err(LifecycleError::AgentOwned {
            kind: ticket.kind(),
        });
    }
    move_along(ticket, to, readiness)
}

/// Apply one named human command (DR-LC-09). The commands are not
/// drags: they serve every kind, and each maps onto the transition
/// table except cancel, which ends any Ticket that has not landed.
pub fn apply_command(
    ticket: &mut Ticket,
    command: HumanCommand,
    readiness: &Readiness,
) -> Result<(), LifecycleError> {
    if ticket.state().is_terminal() {
        return Err(LifecycleError::Terminal);
    }
    if ticket.state() == TicketState::Done {
        return Err(LifecycleError::Complete);
    }
    match command {
        // Cancel is its own act: it ends the Ticket from any open
        // state, exempt from the Bug's draft seal, because a
        // quick-captured Bug must be cancellable the moment it is
        // captured (DR-TK-08).
        HumanCommand::Cancel => {
            ticket.transition_state(TicketState::Cancelled);
            Ok(())
        }
        HumanCommand::Park => move_along(ticket, TicketState::Parked, readiness),
        // Unpark serves parked work alone: a blocked Ticket reaches
        // ready by its unblocking, not by this command, though the
        // edge itself is legal for the drag and the agent surface.
        HumanCommand::Unpark => {
            if ticket.state() != TicketState::Parked {
                return Err(LifecycleError::Illegal {
                    from: ticket.state(),
                    to: TicketState::Ready,
                });
            }
            move_along(ticket, TicketState::Ready, readiness)
        }
        HumanCommand::Schedule => move_along(ticket, TicketState::Scheduled, readiness),
        HumanCommand::Review(decision) => move_along(
            ticket,
            match decision {
                ReviewDecision::Approve => TicketState::Approved,
                ReviewDecision::Reject => TicketState::Active,
            },
            readiness,
        ),
    }
}

/// Apply the audited emergency override (DR-LC-10): recovery moves the
/// Ticket to any named state, past the transition table, the kind
/// gates, and the readiness gates alike, on the strength of the
/// justification the audit row carries. The drag surface never widens;
/// this command is the only way past the rules.
pub fn apply_override(
    ticket: &mut Ticket,
    to: TicketState,
    _justification: &OverrideJustification,
) -> Result<(), LifecycleError> {
    // The justification is validated whole before it exists, so a
    // caller cannot reach this point without a named operator and
    // reason; the audit row that carries them is the application
    // layer's to append.
    if ticket.state() == to {
        return Err(LifecycleError::Unchanged);
    }
    ticket.transition_state(to);
    Ok(())
}

/// Move to one table-legal target, checking the gates both actors
/// answer.
fn move_along(
    ticket: &mut Ticket,
    to: TicketState,
    readiness: &Readiness,
) -> Result<(), LifecycleError> {
    let from = ticket.state();
    if from == to {
        return Err(LifecycleError::Unchanged);
    }
    if !legal_targets(from).contains(&to) {
        return Err(LifecycleError::Illegal { from, to });
    }
    gate(ticket, from, to, readiness)?;
    ticket.transition_state(to);
    Ok(())
}

/// The kind-specific gates every legal move answers (DR-LC-06): a Bug
/// is sealed into draft until its qualification is complete, and no
/// Ticket becomes ready — or starts work from ready — while a computed
/// readiness still holds it back. A review rejection returns work that
/// already started, so it answers no readiness gate.
fn gate(
    ticket: &Ticket,
    from: TicketState,
    to: TicketState,
    readiness: &Readiness,
) -> Result<(), LifecycleError> {
    if from == TicketState::Draft && !ticket.is_qualified() {
        return Err(LifecycleError::UnqualifiedBug);
    }
    let starts_work = from == TicketState::Ready && to == TicketState::Active;
    if to == TicketState::Ready || starts_work {
        let waiting_on = readiness.blocked_by().len();
        if waiting_on > 0 {
            return Err(LifecycleError::NotReady { waiting_on });
        }
    }
    Ok(())
}

#[cfg(test)]
mod lifecycle_transitions {
    use super::{
        Actor, HumanCommand, LifecycleError, OverrideJustification, ReviewDecision, apply_command,
        apply_drag, apply_override, human_may_drag, legal_targets,
    };
    use crate::coverage::{AcceptanceCriterion, UserStoryRef, VerificationStep};
    use crate::dependency::{
        BlockerDescription, DependencyState, ExternalBlocker, ExternalBlockerId, Readiness,
        ReadinessInputs, TicketDependency, compute_readiness,
    };
    use crate::plan::SpecNumber;
    use crate::project::ProjectId;
    use crate::spec::SpecId;
    use crate::ticket::{
        BugQualification, Priority, Severity, TaskMode, TaskSubtype, TaskTiming, Ticket,
        TicketBody, TicketId, TicketKind, TicketNumber, TicketState,
    };

    /// The identity every fixture Ticket carries, so readiness
    /// fixtures can name it as the waiting endpoint.
    const WAITING: u64 = 1;

    fn number(value: u64) -> TicketNumber {
        TicketNumber::new(value).expect("the fixture number is positive")
    }

    /// One complete qualification, with the fields a test varies.
    fn qualified() -> BugQualification {
        let story = UserStoryRef::new(
            SpecNumber::new(1).expect("the fixture number is positive"),
            3,
        )
        .expect("the fixture ordinal is positive");
        BugQualification::new(
            "The integration branch survives every landing.",
            "Re land a reviewed change; the branch list still names it.",
            "macOS 26, Kanban 0.1.0, SQLite 3.50.",
            Severity::High,
            "Every landing so far.",
            "All landing reviews of every Project.",
            "Duplicate landings and lost review state.",
            vec![
                AcceptanceCriterion::new("The integration branch survives a landing.", vec![story])
                    .expect("the fixture criterion links"),
            ],
            vec![
                VerificationStep::new("cargo test -p kanban-storage tickets")
                    .expect("the fixture step carries its command"),
            ],
        )
        .expect("the fixture qualification is complete")
    }

    /// A Bug body, qualified or as quick capture left it.
    fn bug_body(is_qualified: bool) -> TicketBody {
        let body = TicketBody::bug(
            "Landing drops the integration branch",
            None,
            "The integration branch is dropped after a review lands.",
            "The landing log names the drop immediately after the merge.",
        )
        .expect("the fixture body validates");
        let TicketBody::Bug(boxed) = body else {
            unreachable!("the fixture body is a Bug's");
        };
        let qualification = if is_qualified {
            Some(qualified())
        } else {
            None
        };
        TicketBody::Bug(Box::new(crate::ticket::BugTicket::restore(
            boxed.title().to_owned(),
            boxed.spec(),
            boxed.actual_behaviour().to_owned(),
            boxed.reporter_evidence().to_owned(),
            qualification,
            crate::ticket::BugFacts::empty(),
        )))
    }

    /// One Task body, the kind a human may drag.
    fn task_body() -> TicketBody {
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

    /// One Implementation body, complete the moment it is created.
    fn implementation_body() -> TicketBody {
        let story = UserStoryRef::new(
            SpecNumber::new(1).expect("the fixture number is positive"),
            1,
        )
        .expect("the fixture ordinal is positive");
        TicketBody::implementation(
            Some(SpecId::new(7)),
            SpecNumber::new(1).expect("the fixture number is positive"),
            "Specs approve end to end",
            vec![
                AcceptanceCriterion::new("Approval freezes content.", vec![story])
                    .expect("the fixture criterion links"),
            ],
        )
        .expect("the fixture body validates")
    }

    /// One Ticket in the state a test chooses, at version 1.
    fn ticket_of(body: TicketBody, state: TicketState) -> Ticket {
        Ticket::restore(
            TicketId::new(WAITING),
            ProjectId::new(1),
            number(4),
            Priority::Normal,
            state,
            body,
            None,
            1,
        )
    }

    /// A Bug in the state a test chooses, qualified or not.
    fn bug(qualified: bool, state: TicketState) -> Ticket {
        ticket_of(bug_body(qualified), state)
    }

    /// A Task in the state a test chooses.
    fn task(state: TicketState) -> Ticket {
        ticket_of(task_body(), state)
    }

    /// An Implementation in the state a test chooses.
    fn implementation(state: TicketState) -> Ticket {
        ticket_of(implementation_body(), state)
    }

    /// One unsatisfied dependency: `from` has not landed.
    fn waiting(from: u64, state: TicketState) -> DependencyState {
        DependencyState {
            dependency: TicketDependency::new(TicketId::new(from), TicketId::new(WAITING)),
            state,
        }
    }

    /// One recorded external blocker.
    fn external(id: u64) -> ExternalBlocker {
        ExternalBlocker::restore(
            ExternalBlockerId::new(id),
            TicketId::new(WAITING),
            BlockerDescription::new("The vendor SDK 4 upgrade")
                .expect("the fixture description validates"),
        )
    }

    /// The readiness a test computes: nothing blocking unless named.
    fn readiness_of(dependencies: &[DependencyState], blockers: &[ExternalBlocker]) -> Readiness {
        compute_readiness(ReadinessInputs {
            dependencies,
            blockers,
        })
    }

    /// The clear readiness: nothing holds the Ticket back.
    fn clear() -> Readiness {
        readiness_of(&[], &[])
    }

    #[test]
    fn the_canonical_lifecycle_holds_exactly_these_moves() {
        use TicketState as State;
        assert_eq!(
            legal_targets(State::Draft),
            &[
                State::Parked,
                State::Blocked,
                State::Scheduled,
                State::Ready
            ]
        );
        assert_eq!(legal_targets(State::Parked), &[State::Ready]);
        assert_eq!(
            legal_targets(State::Blocked),
            &[State::Parked, State::Ready]
        );
        assert_eq!(
            legal_targets(State::Scheduled),
            &[State::Parked, State::Ready]
        );
        assert_eq!(legal_targets(State::Ready), &[State::Parked, State::Active]);
        assert_eq!(legal_targets(State::Active), &[State::InReview]);
        assert_eq!(
            legal_targets(State::InReview),
            &[State::Active, State::Approved]
        );
        assert_eq!(legal_targets(State::Approved), &[State::Landing]);
        assert_eq!(legal_targets(State::Landing), &[State::Done]);
        assert_eq!(legal_targets(State::Done), &[]);
        assert_eq!(legal_targets(State::Cancelled), &[]);
        assert_eq!(legal_targets(State::Superseded), &[]);
    }

    #[test]
    fn terminal_and_landed_tickets_accept_no_drag() {
        for state in [TicketState::Cancelled, TicketState::Superseded] {
            let error = apply_drag(
                &mut ticket_of(task_body(), state),
                TicketState::Ready,
                Actor::Human,
                &clear(),
            )
            .expect_err("a terminal Ticket accepts no drag");
            assert_eq!(error, LifecycleError::Terminal);
        }
        let error = apply_drag(
            &mut task(TicketState::Done),
            TicketState::Active,
            Actor::Human,
            &clear(),
        )
        .expect_err("done holds no outgoing edge");
        assert_eq!(
            error,
            LifecycleError::Illegal {
                from: TicketState::Done,
                to: TicketState::Active
            }
        );
    }

    #[test]
    fn a_human_drags_a_task_ticket_and_the_change_counts() {
        let mut dragged = task(TicketState::Draft);

        apply_drag(&mut dragged, TicketState::Ready, Actor::Human, &clear())
            .expect("a human drags a Task through a legal transition");

        assert_eq!(dragged.state(), TicketState::Ready);
        assert_eq!(dragged.version(), 2, "the applied move bumps the version");
    }

    #[test]
    fn a_human_drag_of_an_agent_owned_kind_is_refused_with_an_explanation() {
        for body in [implementation_body(), bug_body(true)] {
            let mut refused = ticket_of(body.clone(), TicketState::Draft);
            let kind = refused.kind();
            let error = apply_drag(&mut refused, TicketState::Ready, Actor::Human, &clear())
                .expect_err("Implementation and Bug transitions are agent-owned");

            assert_eq!(error, LifecycleError::AgentOwned { kind });
            assert_eq!(
                error.to_string(),
                format!(
                    "{} transitions are agent-owned; a human may drag only Task Tickets",
                    kind.wire_name()
                )
            );
            assert_eq!(
                refused.state(),
                TicketState::Draft,
                "the refusal moved nothing"
            );
            assert_eq!(refused.version(), 1, "the refusal changed nothing");
        }
        assert!(human_may_drag(TicketKind::Task));
        assert!(!human_may_drag(TicketKind::Implementation));
        assert!(!human_may_drag(TicketKind::Bug));
    }

    #[test]
    fn an_agent_owns_the_agent_owned_kinds_moves() {
        let mut slice = implementation(TicketState::Draft);
        apply_drag(&mut slice, TicketState::Ready, Actor::Agent, &clear())
            .expect("an agent moves an Implementation along a legal edge");
        assert_eq!(slice.state(), TicketState::Ready);

        let mut defect = bug(true, TicketState::Draft);
        apply_drag(&mut defect, TicketState::Ready, Actor::Agent, &clear())
            .expect("an agent moves a qualified Bug along a legal edge");
        assert_eq!(defect.state(), TicketState::Ready);

        let mut chore = task(TicketState::Draft);
        apply_drag(&mut chore, TicketState::Ready, Actor::Agent, &clear())
            .expect("an agent may move a Task too");
        assert_eq!(chore.state(), TicketState::Ready);
    }

    #[test]
    fn an_unqualified_bug_is_sealed_into_draft() {
        for to in [
            TicketState::Parked,
            TicketState::Blocked,
            TicketState::Scheduled,
            TicketState::Ready,
        ] {
            let mut sealed = bug(false, TicketState::Draft);
            let error = apply_drag(&mut sealed, to, Actor::Agent, &clear())
                .expect_err("a captured Bug stays draft until qualified");
            assert_eq!(error, LifecycleError::UnqualifiedBug, "draft to {to:?}");
            assert_eq!(sealed.state(), TicketState::Draft);
        }
        assert_eq!(
            LifecycleError::UnqualifiedBug.to_string(),
            "a Bug stays draft until it carries a complete qualification"
        );

        let mut qualified = bug(true, TicketState::Draft);
        apply_drag(&mut qualified, TicketState::Ready, Actor::Agent, &clear())
            .expect("a qualified Bug leaves draft");
        assert_eq!(qualified.state(), TicketState::Ready);
    }

    #[test]
    fn the_other_kinds_are_qualified_the_moment_they_are_created() {
        assert!(implementation(TicketState::Draft).is_qualified());
        assert!(task(TicketState::Draft).is_qualified());
        assert!(!bug(false, TicketState::Draft).is_qualified());
        assert!(bug(true, TicketState::Draft).is_qualified());
    }

    #[test]
    fn readiness_gates_becoming_ready_and_starting_work() {
        let dependencies = [waiting(9, TicketState::Active)];
        let held = readiness_of(&dependencies, &[]);

        let mut chore = task(TicketState::Draft);
        let error = apply_drag(&mut chore, TicketState::Ready, Actor::Human, &held)
            .expect_err("a Ticket waiting on an unlanded dependency is not ready");
        assert_eq!(error, LifecycleError::NotReady { waiting_on: 1 });
        assert_eq!(
            error.to_string(),
            "the Ticket is held back by 1 unresolved dependencies or external blockers"
        );
        assert_eq!(chore.state(), TicketState::Draft);

        // Parking and blocking answer no readiness gate: setting work
        // aside is always available.
        let mut parked = task(TicketState::Draft);
        apply_drag(&mut parked, TicketState::Parked, Actor::Human, &held)
            .expect("parking is not gated on readiness");

        // The dependency lands and the same move passes.
        let landed = readiness_of(&[waiting(9, TicketState::Done)], &[]);
        let mut ready = task(TicketState::Draft);
        apply_drag(&mut ready, TicketState::Ready, Actor::Human, &landed)
            .expect("a landed dependency stops holding the Ticket");

        // Starting work re-checks readiness: a blocker that appeared
        // while the Ticket stood ready holds it.
        let blocked = readiness_of(&[], &[external(4)]);
        let error = apply_drag(
            &mut ready.clone(),
            TicketState::Active,
            Actor::Human,
            &blocked,
        )
        .expect_err("starting work re-checks readiness");
        assert_eq!(error, LifecycleError::NotReady { waiting_on: 1 });

        // A review rejection returns work that already started, so it
        // answers no readiness gate.
        let mut bounced = task(TicketState::InReview);
        apply_command(
            &mut bounced,
            HumanCommand::Review(ReviewDecision::Reject),
            &blocked,
        )
        .expect("a rejection returns started work whatever waits now");
        assert_eq!(bounced.state(), TicketState::Active);
    }

    #[test]
    fn an_illegal_move_names_both_states() {
        let mut chore = task(TicketState::Active);
        let error = apply_drag(&mut chore, TicketState::Done, Actor::Human, &clear())
            .expect_err("active lands only through review");
        assert_eq!(
            error,
            LifecycleError::Illegal {
                from: TicketState::Active,
                to: TicketState::Done
            }
        );
        assert_eq!(
            error.to_string(),
            "a Ticket moves along the canonical lifecycle; active to done is not one of its moves"
        );
        assert_eq!(
            chore.state(),
            TicketState::Active,
            "the refusal moved nothing"
        );
    }

    #[test]
    fn a_move_to_the_held_state_is_refused() {
        let mut chore = task(TicketState::Ready);
        let error = apply_drag(&mut chore, TicketState::Ready, Actor::Human, &clear())
            .expect_err("a move to the held state is a no-op");
        assert_eq!(error, LifecycleError::Unchanged);
        assert_eq!(chore.version(), 1);
    }

    #[test]
    fn park_sets_aside_work_that_has_not_started_executing() {
        for from in [
            TicketState::Draft,
            TicketState::Blocked,
            TicketState::Scheduled,
            TicketState::Ready,
        ] {
            let mut parked = task(from);
            apply_command(&mut parked, HumanCommand::Park, &clear())
                .unwrap_or_else(|error| panic!("park serves {from:?}: {error}"));
            assert_eq!(parked.state(), TicketState::Parked);
        }
        for from in [
            TicketState::Active,
            TicketState::InReview,
            TicketState::Approved,
            TicketState::Landing,
        ] {
            let mut refused = task(from);
            let error = apply_command(&mut refused, HumanCommand::Park, &clear())
                .expect_err("executing work is not parked; it is cancelled");
            assert_eq!(
                error,
                LifecycleError::Illegal {
                    from,
                    to: TicketState::Parked
                }
            );
        }
        let mut landed = task(TicketState::Done);
        assert_eq!(
            apply_command(&mut landed, HumanCommand::Park, &clear()),
            Err(LifecycleError::Complete)
        );
    }

    #[test]
    fn unpark_returns_parked_work_to_ready() {
        let mut parked = task(TicketState::Parked);
        apply_command(&mut parked, HumanCommand::Unpark, &clear())
            .expect("unpark returns the Ticket to circulation");
        assert_eq!(parked.state(), TicketState::Ready);
        assert_eq!(parked.version(), 2);

        let mut elsewhere = task(TicketState::Blocked);
        assert_eq!(
            apply_command(&mut elsewhere, HumanCommand::Unpark, &clear()),
            Err(LifecycleError::Illegal {
                from: TicketState::Blocked,
                to: TicketState::Ready
            }),
            "unpark serves parked work alone"
        );
    }

    #[test]
    fn scheduling_holds_work_until_its_activation() {
        let mut defect = bug(true, TicketState::Draft);
        apply_command(&mut defect, HumanCommand::Schedule, &clear())
            .expect("a qualified Bug schedules");
        assert_eq!(defect.state(), TicketState::Scheduled);

        let mut chore = task(TicketState::Ready);
        assert_eq!(
            apply_command(&mut chore, HumanCommand::Schedule, &clear()),
            Err(LifecycleError::Illegal {
                from: TicketState::Ready,
                to: TicketState::Scheduled
            }),
            "schedule holds work before it circulates, not after"
        );

        let mut captured = bug(false, TicketState::Draft);
        assert_eq!(
            apply_command(&mut captured, HumanCommand::Schedule, &clear()),
            Err(LifecycleError::UnqualifiedBug),
            "an unqualified Bug schedules nothing"
        );
    }

    #[test]
    fn cancel_ends_any_open_ticket_of_any_kind() {
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
            let mut cancelled = task(from);
            apply_command(&mut cancelled, HumanCommand::Cancel, &clear())
                .unwrap_or_else(|error| panic!("cancel serves {from:?}: {error}"));
            assert_eq!(cancelled.state(), TicketState::Cancelled);
        }

        // Cancel is the one way out of draft an unqualified Bug keeps:
        // quick capture must stay reversible (DR-TK-08).
        let mut captured = bug(false, TicketState::Draft);
        apply_command(&mut captured, HumanCommand::Cancel, &clear())
            .expect("a captured Bug cancels without qualifying");
        assert_eq!(captured.state(), TicketState::Cancelled);
        assert_eq!(
            apply_command(&mut captured, HumanCommand::Cancel, &clear()),
            Err(LifecycleError::Terminal),
            "cancelled is terminal"
        );

        let mut landed = task(TicketState::Done);
        assert_eq!(
            apply_command(&mut landed, HumanCommand::Cancel, &clear()),
            Err(LifecycleError::Complete),
            "done is never cancelled"
        );
    }

    #[test]
    fn review_decisions_resolve_in_review_both_ways() {
        let mut approved = task(TicketState::InReview);
        apply_command(
            &mut approved,
            HumanCommand::Review(ReviewDecision::Approve),
            &clear(),
        )
        .expect("an approval stages the Ticket for landing");
        assert_eq!(approved.state(), TicketState::Approved);
        assert_eq!(approved.version(), 2);

        let mut rejected = task(TicketState::InReview);
        apply_command(
            &mut rejected,
            HumanCommand::Review(ReviewDecision::Reject),
            &clear(),
        )
        .expect("a rejection returns the Ticket to work");
        assert_eq!(rejected.state(), TicketState::Active);

        assert_eq!(ReviewDecision::Approve.as_str(), "approve");
        assert_eq!(ReviewDecision::Reject.as_str(), "reject");

        let mut untouched = task(TicketState::Ready);
        assert_eq!(
            apply_command(
                &mut untouched,
                HumanCommand::Review(ReviewDecision::Approve),
                &clear()
            ),
            Err(LifecycleError::Illegal {
                from: TicketState::Ready,
                to: TicketState::Approved
            }),
            "a review decision resolves in review alone"
        );
    }

    #[test]
    fn the_emergency_override_moves_any_ticket_and_names_its_reason() {
        let justification =
            OverrideJustification::new(" Sid Wood ", "  Recovery after the core crashed mid move ")
                .expect("a named operator and reason justify the override");
        assert_eq!(justification.who(), "Sid Wood");
        assert_eq!(
            justification.why(),
            "Recovery after the core crashed mid move"
        );

        // Backwards past the table: active work returns to ready.
        let mut resumed = task(TicketState::Active);
        apply_override(&mut resumed, TicketState::Ready, &justification)
            .expect("recovery moves against the canonical order");
        assert_eq!(resumed.state(), TicketState::Ready);
        assert_eq!(resumed.version(), 2);

        // Past the kind gates: an unqualified Bug recovers to ready.
        let mut recovered = bug(false, TicketState::Draft);
        apply_override(&mut recovered, TicketState::Ready, &justification)
            .expect("recovery answers no qualification gate");
        assert_eq!(recovered.state(), TicketState::Ready);

        // Out of a terminal state: a mistaken cancel is undone.
        let mut revived = bug(true, TicketState::Cancelled);
        apply_override(&mut revived, TicketState::Ready, &justification)
            .expect("recovery reaches past terminal states");
        assert_eq!(revived.state(), TicketState::Ready);

        // To a terminal state: manual completion when automation broke.
        let mut completed = task(TicketState::Active);
        apply_override(&mut completed, TicketState::Done, &justification)
            .expect("recovery may complete work by hand");
        assert_eq!(completed.state(), TicketState::Done);
    }

    #[test]
    fn an_override_requires_an_operator_and_a_reason() {
        assert_eq!(
            OverrideJustification::new("   ", "Because.").unwrap_err(),
            LifecycleError::Blank("operator")
        );
        assert_eq!(
            OverrideJustification::new("Sid Wood", "  ").unwrap_err(),
            LifecycleError::Blank("reason")
        );
        assert_eq!(
            LifecycleError::Blank("reason").to_string(),
            "an emergency override reason cannot be blank"
        );

        let justification =
            OverrideJustification::new("Sid Wood", "Recovery").expect("the fixture justifies");
        let mut landed = task(TicketState::Done);
        assert_eq!(
            apply_override(&mut landed, TicketState::Done, &justification),
            Err(LifecycleError::Unchanged),
            "an override to the held state recovers nothing"
        );
        assert_eq!(landed.version(), 1);
    }

    #[test]
    fn a_task_walks_the_canonical_order_end_to_end() {
        let mut chore = task(TicketState::Draft);
        let human = Actor::Human;
        let clear = clear();

        apply_command(&mut chore, HumanCommand::Schedule, &clear).expect("draft schedules");
        apply_drag(&mut chore, TicketState::Ready, human, &clear)
            .expect("scheduled work becomes ready on activation");
        apply_drag(&mut chore, TicketState::Active, human, &clear).expect("ready work starts");
        apply_drag(&mut chore, TicketState::InReview, human, &clear)
            .expect("active work enters review");
        apply_command(
            &mut chore,
            HumanCommand::Review(ReviewDecision::Approve),
            &clear,
        )
        .expect("the review approves");
        apply_drag(&mut chore, TicketState::Landing, human, &clear).expect("approved work lands");
        apply_drag(&mut chore, TicketState::Done, human, &clear).expect("landing completes");

        assert_eq!(chore.state(), TicketState::Done);
        assert_eq!(chore.version(), 8, "every applied move counted");
        assert_eq!(
            apply_drag(&mut chore, TicketState::Active, human, &clear),
            Err(LifecycleError::Illegal {
                from: TicketState::Done,
                to: TicketState::Active
            }),
            "done holds no further edge"
        );
    }
}
