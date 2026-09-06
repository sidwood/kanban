// The board's presentation model: the fixed Board Groups from
// CONTEXT.md projected onto the Surface interaction language. The
// group-state mapping, the collapsible axes, the Done table option,
// and the Board/Register presentations are pure functions here; the
// legality of any move stays with the core, so the only transition
// knowledge this module holds is the state a column's drop asks for
// (KAN-T24-AC1, KAN-T24-AC2).
import type { TicketState } from '@kanban/contracts'

export const BOARD_LAYOUTS = ['collapsed', 'expanded'] as const

export type BoardLayout = (typeof BOARD_LAYOUTS)[number]

// The two multi-state regions that open into their states: Backlog
// and the completion gate Staged. Current and Review hold one state
// each, so they are fixed columns rather than axes; Done is its own
// fixed column and the one the table option demotes.
export const BOARD_LAYOUT_AXES = ['backlog', 'completion'] as const

export type BoardLayoutAxis = (typeof BOARD_LAYOUT_AXES)[number]

export const DONE_PRESENTATIONS = ['column', 'table'] as const

export type DonePresentation = (typeof DONE_PRESENTATIONS)[number]

export const BOARD_PRESENTATIONS = ['board', 'register'] as const

export type BoardPresentation = (typeof BOARD_PRESENTATIONS)[number]

export const DRAFT_VISIBILITIES = ['hidden', 'visible'] as const

export type DraftVisibility = (typeof DRAFT_VISIBILITIES)[number]

export type BoardLayoutState = Readonly<Record<BoardLayoutAxis, BoardLayout>>

// The everyday board keeps the six fixed groups: every axis collapsed
// and Draft out of sight until the operator asks for it.
export const DEFAULT_BOARD_LAYOUTS: BoardLayoutState = {
  backlog: 'collapsed',
  completion: 'collapsed',
}

export const DEFAULT_DONE_PRESENTATION: DonePresentation = 'column'

export const DEFAULT_BOARD_PRESENTATION: BoardPresentation = 'board'

export const DEFAULT_DRAFT_VISIBILITY: DraftVisibility = 'hidden'

// The terminal states never appear on the active board; Superseded
// and Cancelled Tickets keep their history off the columns.
const TERMINAL_STATES: readonly TicketState[] = ['cancelled', 'superseded']

export function isOnBoard(state: TicketState): boolean {
  return !TERMINAL_STATES.includes(state)
}

export type BoardGroupId =
  | 'draft'
  | 'backlog'
  | 'current'
  | 'review'
  | 'staged'
  | 'done'

type BoardGroupDefinition = Readonly<{
  id: BoardGroupId
  label: string
  subheading: string
  states: readonly TicketState[]
  /** Where a card the operator parks in this group lands by default. */
  defaultState: TicketState
}>

// The fixed Board Groups and the states each holds, exactly as
// CONTEXT.md fixes them: Draft holds draft; Backlog holds parked,
// blocked, scheduled, and ready; Current holds active; Review holds
// in review; Staged holds approved and landing; Done holds done.
export const BOARD_GROUPS = [
  {
    id: 'draft',
    label: 'Draft',
    subheading: 'Captured · Unqualified · Waiting',
    states: ['draft'],
    defaultState: 'draft',
  },
  {
    id: 'backlog',
    label: 'Backlog',
    subheading: 'Parked · Blocked · Scheduled · Ready',
    states: ['parked', 'blocked', 'scheduled', 'ready'],
    defaultState: 'parked',
  },
  {
    id: 'current',
    label: 'Current',
    subheading: 'Active · In a Lane · Working',
    states: ['active'],
    defaultState: 'active',
  },
  {
    id: 'review',
    label: 'Review',
    subheading: 'In review · Verdicts · Gates',
    states: ['in_review'],
    defaultState: 'in_review',
  },
  {
    id: 'staged',
    label: 'Staged',
    subheading: 'Approved · Landing · Gated',
    states: ['approved', 'landing'],
    defaultState: 'approved',
  },
  {
    id: 'done',
    label: 'Done',
    subheading: 'Landed · Complete · Closed',
    states: ['done'],
    defaultState: 'done',
  },
] as const satisfies readonly BoardGroupDefinition[]

// The nested state columns an axis opens into, each with its own
// name; a group holding a single state lends that state its own.
const NESTED_STATE_COLUMNS = {
  parked: { label: 'Parked', subheading: 'Held · Set aside · Waiting' },
  blocked: { label: 'Blocked', subheading: 'Stuck · Dependency · Halted' },
  scheduled: { label: 'Scheduled', subheading: 'Dated · Queued · Waiting' },
  ready: { label: 'Ready', subheading: 'Claimable · Available · Next' },
  approved: { label: 'Approved', subheading: 'Verdict · Held · Human gate' },
  landing: { label: 'Landing', subheading: 'Seed · Default branch · In flight' },
} as const satisfies Partial<Record<TicketState, { label: string; subheading: string }>>

export type BoardColumnId = BoardGroupId | keyof typeof NESTED_STATE_COLUMNS

export type BoardColumnGroup = Readonly<{
  /** The axis, or the group itself when the group belongs to no axis. */
  id: string
  /** What the region is called whole; expanded, the group wears this
   * above its sibling columns. */
  heading: string
  subheading: string
  /** True once the axis shows sibling columns that read as one item. */
  grouped: boolean
  columns: readonly BoardColumnId[]
}>

export type BoardLayoutAxisControl = Readonly<{
  axis: BoardLayoutAxis
  layout: BoardLayout
  collapsedLabel: string
  expandedLabel: string
}>

// One axis is one group the board can open into its states.
const AXIS_DEFINITIONS = [
  { axis: 'backlog', group: 'backlog' },
  { axis: 'completion', group: 'staged' },
] as const satisfies readonly {
  axis: BoardLayoutAxis
  group: BoardGroupId
}[]

const AXIS_GROUP_IDS: Readonly<Record<BoardLayoutAxis, BoardGroupId>> = {
  backlog: 'backlog',
  completion: 'staged',
}

// The group a column id names, when it names one: a group column
// finds its group, a nested state column finds none.
function groupOf(column: BoardColumnId) {
  return BOARD_GROUPS.find((group) => group.id === column)
}

function axisOfGroup(id: BoardGroupId): BoardLayoutAxis | undefined {
  return AXIS_DEFINITIONS.find((entry) => entry.group === id)?.axis
}

function boardGroupForState(state: TicketState): BoardGroupId | undefined {
  return BOARD_GROUPS.find((group) =>
    (group.states as readonly TicketState[]).includes(state),
  )?.id
}

/** Where a state sits in the register, which never demotes Done. */
export function registerColumnFor(
  state: TicketState,
  layouts: BoardLayoutState = DEFAULT_BOARD_LAYOUTS,
): BoardColumnId | undefined {
  return columnForCard(state, layouts)
}

/**
 * Cards in the Draft column force it visible. With none on the board,
 * the operator's saved choice applies and hidden remains the default.
 */
export function resolveDraftVisibility(
  preference: DraftVisibility,
  draftColumnCardCount: number,
): DraftVisibility {
  return draftColumnCardCount > 0 ? 'visible' : preference
}

export function boardColumnGroups(
  layouts: BoardLayoutState,
  done: DonePresentation = DEFAULT_DONE_PRESENTATION,
  draft: DraftVisibility = DEFAULT_DRAFT_VISIBILITY,
): readonly BoardColumnGroup[] {
  const groups: BoardColumnGroup[] = []
  if (draft === 'visible') {
    groups.push({
      id: 'draft',
      heading: boardColumnLabel('draft'),
      subheading: boardColumnSubheading('draft'),
      grouped: false,
      columns: ['draft'],
    })
  }
  groups.push(axisGroup('backlog', layouts), fixedGroup('current'), fixedGroup('review'), axisGroup('completion', layouts))
  if (done === 'column') {
    groups.push(fixedGroup('done'))
  }
  return groups
}

export function visibleColumnsFor(
  layouts: BoardLayoutState,
  done: DonePresentation = DEFAULT_DONE_PRESENTATION,
  draft: DraftVisibility = DEFAULT_DRAFT_VISIBILITY,
): readonly BoardColumnId[] {
  return boardColumnGroups(layouts, done, draft).flatMap((group) => group.columns)
}

/**
 * A register gives every visible column a table of its own, so Done
 * already has somewhere to sit and the below-the-board table it can be
 * demoted to has no place here.
 */
export function registerColumnsFor(
  layouts: BoardLayoutState,
  draft: DraftVisibility = DEFAULT_DRAFT_VISIBILITY,
): readonly BoardColumnId[] {
  return visibleColumnsFor(layouts, 'column', draft)
}

export function boardLayoutAxisControls(
  layouts: BoardLayoutState,
): readonly BoardLayoutAxisControl[] {
  return AXIS_DEFINITIONS.map(({ axis, group }) => ({
    axis,
    layout: layouts[axis],
    collapsedLabel: boardColumnLabel(group),
    expandedLabel: expandedAxisLabel(group),
  }))
}

/** Where a card with this state sits under the current layout. */
export function columnForCard(
  state: TicketState,
  layouts: BoardLayoutState,
): BoardColumnId | undefined {
  if (!isOnBoard(state)) return undefined
  const group = boardGroupForState(state)
  if (group === undefined) return undefined
  const axis = axisOfGroup(group)
  if (axis !== undefined && layouts[axis] === 'expanded') {
    // An expanded axis shows one column per state it holds.
    return state as BoardColumnId
  }
  return group
}

export function boardColumnLabel(column: BoardColumnId): string {
  return nestedStateColumn(column)?.label ?? groupOf(column)?.label ?? column
}

export function boardColumnSubheading(column: BoardColumnId): string {
  return (
    nestedStateColumn(column)?.subheading ?? groupOf(column)?.subheading ?? ''
  )
}

/** Every state a column collects cards from under the current layout. */
export function boardColumnStates(
  column: BoardColumnId,
  layouts: BoardLayoutState,
): readonly TicketState[] {
  const expandedAxis = AXIS_DEFINITIONS.find(
    (entry) => entry.group === column && layouts[entry.axis] === 'expanded',
  )
  if (expandedAxis) {
    return (groupOf(expandedAxis.group)?.states ?? []) as readonly TicketState[]
  }
  if (nestedStateColumn(column)) return [column as TicketState]
  return (groupOf(column)?.states ?? []) as readonly TicketState[]
}

export function columnHoldsManyStates(
  column: BoardColumnId,
  layouts: BoardLayoutState,
): boolean {
  return boardColumnStates(column, layouts).length > 1
}

/**
 * The state a drop on this column asks the core for. Draft is not a
 * drop target, and the core — not this mapping — decides whether the
 * move is legal for the Ticket's kind.
 */
export function inboundStateForColumn(
  column: BoardColumnId,
  layouts: BoardLayoutState,
): TicketState | undefined {
  if (column === 'draft') return undefined
  const collapsedAxis = AXIS_DEFINITIONS.find(
    (entry) => entry.group === column && layouts[entry.axis] === 'collapsed',
  )
  if (collapsedAxis) return groupOf(collapsedAxis.group)?.defaultState
  if (nestedStateColumn(column)) return column as TicketState
  return groupOf(column)?.defaultState
}

/** A drop resolves to the single state its column means, or nothing. */
export function dropFor(
  state: TicketState,
  target: BoardColumnId,
  layouts: BoardLayoutState,
): Readonly<{ state: TicketState }> | undefined {
  const destination = inboundStateForColumn(target, layouts)
  if (destination === undefined) return undefined
  if (columnForCard(state, layouts) === target) return undefined
  return { state: destination }
}

function fixedGroup(id: BoardGroupId): BoardColumnGroup {
  return {
    id,
    heading: boardColumnLabel(id),
    subheading: boardColumnSubheading(id),
    grouped: false,
    columns: [id],
  }
}

function axisGroup(
  axis: BoardLayoutAxis,
  layouts: BoardLayoutState,
): BoardColumnGroup {
  const groupId = AXIS_GROUP_IDS[axis]
  const group = groupOf(groupId)
  const columns =
    layouts[axis] === 'expanded'
      ? ((group?.states ?? []).filter(
          (state: TicketState) => state in NESTED_STATE_COLUMNS,
        ) as readonly BoardColumnId[])
      : ([groupId] as readonly BoardColumnId[])
  return {
    id: axis,
    heading: boardColumnLabel(groupId),
    subheading: boardColumnSubheading(groupId),
    grouped: columns.length > 1,
    columns,
  }
}

/** A range for three or more columns; two columns read better joined. */
function expandedAxisLabel(groupId: BoardGroupId): string {
  const labels = ((groupOf(groupId)?.states ?? []) as readonly TicketState[])
    .filter((state: TicketState) => state in NESTED_STATE_COLUMNS)
    .map((state) => boardColumnLabel(state as BoardColumnId))
  const first = labels[0] ?? ''
  const last = labels[labels.length - 1] ?? ''
  return labels.length === 2 ? `${first} and ${last}` : `${first} to ${last}`
}

function nestedStateColumn(column: BoardColumnId) {
  return column in NESTED_STATE_COLUMNS
    ? NESTED_STATE_COLUMNS[column as keyof typeof NESTED_STATE_COLUMNS]
    : undefined
}
