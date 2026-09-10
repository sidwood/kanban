import { describe, expect, it } from 'vitest'
import type { BoardFilter, BoardFilterOptions } from '@kanban/contracts'
import {
  BOARD_FILTER_AXES,
  activeFilterCount,
  filterAxes,
  filterChips,
  scopedFilter,
  withAxisValue,
  withoutValue,
} from './board-filters'
import { emptyFilter } from './global-board-filters'

const options: BoardFilterOptions = {
  initiatives: [{ id: 1, label: 'Personal tooling' }],
  projects: [
    { id: 1, label: 'CORE — Control plane' },
    { id: 2, label: 'EDGE — Edge tooling' },
  ],
  plans: [{ id: 3, label: 'CORE-P1' }],
  specs: [{ id: 4, label: 'CORE-S2 · Board' }],
  lanes: [{ id: 5, label: 'CORE lane 5' }],
  profiles: ['standard', 'deep'],
  attention: ['blocker', 'stale_run'],
}

describe('the eight board filters', () => {
  it('offer exactly the handover axes in order', () => {
    expect([...BOARD_FILTER_AXES]).toEqual([
      'initiatives',
      'projects',
      'kinds',
      'states',
      'priorities',
      'lanes',
      'profiles',
      'attention',
    ])
  })

  it('offer the values the core listed, and the closed vocabularies', () => {
    const axes = filterAxes(emptyFilter(), options)
    expect(axes.map((axis) => axis.label)).toEqual([
      'Initiative',
      'Project',
      'Kind',
      'State',
      'Priority',
      'Lane',
      'Profile',
      'Attention',
    ])
    expect(axes[0].options).toEqual([{ value: '1', label: 'Personal tooling' }])
    expect(axes[1].options.map((option) => option.label)).toEqual([
      'CORE — Control plane',
      'EDGE — Edge tooling',
    ])
    expect(axes[2].options.map((option) => option.value)).toEqual(['implementation', 'bug', 'task'])
    // Terminal states are never on the board, so the State axis never offers them.
    expect(axes[3].options.map((option) => option.value)).toEqual([
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
    ])
    expect(axes[4].options.map((option) => option.value)).toEqual(['urgent', 'high', 'normal', 'low'])
    expect(axes[5].options).toEqual([{ value: '5', label: 'CORE lane 5' }])
    expect(axes[6].options.map((option) => option.value)).toEqual(['standard', 'deep'])
    expect(axes[7].options.map((option) => option.label)).toEqual(['Blocker', 'Stale run'])
  })

  it('offer the vocabularies before the options arrive', () => {
    const axes = filterAxes(emptyFilter(), null)
    expect(axes[0].options).toEqual([])
    expect(axes[2].options).toHaveLength(3)
  })

  it('carry the selected value of each axis', () => {
    const filter: BoardFilter = { ...emptyFilter(), projects: [2], kinds: ['bug'] }
    const axes = filterAxes(filter, options)
    expect(axes[1].selected).toEqual(['2'])
    expect(axes[2].selected).toEqual(['bug'])
    expect(axes[0].selected).toEqual([])
  })

  it('sets and clears one axis value at a time', () => {
    const set = withAxisValue(emptyFilter(), 'projects', '2')
    expect(set.projects).toEqual([2])
    expect(withAxisValue(set, 'projects', null).projects).toEqual([])
    expect(withAxisValue(emptyFilter(), 'kinds', 'task').kinds).toEqual(['task'])
    expect(withAxisValue(emptyFilter(), 'lanes', '5').lanes).toEqual([5])
  })

  it('removes one value from an axis and keeps the rest', () => {
    const filter: BoardFilter = { ...emptyFilter(), states: ['ready', 'active'], projects: [1] }
    const without = withoutValue(filter, 'states', 'ready')
    expect(without.states).toEqual(['active'])
    expect(without.projects).toEqual([1])
    expect(withoutValue(without, 'projects', '1').projects).toEqual([])
  })

  it('renders every held value as a removable chip, labelled by the core', () => {
    const filter: BoardFilter = {
      ...emptyFilter(),
      projects: [2],
      states: ['in_review'],
      plans: [3],
      attention: ['stale_run'],
      profiles: ['deep'],
    }
    const chips = filterChips(filter, options)
    expect(chips).toEqual([
      { axis: 'projects', value: '2', label: 'Project', valueLabel: 'EDGE — Edge tooling' },
      { axis: 'plans', value: '3', label: 'Plan', valueLabel: 'CORE-P1' },
      { axis: 'states', value: 'in_review', label: 'State', valueLabel: 'In Review' },
      { axis: 'profiles', value: 'deep', label: 'Profile', valueLabel: 'deep' },
      { axis: 'attention', value: 'stale_run', label: 'Attention', valueLabel: 'Stale run' },
    ])
  })

  it('wears no chip for the Project a scope pins', () => {
    const filter: BoardFilter = { ...emptyFilter(), projects: [4], kinds: ['task'] }
    expect(filterChips(filter, options, 4).map((chip) => chip.axis)).toEqual(['kinds'])
    expect(filterChips(filter, options, 'all').map((chip) => chip.axis)).toEqual(['projects', 'kinds'])
  })

  it('still shows a terminal state a record holds, so it can be removed', () => {
    const chips = filterChips({ ...emptyFilter(), states: ['cancelled'] }, options)
    expect(chips).toEqual([{ axis: 'states', value: 'cancelled', label: 'State', valueLabel: 'Cancelled' }])
  })

  it('names an identity the options have not resolved by its number', () => {
    const chips = filterChips({ ...emptyFilter(), lanes: [9] }, options)
    expect(chips).toEqual([{ axis: 'lanes', value: '9', label: 'Lane', valueLabel: 'Lane 9' }])
  })

  it('pins the scope\'s Project onto the filter it sends', () => {
    const filter: BoardFilter = { ...emptyFilter(), projects: [2], kinds: ['task'] }
    expect(scopedFilter(filter, 4)).toEqual({ ...emptyFilter(), projects: [4], kinds: ['task'] })
    expect(scopedFilter(filter, 'all')).toEqual(filter)
  })

  it('counts the axes the operator chose, never the scope', () => {
    const filter: BoardFilter = { ...emptyFilter(), projects: [4], kinds: ['task'] }
    expect(activeFilterCount(filter, 4)).toBe(1)
    expect(activeFilterCount(filter, 'all')).toBe(2)
    expect(activeFilterCount(emptyFilter(), 'all')).toBe(0)
  })
})
