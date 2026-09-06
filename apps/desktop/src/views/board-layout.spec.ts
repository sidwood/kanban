import { describe, expect, it } from 'vitest'
import type { TicketState } from '@kanban/contracts'
import {
  BOARD_LAYOUT_AXES,
  type BoardColumnId,
  DEFAULT_BOARD_LAYOUTS,
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
  resolveDraftVisibility,
  visibleColumnsFor,
} from './board-layout'
import {
  BOARD_LAYOUT_STORAGE_KEY,
  loadBoardChoices,
  saveBoardChoices,
} from './board-layout.storage'

function memoryStore(initial: string | null = null) {
  let value = initial
  return {
    getItem: () => value,
    setItem: (_key: string, next: string) => {
      value = next
    },
    removeItem: () => {
      value = null
    },
  }
}

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
    const groups = boardColumnGroups(DEFAULT_BOARD_LAYOUTS, 'column', 'hidden')

    expect(groups.map((group) => group.heading)).toEqual([
      'Backlog',
      'Current',
      'Review',
      'Staged',
      'Done',
    ])
    expect(visibleColumnsFor(DEFAULT_BOARD_LAYOUTS, 'column', 'hidden')).toEqual([
      'backlog',
      'current',
      'review',
      'staged',
      'done',
    ])
  })

  it('bring Draft back when the operator asks for it', () => {
    expect(visibleColumnsFor(DEFAULT_BOARD_LAYOUTS, 'column', 'visible')[0]).toBe(
      'draft',
    )
  })
})

describe('the collapsible axes', () => {
  it('expand Backlog into its four states under one group', () => {
    const groups = boardColumnGroups(expandedBacklog, 'column', 'hidden')
    const backlog = groups.find((group) => group.id === 'backlog')

    expect(backlog?.grouped).toBe(true)
    expect(backlog?.columns).toEqual(['parked', 'blocked', 'scheduled', 'ready'])
    expect(backlog?.heading).toBe('Backlog')
    expect(columnForCard('blocked', expandedBacklog)).toBe('blocked')
    expect(boardColumnLabel('scheduled')).toBe('Scheduled')
  })

  it('expand Staged into Approved and Landing, Done staying its own column', () => {
    const groups = boardColumnGroups(expandedCompletion, 'column', 'hidden')
    const completion = groups.find((group) => group.id === 'completion')

    expect(completion?.grouped).toBe(true)
    expect(completion?.columns).toEqual(['approved', 'landing'])
    expect(columnForCard('landing', expandedCompletion)).toBe('landing')
    expect(visibleColumnsFor(expandedCompletion, 'column', 'hidden')).toEqual([
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
    expect(visibleColumnsFor(DEFAULT_BOARD_LAYOUTS, 'table', 'hidden')).not.toContain(
      'done',
    )
  })

  it('keeps Done a column on the board by default', () => {
    expect(visibleColumnsFor(DEFAULT_BOARD_LAYOUTS, 'column', 'hidden')).toContain(
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
    expect(registerColumnsFor(DEFAULT_BOARD_LAYOUTS, 'hidden')).toEqual([
      'backlog',
      'current',
      'review',
      'staged',
      'done',
    ])
    expect(registerColumnsFor(expandedBacklog, 'visible')).toEqual([
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

describe('draft visibility', () => {
  it('forces Draft visible while cards sit in it', () => {
    expect(resolveDraftVisibility('hidden', 2)).toBe('visible')
    expect(resolveDraftVisibility('hidden', 0)).toBe('hidden')
    expect(resolveDraftVisibility('visible', 0)).toBe('visible')
  })
})

describe('stored board choices', () => {
  it('default every choice when nothing is stored', () => {
    const choices = loadBoardChoices(memoryStore())

    expect(choices).toEqual({
      layouts: DEFAULT_BOARD_LAYOUTS,
      done: 'column',
      presentation: 'board',
      draft: 'hidden',
    })
  })

  it('round-trip every choice together', () => {
    const store = memoryStore()
    saveBoardChoices(
      {
        layouts: { backlog: 'expanded', completion: 'expanded' },
        done: 'table',
        presentation: 'register',
        draft: 'visible',
      },
      store,
    )

    expect(loadBoardChoices(store)).toEqual({
      layouts: { backlog: 'expanded', completion: 'expanded' },
      done: 'table',
      presentation: 'register',
      draft: 'visible',
    })
  })

  it('keep the sound choices when one value rots', () => {
    const store = memoryStore(
      JSON.stringify({
        version: 1,
        layouts: { backlog: 'sideways', completion: 'expanded' },
        done: 'somewhere else',
        presentation: 'register',
        draft: 'visible',
      }),
    )

    expect(loadBoardChoices(store)).toEqual({
      layouts: { backlog: 'collapsed', completion: 'expanded' },
      done: 'column',
      presentation: 'register',
      draft: 'visible',
    })
  })

  it('ignore a stored record from another version', () => {
    const store = memoryStore(
      JSON.stringify({
        version: 0,
        layouts: { backlog: 'expanded', completion: 'expanded' },
        done: 'table',
        presentation: 'register',
        draft: 'visible',
      }),
    )

    expect(loadBoardChoices(store).presentation).toBe('board')
  })

  it('survive a refusing storage', () => {
    const refusing = {
      getItem: () => {
        throw new Error('private mode')
      },
      setItem: () => {
        throw new Error('quota')
      },
      removeItem: () => undefined,
    }

    expect(() =>
      saveBoardChoices(
        {
          layouts: DEFAULT_BOARD_LAYOUTS,
          done: 'table',
          presentation: 'board',
          draft: 'hidden',
        },
        refusing,
      ),
    ).not.toThrow()
    expect(loadBoardChoices(refusing).done).toBe('column')
    expect(BOARD_LAYOUT_STORAGE_KEY).toContain('kanban.board')
  })
})
