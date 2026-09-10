import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type {
  BoardFilter,
  BoardFilterOptions,
  BoardGlobalCard,
  BoardGlobalResponse,
  CriterionBindingRecord,
  RunRecord,
  TicketReadinessResponse,
  TicketRecord,
} from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { emptyFilter } from '../views/global-board-filters'
import { useBoardStore } from './board'

const task = (overrides: Partial<TicketRecord> = {}): TicketRecord => ({
  id: 7,
  project_id: 1,
  number: 12,
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
  version: 3,
  ...overrides,
})

const GROUP_OF: Record<TicketRecord['state'], BoardGlobalCard['group'] | null> = {
  draft: 'draft',
  parked: 'backlog',
  blocked: 'backlog',
  scheduled: 'backlog',
  ready: 'backlog',
  active: 'current',
  in_review: 'review',
  approved: 'staged',
  landing: 'staged',
  done: 'done',
  cancelled: null,
  superseded: null,
}

// The projection the core returns for a set of Tickets: grouped by
// the fixed mapping, terminal states left out, in the order given.
function projection(
  tickets: readonly TicketRecord[],
  extras: Partial<Record<number, Partial<BoardGlobalCard>>> = {},
): BoardGlobalCard[] {
  return tickets.flatMap((ticket) => {
    const group = GROUP_OF[ticket.state]
    if (group === null) return []
    return [
      {
        group,
        project_code: ticket.project_id === 2 ? 'EDGE' : 'CORE',
        spec_number: null,
        lane_id: null,
        ticket,
        ...extras[ticket.id],
      },
    ]
  })
}

const options: BoardFilterOptions = {
  initiatives: [],
  projects: [
    { id: 1, label: 'CORE — Control plane' },
    { id: 2, label: 'EDGE — Edge tooling' },
  ],
  plans: [],
  specs: [],
  lanes: [],
  profiles: ['standard'],
  attention: ['blocker'],
}

const readiness = (
  ticket_id: number,
  blocked_by: TicketReadinessResponse['blocked_by'] = [],
): TicketReadinessResponse => ({
  blocked_by,
  ready: blocked_by.length === 0,
  state: 'ready',
  ticket_id,
})

const run = (overrides: Partial<RunRecord> = {}): RunRecord => ({
  id: 1,
  project_id: 1,
  ticket_id: 7,
  dispatch_request_id: 1,
  status: 'executing',
  requested: { name: 'deep', harness: 'cli', model: 'opus', effort: 'high', usage_pool: 'a' },
  effective: { name: 'standard', harness: 'cli', model: 'sonnet', effort: 'high', usage_pool: 'a' },
  fallback: true,
  fallback_path: ['deep', 'standard'],
  created_at: 10,
  version: 1,
  ...overrides,
})

// A recording transport: every operation is captured, and the query
// and command answers are steerable from the test.
function harness() {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  const query = vi.fn()
  const command = vi.fn()
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      return query(name, request)
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      return command(name, request)
    },
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, operations, query, command }
}

// Answers the queries a board load spends: the projection, the
// readiness of every card that can still be held back, the criterion
// bindings its progress is counted from, where the core says each
// Task may move, and the runs of every Project on the board.
function serving(
  tickets: readonly TicketRecord[],
  blockers: Record<number, TicketReadinessResponse['blocked_by']> = {},
  runs: readonly RunRecord[] = [],
  extras: Partial<Record<number, Partial<BoardGlobalCard>>> = {},
  bindings: Record<number, CriterionBindingRecord[]> = {},
  targets: Record<number, TicketRecord['state'][]> = {},
) {
  return (name: string, request: unknown) => {
    if (name === 'board.global') {
      const { filter } = request as { filter: BoardFilter }
      const selected = tickets.filter((ticket) => {
        if (filter.projects?.length && !filter.projects.includes(ticket.project_id)) return false
        if (filter.kinds?.length && !filter.kinds.includes(ticket.kind)) return false
        return true
      })
      return Promise.resolve({
        cards: projection(selected, extras),
        options,
      } satisfies BoardGlobalResponse)
    }
    if (name === 'ticket.readiness') {
      const { ticket_id } = request as { ticket_id: number }
      return Promise.resolve(readiness(ticket_id, blockers[ticket_id] ?? []))
    }
    if (name === 'run.list') {
      const { project_id } = request as { project_id: number }
      return Promise.resolve({
        project_id,
        runs: runs.filter((entry) => entry.project_id === project_id),
      })
    }
    if (name === 'ticket.review.config') {
      return Promise.resolve({ config: null })
    }
    if (name === 'criterion.bindings') {
      const { ticket_id } = request as { ticket_id: number }
      return Promise.resolve({ bindings: bindings[ticket_id] ?? [] })
    }
    if (name === 'ticket.transitions') {
      const { ticket_id } = request as { ticket_id: number }
      const found = tickets.find((entry) => entry.id === ticket_id)
      return Promise.resolve({
        ticket_id,
        state: found?.state ?? 'ready',
        targets: targets[ticket_id] ?? [],
      })
    }
    throw new Error(`unexpected query ${name}`)
  }
}

function requestsOf(
  operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }>,
  name: string,
): unknown[] {
  return operations.filter((entry) => entry.name === name).map((entry) => entry.request)
}

// An answer the test settles by hand: the load or command still on
// the wire when another scope takes the board.
function deferred<T>() {
  let settle!: (value: T) => void
  let fail!: (reason: unknown) => void
  const promise = new Promise<T>((resolve, reject) => {
    settle = resolve
    fail = reject
  })
  return { promise, settle, fail }
}

describe('board store', () => {
  it('loads one Project\'s projection through board.global with the Project pinned', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    const tickets = [task(), task({ id: 8, state: 'cancelled' })]
    query.mockImplementation(serving(tickets))
    const board = useBoardStore()

    await board.refresh(transport, 1, emptyFilter())

    expect(query).toHaveBeenCalledWith('board.global', { filter: { projects: [1] } })
    expect(board.cards.map((card) => card.ticket.id)).toEqual([7])
    expect(board.options).toEqual(options)
    expect(board.scope).toBe(1)
    expect(board.loaded).toBe(true)
    expect(board.error).toBeNull()
  })

  it('loads every Project\'s projection under the operator\'s filter', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation(serving([task(), task({ id: 9, project_id: 2, kind: 'bug' })]))
    const board = useBoardStore()

    await board.refresh(transport, 'all', { ...emptyFilter(), kinds: ['bug'] })

    expect(query).toHaveBeenCalledWith('board.global', { filter: { kinds: ['bug'] } })
    expect(board.cards.map((card) => card.ticket.id)).toEqual([9])
  })

  it('reads the scope\'s whole board beside every filtered one', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    const tickets = [task(), task({ id: 9, project_id: 2, kind: 'bug' })]
    query.mockImplementation(serving(tickets))
    const board = useBoardStore()

    await board.refresh(transport, 'all', { ...emptyFilter(), kinds: ['bug'] })
    // The whole scope is read beside the filter so the toolbar can
    // say what the filter hides.
    expect(query).toHaveBeenCalledWith('board.global', { filter: {} })
    expect(board.scopeTotal).toBe(2)
    expect(board.cards).toHaveLength(1)

    // A Ticket arriving while the filter stands shows in the total.
    tickets.push(task({ id: 10, number: 20 }))
    query.mockClear()
    await board.refresh(transport, 'all', { ...emptyFilter(), kinds: ['bug'] })
    expect(query.mock.calls.filter(([name]) => name === 'board.global')).toHaveLength(2)
    expect(board.scopeTotal).toBe(3)

    // No filter: the one query is the whole scope.
    query.mockClear()
    await board.refresh(transport, 'all', emptyFilter())
    expect(query.mock.calls.filter(([name]) => name === 'board.global')).toHaveLength(1)
    expect(board.scopeTotal).toBe(3)
  })

  it('reads the reviewers configured on every Implementation, and none elsewhere', async () => {
    setActivePinia(createPinia())
    const { transport, query, operations } = harness()
    const base = serving([
      task(),
      task({ id: 8, number: 13, kind: 'implementation', slice: 'Serve', title: null }),
      task({ id: 9, number: 14, kind: 'implementation', slice: 'Carry', title: null }),
    ])
    query.mockImplementation((name: string, request: unknown) => {
      if (name === 'ticket.review.config') {
        const { ticket_id } = request as { ticket_id: number }
        return Promise.resolve({
          config:
            ticket_id === 8
              ? {
                  ticket_id,
                  version: 1,
                  stages: [
                    {
                      slots: [
                        { occupant: { kind: 'profile', name: 'review-strict' }, requirement: 'required' },
                        { occupant: { kind: 'human' }, requirement: 'required' },
                      ],
                    },
                    { slots: [{ occupant: { kind: 'profile', name: 'deep' }, requirement: 'optional' }] },
                  ],
                }
              : null,
        })
      }
      return base(name, request)
    })
    const board = useBoardStore()

    await board.refresh(transport, 1, emptyFilter())

    expect(requestsOf(operations, 'ticket.review.config')).toEqual([{ ticket_id: 8 }, { ticket_id: 9 }])
    expect(board.reviewersFor(8)).toEqual(['review-strict', 'Human', 'deep'])
    expect(board.reviewersFor(9)).toEqual([])
    expect(board.reviewersFor(7)).toEqual([])
  })

  it('re-reads one Project\'s runs and keeps the others', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation(
      serving(
        [task({ state: 'active' }), task({ id: 9, project_id: 2, number: 3 })],
        {},
        [run(), run({ id: 2, project_id: 2, ticket_id: 9 })],
      ),
    )
    const board = useBoardStore()
    await board.refresh(transport, 'all', emptyFilter())

    query.mockImplementation(
      serving([], {}, [run({ id: 3, project_id: 2, ticket_id: 9, status: 'superseded' })]),
    )
    await board.refreshRuns(transport, 2)

    expect(board.attemptsFor(9).map((entry) => entry.id)).toEqual([3])
    expect(board.attemptsFor(7).map((entry) => entry.id)).toEqual([1])
  })

  it('collects the readiness projection beside the cards that can still be held back', async () => {
    setActivePinia(createPinia())
    const { transport, query, operations } = harness()
    const blocker = { External: { blocker_id: 1, description: 'Vendor API quota' } }
    query.mockImplementation(
      serving(
        [task(), task({ id: 8, state: 'done' }), task({ id: 9, state: 'blocked' })],
        { 9: [blocker] },
      ),
    )
    const board = useBoardStore()

    await board.refresh(transport, 1, emptyFilter())

    expect(requestsOf(operations, 'ticket.readiness')).toEqual([
      { ticket_id: 7 },
      { ticket_id: 9 },
    ])
    expect(board.blockersFor(9)).toEqual([blocker])
    expect(board.blockersFor(7)).toEqual([])
    expect(board.blockersFor(8)).toEqual([])
  })

  it('loads the runs of every Project on the board, once each', async () => {
    setActivePinia(createPinia())
    const { transport, query, operations } = harness()
    query.mockImplementation(
      serving(
        [task({ state: 'active' }), task({ id: 9, project_id: 2, number: 3 }), task({ id: 10, number: 4 })],
        {},
        [run(), run({ id: 2, project_id: 2, ticket_id: 9, status: 'superseded', created_at: 5 })],
      ),
    )
    const board = useBoardStore()

    await board.refresh(transport, 'all', emptyFilter())

    expect(requestsOf(operations, 'run.list')).toEqual([{ project_id: 1 }, { project_id: 2 }])
    expect(board.executionFor(7)).toEqual({ effective: 'standard', fallback: true })
    expect(board.executionFor(9)).toBeNull()
    expect(board.attemptsFor(9).map((entry) => entry.id)).toEqual([2])
  })

  it('clears blocker entries a refresh leaves behind', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    const blocker = { External: { blocker_id: 1, description: 'Vendor API quota' } }
    query.mockImplementation(serving([task({ state: 'blocked' })], { 7: [blocker] }))
    const board = useBoardStore()
    await board.refresh(transport, 1, emptyFilter())
    expect(board.blockersFor(7)).toEqual([blocker])

    query.mockImplementation(serving([task({ id: 11, number: 13 })]))
    await board.refresh(transport, 1, emptyFilter())

    expect(board.blockersFor(7)).toEqual([])
  })

  it('reports a failed load without pretending to be loaded', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation(() =>
      Promise.reject({ code: 'unavailable', message: 'the core is offline' }),
    )
    const board = useBoardStore()

    await board.refresh(transport, 1, emptyFilter())

    expect(board.error).toBe('the core is offline')
    expect(board.loaded).toBe(false)
    expect(board.cards).toEqual([])
  })

  it('moves a Ticket against its current version and keeps the record the core returns', async () => {
    setActivePinia(createPinia())
    const { transport, query, command, operations } = harness()
    query.mockImplementation(serving([task()]))
    command.mockImplementation((_name: string, request: unknown) => {
      const { ticket_id, to } = request as { ticket_id: number; to: TicketRecord['state'] }
      return Promise.resolve(task({ id: ticket_id, state: to, version: 4 }))
    })
    const board = useBoardStore()
    await board.refresh(transport, 1, emptyFilter())

    expect(await board.move(transport, 7, 'active')).toBe(true)

    expect(command).toHaveBeenCalledWith(
      'ticket.transition',
      expect.objectContaining({
        ticket_id: 7,
        to: 'active',
        mutation: expect.objectContaining({ optimistic_version: 3 }),
      }),
    )
    const moved = board.cardOf(7)
    expect(moved?.ticket.state).toBe('active')
    expect(moved?.ticket.version).toBe(4)
    // The card follows its state into the group the mapping fixes.
    expect(moved?.group).toBe('current')
    // The move may have changed what holds the Ticket back.
    expect(requestsOf(operations, 'ticket.readiness').at(-1)).toEqual({ ticket_id: 7 })
    expect(board.error).toBeNull()
  })

  it('drops the readiness of a Ticket a move finishes', async () => {
    setActivePinia(createPinia())
    const { transport, query, command, operations } = harness()
    const blocker = { External: { blocker_id: 1, description: 'Vendor API quota' } }
    query.mockImplementation(serving([task({ state: 'landing' })], { 7: [blocker] }))
    command.mockImplementation(() => Promise.resolve(task({ state: 'done', version: 4 })))
    const board = useBoardStore()
    await board.refresh(transport, 1, emptyFilter())
    expect(board.blockersFor(7)).toEqual([blocker])
    const before = requestsOf(operations, 'ticket.readiness').length

    await board.move(transport, 7, 'done')

    expect(board.blockersFor(7)).toEqual([])
    expect(requestsOf(operations, 'ticket.readiness')).toHaveLength(before)
  })

  it('reports a drag the core refuses and keeps the Ticket as it stands', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockImplementation(serving([task({ kind: 'bug' })]))
    command.mockImplementation(() =>
      Promise.reject({
        code: 'invalid_request',
        message: 'bug transitions are agent-owned; a human may drag only Task Tickets',
      }),
    )
    const board = useBoardStore()
    await board.refresh(transport, 1, emptyFilter())

    expect(await board.move(transport, 7, 'active')).toBe(false)

    expect(board.error).toBe('bug transitions are agent-owned; a human may drag only Task Tickets')
    expect(board.cardOf(7)?.ticket.state).toBe('ready')
    expect(board.cardOf(7)?.ticket.version).toBe(3)
  })

  it('refuses to move a Ticket it does not hold', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockImplementation(serving([task()]))
    const board = useBoardStore()
    await board.refresh(transport, 1, emptyFilter())

    expect(await board.move(transport, 99, 'active')).toBe(false)

    expect(command).not.toHaveBeenCalled()
    expect(board.error).toContain('does not hold Ticket 99')
  })

  it('empties the board the moment another scope\'s load begins', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation(serving([task()]))
    const board = useBoardStore()
    await board.refresh(transport, 1, emptyFilter())
    expect(board.cards).toHaveLength(1)

    const slow = deferred<BoardGlobalResponse>()
    query.mockImplementation(() => slow.promise)
    const pending = board.refresh(transport, 2, emptyFilter())

    expect(board.cards).toEqual([])
    expect(board.loaded).toBe(false)
    expect(board.scope).toBe(2)

    slow.settle({ cards: projection([task({ id: 21, project_id: 2, number: 22 })]), options })
    await pending
    expect(board.cards.map((card) => card.ticket.id)).toEqual([21])
  })

  it('keeps the cards on show while the same scope re-queries under a new filter', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation(serving([task()]))
    const board = useBoardStore()
    await board.refresh(transport, 1, emptyFilter())

    const slow = deferred<BoardGlobalResponse>()
    query.mockImplementation(() => slow.promise)
    const pending = board.refresh(transport, 1, { ...emptyFilter(), kinds: ['bug'] })

    expect(board.cards).toHaveLength(1)
    expect(board.loaded).toBe(true)
    expect(board.loading).toBe(true)

    slow.settle({ cards: [], options })
    await pending
    expect(board.cards).toEqual([])
    expect(board.loading).toBe(false)
  })

  it('rejects a slower response for the scope the board has left', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    const slow = deferred<BoardGlobalResponse>()
    query.mockImplementation((name: string, request: unknown) => {
      if (name === 'board.global') {
        const { filter } = request as { filter: BoardFilter }
        if (filter.projects?.[0] === 2) return slow.promise
      }
      return serving([task()])(name, request)
    })
    const board = useBoardStore()

    const abandoned = board.refresh(transport, 2, emptyFilter())
    await board.refresh(transport, 1, emptyFilter())
    expect(board.cards.map((card) => card.ticket.id)).toEqual([7])

    slow.settle({ cards: projection([task({ id: 21, project_id: 2, number: 22 })]), options })
    await abandoned

    expect(board.cards.map((card) => card.ticket.id)).toEqual([7])
    expect(board.scope).toBe(1)
  })

  it('rejects a slower failure for the scope the board has left', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    const slow = deferred<BoardGlobalResponse>()
    query.mockImplementation((name: string, request: unknown) => {
      if (name === 'board.global') {
        const { filter } = request as { filter: BoardFilter }
        if (filter.projects?.[0] === 2) return slow.promise
      }
      return serving([task()])(name, request)
    })
    const board = useBoardStore()

    const abandoned = board.refresh(transport, 2, emptyFilter())
    await board.refresh(transport, 1, emptyFilter())

    slow.fail({ code: 'unavailable', message: 'the core is offline' })
    await abandoned

    expect(board.error).toBeNull()
    expect(board.cards.map((card) => card.ticket.id)).toEqual([7])
  })

  it('forgets the board, and a load superseded by that writes nothing', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    const slow = deferred<BoardGlobalResponse>()
    query.mockImplementation(() => slow.promise)
    const board = useBoardStore()

    const pending = board.refresh(transport, 1, emptyFilter())
    board.clear()
    slow.settle({ cards: projection([task()]), options })
    await pending

    expect(board.cards).toEqual([])
    expect(board.scope).toBeNull()
    expect(board.loaded).toBe(false)
  })

  it('renders nothing from a move that lands after the board has left', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockImplementation(serving([task()]))
    const slow = deferred<TicketRecord>()
    command.mockImplementation(() => slow.promise)
    const board = useBoardStore()
    await board.refresh(transport, 1, emptyFilter())

    const moving = board.move(transport, 7, 'active')
    query.mockImplementation(serving([task({ id: 21, project_id: 2, number: 22 })]))
    await board.refresh(transport, 2, emptyFilter())

    slow.settle(task({ state: 'active', version: 4 }))
    expect(await moving).toBe(false)

    expect(board.cards.map((card) => card.ticket.id)).toEqual([21])
    expect(board.error).toBeNull()
  })
  it('reads the criterion bindings and the legal moves beside the projection', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    const tickets = [task(), task({ id: 8, kind: 'implementation', state: 'active' })]
    query.mockImplementation(
      serving(
        tickets,
        {},
        [],
        {},
        {
          7: [
            {
              ticket_id: 7,
              criterion_index: 0,
              kind: 'task',
              evidence_id: 1,
              tip: 'a1b2c3',
              review: 'validated',
              satisfied: true,
              void: false,
            },
          ],
        },
        { 7: ['parked', 'active'] },
      ),
    )
    const board = useBoardStore()

    await board.refresh(transport, 1, emptyFilter())

    expect(board.bindingsFor(7).map((binding) => binding.criterion_index)).toEqual([0])
    expect(board.legalTargetsFor(7)).toEqual(['parked', 'active'])
    // Only a Task answers a human drag, so only a Task's moves are read.
    expect(
      query.mock.calls.filter(([name]) => name === 'ticket.transitions').map(([, request]) => request),
    ).toEqual([{ ticket_id: 7 }])
    expect(board.legalTargetsFor(8)).toEqual([])
  })

  it('forgets the bindings and legal moves of the scope it leaves', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation(serving([task()], {}, [], {}, {}, { 7: ['parked', 'active'] }))
    const board = useBoardStore()
    await board.refresh(transport, 1, emptyFilter())
    expect(board.legalTargetsFor(7)).toEqual(['parked', 'active'])

    board.clear()

    expect(board.legalTargetsFor(7)).toEqual([])
    expect(board.bindingsFor(7)).toEqual([])
  })
})
