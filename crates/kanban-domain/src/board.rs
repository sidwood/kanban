//! The board group projection (CONTEXT.md, DR-LC-02 to DR-LC-05): the
//! fixed Board Groups are a pure projection over lifecycle state —
//! Draft, Backlog, Current, Review, Staged, Done — with Backlog
//! holding parked, blocked, scheduled, and ready, Staged holding
//! approved and landing, and the terminal states reaching no group at
//! all. Card ordering stays a presentation rule (DR-LC-11): priority
//! and readiness decide it, never manual ordering, so nothing here
//! orders anything. This module reads the lifecycle vocabulary and
//! mutates nothing.

use crate::ticket::TicketState;

/// The fixed Board Groups (DR-LC-03): the board columns every surface
/// agrees on, in their fixed order. A group is the board's projection
/// of lifecycle state, never a state itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardGroup {
    /// Captured work that has not qualified or circulated.
    Draft,
    /// Work waiting in its before-active states (DR-LC-04): parked,
    /// blocked, scheduled, and ready.
    Backlog,
    /// Work executing in a Lane.
    Current,
    /// Work under review.
    Review,
    /// Work past its review (DR-LC-05): approved and landing.
    Staged,
    /// Landed work.
    Done,
}

impl BoardGroup {
    /// Every group, in the fixed board order.
    pub const ALL: &'static [Self] = &[
        Self::Draft,
        Self::Backlog,
        Self::Current,
        Self::Review,
        Self::Staged,
        Self::Done,
    ];

    /// The stored and wire name of this group.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Backlog => "backlog",
            Self::Current => "current",
            Self::Review => "review",
            Self::Staged => "staged",
            Self::Done => "done",
        }
    }

    /// The group a stored row names, or `None` outside the
    /// vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|group| group.wire_name() == stored)
    }

    /// The lifecycle states this group holds, in canonical order
    /// (CONTEXT.md): a group is exactly its states and nothing else.
    pub fn states(self) -> &'static [TicketState] {
        use TicketState as State;
        match self {
            Self::Draft => &[State::Draft],
            Self::Backlog => &[
                State::Parked,
                State::Blocked,
                State::Scheduled,
                State::Ready,
            ],
            Self::Current => &[State::Active],
            Self::Review => &[State::InReview],
            Self::Staged => &[State::Approved, State::Landing],
            Self::Done => &[State::Done],
        }
    }
}

/// The group a lifecycle state projects onto (DR-LC-04, DR-LC-05):
/// every state reaches exactly one group except the terminal states,
/// which reach none — cancelled and superseded never appear on the
/// active board (DR-LC-02).
pub fn board_group_for(state: TicketState) -> Option<BoardGroup> {
    if state.is_terminal() {
        return None;
    }
    BoardGroup::ALL
        .iter()
        .copied()
        .find(|group| group.states().contains(&state))
}

#[cfg(test)]
mod board_projection {
    use crate::board::{BoardGroup, board_group_for};
    use crate::ticket::TicketState;

    #[test]
    fn every_active_state_places_into_its_group() {
        use TicketState as State;
        let placements = [
            (State::Draft, BoardGroup::Draft),
            (State::Parked, BoardGroup::Backlog),
            (State::Blocked, BoardGroup::Backlog),
            (State::Scheduled, BoardGroup::Backlog),
            (State::Ready, BoardGroup::Backlog),
            (State::Active, BoardGroup::Current),
            (State::InReview, BoardGroup::Review),
            (State::Approved, BoardGroup::Staged),
            (State::Landing, BoardGroup::Staged),
            (State::Done, BoardGroup::Done),
        ];

        for (state, group) in placements {
            assert_eq!(
                board_group_for(state),
                Some(group),
                "{state:?} places into {group:?}"
            );
        }
    }

    #[test]
    fn the_fixed_groups_hold_exactly_the_states_context_fixes() {
        use TicketState as State;
        assert_eq!(BoardGroup::Draft.states(), &[State::Draft]);
        assert_eq!(
            BoardGroup::Backlog.states(),
            &[
                State::Parked,
                State::Blocked,
                State::Scheduled,
                State::Ready
            ]
        );
        assert_eq!(BoardGroup::Current.states(), &[State::Active]);
        assert_eq!(BoardGroup::Review.states(), &[State::InReview]);
        assert_eq!(
            BoardGroup::Staged.states(),
            &[State::Approved, State::Landing]
        );
        assert_eq!(BoardGroup::Done.states(), &[State::Done]);
    }

    #[test]
    fn the_groups_partition_the_active_states_exactly_once() {
        for state in TicketState::ALL {
            let holding: Vec<BoardGroup> = BoardGroup::ALL
                .iter()
                .copied()
                .filter(|group| group.states().contains(state))
                .collect();
            match board_group_for(*state) {
                Some(group) => {
                    assert_eq!(
                        holding.len(),
                        1,
                        "{state:?} sits in one group alone, not {holding:?}"
                    );
                    assert_eq!(holding[0], group);
                }
                None => assert!(
                    holding.is_empty(),
                    "{state:?} reaches no group, not {holding:?}"
                ),
            }
        }
    }

    #[test]
    fn terminal_states_never_reach_a_group() {
        use TicketState as State;
        for state in [State::Cancelled, State::Superseded] {
            assert_eq!(
                board_group_for(state),
                None,
                "{state:?} never appears on the active board"
            );
            for group in BoardGroup::ALL {
                assert!(
                    !group.states().contains(&state),
                    "{} never holds {state:?}",
                    group.wire_name()
                );
            }
        }
    }

    #[test]
    fn the_groups_stand_in_their_fixed_order() {
        let names: Vec<&str> = BoardGroup::ALL
            .iter()
            .map(|group| group.wire_name())
            .collect();
        assert_eq!(
            names,
            ["draft", "backlog", "current", "review", "staged", "done"]
        );
    }

    #[test]
    fn group_wire_names_round_trip() {
        for group in BoardGroup::ALL {
            assert_eq!(BoardGroup::parse(group.wire_name()), Some(*group));
        }
        assert_eq!(BoardGroup::parse("archived"), None);
    }
}
