//! The Saved View entity (DR-BP-05, DR-BP-06): one named operator
//! perspective over the board. A view owns its whole set of
//! presentation decisions together — the ten-axis filter, the groups
//! opened into their states, the columns hidden from the board, the
//! Board or Register mode, the Done placement, and the sorting key —
//! so restoring a view restores every one of them and writing one
//! never drops the others. Sorting stays a choice between closed
//! deterministic orders (DR-LC-11): no view, and nothing else, ever
//! orders cards by hand. One global default view and one default per
//! Project exist without being asked (DR-BP-06): they are generated
//! for every scope, and a scope whose default was removed generates
//! it again on the next read.

use std::fmt;

use crate::board::BoardGroup;
use crate::board_query::BoardFilter;
use crate::project::ProjectId;

/// The groups the board can open into their states: the two
/// multi-state regions, Backlog and Staged. Every other group is one
/// fixed column, so expanding it means nothing.
pub const EXPANDABLE_GROUPS: &[BoardGroup] = &[BoardGroup::Backlog, BoardGroup::Staged];

/// The name every generated default view carries, in every scope.
pub const DEFAULT_VIEW_NAME: &str = "All work";

/// Why a Saved View rule was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedViewError {
    /// A text field holds nothing but whitespace. The value names the
    /// field.
    Blank(&'static str),
    /// The named group cannot open into its states, so a view cannot
    /// hold it expanded.
    NotExpandable(BoardGroup),
}

impl fmt::Display for SavedViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blank(field) => write!(f, "a saved view {field} cannot be blank"),
            Self::NotExpandable(group) => write!(
                f,
                "the {} group cannot expand into its states",
                group.wire_name()
            ),
        }
    }
}

impl std::error::Error for SavedViewError {}

/// A validated, trimmed view name. Names tell perspectives apart
/// inside one scope; two views of one scope never share one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewName(String);

impl ViewName {
    /// Accept any name that holds at least one non-whitespace
    /// character; surrounding whitespace is not part of the name.
    pub fn new(raw: &str) -> Result<Self, SavedViewError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(SavedViewError::Blank("name"));
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The trimmed name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ViewName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The storage-assigned identity of one Saved View.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SavedViewId(u64);

impl SavedViewId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SavedViewId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The scope a view belongs to: the whole board, or one Project's
/// work (DR-BP-06). The global scope holds exactly one default view;
/// every Project holds exactly one of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewScope {
    /// Every Project's work on one surface.
    Global,
    /// One Project's work.
    Project(ProjectId),
}

/// The mode a view restores: the grouped columns or the register
/// tables — the Board/Register switch of the Surface presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Grouped columns.
    Board,
    /// One table per visible column.
    Register,
}

impl ViewMode {
    /// Every mode, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Board, Self::Register];

    /// The stored and wire name of this mode.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Board => "board",
            Self::Register => "register",
        }
    }

    /// The mode a stored row names, or `None` outside the vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|mode| mode.wire_name() == stored)
    }
}

/// Where Done work sits under a view: its own column among the
/// groups, or the table below the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DonePlacement {
    /// Done stands with the other groups.
    Column,
    /// Done sits below the board as a table.
    Table,
}

impl DonePlacement {
    /// Every placement, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Column, Self::Table];

    /// The stored and wire name of this placement.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Column => "column",
            Self::Table => "table",
        }
    }

    /// The placement a stored row names, or `None` outside the
    /// vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|placement| placement.wire_name() == stored)
    }
}

/// The deterministic order a view reads cards in (DR-LC-11): which
/// key leads the order the board keeps. Both keys are closed and
/// deterministic — priority first with readiness beneath it, or
/// readiness first with priority beneath it — and the minted
/// Project and number break every tie, so position is never a
/// decision either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewSorting {
    /// Priority leads: urgent before high before normal before low,
    /// readiness beneath.
    Priority,
    /// Readiness leads: the card closer to landing sits higher,
    /// priority beneath.
    Readiness,
}

impl ViewSorting {
    /// Every key, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Priority, Self::Readiness];

    /// The stored and wire name of this key.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Priority => "priority",
            Self::Readiness => "readiness",
        }
    }

    /// The key a stored row names, or `None` outside the vocabulary.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|key| key.wire_name() == stored)
    }
}

/// One named operator perspective, owning its whole set of
/// presentation decisions together. The version counts applied
/// changes: a view lands at 1 and every later legal change bumps it,
/// so a stored version is all a caller needs for optimistic checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedView {
    id: SavedViewId,
    name: ViewName,
    scope: ViewScope,
    filter: BoardFilter,
    expanded: Vec<BoardGroup>,
    hidden: Vec<BoardGroup>,
    mode: ViewMode,
    done: DonePlacement,
    sorting: ViewSorting,
    default: bool,
    version: u64,
}

impl SavedView {
    /// Assemble a fresh named view: version 1, not a scope default.
    /// The expanded groups must all open into their states; both
    /// group sets are kept in the fixed group order without
    /// duplicates.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        name: ViewName,
        scope: ViewScope,
        filter: BoardFilter,
        expanded: &[BoardGroup],
        hidden: &[BoardGroup],
        mode: ViewMode,
        done: DonePlacement,
        sorting: ViewSorting,
    ) -> Result<Self, SavedViewError> {
        let expanded = expandable_of(expanded)?;
        let hidden = canonical(hidden);
        Ok(Self {
            id: SavedViewId::new(0),
            name,
            scope,
            filter,
            expanded,
            hidden,
            mode,
            done,
            sorting,
            default: false,
            version: 1,
        })
    }

    /// The generated default view of one scope (DR-BP-06): the
    /// everyday perspective — nothing expanded, Draft hidden, the
    /// board mode, Done in its column, priority first — over the
    /// whole scope's work. A Project's default scopes the filter to
    /// the Project; the global default leaves it empty, the whole
    /// board.
    pub fn generate(id: SavedViewId, scope: ViewScope) -> Self {
        let filter = match scope {
            ViewScope::Global => BoardFilter::default(),
            ViewScope::Project(project) => BoardFilter {
                projects: vec![project],
                ..BoardFilter::default()
            },
        };
        Self {
            id,
            name: ViewName(String::from(DEFAULT_VIEW_NAME)),
            scope,
            filter,
            expanded: Vec::new(),
            hidden: vec![BoardGroup::Draft],
            mode: ViewMode::Board,
            done: DonePlacement::Column,
            sorting: ViewSorting::Priority,
            default: true,
            version: 1,
        }
    }

    /// Rehydrate a stored view exactly as it was recorded.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        id: SavedViewId,
        name: ViewName,
        scope: ViewScope,
        filter: BoardFilter,
        expanded: Vec<BoardGroup>,
        hidden: Vec<BoardGroup>,
        mode: ViewMode,
        done: DonePlacement,
        sorting: ViewSorting,
        default: bool,
        version: u64,
    ) -> Self {
        Self {
            id,
            name,
            scope,
            filter,
            expanded,
            hidden,
            mode,
            done,
            sorting,
            default,
            version,
        }
    }

    /// The view's storage-assigned identity.
    pub fn id(&self) -> SavedViewId {
        self.id
    }

    /// The view's name, the perspective the operator reads.
    pub fn name(&self) -> &ViewName {
        &self.name
    }

    /// The scope the view belongs to.
    pub fn scope(&self) -> ViewScope {
        self.scope
    }

    /// The owned filter: the ten axes the view selects by.
    pub fn filter(&self) -> &BoardFilter {
        &self.filter
    }

    /// The owned expanded groups, in the fixed group order: the
    /// groups the view opens into their states.
    pub fn expanded(&self) -> &[BoardGroup] {
        &self.expanded
    }

    /// The owned hidden columns, in the fixed group order: the
    /// groups the view keeps off the board.
    pub fn hidden(&self) -> &[BoardGroup] {
        &self.hidden
    }

    /// The owned mode: board or register.
    pub fn mode(&self) -> ViewMode {
        self.mode
    }

    /// The owned Done placement: column or table.
    pub fn done(&self) -> DonePlacement {
        self.done
    }

    /// The owned sorting key.
    pub fn sorting(&self) -> ViewSorting {
        self.sorting
    }

    /// Whether this view is its scope's generated default.
    pub fn is_default(&self) -> bool {
        self.default
    }

    /// The number of applied changes, for optimistic version checks.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Rename the view under its own identity. The name is validated
    /// where it was built; only the change is recorded here.
    pub fn rename(&mut self, name: ViewName) {
        self.name = name;
        self.version += 1;
    }

    /// Adopt a whole replacement set of owned properties: the six
    /// decisions land together or none does, and a refusal changes
    /// nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn adopt(
        &mut self,
        filter: BoardFilter,
        expanded: &[BoardGroup],
        hidden: &[BoardGroup],
        mode: ViewMode,
        done: DonePlacement,
        sorting: ViewSorting,
    ) -> Result<(), SavedViewError> {
        let expanded = expandable_of(expanded)?;
        let hidden = canonical(hidden);
        self.filter = filter;
        self.expanded = expanded;
        self.hidden = hidden;
        self.mode = mode;
        self.done = done;
        self.sorting = sorting;
        self.version += 1;
        Ok(())
    }
}

/// Keep the groups a set names in the fixed group order, without
/// duplicates: one canonical spelling for every stored set.
fn canonical(groups: &[BoardGroup]) -> Vec<BoardGroup> {
    BoardGroup::ALL
        .iter()
        .copied()
        .filter(|group| groups.contains(group))
        .collect()
}

/// The canonical form of an expanded set, refusing any group that
/// cannot open into its states.
fn expandable_of(groups: &[BoardGroup]) -> Result<Vec<BoardGroup>, SavedViewError> {
    for group in groups {
        if !EXPANDABLE_GROUPS.contains(group) {
            return Err(SavedViewError::NotExpandable(*group));
        }
    }
    Ok(canonical(groups))
}

#[cfg(test)]
mod saved_view_schema {
    use super::{
        BoardFilter, DEFAULT_VIEW_NAME, DonePlacement, ProjectId, SavedView, SavedViewError,
        ViewMode, ViewName, ViewScope, ViewSorting,
    };
    use crate::board::BoardGroup;
    use crate::project::ProjectId as Project;

    fn named(raw: &str) -> ViewName {
        ViewName::new(raw).expect("a non-blank name is accepted")
    }

    #[test]
    fn a_blank_name_is_refused_and_a_name_is_stored_trimmed() {
        for blank in ["", " ", " \t "] {
            assert_eq!(ViewName::new(blank), Err(SavedViewError::Blank("name")));
        }
        assert_eq!(named("  Review queue  ").as_str(), "Review queue");
    }

    #[test]
    fn a_fresh_view_owns_every_property_at_version_one() {
        let view = SavedView::create(
            named("Review queue"),
            ViewScope::Global,
            BoardFilter {
                states: vec![crate::ticket::TicketState::InReview],
                ..BoardFilter::default()
            },
            &[BoardGroup::Staged],
            &[BoardGroup::Draft, BoardGroup::Done],
            ViewMode::Register,
            DonePlacement::Table,
            ViewSorting::Readiness,
        )
        .expect("a complete view is created");

        assert_eq!(view.name().as_str(), "Review queue");
        assert_eq!(view.scope(), ViewScope::Global);
        assert_eq!(
            view.filter().states,
            vec![crate::ticket::TicketState::InReview]
        );
        assert_eq!(view.expanded(), &[BoardGroup::Staged]);
        assert_eq!(view.hidden(), &[BoardGroup::Draft, BoardGroup::Done]);
        assert_eq!(view.mode(), ViewMode::Register);
        assert_eq!(view.done(), DonePlacement::Table);
        assert_eq!(view.sorting(), ViewSorting::Readiness);
        assert!(!view.is_default());
        assert_eq!(view.version(), 1);
    }

    #[test]
    fn both_group_sets_keep_the_fixed_order_without_duplicates() {
        let view = SavedView::create(
            named("Everyday"),
            ViewScope::Global,
            BoardFilter::default(),
            &[BoardGroup::Staged, BoardGroup::Backlog, BoardGroup::Staged],
            &[
                BoardGroup::Done,
                BoardGroup::Draft,
                BoardGroup::Done,
                BoardGroup::Current,
            ],
            ViewMode::Board,
            DonePlacement::Column,
            ViewSorting::Priority,
        )
        .expect("duplicates are absorbed, not refused");

        assert_eq!(
            view.expanded(),
            &[BoardGroup::Backlog, BoardGroup::Staged],
            "expanded keeps the fixed group order"
        );
        assert_eq!(
            view.hidden(),
            &[BoardGroup::Draft, BoardGroup::Current, BoardGroup::Done],
            "hidden keeps the fixed group order"
        );
    }

    #[test]
    fn a_group_that_cannot_expand_is_refused() {
        let error = SavedView::create(
            named("Wide"),
            ViewScope::Global,
            BoardFilter::default(),
            &[BoardGroup::Current],
            &[],
            ViewMode::Board,
            DonePlacement::Column,
            ViewSorting::Priority,
        )
        .expect_err("Current is one fixed column, not an axis");

        assert_eq!(
            error,
            SavedViewError::NotExpandable(BoardGroup::Current),
            "the refusal names the group"
        );
        assert_eq!(
            error.to_string(),
            "the current group cannot expand into its states"
        );
    }

    #[test]
    fn the_global_default_is_the_everyday_whole_board_perspective() {
        let view = SavedView::generate(super::SavedViewId::new(7), ViewScope::Global);

        assert_eq!(view.id().value(), 7);
        assert_eq!(view.name().as_str(), DEFAULT_VIEW_NAME);
        assert_eq!(view.scope(), ViewScope::Global);
        assert!(view.filter().is_empty(), "the whole board shows");
        assert!(view.expanded().is_empty());
        assert_eq!(view.hidden(), &[BoardGroup::Draft]);
        assert_eq!(view.mode(), ViewMode::Board);
        assert_eq!(view.done(), DonePlacement::Column);
        assert_eq!(view.sorting(), ViewSorting::Priority);
        assert!(view.is_default());
        assert_eq!(view.version(), 1);
    }

    #[test]
    fn a_project_default_scopes_the_filter_to_its_project() {
        let view = SavedView::generate(
            super::SavedViewId::new(9),
            ViewScope::Project(Project::new(3)),
        );

        assert_eq!(view.scope(), ViewScope::Project(Project::new(3)));
        assert_eq!(
            view.filter().projects,
            vec![Project::new(3)],
            "the generated Project default opens on its own Project"
        );
        assert_eq!(view.name().as_str(), DEFAULT_VIEW_NAME);
        assert!(view.is_default());
    }

    #[test]
    fn adopting_replaces_every_owned_property_together() {
        let mut view = SavedView::create(
            named("Everyday"),
            ViewScope::Global,
            BoardFilter::default(),
            &[],
            &[BoardGroup::Draft],
            ViewMode::Board,
            DonePlacement::Column,
            ViewSorting::Priority,
        )
        .expect("the view is created");

        view.adopt(
            BoardFilter {
                priorities: vec![crate::ticket::Priority::Urgent],
                ..BoardFilter::default()
            },
            &[BoardGroup::Backlog],
            &[BoardGroup::Draft, BoardGroup::Review],
            ViewMode::Register,
            DonePlacement::Table,
            ViewSorting::Readiness,
        )
        .expect("the whole set lands");

        assert_eq!(
            view.filter().priorities,
            vec![crate::ticket::Priority::Urgent]
        );
        assert_eq!(view.expanded(), &[BoardGroup::Backlog]);
        assert_eq!(view.hidden(), &[BoardGroup::Draft, BoardGroup::Review]);
        assert_eq!(view.mode(), ViewMode::Register);
        assert_eq!(view.done(), DonePlacement::Table);
        assert_eq!(view.sorting(), ViewSorting::Readiness);
        assert_eq!(view.version(), 2);
    }

    #[test]
    fn a_refused_adoption_changes_nothing() {
        let mut view = SavedView::create(
            named("Everyday"),
            ViewScope::Global,
            BoardFilter::default(),
            &[BoardGroup::Backlog],
            &[],
            ViewMode::Board,
            DonePlacement::Column,
            ViewSorting::Priority,
        )
        .expect("the view is created");

        let error = view
            .adopt(
                BoardFilter::default(),
                &[BoardGroup::Done],
                &[],
                ViewMode::Register,
                DonePlacement::Table,
                ViewSorting::Readiness,
            )
            .expect_err("Done cannot expand");

        assert_eq!(error, SavedViewError::NotExpandable(BoardGroup::Done));
        assert_eq!(view.expanded(), &[BoardGroup::Backlog]);
        assert_eq!(view.mode(), ViewMode::Board);
        assert_eq!(
            view.version(),
            1,
            "the refusal changed nothing, version included"
        );
    }

    #[test]
    fn renaming_records_the_change() {
        let mut view = SavedView::create(
            named("Everyday"),
            ViewScope::Global,
            BoardFilter::default(),
            &[],
            &[],
            ViewMode::Board,
            DonePlacement::Column,
            ViewSorting::Priority,
        )
        .expect("the view is created");

        view.rename(named("Deep work"));

        assert_eq!(view.name().as_str(), "Deep work");
        assert_eq!(view.version(), 2);
    }

    #[test]
    fn restore_rehydrates_every_recorded_fact() {
        let view = SavedView::restore(
            super::SavedViewId::new(11),
            named("Review queue"),
            ViewScope::Project(Project::new(2)),
            BoardFilter {
                projects: vec![ProjectId::new(2)],
                ..BoardFilter::default()
            },
            vec![BoardGroup::Backlog, BoardGroup::Staged],
            vec![BoardGroup::Draft],
            ViewMode::Register,
            DonePlacement::Table,
            ViewSorting::Readiness,
            true,
            6,
        );

        assert_eq!(view.id().value(), 11);
        assert_eq!(view.name().as_str(), "Review queue");
        assert_eq!(view.scope(), ViewScope::Project(Project::new(2)));
        assert_eq!(view.filter().projects, vec![ProjectId::new(2)]);
        assert_eq!(view.expanded(), &[BoardGroup::Backlog, BoardGroup::Staged]);
        assert_eq!(view.hidden(), &[BoardGroup::Draft]);
        assert_eq!(view.mode(), ViewMode::Register);
        assert_eq!(view.done(), DonePlacement::Table);
        assert_eq!(view.sorting(), ViewSorting::Readiness);
        assert!(view.is_default());
        assert_eq!(view.version(), 6);
    }

    #[test]
    fn the_vocabularies_round_trip_their_wire_names() {
        use super::{DonePlacement as Done, ViewMode as Mode, ViewSorting as Sort};

        for mode in Mode::ALL {
            assert_eq!(Mode::parse(mode.wire_name()), Some(*mode));
        }
        assert_eq!(Mode::parse("kanban"), None);
        for placement in Done::ALL {
            assert_eq!(Done::parse(placement.wire_name()), Some(*placement));
        }
        assert_eq!(Done::parse("below"), None);
        for key in Sort::ALL {
            assert_eq!(Sort::parse(key.wire_name()), Some(*key));
        }
        assert_eq!(Sort::parse("manual"), None);
    }
}
