// KAN-T25-AC2: card position is never a decision. Cards order
// deterministically by priority, then by readiness — where the state
// sits in the canonical lifecycle — with the minted number breaking
// ties, so no manual ordering exists and relative order is stable
// under reload (DR-LC-11).
import { mount, flushPromises } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { BoardGlobalCard, TicketRecord, ViewSorting } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import { defaultViews, harness, projection, ticket as record } from '../test/shell-harness'
import BoardView from './BoardView.vue'
import { orderCards, orderGlobalCards } from './board-ordering'

const ticket = (overrides: Partial<TicketRecord> = {}): TicketRecord =>
  record({ id: 1, number: 1, version: 1, ...overrides })

const ids = (cards: readonly TicketRecord[]): number[] => cards.map((card) => card.id)

const cardIds = (cards: readonly BoardGlobalCard[]): number[] =>
  cards.map((card) => card.ticket.id)

describe('deterministic card ordering', () => {
  it('orders by priority first, whatever arrives however', () => {
    const cards = [
      ticket({ id: 1, number: 30, priority: 'low' }),
      ticket({ id: 2, number: 2 }),
      ticket({ id: 3, number: 7, priority: 'urgent' }),
      ticket({ id: 4, number: 9, priority: 'high' }),
    ]

    expect(ids(orderCards(cards))).toEqual([3, 4, 2, 1])
  })

  it('orders equal priorities by readiness inside the column', () => {
    const backlog = [
      ticket({ id: 1, number: 1, state: 'parked' }),
      ticket({ id: 2, number: 5, state: 'ready' }),
      ticket({ id: 3, number: 3, state: 'blocked' }),
      ticket({ id: 4, number: 4, state: 'scheduled' }),
    ]
    const staged = [
      ticket({ id: 5, number: 6, state: 'approved' }),
      ticket({ id: 6, number: 2, state: 'landing' }),
    ]

    expect(ids(orderCards(backlog))).toEqual([2, 4, 3, 1])
    expect(ids(orderCards(staged))).toEqual([6, 5])
  })

  it('breaks ties by the minted number, ascending', () => {
    const cards = [
      ticket({ id: 1, number: 12 }),
      ticket({ id: 2, number: 3 }),
      ticket({ id: 3, number: 7 }),
    ]

    expect(ids(orderCards(cards))).toEqual([2, 3, 1])
  })

  it('follows a priority change, the operator\'s one ordering lever', () => {
    const cards = [ticket({ id: 1, number: 2 }), ticket({ id: 2, number: 8 })]
    expect(ids(orderCards(cards))).toEqual([1, 2])

    const demoted = cards.map((card) =>
      card.id === 1 ? { ...card, priority: 'low' as const } : card,
    )
    expect(ids(orderCards(demoted))).toEqual([2, 1])
  })

  it('derives the same order from any arrival order', () => {
    const cards = [
      ticket({ id: 1, number: 1, state: 'parked' }),
      ticket({ id: 2, number: 5, state: 'ready' }),
      ticket({ id: 3, number: 3, state: 'blocked', priority: 'high' }),
      ticket({ id: 4, number: 4, state: 'scheduled' }),
      ticket({ id: 5, number: 2, priority: 'urgent' }),
      ticket({ id: 6, number: 6, state: 'done', priority: 'low' }),
    ]
    const expected = ids(orderCards(cards))

    const [first, second, third, fourth, fifth, sixth] = cards
    const shuffles = [
      [...cards].reverse(),
      [fourth, first, sixth, third, fifth, second],
      [fifth, second, fourth, sixth, first, third],
    ]
    for (const shuffle of shuffles) {
      expect(ids(orderCards(shuffle)), JSON.stringify(ids(shuffle))).toEqual(expected)
    }
  })

  it('orders by the sorting key the active view owns', () => {
    const cards = [
      ticket({ id: 1, number: 1, state: 'parked' }),
      ticket({ id: 2, number: 2, state: 'ready', priority: 'low' }),
      ticket({ id: 3, number: 3, state: 'blocked', priority: 'urgent' }),
    ]

    // Priority leads: urgent blocked, normal parked, low ready.
    expect(ids(orderCards(cards))).toEqual([3, 1, 2])
    // Readiness leads: ready above blocked above parked, priority
    // only breaking ties beneath it — both orders deterministic.
    expect(ids(orderCards(cards, 'readiness'))).toEqual([2, 3, 1])
  })

  it('re-keys the core\'s projection by readiness and leaves the canonical key untouched', () => {
    const cards = projection([
      ticket({ id: 1, number: 1, state: 'parked' }),
      ticket({ id: 2, number: 2, state: 'ready', priority: 'low' }),
      ticket({ id: 3, number: 3, state: 'blocked', priority: 'urgent' }),
    ])

    // The core's order is the priority order: the same array, as it
    // arrived.
    expect(cardIds(cards)).toEqual([3, 1, 2])
    expect(orderGlobalCards(cards)).toBe(cards)
    expect(orderGlobalCards(cards, 'priority')).toBe(cards)
    // Readiness first re-keys it the way orderCards would.
    expect(cardIds(orderGlobalCards(cards, 'readiness'))).toEqual([2, 3, 1])
  })

  it('leaves every readiness tie where the core put it', () => {
    const cards = projection([
      ticket({ id: 1, number: 9, state: 'ready' }),
      ticket({ id: 2, number: 4, state: 'ready' }),
      ticket({ id: 3, number: 6, state: 'ready', priority: 'high' }),
    ])

    expect(cardIds(cards)).toEqual([3, 2, 1])
    expect(cardIds(orderGlobalCards(cards, 'readiness'))).toEqual([3, 2, 1])
  })
})

function backlogCards(): TicketRecord[] {
  return [
    ticket({ id: 1, number: 1, state: 'parked' }),
    ticket({ id: 2, number: 5, state: 'ready' }),
    ticket({ id: 3, number: 3, state: 'blocked' }),
    ticket({ id: 4, number: 4, state: 'scheduled', priority: 'high' }),
    ticket({ id: 5, number: 2, priority: 'urgent', state: 'parked' }),
  ]
}

const mountedBoards: VueWrapper[] = []

// The board itself, ordered end to end over the shared harness: the
// columns the operator scans hold the deterministic order, and a
// reload holds the same relative order from a different arrival
// order. The Project's default view carries the sorting key a test
// names.
async function mountedBoard(tickets: TicketRecord[], sorting: ViewSorting = 'priority') {
  const shell = harness({
    tickets,
    views: defaultViews().map((view) => ({ ...view, sorting })),
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

function columnCardIds(wrapper: VueWrapper, column: string): number[] {
  return wrapper
    .findAll(`[data-testid="kanban-column-${column}"] [data-testid^="kanban-card-"]`)
    .map((card) =>
      Number((card.attributes('data-testid') ?? '').replace('kanban-card-', '')),
    )
}

beforeEach(() => {
  localStorage.clear()
  document.documentElement.classList.remove('dark')
})

afterEach(() => {
  for (const wrapper of mountedBoards.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

describe('the ordered board', () => {
  it('renders each column in the deterministic order', async () => {
    const wrapper = await mountedBoard([
      ...backlogCards().reverse(),
      ticket({ id: 6, number: 8, state: 'approved' }),
      ticket({ id: 7, number: 6, state: 'landing' }),
    ])

    expect(columnCardIds(wrapper, 'backlog')).toEqual([5, 4, 2, 3, 1])
    expect(columnCardIds(wrapper, 'staged')).toEqual([7, 6])
  })

  it('holds the same relative order across a reload', async () => {
    const first = await mountedBoard([...backlogCards()])
    const firstOrder = columnCardIds(first, 'backlog')
    first.unmount()
    mountedBoards.splice(mountedBoards.indexOf(first), 1)

    const second = await mountedBoard([...backlogCards().reverse()])

    expect(columnCardIds(second, 'backlog')).toEqual(firstOrder)
    expect(firstOrder).toEqual([5, 4, 2, 3, 1])
  })

  it('renders under the sorting key the active view owns', async () => {
    const wrapper = await mountedBoard([...backlogCards()], 'readiness')

    // Readiness leads: ready, scheduled, blocked, then the parked
    // pair by priority beneath it.
    expect(wrapper.get('[data-testid="sort-readiness"]').attributes('aria-pressed')).toBe('true')
    expect(columnCardIds(wrapper, 'backlog')).toEqual([2, 4, 3, 5, 1])
  })
})
