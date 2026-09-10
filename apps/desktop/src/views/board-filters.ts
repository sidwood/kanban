// The board's filter surface: the eight axes the handover fixes —
// Initiative, Project, Kind, State, Priority, Lane, Profile, Attention
// — each offering the values the core listed or the closed vocabulary
// the contracts carry, and the chips every held value renders as. The
// core filters; this module only shapes what the operator chooses.
import type {
  AttentionState,
  BoardFilter,
  BoardFilterOptions,
  TicketKind,
  TicketPriority,
  TicketState,
} from '@kanban/contracts'
import type { BoardScope } from '../stores/scope'
import { KIND_LABELS, STATUS_LABELS } from './board-card'
import { isOnBoard } from './board-layout'
import { ATTENTION_LABELS, PRIORITY_LABELS, emptyFilter } from './global-board-filters'

export const BOARD_FILTER_AXES = [
  'initiatives',
  'projects',
  'kinds',
  'states',
  'priorities',
  'lanes',
  'profiles',
  'attention',
] as const

export type BoardFilterAxis = (typeof BOARD_FILTER_AXES)[number]

/** Every axis a filter can hold, the two the flyout does not offer
 * included: a value another client saved still shows as a chip. */
export type FilterAxis = keyof BoardFilter

const AXIS_LABELS: Record<FilterAxis, string> = {
  initiatives: 'Initiative',
  projects: 'Project',
  plans: 'Plan',
  specs: 'Spec',
  kinds: 'Kind',
  states: 'State',
  priorities: 'Priority',
  lanes: 'Lane',
  profiles: 'Profile',
  attention: 'Attention',
}

const ID_AXES: readonly FilterAxis[] = ['initiatives', 'projects', 'plans', 'specs', 'lanes']

/** The chip order: identity axes first, then the vocabularies. */
const CHIP_ORDER: readonly FilterAxis[] = [
  'initiatives',
  'projects',
  'plans',
  'specs',
  'kinds',
  'states',
  'priorities',
  'lanes',
  'profiles',
  'attention',
]

export interface FilterOption {
  value: string
  label: string
}

export interface FilterAxisModel {
  axis: BoardFilterAxis
  label: string
  options: readonly FilterOption[]
  /** The values the axis holds, as strings. */
  selected: readonly string[]
}

export interface FilterChip {
  axis: FilterAxis
  value: string
  label: string
  valueLabel: string
}

// The kinds the vocabulary names, and every state the board can show.
const KIND_OPTIONS: readonly FilterOption[] = (
  Object.entries(KIND_LABELS) as [TicketKind, string][]
).map(([value, label]) => ({ value, label: label.replace(' Ticket', '') }))

const STATE_OPTIONS: readonly FilterOption[] = (
  Object.entries(STATUS_LABELS) as [TicketState, string][]
)
  .filter(([value]) => isOnBoard(value))
  .map(([value, label]) => ({ value, label }))

const PRIORITY_OPTIONS: readonly FilterOption[] = (
  Object.entries(PRIORITY_LABELS) as [TicketPriority, string][]
).map(([value, label]) => ({ value, label }))

function idOptions(entries: readonly { id: number; label: string }[] | undefined): FilterOption[] {
  return (entries ?? []).map((entry) => ({ value: String(entry.id), label: entry.label }))
}

function held(filter: BoardFilter, axis: FilterAxis): readonly string[] {
  return ((filter[axis] ?? []) as readonly (string | number)[]).map(String)
}

function optionsFor(axis: BoardFilterAxis, options: BoardFilterOptions | null): readonly FilterOption[] {
  switch (axis) {
    case 'initiatives':
      return idOptions(options?.initiatives)
    case 'projects':
      return idOptions(options?.projects)
    case 'lanes':
      return idOptions(options?.lanes)
    case 'kinds':
      return KIND_OPTIONS
    case 'states':
      return STATE_OPTIONS
    case 'priorities':
      return PRIORITY_OPTIONS
    case 'profiles':
      return (options?.profiles ?? []).map((name) => ({ value: name, label: name }))
    case 'attention':
      return (options?.attention ?? []).map((value) => ({
        value,
        label: ATTENTION_LABELS[value as AttentionState] ?? value,
      }))
  }
}

/** The eight axes, each with the values it offers and the ones it holds. */
export function filterAxes(
  filter: BoardFilter,
  options: BoardFilterOptions | null,
): readonly FilterAxisModel[] {
  return BOARD_FILTER_AXES.map((axis) => ({
    axis,
    label: AXIS_LABELS[axis],
    options: optionsFor(axis, options),
    selected: held(filter, axis),
  }))
}

function parseValue(axis: FilterAxis, value: string): string | number {
  return ID_AXES.includes(axis) ? Number(value) : value
}

/** The filter with one axis holding one value, or nothing. */
export function withAxisValue(
  filter: BoardFilter,
  axis: FilterAxis,
  value: string | null,
): BoardFilter {
  return {
    ...emptyFilter(),
    ...filter,
    [axis]: value === null || value === '' ? [] : [parseValue(axis, value)],
  } as BoardFilter
}

/** The filter with one value taken off one axis, the rest untouched. */
export function withoutValue(filter: BoardFilter, axis: FilterAxis, value: string): BoardFilter {
  const values = (filter[axis] ?? []) as readonly (string | number)[]
  return {
    ...emptyFilter(),
    ...filter,
    [axis]: values.filter((entry) => String(entry) !== value),
  } as BoardFilter
}

function valueLabel(axis: FilterAxis, value: string, options: BoardFilterOptions | null): string {
  switch (axis) {
    case 'initiatives':
    case 'projects':
    case 'plans':
    case 'specs':
    case 'lanes': {
      const found = options?.[axis]?.find((entry) => String(entry.id) === value)
      if (found) return found.label
      return `${AXIS_LABELS[axis]} ${value}`
    }
    case 'kinds':
      return KIND_LABELS[value as TicketKind]?.replace(' Ticket', '') ?? value
    case 'states':
      return STATUS_LABELS[value as TicketState] ?? value
    case 'priorities':
      return PRIORITY_LABELS[value as TicketPriority] ?? value
    case 'attention':
      return ATTENTION_LABELS[value as AttentionState] ?? value
    case 'profiles':
      return value
  }
}

/** One removable chip per held value, in the fixed axis order. A
 * Project scope's own Project is the scope, not a choice, so it wears
 * no chip there. */
export function filterChips(
  filter: BoardFilter,
  options: BoardFilterOptions | null,
  scope: BoardScope = 'all',
): readonly FilterChip[] {
  const chips: FilterChip[] = []
  for (const axis of CHIP_ORDER) {
    if (axis === 'projects' && scope !== 'all') continue
    for (const value of held(filter, axis)) {
      chips.push({
        axis,
        value,
        label: AXIS_LABELS[axis],
        valueLabel: valueLabel(axis, value, options),
      })
    }
  }
  return chips
}

/** The filter the scope sends: a Project scope pins its Project onto
 * the Project axis, whatever the view held there. */
export function scopedFilter(filter: BoardFilter, scope: BoardScope): BoardFilter {
  if (scope === 'all') return { ...emptyFilter(), ...filter }
  return { ...emptyFilter(), ...filter, projects: [scope] }
}

/** How many axes the operator chose: the scope's own Project is not
 * a choice. */
export function activeFilterCount(filter: BoardFilter, scope: BoardScope): number {
  return CHIP_ORDER.filter((axis) => {
    if (axis === 'projects' && scope !== 'all') return false
    return held(filter, axis).length > 0
  }).length
}
