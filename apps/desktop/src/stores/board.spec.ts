import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type {
  TicketListResponse,
  TicketReadinessBlocker,
  TicketReadinessResponse,
  TicketRecord,
} from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
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

const readiness = (
  ticket_id: number,
  blocked_by: TicketReadinessResponse['blocked_by'] = [],
): TicketReadinessResponse => ({
  blocked_by,
  ready: blocked_by.length === 0,
  state: 'ready',
  ticket_id,
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

// Answers the queries a board load spends: the Ticket list plus the
// readiness projection of every Ticket in it.
function serving(tickets: TicketRecord[], blockers: Record<number, TicketReadinessResponse['blocked_by']> = {}) {
  return (name: string, request: unknown) => {
    if (name === 'ticket.readiness') {
      const { ticket_id } = request as { ticket_id: number }
      return Promise.resolve(readiness(ticket_id, blockers[ticket_id] ?? []))
    }
    return Promise.resolve({ tickets } satisfies TicketListResponse)
  }
}

// The readiness queries a refresh spent, as bare requests.
function readinessRequests(
  operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }>,
): unknown[] {
  return operations
    .filter((entry) => entry.name === 'ticket.readiness')
    .map((entry) => entry.request)
}

// An answer the test settles by hand: the load or command still on
// the wire when another Project takes the board.
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
  it('loads the Project\'s Tickets through the generated client', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    const tickets = [task(), task({ id: 8, state: 'cancelled' })]
    query.mockImplementation(serving(tickets))
    const board = useBoardStore()

    await board.refresh(transport, 1)

    expect(query).toHaveBeenCalledWith('ticket.list', { project_id: 1 })
    expect(board.tickets).toEqual(tickets)
    expect(board.loaded).toBe(true)
    expect(board.error).toBeNull()
  })

  it('collects the readiness projection beside the Tickets it loads', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    const waiting: TicketReadinessBlocker = {
      Ticket: {
        from_number: 3,
        from_project_id: 1,
        from_state: 'active',
        from_ticket_id: 3,
      },
    }
    query.mockImplementation(serving([task(), task({ id: 8 })], { 7: [waiting] }))
    const board = useBoardStore()

    await board.refresh(transport, 1)

    expect(query).toHaveBeenCalledWith('ticket.readiness', { ticket_id: 7 })
    expect(query).toHaveBeenCalledWith('ticket.readiness', { ticket_id: 8 })
    expect(board.blockersFor(7)).toEqual([waiting])
    expect(board.blockersFor(8)).toEqual([])
    // A Ticket the board never loaded holds nothing back.
    expect(board.blockersFor(99)).toEqual([])
  })

  it('asks readiness only for the cards that can still be held back', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query } = harness()
    const history = [
      task({ id: 21, state: 'done' }),
      task({ id: 22, state: 'cancelled' }),
      task({ id: 23, state: 'superseded' }),
    ]
    const live = [task(), task({ id: 8, state: 'blocked' })]
    query.mockImplementation(serving([...history, ...live]))
    const board = useBoardStore()

    await board.refresh(transport, 1)

    // Finished history needs no projection; the live cards,
    // whatever holds them back, are each asked once.
    expect(readinessRequests(operations)).toEqual([
      { ticket_id: 7 },
      { ticket_id: 8 },
    ])
  })

  it('spends no more readiness calls as history grows', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query } = harness()
    const live = [task(), task({ id: 8, state: 'blocked' })]
    const historyOf = (count: number): TicketRecord[] =>
      Array.from({ length: count }, (_, index) =>
        task({ id: 100 + index, state: 'done' }),
      )
    const board = useBoardStore()

    query.mockImplementation(serving([...live, ...historyOf(3)]))
    await board.refresh(transport, 1)
    const againstThree = readinessRequests(operations)

    operations.length = 0
    query.mockImplementation(serving([...live, ...historyOf(90)]))
    await board.refresh(transport, 1)
    const againstNinety = readinessRequests(operations)

    expect(againstNinety).toEqual(againstThree)
  })

  it('clears blocker entries a refresh leaves behind', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    const waiting: TicketReadinessBlocker = {
      Ticket: {
        from_number: 3,
        from_project_id: 1,
        from_state: 'active',
        from_ticket_id: 3,
      },
    }
    query.mockImplementation(serving([task(), task({ id: 8 })], { 7: [waiting], 8: [waiting] }))
    const board = useBoardStore()
    await board.refresh(transport, 1)
    expect(board.blockersFor(7)).toEqual([waiting])

    // Ticket 7 lands done and Ticket 8 leaves the Project: neither
    // keeps the blockers that spoke for it.
    query.mockImplementation(serving([task({ state: 'done' })]))
    await board.refresh(transport, 1)

    expect(board.blockers).toEqual({})
  })

  it('drops the readiness of a Ticket a move finishes', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    const waiting: TicketReadinessBlocker = {
      Ticket: {
        from_number: 3,
        from_project_id: 1,
        from_state: 'active',
        from_ticket_id: 3,
      },
    }
    query.mockImplementation(serving([task()], { 7: [waiting] }))
    const board = useBoardStore()
    await board.refresh(transport, 1)
    operations.length = 0
    command.mockResolvedValue(task({ state: 'done', version: 4 }))

    await board.move(transport, 7, 'done')

    expect(readinessRequests(operations)).toEqual([])
    expect(board.blockers).toEqual({})
  })

  it('reports a failed load without pretending to be loaded', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockRejectedValue({ code: 'unavailable', message: 'the core is offline' })
    const board = useBoardStore()

    await board.refresh(transport, 1)

    expect(board.error).toBe('the core is offline')
    expect(board.loaded).toBe(false)
    expect(board.tickets).toEqual([])
  })

  it('moves a Ticket against its current version and keeps the record the core returns', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockImplementation(serving([task()]))
    const board = useBoardStore()
    await board.refresh(transport, 1)
    command.mockResolvedValue(task({ state: 'active', version: 4 }))

    const landed = await board.move(transport, 7, 'active')

    expect(landed).toBe(true)
    const sent = operations.find((entry) => entry.name === 'ticket.transition')
    expect(sent?.kind).toBe('command')
    expect(sent?.request).toEqual({
      mutation: {
        optimistic_version: 3,
        idempotency_key: expect.stringMatching(/[\w-]{8,}/),
      },
      ticket_id: 7,
      to: 'active',
    })
    expect(board.tickets[0]).toMatchObject({ id: 7, state: 'active', version: 4 })
    expect(board.error).toBeNull()
  })

  it('refreshes the readiness of the Ticket a move landed', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockImplementation(serving([task()]))
    const board = useBoardStore()
    await board.refresh(transport, 1)
    query.mockClear()
    command.mockResolvedValue(task({ state: 'active', version: 4 }))

    await board.move(transport, 7, 'active')

    expect(query).toHaveBeenCalledWith('ticket.readiness', { ticket_id: 7 })
  })

  it('reports a drag the core refuses and keeps the Ticket as it stands', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    const refused = task({ kind: 'bug', state: 'ready' })
    query.mockImplementation(serving([refused]))
    const board = useBoardStore()
    await board.refresh(transport, 1)
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'bug transitions are agent-owned; a human may drag only Task Tickets',
    })

    const landed = await board.move(transport, 7, 'active')

    expect(landed).toBe(false)
    expect(board.error).toBe(
      'bug transitions are agent-owned; a human may drag only Task Tickets',
    )
    expect(board.tickets[0]).toMatchObject({ kind: 'bug', state: 'ready', version: 3 })
  })

  it('refuses to move a Ticket it does not hold', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockImplementation(serving([]))
    const board = useBoardStore()
    await board.refresh(transport, 1)

    const landed = await board.move(transport, 99, 'active')

    expect(landed).toBe(false)
    expect(command).not.toHaveBeenCalled()
    expect(board.error).toBe('the board does not hold Ticket 99')
  })

  it('empties the board the moment another Project\'s load begins', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation(serving([task()]))
    const board = useBoardStore()
    await board.refresh(transport, 1)
    expect(board.tickets).toHaveLength(1)

    const second = deferred<TicketListResponse>()
    query.mockImplementation(() => second.promise)
    void board.refresh(transport, 2)

    // The previous Project's cards never wait for the next load to
    // settle before leaving (KAN-T125-AC1).
    expect(board.projectId).toBe(2)
    expect(board.tickets).toEqual([])
    expect(board.blockers).toEqual({})
    expect(board.loaded).toBe(false)
    second.settle({ tickets: [] })
  })

  it('keeps no other Project\'s cards when a load fails', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation(serving([task()]))
    const board = useBoardStore()
    await board.refresh(transport, 1)

    query.mockRejectedValue({ code: 'unavailable', message: 'the core is offline' })
    await board.refresh(transport, 2)

    expect(board.error).toBe('the core is offline')
    expect(board.tickets).toEqual([])
    expect(board.blockers).toEqual({})
    expect(board.loaded).toBe(false)
  })

  it('rejects a slower response for the Project the board has left', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    const slow = deferred<TicketListResponse>()
    query.mockImplementation((name: string, request: unknown) => {
      const { project_id } = request as { project_id: number }
      if (name === 'ticket.list' && project_id === 1) return slow.promise
      return serving([task({ id: 21, project_id: 2, number: 13 })])(name, request)
    })
    const board = useBoardStore()
    const loadingOne = board.refresh(transport, 1)

    await board.refresh(transport, 2)
    slow.settle({ tickets: [task()] })
    await loadingOne

    expect(board.projectId).toBe(2)
    expect(board.tickets.map((ticket) => ticket.project_id)).toEqual([2])
  })

  it('rejects a slower failure for the Project the board has left', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    const slow = deferred<TicketListResponse>()
    query.mockImplementation((name: string, request: unknown) => {
      const { project_id } = request as { project_id: number }
      if (name === 'ticket.list' && project_id === 1) return slow.promise
      return serving([task({ id: 21, project_id: 2, number: 13 })])(name, request)
    })
    const board = useBoardStore()
    const loadingOne = board.refresh(transport, 1)

    await board.refresh(transport, 2)
    slow.fail({ code: 'unavailable', message: 'the core is offline' })
    await loadingOne

    expect(board.error).toBeNull()
    expect(board.tickets.map((ticket) => ticket.project_id)).toEqual([2])
  })

  it('forgets the board, and a load superseded by that writes nothing', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation(serving([task()]))
    const board = useBoardStore()
    await board.refresh(transport, 1)

    const slow = deferred<TicketListResponse>()
    query.mockImplementation(() => slow.promise)
    const loading = board.refresh(transport, 2)
    board.clear()
    slow.settle({ tickets: [task({ id: 30, project_id: 3 })] })
    await loading

    expect(board.projectId).toBeNull()
    expect(board.tickets).toEqual([])
    expect(board.blockers).toEqual({})
    expect(board.loaded).toBe(false)
    expect(board.error).toBeNull()
  })

  it('refuses a drag for a Ticket of a Project the board has left', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockImplementation(serving([task()]))
    const board = useBoardStore()
    await board.refresh(transport, 1)

    const slow = deferred<TicketListResponse>()
    query.mockImplementation(() => slow.promise)
    void board.refresh(transport, 2)

    const landed = await board.move(transport, 7, 'done')

    // A card captured under one Project's heading never mutates
    // through another Project's board (KAN-T125-AC3).
    expect(landed).toBe(false)
    expect(command).not.toHaveBeenCalled()
    slow.settle({ tickets: [] })
  })

  it('renders nothing from a move that lands after the board has left', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockImplementation(serving([task()]))
    const board = useBoardStore()
    await board.refresh(transport, 1)

    const answered = deferred<TicketRecord>()
    command.mockImplementation(() => answered.promise)
    const moving = board.move(transport, 7, 'done')
    const slow = deferred<TicketListResponse>()
    query.mockImplementation(() => slow.promise)
    void board.refresh(transport, 2)
    answered.settle(task({ state: 'done', version: 4 }))
    const landed = await moving

    // The core may have accepted the move; the board it left renders
    // no trace of it and reports no refusal it cannot own.
    expect(landed).toBe(false)
    expect(board.projectId).toBe(2)
    expect(board.tickets).toEqual([])
    expect(board.error).toBeNull()
    slow.settle({ tickets: [] })
  })
})
