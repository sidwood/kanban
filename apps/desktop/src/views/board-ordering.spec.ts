// KAN-T25-AC2: card position is never a decision. Cards order
// deterministically by priority, then by readiness — where the state
// sits in the canonical lifecycle — with the minted number breaking
// ties, so no manual ordering exists and relative order is stable
// under reload (DR-LC-11).
import { mount, flushPromises } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type {
  LaneListResponse,
  ProjectListResponse,
  SpecListResponse,
  TicketListResponse,
  TicketReadinessResponse,
  TicketRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import BoardView from './BoardView.vue'
import { orderCards } from './board-ordering'

const ticket = (overrides: Partial<TicketRecord> = {}): TicketRecord => ({
  id: 1,
  project_id: 1,
  number: 1,
  kind: 'task',
  priority: 'normal',
  state: 'ready',
  spec_id: null,
  title: 'Archive the old exports',
  slice: null,
  criteria: [],
  bug: null,
  subtype: 'operational',
  mode: 'human',
  completion: ['The old exports are archived.'],
  scheduled_for: null,
  due: null,
  profile: null,
  version: 1,
  ...overrides,
})

const ids = (cards: readonly TicketRecord[]): number[] => cards.map((card) => card.id)

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
})

// One saved view for each scope the board loads: the generated
// defaults, the Project one carrying the sorting key a test names.
function viewList(sorting: 'priority' | 'readiness') {
  return {
    views: [
      {
        id: 1,
        name: 'All work',
        scope: 'global',
        filter: {},
        expanded_groups: [],
        hidden_columns: ['draft'],
        mode: 'board',
        done_placement: 'column',
        sorting: 'priority',
        is_default: true,
        version: 1,
      },
      {
        id: 2,
        name: 'All work',
        scope: { project: 1 },
        filter: { projects: [1] },
        expanded_groups: [],
        hidden_columns: ['draft'],
        mode: 'board',
        done_placement: 'column',
        sorting,
        is_default: true,
        version: 1,
      },
    ],
  }
}

// The board itself, ordered end to end: the columns the operator
// scans hold the deterministic order, and a reload holds the same
// relative order from a different arrival order.
function harness(ticketsForLoad: () => TicketRecord[], sorting: 'priority' | 'readiness' = 'priority') {
  const query = vi.fn((name: string, request: unknown) => {
    if (name === 'view.list') {
      return Promise.resolve(viewList(sorting))
    }
    if (name === 'project.list') {
      return Promise.resolve({
        projects: [
          {
            id: 1,
            code: 'KAN',
            name: 'Control plane',
            repository: '/repositories/kanban',
            seed_workspace: '/workspaces/kanban.seed',
            default_branch: 'main',
            herdr_session: 'kanban-main',
            herdr_workspace: 'kanban.seed',
            initiative_id: null,
            archived: false,
            counters: { plan: 0, spec: 0, ticket: 0 },
            version: 1,
          },
        ],
      } satisfies ProjectListResponse)
    }
    // BoardView loads Lanes and runs beside Tickets (KAN-T26); an
    // unanswered lane.list or run.list leaves the store's list
    // undefined and card render throws before the ordered columns can
    // appear. The Specs answer the same way (KAN-T126): the board
    // carries them for the Spec identities its cards render.
    if (name === 'lane.list') {
      return Promise.resolve({ lanes: [] } satisfies LaneListResponse)
    }
    if (name === 'run.list') {
      return Promise.resolve({ project_id: 1, runs: [] })
    }
    if (name === 'spec.list') {
      return Promise.resolve({ specs: [] } satisfies SpecListResponse)
    }
    if (name === 'ticket.readiness') {
      const { ticket_id } = request as { ticket_id: number }
      return Promise.resolve({
        blocked_by: [],
        ready: true,
        state: 'ready',
        ticket_id,
      } satisfies TicketReadinessResponse)
    }
    return Promise.resolve({
      tickets: ticketsForLoad(),
    } satisfies TicketListResponse)
  })
  const command = vi.fn((name: string, request: unknown) => {
    if (name === 'view.update') {
      const body = request as { view_id: number } & Record<string, unknown>
      const standing = viewList('priority').views.find((view) => view.id === body.view_id)
      return Promise.resolve({ ...standing, ...body, version: 2 })
    }
    return Promise.resolve({})
  })
  const transport = {
    query,
    command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return transport
}

function backlogCards(): TicketRecord[] {
  return [
    ticket({ id: 1, number: 1, state: 'parked' }),
    ticket({ id: 2, number: 5, state: 'ready' }),
    ticket({ id: 3, number: 3, state: 'blocked' }),
    ticket({ id: 4, number: 4, state: 'scheduled', priority: 'high' }),
    ticket({ id: 5, number: 2, priority: 'urgent', state: 'parked' }),
  ]
}

async function mountedBoard(
  tickets: TicketRecord[],
  sorting: 'priority' | 'readiness' = 'priority',
) {
  document.documentElement.classList.remove('dark')
  localStorage.clear()
  const transport = harness(() => tickets, sorting)
  router.push('/projects/1/board')
  await router.isReady()
  const wrapper = mount(BoardView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return wrapper
}

function columnCardIds(
  wrapper: Awaited<ReturnType<typeof mountedBoard>>,
  column: string,
): number[] {
  return wrapper
    .findAll(`[data-testid="kanban-column-${column}"] [data-testid^="kanban-card-"]`)
    .map((card) =>
      Number((card.attributes('data-testid') ?? '').replace('kanban-card-', '')),
    )
}

afterEach(() => {
  document.documentElement.classList.remove('dark')
  localStorage.clear()
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
    const second = await mountedBoard([...backlogCards().reverse()])

    expect(columnCardIds(second, 'backlog')).toEqual(columnCardIds(first, 'backlog'))
    expect(columnCardIds(first, 'backlog')).toEqual([5, 4, 2, 3, 1])
  })

  it('renders under the sorting key the active view owns', async () => {
    const wrapper = await mountedBoard([...backlogCards()], 'readiness')

    // Readiness leads: ready, scheduled, blocked, then the parked
    // pair by priority beneath it.
    expect(columnCardIds(wrapper, 'backlog')).toEqual([2, 4, 3, 5, 1])
  })
})
