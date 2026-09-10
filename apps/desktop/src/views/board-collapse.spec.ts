import { describe, expect, it } from 'vitest'
import {
  COLLAPSED_COLUMN_WIDTH_PX,
  DEFAULT_BOARD_LAYOUTS,
  canonicalGroups,
  collapsedCount,
  columnForGroupedCard,
  columnsOfGroup,
  draftControl,
  registerTableLabel,
} from './board-layout'

const expandedBacklog = { ...DEFAULT_BOARD_LAYOUTS, backlog: 'expanded' } as const

describe('column collapse', () => {
  it('leaves a 48px rail', () => {
    expect(COLLAPSED_COLUMN_WIDTH_PX).toBe(48)
  })

  it('counts only the collapsed columns the board shows', () => {
    const collapsed = ['review', 'parked'] as const
    expect(collapsedCount(collapsed, ['backlog', 'current', 'review', 'staged', 'done'])).toBe(1)
    expect(collapsedCount(collapsed, ['parked', 'blocked', 'review'])).toBe(2)
  })

  it('names the columns one group shows, so a control over it reaches them all', () => {
    expect(columnsOfGroup('backlog', DEFAULT_BOARD_LAYOUTS)).toEqual(['backlog'])
    expect(columnsOfGroup('backlog', expandedBacklog)).toEqual([
      'parked',
      'blocked',
      'scheduled',
      'ready',
    ])
    // A group that belongs to no axis is always its own column.
    expect(columnsOfGroup('review', expandedBacklog)).toEqual(['review'])
    expect(columnsOfGroup('staged', expandedBacklog)).toEqual(['staged'])
  })
})

describe('grouped cards', () => {
  it('sit in the group the core placed them in while the axis is aggregated', () => {
    expect(columnForGroupedCard('backlog', 'ready', DEFAULT_BOARD_LAYOUTS)).toBe('backlog')
    expect(columnForGroupedCard('staged', 'landing', DEFAULT_BOARD_LAYOUTS)).toBe('staged')
    expect(columnForGroupedCard('current', 'active', DEFAULT_BOARD_LAYOUTS)).toBe('current')
  })

  it('open into their state column when the axis expands', () => {
    expect(columnForGroupedCard('backlog', 'ready', expandedBacklog)).toBe('ready')
    expect(columnForGroupedCard('backlog', 'parked', expandedBacklog)).toBe('parked')
    expect(columnForGroupedCard('staged', 'landing', expandedBacklog)).toBe('staged')
  })
})

describe('register tables', () => {
  it('name a nested state under its group', () => {
    expect(registerTableLabel('parked')).toBe('Backlog · Parked')
    expect(registerTableLabel('landing')).toBe('Staged · Landing')
    expect(registerTableLabel('backlog')).toBe('Backlog')
    expect(registerTableLabel('done')).toBe('Done')
  })
})

describe('the canonical group order', () => {
  it('holds groups in board order, each once', () => {
    expect(canonicalGroups(['staged', 'backlog', 'staged'])).toEqual(['backlog', 'staged'])
    expect(canonicalGroups(['done', 'draft'])).toEqual(['draft', 'done'])
    expect(canonicalGroups([])).toEqual([])
  })
})

describe('the Draft control', () => {
  it('offers to show an empty auto Draft', () => {
    expect(draftControl(['draft'], 0)).toEqual({
      label: 'Show Draft',
      disabled: false,
      next: [],
      shown: false,
    })
  })

  it('offers to hide an always-shown Draft', () => {
    expect(draftControl([], 0)).toEqual({
      label: 'Hide Draft',
      disabled: false,
      next: ['draft'],
      shown: true,
    })
    expect(draftControl(['done'], 2).next).toEqual(['draft', 'done'])
  })

  it('says a populated auto Draft shows on its own and cannot be hidden', () => {
    expect(draftControl(['draft'], 3)).toEqual({
      label: 'Draft (auto)',
      disabled: true,
      next: ['draft'],
      shown: true,
    })
  })
})
