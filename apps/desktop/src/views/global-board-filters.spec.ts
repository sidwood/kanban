import { describe, expect, it } from 'vitest'
import type { BoardGlobalCard, TicketRecord } from '@kanban/contracts'
import {
  ATTENTION_LABELS,
  GLOBAL_BOARD_GROUPS,
  activeAxisCount,
  cardsOfGroup,
  emptyFilter,
  toggleValue,
} from './global-board-filters'

const card = (group: BoardGlobalCard['group'], number: number): BoardGlobalCard => ({
  group,
  project_code: 'CORE',
  spec_number: null,
  lane_id: null,
  ticket: { number } as TicketRecord,
})

describe('global board filters', () => {
  it('starts from a filter that holds nothing', () => {
    const filter = emptyFilter()

    expect(filter).toEqual({
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
    })
  })

  it('toggles one value in and out of one axis', () => {
    expect(toggleValue([1, 2], 3)).toEqual([1, 2, 3])
    expect(toggleValue([1, 2], 2)).toEqual([1])
    expect(toggleValue(['urgent'], 'low')).toEqual(['urgent', 'low'])
    expect(toggleValue([], 'blocker')).toEqual(['blocker'])
  })

  it('counts only the axes holding a value', () => {
    expect(activeAxisCount(emptyFilter())).toBe(0)
    expect(
      activeAxisCount({
        ...emptyFilter(),
        projects: [2, 3],
        kinds: ['task'],
        attention: ['stale_run'],
      }),
    ).toBe(3)
  })

  it('labels the closed attention vocabulary', () => {
    expect(Object.keys(ATTENTION_LABELS)).toHaveLength(8)
    expect(ATTENTION_LABELS.missing_result).toBe('Missing result')
    expect(ATTENTION_LABELS.disconnected_session).toBe('Disconnected session')
  })

  it('fixes the six groups in their board order', () => {
    expect(GLOBAL_BOARD_GROUPS.map((entry) => entry.group)).toEqual([
      'draft',
      'backlog',
      'current',
      'review',
      'staged',
      'done',
    ])
  })

  it('keeps the projection order inside every group and partitions each card once', () => {
    // The core already ordered the cards; the view's mapping only
    // collects, never re-sorts and never re-maps.
    const cards = [
      card('backlog', 2),
      card('current', 5),
      card('backlog', 7),
      card('staged', 1),
      card('done', 9),
      card('backlog', 4),
    ]

    expect(cardsOfGroup(cards, 'backlog').map((entry) => entry.ticket.number)).toEqual([
      2, 7, 4,
    ])
    expect(cardsOfGroup(cards, 'staged').map((entry) => entry.ticket.number)).toEqual([1])

    const everywhere = GLOBAL_BOARD_GROUPS.flatMap((entry) => cardsOfGroup(cards, entry.group))
    expect(everywhere).toHaveLength(cards.length)
    expect(new Set(everywhere).size).toBe(cards.length)
  })
})
