// The global board's filter surface: the closed vocabularies the
// operator selects a value from, the wire filter's empty shape and
// its toggling, and the group mapping the core's projection already
// fixed. The core — not this module — filters, groups, and orders:
// nothing here recomputes the projection, it only collects the cards
// a group already holds, in the order they arrived. Lane, profile,
// and attention values populate as their feeds land; the axes and
// their vocabularies are complete now.
import type {
  AttentionState,
  BoardFilter,
  BoardGlobalCard,
  BoardGroup,
  TicketPriority,
} from '@kanban/contracts'
import { BOARD_GROUPS } from './board-layout'

/** The attention classes the operator reads, for the closed
 * vocabulary the wire carries. */
export const ATTENTION_LABELS: Record<AttentionState, string> = {
  blocker: 'Blocker',
  missing_result: 'Missing result',
  human_decision: 'Human decision',
  review_request: 'Review request',
  failed_schedule: 'Failed schedule',
  invalid_approval: 'Invalid approval',
  disconnected_session: 'Disconnected session',
  stale_run: 'Stale run',
}

/** The priorities the operator selects from. */
export const PRIORITY_LABELS: Record<TicketPriority, string> = {
  urgent: 'Urgent',
  high: 'High',
  normal: 'Normal',
  low: 'Low',
}

// The six fixed groups in their board order, wearing the labels the
// Project board already fixed: one vocabulary for every board.
export const GLOBAL_BOARD_GROUPS: readonly { group: BoardGroup; label: string }[] =
  BOARD_GROUPS.map(({ id, label }) => ({ group: id as BoardGroup, label }))

/** The wire filter holding nothing: the whole board. */
export function emptyFilter(): BoardFilter {
  return {
    initiatives: [],
    projects: [],
    plans: [],
    specs: [],
    kinds: [],
    states: [],
    priorities: [],
    lanes: [],
    profiles: [],
    attention: [],
  }
}

/** One axis with one value toggled: present when absent, gone when
 * present, the rest untouched. */
export function toggleValue<T>(values: readonly T[], value: T): T[] {
  return values.includes(value) ? values.filter((entry) => entry !== value) : [...values, value]
}

/** How many axes hold at least one value. */
export function activeAxisCount(filter: BoardFilter): number {
  return Object.values(filter).filter((values) => (values?.length ?? 0) > 0).length
}

/** The filter as the wire carries it: an axis holding nothing is
 * absent, not empty — the same shape the payloads define. */
export function wireFilter(filter: BoardFilter): BoardFilter {
  const wire: BoardFilter = {}
  for (const [axis, values] of Object.entries(filter)) {
    if (values.length > 0) {
      ;(wire as Record<string, unknown>)[axis] = values
    }
  }
  return wire
}

/** The cards one group holds, in the projection's own order — the
 * mapping the core fixed, collected, never recomputed. */
export function cardsOfGroup(
  cards: readonly BoardGlobalCard[],
  group: BoardGroup,
): readonly BoardGlobalCard[] {
  return cards.filter((card) => card.group === group)
}
