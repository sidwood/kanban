import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { TicketListResponse, TicketRecord } from '@kanban/contracts'
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

describe('board store', () => {
  it('loads the Project\'s Tickets through the generated client', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    const tickets = [task(), task({ id: 8, state: 'cancelled' })]
    query.mockResolvedValue({ tickets } satisfies TicketListResponse)
    const board = useBoardStore()

    await board.refresh(transport, 1)

    expect(query).toHaveBeenCalledWith('ticket.list', { project_id: 1 })
    expect(board.tickets).toEqual(tickets)
    expect(board.loaded).toBe(true)
    expect(board.error).toBeNull()
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
    query.mockResolvedValue({ tickets: [task()] } satisfies TicketListResponse)
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

  it('reports a drag the core refuses and keeps the Ticket as it stands', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    const refused = task({ kind: 'bug', state: 'ready' })
    query.mockResolvedValue({ tickets: [refused] } satisfies TicketListResponse)
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
    query.mockResolvedValue({ tickets: [] } satisfies TicketListResponse)
    const board = useBoardStore()
    await board.refresh(transport, 1)

    const landed = await board.move(transport, 99, 'active')

    expect(landed).toBe(false)
    expect(command).not.toHaveBeenCalled()
    expect(board.error).toBe('the board does not hold Ticket 99')
  })
})
