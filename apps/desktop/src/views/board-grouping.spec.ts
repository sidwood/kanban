// KAN-T25-AC1: the board groups map the lifecycle exactly. These
// tests pin the presentation copy of the projection the domain owns
// (board.rs): every active state reaches exactly one of the six fixed
// groups — Backlog holds parked, blocked, scheduled, and ready, Staged
// holds approved and landing — and the terminal states never appear,
// under any layout.
import { describe, expect, it } from 'vitest'
import type { TicketState } from '@kanban/contracts'
import {
  BOARD_GROUPS,
  type BoardLayoutState,
  DEFAULT_BOARD_LAYOUTS,
  columnForCard,
  isOnBoard,
} from './board-layout'

const EVERY_STATE: readonly TicketState[] = [
  'draft',
  'parked',
  'blocked',
  'scheduled',
  'ready',
  'active',
  'in_review',
  'approved',
  'landing',
  'done',
  'cancelled',
  'superseded',
]

const TERMINAL_STATES: readonly TicketState[] = ['cancelled', 'superseded']

const LAYOUT_COMBINATIONS: readonly BoardLayoutState[] = [
  DEFAULT_BOARD_LAYOUTS,
  { backlog: 'expanded', completion: 'collapsed' },
  { backlog: 'collapsed', completion: 'expanded' },
  { backlog: 'expanded', completion: 'expanded' },
]

const statesPlaced = () =>
  new Set(BOARD_GROUPS.flatMap((group) => group.states as readonly TicketState[]))

describe('the board groups', () => {
  it('stand in their fixed order with their fixed names', () => {
    expect(BOARD_GROUPS.map((group) => group.id)).toEqual([
      'draft',
      'backlog',
      'current',
      'review',
      'staged',
      'done',
    ])
    expect(BOARD_GROUPS.map((group) => group.label)).toEqual([
      'Draft',
      'Backlog',
      'Current',
      'Review',
      'Staged',
      'Done',
    ])
  })

  it('map every active state into its group', () => {
    const placements: readonly [TicketState, string][] = [
      ['draft', 'draft'],
      ['parked', 'backlog'],
      ['blocked', 'backlog'],
      ['scheduled', 'backlog'],
      ['ready', 'backlog'],
      ['active', 'current'],
      ['in_review', 'review'],
      ['approved', 'staged'],
      ['landing', 'staged'],
      ['done', 'done'],
    ]

    for (const [state, group] of placements) {
      expect(columnForCard(state, DEFAULT_BOARD_LAYOUTS), state).toBe(group)
      expect(isOnBoard(state), state).toBe(true)
    }
  })

  it('hold Backlog and Staged over exactly their states', () => {
    const backlog = BOARD_GROUPS.find((group) => group.id === 'backlog')
    const staged = BOARD_GROUPS.find((group) => group.id === 'staged')

    expect(backlog?.states).toEqual(['parked', 'blocked', 'scheduled', 'ready'])
    expect(staged?.states).toEqual(['approved', 'landing'])
  })

  it('partition the active states exactly once each', () => {
    const placed = statesPlaced()

    for (const state of EVERY_STATE) {
      const holdings = BOARD_GROUPS.filter((group) =>
        (group.states as readonly TicketState[]).includes(state),
      )

      if (TERMINAL_STATES.includes(state)) {
        expect(holdings, state).toHaveLength(0)
        expect(placed.has(state), state).toBe(false)
      } else {
        expect(holdings, state).toHaveLength(1)
        expect(placed.has(state), state).toBe(true)
      }
    }
  })

  it('never place a terminal state, under any layout', () => {
    for (const state of TERMINAL_STATES) {
      expect(isOnBoard(state), state).toBe(false)
      for (const layouts of LAYOUT_COMBINATIONS) {
        expect(columnForCard(state, layouts), `${state} under ${JSON.stringify(layouts)}`)
          .toBeUndefined()
      }
    }
  })

  it('keep every state inside its own group when an axis opens', () => {
    const expanded: BoardLayoutState = {
      backlog: 'expanded',
      completion: 'expanded',
    }

    // Opening an axis splits its states into sibling columns of the
    // same group; it never moves a state into another group.
    expect(columnForCard('parked', expanded)).toBe('parked')
    expect(columnForCard('ready', expanded)).toBe('ready')
    expect(columnForCard('approved', expanded)).toBe('approved')
    expect(columnForCard('landing', expanded)).toBe('landing')
    expect(columnForCard('draft', expanded)).toBe('draft')
    expect(columnForCard('active', expanded)).toBe('current')
  })
})
