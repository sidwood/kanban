import { describe, expect, it } from 'vitest'
import type { TicketState } from '@kanban/contracts'
import {
  BOARD_LAYOUT_AXES,
  type BoardColumnId,
  DEFAULT_BOARD_LAYOUTS,
  DEFAULT_HIDDEN_COLUMNS,
  boardColumnGroups,
  boardColumnLabel,
  boardColumnStates,
  boardLayoutAxisControls,
  columnForCard,
  columnHoldsManyStates,
  dropFor,
  inboundStateForColumn,
  isOnBoard,
  registerColumnsFor,
  resolveHiddenColumns,
  visibleColumnsFor,
} from './board-layout'

const expandedBacklog = { ...DEFAULT_BOARD_LAYOUTS, backlog: 'expanded' } as const
const expandedCompletion = {
  ...DEFAULT_BOARD_LAYOUTS,
  completion: 'expanded',
} as const

describe('the fixed board groups', () => {
  it('place every active state in its group', () => {
    const placements: readonly [TicketState, BoardColumnId][] = [
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

    for (const [state, column] of placements) {
      expect(columnForCard(state, DEFAULT_BOARD_LAYOUTS), state).toBe(column)
    }
  })

  it('never place terminal states on the board', () => {
    expect(isOnBoard('cancelled')).toBe(false)
    expect(isOnBoard('superseded')).toBe(false)
    expect(columnForCard('cancelled', DEFAULT_BOARD_LAYOUTS)).toBeUndefined()
    expect(columnForCard('superseded', DEFAULT_BOARD_LAYOUTS)).toBeUndefined()
  })

  it('show the collapsed everyday board as the six fixed groups, Draft hidden', () => {
    const groups = boardColumnGroups(DEFAULT_BOARD_LAYOUTS, 'column', DEFAULT_HIDDEN_COLUMNS)

    expect(groups.map((group) => group.heading)).toEqual([
      'Backlog',
      'Current',
      'Review',
      'Staged',
      'Done',
    ])
    expect(visibleColumnsFor(DEFAULT_BOARD_LAYOUTS, 'column', DEFAULT_HIDDEN_COLUMNS)).toEqual([
      'backlog',
      'current',
      'review',
      'staged',
      'done',
    ])
  })

  it('bring Draft back when the operator asks for it', () => {
    expect(visibleColumnsFor(DEFAULT_BOARD_LAYOUTS, 'column', [])[0]).toBe('draft')
  })
})

describe('the collapsible axes', () => {
  it('expand Backlog into its four states under one group', () => {
    const groups = boardColumnGroups(expandedBacklog, 'column', DEFAULT_HIDDEN_COLUMNS)
    const backlog = groups.find((group) => group.id === 'backlog')

    expect(backlog?.grouped).toBe(true)
    expect(backlog?.columns).toEqual(['parked', 'blocked', 'scheduled', 'ready'])
    expect(backlog?.heading).toBe('Backlog')
    expect(columnForCard('blocked', expandedBacklog)).toBe('blocked')
    expect(boardColumnLabel('scheduled')).toBe('Scheduled')
  })

  it('expand Staged into Approved and Landing, Done staying its own column', () => {
    const groups = boardColumnGroups(expandedCompletion, 'column', DEFAULT_HIDDEN_COLUMNS)
    const completion = groups.find((group) => group.id === 'completion')

    expect(completion?.grouped).toBe(true)
    expect(completion?.columns).toEqual(['approved', 'landing'])
    expect(columnForCard('landing', expandedCompletion)).toBe('landing')
    expect(visibleColumnsFor(expandedCompletion, 'column', DEFAULT_HIDDEN_COLUMNS)).toEqual([
      'backlog',
      'current',
      'review',
      'approved',
      'landing',
      'done',
    ])
  })

  it('offer one control per axis, named by what it shows', () => {
    const controls = boardLayoutAxisControls(DEFAULT_BOARD_LAYOUTS)

    expect(controls.map((control) => control.axis)).toEqual([...BOARD_LAYOUT_AXES])
    expect(controls.map((control) => control.collapsedLabel)).toEqual([
      'Backlog',
      'Staged',
    ])
    expect(controls.map((control) => control.expandedLabel)).toEqual([
      'Parked to Ready',
      'Approved and Landing',
    ])
  })
})

describe('columns holding several states', () => {
  it('say which states sit inside them', () => {
    expect(boardColumnStates('backlog', DEFAULT_BOARD_LAYOUTS)).toEqual([
      'parked',
      'blocked',
      'scheduled',
      'ready',
    ])
    expect(boardColumnStates('staged', DEFAULT_BOARD_LAYOUTS)).toEqual([
      'approved',
      'landing',
    ])
    expect(boardColumnStates('done', DEFAULT_BOARD_LAYOUTS)).toEqual(['done'])
    expect(boardColumnStates('parked', expandedBacklog)).toEqual(['parked'])
  })

  it('flag exactly the columns that hold more than one state', () => {
    expect(columnHoldsManyStates('backlog', DEFAULT_BOARD_LAYOUTS)).toBe(true)
    expect(columnHoldsManyStates('staged', DEFAULT_BOARD_LAYOUTS)).toBe(true)
    expect(columnHoldsManyStates('done', DEFAULT_BOARD_LAYOUTS)).toBe(false)
    expect(columnHoldsManyStates('current', DEFAULT_BOARD_LAYOUTS)).toBe(false)
    expect(columnHoldsManyStates('parked', expandedBacklog)).toBe(false)
  })
})

describe('the Done table option', () => {
  it('takes the Done column below the board without touching the axis choice', () => {
    expect(visibleColumnsFor(DEFAULT_BOARD_LAYOUTS, 'table', DEFAULT_HIDDEN_COLUMNS)).not.toContain(
      'done',
    )
  })

  it('keeps Done a column on the board by default', () => {
    expect(visibleColumnsFor(DEFAULT_BOARD_LAYOUTS, 'column', DEFAULT_HIDDEN_COLUMNS)).toContain(
      'done',
    )
  })
})

describe('drop targets', () => {
  it('map every column to the state a drop asks the core for', () => {
    const targets: readonly [BoardColumnId, TicketState | undefined][] = [
      ['backlog', 'parked'],
      ['current', 'active'],
      ['review', 'in_review'],
      ['staged', 'approved'],
      ['done', 'done'],
      ['parked', 'parked'],
      ['ready', 'ready'],
      ['landing', 'landing'],
      ['draft', undefined],
    ]

    for (const [column, state] of targets) {
      expect(inboundStateForColumn(column, DEFAULT_BOARD_LAYOUTS), column).toBe(state)
    }
    expect(inboundStateForColumn('parked', expandedBacklog)).toBe('parked')
  })

  it('refuse a drop that goes nowhere', () => {
    expect(dropFor('parked', 'backlog', DEFAULT_BOARD_LAYOUTS)).toBeUndefined()
    expect(dropFor('done', 'done', DEFAULT_BOARD_LAYOUTS)).toBeUndefined()
    expect(dropFor('draft', 'draft', DEFAULT_BOARD_LAYOUTS)).toBeUndefined()
  })

  it('resolve a drop to the state its column means', () => {
    expect(dropFor('draft', 'backlog', DEFAULT_BOARD_LAYOUTS)).toEqual({
      state: 'parked',
    })
    expect(dropFor('ready', 'current', DEFAULT_BOARD_LAYOUTS)).toEqual({
      state: 'active',
    })
    expect(dropFor('active', 'done', DEFAULT_BOARD_LAYOUTS)).toEqual({
      state: 'done',
    })
  })
})

describe('the register presentation', () => {
  it('gives every visible column a table, keeping Done a column', () => {
    expect(registerColumnsFor(DEFAULT_BOARD_LAYOUTS, DEFAULT_HIDDEN_COLUMNS)).toEqual([
      'backlog',
      'current',
      'review',
      'staged',
      'done',
    ])
    expect(registerColumnsFor(expandedBacklog, [])).toEqual([
      'draft',
      'parked',
      'blocked',
      'scheduled',
      'ready',
      'current',
      'review',
      'staged',
      'done',
    ])
  })
})

describe('hidden columns', () => {
  it('forces Draft visible while cards sit in it', () => {
    expect(resolveHiddenColumns(['draft'], 2)).toEqual([])
    expect(resolveHiddenColumns(['draft'], 0)).toEqual(['draft'])
    expect(resolveHiddenColumns([], 0)).toEqual([])
  })

  it('hide any group a view names, Draft forced visible aside', () => {
    const hidden = ['draft', 'current', 'done'] as const
    expect(resolveHiddenColumns(hidden, 0)).toEqual(['draft', 'current', 'done'])
    expect(resolveHiddenColumns(hidden, 3)).toEqual(['current', 'done'])
    expect(visibleColumnsFor(DEFAULT_BOARD_LAYOUTS, 'column', ['review'])).toEqual([
      'draft',
      'backlog',
      'current',
      'staged',
      'done',
    ])
  })
})

