//! The operator's shell preferences (KAN-S5-US1, KAN-S5-US3): how the
//! surface is arranged rather than what the work is — whether the
//! navigation rail stands open, and which board columns are collapsed
//! to their rail in each scope. These are per-operator data in the
//! authoritative store, not browser state: the arrangement survives a
//! reload, a new window, and a cleared browser origin, because the
//! core holds it. A Saved View owns which columns are *hidden* and
//! which groups are open (DR-BP-05); collapse is not a view's, so it
//! lives here beside the rail.
//!
//! The one rule the record carries is that both sets are canonical: a
//! scope's collapsed columns are named once each, in board order, and
//! a scope with nothing collapsed is not recorded at all. Writing the
//! same arrangement twice is therefore the same record.

use crate::board::BoardColumn;
use crate::saved_view::ViewScope;

/// The columns one scope keeps collapsed, in board order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedCollapse {
    scope: ViewScope,
    columns: Vec<BoardColumn>,
}

impl ScopedCollapse {
    /// The scope these columns belong to.
    pub fn scope(&self) -> ViewScope {
        self.scope
    }

    /// The collapsed columns, in board order and named once each.
    pub fn columns(&self) -> &[BoardColumn] {
        &self.columns
    }
}

/// How the operator keeps the shell arranged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellPreferences {
    rail_open: bool,
    collapsed: Vec<ScopedCollapse>,
    version: u64,
}

impl ShellPreferences {
    /// The arrangement a shell that has never been rearranged holds:
    /// the rail open and every column expanded.
    pub fn everyday() -> Self {
        Self {
            rail_open: true,
            collapsed: Vec::new(),
            version: 0,
        }
    }

    /// Rebuild the record from what a caller holds, canonicalising
    /// both sets: each scope appears once, keeping the last set given
    /// for it; each scope's columns are named once each in board
    /// order; and a scope with nothing collapsed is dropped.
    pub fn restore(
        rail_open: bool,
        collapsed: impl IntoIterator<Item = (ViewScope, Vec<BoardColumn>)>,
        version: u64,
    ) -> Self {
        let mut scopes: Vec<(ViewScope, Vec<BoardColumn>)> = Vec::new();
        for (scope, columns) in collapsed {
            let canonical: Vec<BoardColumn> = BoardColumn::ALL
                .iter()
                .copied()
                .filter(|column| columns.contains(column))
                .collect();
            match scopes.iter_mut().find(|(held, _)| *held == scope) {
                Some(entry) => entry.1 = canonical,
                None => scopes.push((scope, canonical)),
            }
        }
        Self {
            rail_open,
            collapsed: scopes
                .into_iter()
                .filter(|(_, columns)| !columns.is_empty())
                .map(|(scope, columns)| ScopedCollapse { scope, columns })
                .collect(),
            version,
        }
    }

    /// Whether the navigation rail stands open with its labels.
    pub fn rail_open(&self) -> bool {
        self.rail_open
    }

    /// The collapsed columns of every scope that keeps any.
    pub fn collapsed(&self) -> &[ScopedCollapse] {
        &self.collapsed
    }

    /// The columns one scope keeps collapsed; none unless it keeps
    /// some.
    pub fn collapsed_in(&self, scope: ViewScope) -> &[BoardColumn] {
        self.collapsed
            .iter()
            .find(|entry| entry.scope == scope)
            .map(|entry| entry.columns.as_slice())
            .unwrap_or(&[])
    }

    /// The record's optimistic version.
    pub fn version(&self) -> u64 {
        self.version
    }
}

#[cfg(test)]
mod shell_preferences {
    use super::ShellPreferences;
    use crate::board::BoardColumn;
    use crate::project::ProjectId;
    use crate::saved_view::ViewScope;

    fn project(id: u64) -> ViewScope {
        ViewScope::Project(ProjectId::new(id))
    }

    #[test]
    fn an_unarranged_shell_keeps_its_rail_open_and_nothing_collapsed() {
        let everyday = ShellPreferences::everyday();

        assert!(everyday.rail_open());
        assert_eq!(everyday.collapsed(), &[]);
        assert_eq!(everyday.version(), 0);
    }

    #[test]
    fn a_scopes_collapsed_columns_are_named_once_each_in_board_order() {
        let preferences = ShellPreferences::restore(
            false,
            [(
                project(2),
                vec![
                    BoardColumn::Ready,
                    BoardColumn::Parked,
                    BoardColumn::Ready,
                    BoardColumn::Backlog,
                ],
            )],
            3,
        );

        assert_eq!(
            preferences.collapsed_in(project(2)),
            &[
                BoardColumn::Backlog,
                BoardColumn::Parked,
                BoardColumn::Ready
            ]
        );
        assert!(!preferences.rail_open());
        assert_eq!(preferences.version(), 3);
    }

    #[test]
    fn a_scope_with_nothing_collapsed_is_not_recorded_at_all() {
        let preferences = ShellPreferences::restore(
            true,
            [
                (ViewScope::Global, vec![BoardColumn::Review]),
                (project(2), Vec::new()),
            ],
            1,
        );

        assert_eq!(preferences.collapsed().len(), 1);
        assert_eq!(preferences.collapsed()[0].scope(), ViewScope::Global);
        assert_eq!(preferences.collapsed_in(project(2)), &[]);
    }

    #[test]
    fn one_scope_is_recorded_once_however_often_it_is_given() {
        let preferences = ShellPreferences::restore(
            true,
            [
                (ViewScope::Global, vec![BoardColumn::Review]),
                (ViewScope::Global, vec![BoardColumn::Done]),
            ],
            1,
        );

        assert_eq!(preferences.collapsed().len(), 1);
        assert_eq!(
            preferences.collapsed_in(ViewScope::Global),
            &[BoardColumn::Done]
        );
    }

    #[test]
    fn the_same_arrangement_written_twice_is_the_same_record() {
        let once = ShellPreferences::restore(
            true,
            [(
                ViewScope::Global,
                vec![BoardColumn::Done, BoardColumn::Review],
            )],
            1,
        );
        let again = ShellPreferences::restore(
            true,
            [(
                ViewScope::Global,
                vec![BoardColumn::Review, BoardColumn::Done],
            )],
            1,
        );

        assert_eq!(once, again);
    }
}
