// KAN-T30-AC1: the Done table lists completed work with the same
// filters and ordering as the board — terminal states stay off the
// surface, and the deterministic order the columns scan is the order
// the table scans.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { TicketRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import { defaultViews, harness, ticket } from '../test/shell-harness'
import BoardView from './BoardView.vue'
import { orderCards } from './board-ordering'
import { columnForCard, DEFAULT_BOARD_LAYOUTS } from './board-layout'

const mountedBoards: VueWrapper[] = []

// The board over the shared harness, with every scope's default view
// placing Done where the test says.
async function mountedBoard(tickets: TicketRecord[], donePlacement: 'column' | 'table' = 'table') {
  const shell = harness({
    tickets,
    views: defaultViews().map((view) => ({ ...view, done_placement: donePlacement })),
  })
  await router.push('/projects/1/board')
  await router.isReady()
  const wrapper = mount(BoardView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: shell.transport },
    },
  })
  mountedBoards.push(wrapper)
  await flushPromises()
  return wrapper
}

function doneTableIds(wrapper: VueWrapper): number[] {
  return wrapper
    .findAll('[data-testid="done-table"] [data-testid^="done-row-"]')
    .map((row) => Number((row.attributes('data-testid') ?? '').replace('done-row-', '')))
}

function doneColumnIds(wrapper: VueWrapper): number[] {
  return wrapper
    .findAll('[data-testid="kanban-column-done"] [data-testid^="kanban-card-"]')
    .map((card) => Number((card.attributes('data-testid') ?? '').replace('kanban-card-', '')))
}

beforeEach(() => {
  localStorage.clear()
  document.documentElement.classList.remove('dark')
})

afterEach(() => {
  for (const wrapper of mountedBoards.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

describe('done table', () => {
  it('lists only on-board done tickets, never terminal states', async () => {
    const tickets = [
      ticket({ id: 10, number: 15, state: 'done', title: 'Landed the export path' }),
      ticket({ id: 12, number: 16, state: 'cancelled' }),
      ticket({ id: 13, number: 17, state: 'superseded' }),
      ticket({ id: 14, number: 18, state: 'done', title: 'Closed the loop' }),
    ]
    const wrapper = await mountedBoard(tickets)

    // The projection never carries a terminal state, so neither does
    // the table nor any column.
    expect(doneTableIds(wrapper)).toEqual([10, 14])
    expect(wrapper.find('[data-testid="done-count"]').text()).toBe('2')
    expect(wrapper.find('[data-testid="done-row-12"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="done-row-13"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="kanban-card-12"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="kanban-card-13"]').exists()).toBe(false)
  })

  it('orders done rows the same way the board column would', async () => {
    const tickets = [
      ticket({ id: 10, number: 30, state: 'done', priority: 'low', title: 'Low priority' }),
      ticket({ id: 11, number: 5, state: 'done', priority: 'urgent', title: 'Urgent first' }),
      ticket({ id: 12, number: 12, state: 'done', priority: 'normal', title: 'Middle tie' }),
      ticket({ id: 13, number: 8, state: 'done', priority: 'normal', title: 'Earlier number' }),
    ]
    const expected = orderCards(tickets)
      .filter((entry) => columnForCard(entry.state, DEFAULT_BOARD_LAYOUTS) === 'done')
      .map((entry) => entry.id)

    const wrapper = await mountedBoard(tickets.reverse())
    expect(doneTableIds(wrapper)).toEqual([11, 13, 12, 10])
    expect(doneTableIds(wrapper)).toEqual(expected)
  })

  it('matches the done column before demotion and the table after', async () => {
    const tickets = [
      ticket({ id: 10, number: 15, state: 'done', priority: 'high', title: 'First done' }),
      ticket({ id: 11, number: 20, state: 'done', priority: 'normal', title: 'Second done' }),
    ]
    const wrapper = await mountedBoard(tickets, 'column')

    expect(doneColumnIds(wrapper)).toEqual([10, 11])
    expect(wrapper.find('[data-testid="done-table"]').exists()).toBe(false)

    await wrapper.find('[data-testid="move-done-below-board"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-testid="kanban-column-done"]').exists()).toBe(false)
    expect(doneTableIds(wrapper)).toEqual([10, 11])
    expect(wrapper.find('[data-testid="done-count"]').text()).toBe('2')
  })
})
