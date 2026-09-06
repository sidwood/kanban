//! The global board query's pure rules (DR-BP-01): the closed filter
//! vocabulary every axis of the global board draws from, the matching
//! rule that composes those axes, and the deterministic card order the
//! filtered projection keeps (DR-LC-11). The board itself owns no
//! state here: a card arrives as one Ticket beside the facts its
//! Project, Spec, Lane, and attention projection resolve, and the
//! rules answer two questions — does this card pass the filter, and
//! where does it sit among the others. Initiative, Project, Plan,
//! Spec, kind, state, priority, Lane, execution profile, and attention
//! state are the ten axes (DR-BP-01); every axis selects by set
//! membership, an empty set constrains nothing, and separate axes
//! compose as an intersection, so a card the operator sees satisfies
//! every selected axis at once. Attention state is the one vocabulary
//! the board names before its feed exists: the classes are fixed here
//! and the per-Ticket projection populates as KAN-S11 lands.

use crate::initiative::InitiativeId;
use crate::lane::LaneId;
use crate::plan::PlanId;
use crate::profile::ProfileName;
use crate::project::ProjectId;
use crate::spec::SpecId;
use crate::ticket::{Priority, Ticket, TicketKind, TicketState};

/// Why one Ticket demands operator attention: the closed Attention
/// Item classes from CONTEXT.md. Attention Items are a projection
/// (KAN-S11); this vocabulary is the board's filter surface, complete
/// before the feed lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionState {
    /// A registered dependency or external blocker holds the work.
    Blocker,
    /// A settled run finished without its required submission.
    MissingResult,
    /// The work waits on a human decision.
    HumanDecision,
    /// A review asks the operator in.
    ReviewRequest,
    /// A schedule failed to activate or mint its occurrence.
    FailedSchedule,
    /// An approval no longer validates.
    InvalidApproval,
    /// The session observing the work disconnected.
    DisconnectedSession,
    /// A run passed its stall deadline.
    StaleRun,
}

impl AttentionState {
    /// Every class, in vocabulary order.
    pub const ALL: &'static [Self] = &[
        Self::Blocker,
        Self::MissingResult,
        Self::HumanDecision,
        Self::ReviewRequest,
        Self::FailedSchedule,
        Self::InvalidApproval,
        Self::DisconnectedSession,
        Self::StaleRun,
    ];

    /// The stored and wire name of this class.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Blocker => "blocker",
            Self::MissingResult => "missing_result",
            Self::HumanDecision => "human_decision",
            Self::ReviewRequest => "review_request",
            Self::FailedSchedule => "failed_schedule",
            Self::InvalidApproval => "invalid_approval",
            Self::DisconnectedSession => "disconnected_session",
            Self::StaleRun => "stale_run",
        }
    }

    /// The class a stored row names, or `None` outside the vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|class| class.wire_name() == stored)
    }
}

/// One card as the global board sees it: the Ticket itself, carrying
/// its Project, Spec, kind, state, priority, and assigned profile, and
/// the three facts only a wider read resolves — the Initiative its
/// Project sits under, the Plan its Spec belongs to, the Lane holding
/// it — plus the attention classes currently raised on it.
pub struct BoardCard<'a> {
    /// The Ticket this card projects.
    pub ticket: &'a Ticket,
    /// The Initiative the Ticket's Project sits under, if any.
    pub initiative: Option<InitiativeId>,
    /// The Plan the Ticket's Spec belongs to, if the Ticket attaches
    /// to a planned Spec.
    pub plan: Option<PlanId>,
    /// The Lane holding the Ticket as its active occupant, if one
    /// does.
    pub lane: Option<LaneId>,
    /// The attention classes currently raised on the Ticket; empty
    /// until the attention projection lands (KAN-S11).
    pub attention: &'a [AttentionState],
}

impl<'a> BoardCard<'a> {
    /// The Ticket's immutable identity.
    pub fn id(&self) -> u64 {
        self.ticket.id().value()
    }

    /// The Ticket's Project.
    pub fn project(&self) -> ProjectId {
        self.ticket.project()
    }

    /// The Ticket's lifecycle state.
    pub fn state(&self) -> TicketState {
        self.ticket.state()
    }
}

/// The global board's filter: one set per axis (DR-BP-01). An empty
/// set leaves its axis unconstrained; a card passes the filter when it
/// passes every non-empty axis, and within one axis when it holds any
/// of the selected values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoardFilter {
    /// The Initiatives whose Projects' work shows.
    pub initiatives: Vec<InitiativeId>,
    /// The Projects whose work shows.
    pub projects: Vec<ProjectId>,
    /// The Plans whose Specs' work shows.
    pub plans: Vec<PlanId>,
    /// The Specs whose work shows.
    pub specs: Vec<SpecId>,
    /// The kinds that show.
    pub kinds: Vec<TicketKind>,
    /// The lifecycle states that show.
    pub states: Vec<TicketState>,
    /// The priorities that show.
    pub priorities: Vec<Priority>,
    /// The Lanes whose held work shows.
    pub lanes: Vec<LaneId>,
    /// The Execution Profiles whose assigned work shows, by name.
    pub profiles: Vec<ProfileName>,
    /// The attention classes whose raised work shows.
    pub attention: Vec<AttentionState>,
}

impl BoardFilter {
    /// Whether no axis constrains anything: an empty filter is the
    /// whole board.
    pub fn is_empty(&self) -> bool {
        self.initiatives.is_empty()
            && self.projects.is_empty()
            && self.plans.is_empty()
            && self.specs.is_empty()
            && self.kinds.is_empty()
            && self.states.is_empty()
            && self.priorities.is_empty()
            && self.lanes.is_empty()
            && self.profiles.is_empty()
            && self.attention.is_empty()
    }
}

/// Whether one axis with values admits the card's value for it.
fn selected<T: PartialEq>(values: &[T], held: Option<&T>) -> bool {
    values.is_empty()
        || values
            .iter()
            .any(|value| held.is_some_and(|value2| value2 == value))
}

/// Whether one card passes the filter: every non-empty axis must
/// admit it, and an axis with values admits the card only when the
/// card holds one of them. A card without the fact an axis selects —
/// no Initiative, no Spec, no Lane, no profile, no raised attention —
/// passes that axis only while the axis is empty.
pub fn admits(filter: &BoardFilter, card: &BoardCard) -> bool {
    let ticket = card.ticket;
    selected(&filter.initiatives, card.initiative.as_ref())
        && selected(&filter.projects, Some(&ticket.project()))
        && selected(&filter.plans, card.plan.as_ref())
        && selected(&filter.specs, ticket.spec().as_ref())
        && selected(&filter.kinds, Some(&ticket.kind()))
        && selected(&filter.states, Some(&ticket.state()))
        && selected(&filter.priorities, Some(&ticket.priority()))
        && selected(&filter.lanes, card.lane.as_ref())
        && selected(&filter.profiles, ticket.profile())
        && (filter.attention.is_empty()
            || card
                .attention
                .iter()
                .any(|class| filter.attention.contains(class)))
}

/// The canonical lifecycle position of one state: its place in the
/// closed vocabulary (DR-LC-01), so ordering reads the same order
/// every state surface agrees on.
fn readiness_position(state: TicketState) -> usize {
    TicketState::ALL
        .iter()
        .position(|candidate| *candidate == state)
        .expect("the closed vocabulary holds every state")
}

/// The deterministic order the filtered projection keeps (DR-LC-11):
/// priority first (urgent before high before normal before low), then
/// readiness — inside a group the card closer to landing sits higher —
/// then the Project and the minted number as the global tiebreak, so
/// no two cards of the whole board ever contend for a place and
/// position is never a decision.
pub fn compare_cards(a: &BoardCard, b: &BoardCard) -> std::cmp::Ordering {
    a.ticket
        .priority()
        .queue_rank()
        .cmp(&b.ticket.priority().queue_rank())
        .then_with(|| {
            readiness_position(b.ticket.state()).cmp(&readiness_position(a.ticket.state()))
        })
        .then_with(|| a.ticket.project().value().cmp(&b.ticket.project().value()))
        .then_with(|| a.ticket.number().value().cmp(&b.ticket.number().value()))
}

/// Sort a projection's cards into the deterministic order.
pub fn sort_cards(cards: &mut [BoardCard<'_>]) {
    cards.sort_by(|a, b| compare_cards(a, b));
}

#[cfg(test)]
mod tests {
    use super::{AttentionState, BoardCard, BoardFilter, admits, compare_cards, sort_cards};
    use crate::initiative::InitiativeId;
    use crate::lane::LaneId;
    use crate::plan::{PlanId, SpecNumber};
    use crate::profile::ProfileName;
    use crate::project::ProjectId;
    use crate::spec::SpecId;
    use crate::ticket::{
        Priority, TaskMode, TaskSubtype, TaskTiming, Ticket, TicketBody, TicketId, TicketKind,
        TicketNumber, TicketState,
    };

    /// One Ticket restored exactly as a stored row, varied by the
    /// fields a test names. Defaults: Task kind, draft, normal
    /// priority, Project 1, number 1, no Spec, no profile.
    #[allow(clippy::too_many_arguments)]
    fn ticket(
        project: u64,
        number: u64,
        priority: Priority,
        state: TicketState,
        spec: Option<SpecId>,
        profile: Option<ProfileName>,
        kind: TicketKind,
    ) -> Ticket {
        let body = match kind {
            TicketKind::Implementation => TicketBody::implementation(
                spec,
                SpecNumber::new(1).expect("the fixture number validates"),
                "Spec authoring creates content versions end to end".to_owned(),
                vec![],
            )
            .expect("the fixture Implementation validates"),
            TicketKind::Bug => TicketBody::bug(
                "Landing drops the integration branch".to_owned(),
                spec,
                "The integration branch is dropped after a review lands.".to_owned(),
                "The landing log names the drop immediately after the merge.".to_owned(),
            )
            .expect("the fixture Bug validates"),
            TicketKind::Task => TicketBody::task(
                "Archive the old register".to_owned(),
                spec,
                Some(TaskSubtype::Operational),
                Some(TaskMode::Human),
                vec![
                    crate::ticket::CompletionCriterion::new(
                        "The old register is archived and restorable.",
                    )
                    .expect("the fixture criterion validates"),
                ],
                TaskTiming::none(),
            )
            .expect("the fixture Task validates"),
        };
        Ticket::restore(
            TicketId::new(number),
            ProjectId::new(project),
            TicketNumber::new(number).expect("the fixture number validates"),
            priority,
            state,
            body,
            None,
            profile,
            None,
            1,
        )
    }

    /// The facts beside a Ticket, varied by the axes a test names.
    /// Defaults resolve nothing: no Initiative, no Plan, no Lane, no
    /// raised attention.
    fn card<'a>(
        ticket: &'a Ticket,
        initiative: Option<u64>,
        plan: Option<u64>,
        lane: Option<u64>,
        attention: &'a [AttentionState],
    ) -> BoardCard<'a> {
        BoardCard {
            ticket,
            initiative: initiative.map(InitiativeId::new),
            plan: plan.map(PlanId::new),
            lane: lane.map(LaneId::new),
            attention,
        }
    }

    /// A Task Ticket with the fields one test varies.
    fn task(project: u64, number: u64, priority: Priority, state: TicketState) -> Ticket {
        ticket(
            project,
            number,
            priority,
            state,
            None,
            None,
            TicketKind::Task,
        )
    }

    const CLEAR: &[AttentionState] = &[];

    #[test]
    fn an_empty_filter_admits_every_card() {
        let standing = task(1, 1, Priority::Normal, TicketState::Active);
        let unattached = card(&standing, None, None, None, CLEAR);

        assert!(admits(&BoardFilter::default(), &unattached));
        assert!(BoardFilter::default().is_empty());
    }

    #[test]
    fn the_initiative_axis_selects_the_projects_work() {
        let owned = task(1, 1, Priority::Normal, TicketState::Active);
        let under = card(&owned, Some(2), None, None, CLEAR);
        let elsewhere = card(&owned, Some(3), None, None, CLEAR);
        let floating = card(&owned, None, None, None, CLEAR);

        let filter = BoardFilter {
            initiatives: vec![InitiativeId::new(2), InitiativeId::new(4)],
            ..BoardFilter::default()
        };

        assert!(admits(&filter, &under), "a selected Initiative admits");
        assert!(!admits(&filter, &elsewhere), "another Initiative stays out");
        assert!(
            !admits(&filter, &floating),
            "a Project under no Initiative stays out while the axis selects"
        );
    }

    #[test]
    fn the_project_axis_selects_one_projects_work() {
        let core = task(1, 1, Priority::Normal, TicketState::Active);
        let edge = task(2, 2, Priority::Normal, TicketState::Active);
        let core_card = card(&core, None, None, None, CLEAR);
        let edge_card = card(&edge, None, None, None, CLEAR);

        let filter = BoardFilter {
            projects: vec![ProjectId::new(1)],
            ..BoardFilter::default()
        };

        assert!(admits(&filter, &core_card));
        assert!(!admits(&filter, &edge_card));
    }

    #[test]
    fn the_plan_axis_selects_planned_specs_work() {
        let planned = task(1, 1, Priority::Normal, TicketState::Active);
        let planned_card = card(&planned, None, Some(7), None, CLEAR);
        let other_plan = card(&planned, None, Some(8), None, CLEAR);
        let unplanned = card(&planned, None, None, None, CLEAR);

        let filter = BoardFilter {
            plans: vec![PlanId::new(7)],
            ..BoardFilter::default()
        };

        assert!(admits(&filter, &planned_card));
        assert!(!admits(&filter, &other_plan));
        assert!(
            !admits(&filter, &unplanned),
            "work no Plan holds stays out while the axis selects"
        );
    }

    #[test]
    fn the_spec_axis_selects_attached_work() {
        let attached = ticket(
            1,
            1,
            Priority::Normal,
            TicketState::Active,
            Some(SpecId::new(4)),
            None,
            TicketKind::Bug,
        );
        let attached_card = card(&attached, None, None, None, CLEAR);
        let other_spec = ticket(
            1,
            1,
            Priority::Normal,
            TicketState::Active,
            Some(SpecId::new(5)),
            None,
            TicketKind::Bug,
        );
        let other_card = card(&other_spec, None, None, None, CLEAR);
        let standalone = task(1, 1, Priority::Normal, TicketState::Active);
        let free_card = card(&standalone, None, None, None, CLEAR);

        let filter = BoardFilter {
            specs: vec![SpecId::new(4)],
            ..BoardFilter::default()
        };

        assert!(admits(&filter, &attached_card));
        assert!(!admits(&filter, &other_card));
        assert!(
            !admits(&filter, &free_card),
            "a Ticket attached to no Spec stays out while the axis selects"
        );
    }

    #[test]
    fn the_kind_state_and_priority_axes_select_their_vocabularies() {
        let active = task(1, 1, Priority::Urgent, TicketState::Active);
        let active_card = card(&active, None, None, None, CLEAR);
        let parked = task(1, 2, Priority::Normal, TicketState::Parked);
        let parked_card = card(&parked, None, None, None, CLEAR);
        let bug = ticket(
            1,
            3,
            Priority::Normal,
            TicketState::Active,
            None,
            None,
            TicketKind::Bug,
        );
        let bug_card = card(&bug, None, None, None, CLEAR);

        let filter = BoardFilter {
            kinds: vec![TicketKind::Task],
            states: vec![TicketState::Active],
            priorities: vec![Priority::Urgent],
            ..BoardFilter::default()
        };

        assert!(admits(&filter, &active_card));
        for refused in [&parked_card, &bug_card] {
            assert!(!admits(&filter, refused));
        }

        // Within one axis the selected values unite: two priorities
        // selected together still admit either.
        let united = BoardFilter {
            priorities: vec![Priority::Urgent, Priority::Normal],
            ..BoardFilter::default()
        };
        assert!(admits(&united, &active_card));
        assert!(admits(&united, &parked_card));
        assert!(!admits(
            &united,
            &card(
                &task(1, 4, Priority::Low, TicketState::Active),
                None,
                None,
                None,
                CLEAR
            )
        ));
    }

    #[test]
    fn the_lane_axis_selects_held_work() {
        let held = task(1, 1, Priority::Normal, TicketState::Active);
        let held_card = card(&held, None, None, Some(5), CLEAR);
        let other_lane = card(&held, None, None, Some(6), CLEAR);
        let unheld = card(&held, None, None, None, CLEAR);

        let filter = BoardFilter {
            lanes: vec![LaneId::new(5)],
            ..BoardFilter::default()
        };

        assert!(admits(&filter, &held_card));
        assert!(!admits(&filter, &other_lane));
        assert!(
            !admits(&filter, &unheld),
            "work no Lane holds stays out while the axis selects"
        );
    }

    #[test]
    fn the_profile_axis_selects_assigned_work() {
        let named = ProfileName::new("standard").expect("the fixture name validates");
        let assigned = ticket(
            1,
            1,
            Priority::Normal,
            TicketState::Active,
            None,
            Some(named.clone()),
            TicketKind::Task,
        );
        let assigned_card = card(&assigned, None, None, None, CLEAR);
        let unassigned = task(1, 2, Priority::Normal, TicketState::Active);
        let unassigned_card = card(&unassigned, None, None, None, CLEAR);

        let filter = BoardFilter {
            profiles: vec![named],
            ..BoardFilter::default()
        };

        assert!(admits(&filter, &assigned_card));
        assert!(
            !admits(&filter, &unassigned_card),
            "a Ticket with no assignment stays out while the axis selects"
        );
    }

    #[test]
    fn the_attention_axis_selects_raised_work_by_class() {
        let raised = task(1, 1, Priority::Normal, TicketState::Active);
        let raised_card = card(
            &raised,
            None,
            None,
            None,
            &[AttentionState::Blocker, AttentionState::StaleRun],
        );
        let clear = card(&raised, None, None, None, CLEAR);

        let mut filter = BoardFilter {
            attention: vec![AttentionState::StaleRun],
            ..BoardFilter::default()
        };
        assert!(admits(&filter, &raised_card), "a raised class admits");

        filter.attention = vec![AttentionState::Blocker];
        assert!(admits(&filter, &raised_card), "any raised class admits");

        filter.attention = vec![AttentionState::MissingResult];
        assert!(
            !admits(&filter, &raised_card),
            "a class the work never raised stays out"
        );
        assert!(
            !admits(&filter, &clear),
            "work raising nothing stays out while the axis selects"
        );
    }

    #[test]
    fn filters_compose_as_one_intersection() {
        // Project 1 under Initiative 2, held by Lane 5, urgent, active.
        let seen = task(1, 1, Priority::Urgent, TicketState::Active);
        let seen_card = card(&seen, Some(2), None, Some(5), CLEAR);
        // Project 2: right Lane, wrong Project.
        let elsewhere = task(2, 2, Priority::Urgent, TicketState::Active);
        let elsewhere_card = card(&elsewhere, Some(2), None, Some(5), CLEAR);
        // Project 1, urgent, but parked and unheld.
        let waiting = task(1, 3, Priority::Urgent, TicketState::Parked);
        let waiting_card = card(&waiting, Some(2), None, None, CLEAR);

        let filter = BoardFilter {
            projects: vec![ProjectId::new(1)],
            initiatives: vec![InitiativeId::new(2)],
            lanes: vec![LaneId::new(5)],
            priorities: vec![Priority::Urgent],
            states: vec![TicketState::Active],
            ..BoardFilter::default()
        };

        assert!(admits(&filter, &seen_card));
        assert!(
            !admits(&filter, &elsewhere_card),
            "one failing axis excludes, whatever the others say"
        );
        assert!(!admits(&filter, &waiting_card));
    }

    #[test]
    fn attention_wire_names_round_trip() {
        for class in AttentionState::ALL {
            assert_eq!(AttentionState::parse(class.wire_name()), Some(*class));
        }
        assert_eq!(AttentionState::parse("needs_input"), None);
        assert_eq!(AttentionState::ALL.len(), 8);
    }

    #[test]
    fn cards_order_by_priority_then_readiness() {
        let landing = task(1, 1, Priority::Normal, TicketState::Landing);
        let approved = task(1, 2, Priority::Normal, TicketState::Approved);
        let ready = task(1, 3, Priority::Normal, TicketState::Ready);
        let scheduled = task(1, 4, Priority::Normal, TicketState::Scheduled);
        let urgent_parked = task(1, 5, Priority::Urgent, TicketState::Parked);
        let low_active = task(1, 6, Priority::Low, TicketState::Active);

        let landing_card = card(&landing, None, None, None, CLEAR);
        let approved_card = card(&approved, None, None, None, CLEAR);
        let ready_card = card(&ready, None, None, None, CLEAR);
        let scheduled_card = card(&scheduled, None, None, None, CLEAR);
        let urgent_card = card(&urgent_parked, None, None, None, CLEAR);
        let low_card = card(&low_active, None, None, None, CLEAR);

        // Priority decides first, whatever readiness says.
        assert!(compare_cards(&urgent_card, &landing_card).is_lt());
        assert!(compare_cards(&low_card, &scheduled_card).is_gt());
        // Inside one priority, the card closer to landing sits higher.
        assert!(compare_cards(&landing_card, &approved_card).is_lt());
        assert!(compare_cards(&ready_card, &scheduled_card).is_lt());
    }

    #[test]
    fn the_global_order_breaks_ties_by_project_then_number() {
        let core_three = task(1, 3, Priority::Normal, TicketState::Active);
        let core_seven = task(1, 7, Priority::Normal, TicketState::Active);
        let edge_one = task(2, 1, Priority::Normal, TicketState::Active);
        let edge_two = task(2, 2, Priority::Normal, TicketState::Active);

        let mut cards = vec![
            card(&edge_two, None, None, None, CLEAR),
            card(&core_seven, None, None, None, CLEAR),
            card(&edge_one, None, None, None, CLEAR),
            card(&core_three, None, None, None, CLEAR),
        ];
        sort_cards(&mut cards);

        let order: Vec<u64> = cards.iter().map(|card| card.id()).collect();
        assert_eq!(
            order,
            vec![3, 7, 1, 2],
            "same priority and readiness order by Project then minted number"
        );
    }
}
