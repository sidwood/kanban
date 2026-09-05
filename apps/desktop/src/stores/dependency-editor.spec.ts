import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type {
  TicketDependenciesResponse,
  TicketListResponse,
  TicketReadinessResponse,
} from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useDependencyEditorStore } from './dependency-editor'

const coreTicket = {
  id: 1,
  project_id: 1,
  number: 1,
  kind: 'bug' as const,
  priority: 'normal' as const,
  state: 'active' as const,
  spec_id: null,
  title: 'Landing drops the integration branch',
  slice: null,
  criteria: [],
  version: 4,
}

const edgeTicket = {
  ...coreTicket,
  id: 2,
  project_id: 2,
  number: 1,
  state: 'draft' as const,
  title: 'Archive the old register',
  version: 1,
}

const dependencies: TicketDependenciesResponse = {
  ticket_id: 2,
  version: 5,
  dependencies: [
    {
      from_ticket_id: 1,
      from_project_id: 1,
      from_number: 1,
      from_state: 'active',
    },
  ],
  blockers: [{ id: 3, ticket_id: 2, description: 'The vendor SDK 4 upgrade' }],
}

const readiness: TicketReadinessResponse = {
  ticket_id: 2,
  state: 'draft',
  ready: false,
  blocked_by: [
    {
      Ticket: {
        from_ticket_id: 1,
        from_project_id: 1,
        from_number: 1,
        from_state: 'active',
      },
    },
    { External: { blocker_id: 3, description: 'The vendor SDK 4 upgrade' } },
  ],
}

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

describe('dependency editor store', () => {
  it('refresh loads every ticket of the project through the generated client', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation((_name: string, request: unknown) => {
      const asked = request as { project_id: number }
      return Promise.resolve({
        tickets: [edgeTicket, coreTicket].filter((ticket) => ticket.project_id === asked.project_id),
      } satisfies TicketListResponse)
    })
    const editor = useDependencyEditorStore()

    await editor.refresh(transport, 2)
    await editor.loadSource(transport, 1)

    expect(editor.tickets.map((ticket) => ticket.id)).toEqual([2])
    expect(editor.sourceTickets.map((ticket) => ticket.id)).toEqual([1])
    expect(editor.error).toBeNull()
  })

  it('open loads the dependencies and the computed readiness together', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation((name: string) => {
      if (name === 'ticket.dependencies') {
        return Promise.resolve(dependencies)
      }
      return Promise.resolve(readiness)
    })
    const editor = useDependencyEditorStore()

    await editor.open(transport, 2)

    expect(editor.dependencies).toEqual(dependencies)
    expect(editor.readiness?.ready).toBe(false)
    expect(editor.readiness?.blocked_by).toHaveLength(2)
  })

  it('adding a dependency guards on the open version and sends both endpoints', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockResolvedValue(dependencies satisfies TicketDependenciesResponse)
    query.mockImplementation((name: string) =>
      name === 'ticket.dependencies'
        ? Promise.resolve(dependencies)
        : Promise.resolve(readiness),
    )
    command.mockResolvedValue(dependencies satisfies TicketDependenciesResponse)
    const editor = useDependencyEditorStore()
    await editor.open(transport, 2)

    const landed = await editor.addDependency(transport, 2, 1)

    expect(landed).toBe(true)
    const added = operations.find((entry) => entry.name === 'ticket.dependency.add')
    expect(added?.kind).toBe('command')
    expect(added?.request).toEqual({
      mutation: { optimistic_version: 5, idempotency_key: expect.stringMatching(/[\w-]{8,}/) },
      from_ticket: 1,
      to_ticket: 2,
    })
  })

  it('adding a blocker sends the description against the open version', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockImplementation((name: string) =>
      name === 'ticket.dependencies'
        ? Promise.resolve(dependencies)
        : Promise.resolve(readiness),
    )
    command.mockResolvedValue(dependencies satisfies TicketDependenciesResponse)
    const editor = useDependencyEditorStore()
    await editor.open(transport, 2)

    await editor.addBlocker(transport, 2, 'Design sign-off')

    const added = operations.find((entry) => entry.name === 'ticket.blocker.add')
    expect(added?.request).toEqual({
      mutation: expect.any(Object),
      ticket_id: 2,
      description: 'Design sign-off',
    })
  })

  it('removing a dependency and a blocker names their identities', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockImplementation((name: string) =>
      name === 'ticket.dependencies'
        ? Promise.resolve(dependencies)
        : Promise.resolve(readiness),
    )
    command.mockResolvedValue(dependencies satisfies TicketDependenciesResponse)
    const editor = useDependencyEditorStore()
    await editor.open(transport, 2)

    await editor.removeDependency(transport, 2, 1)
    await editor.removeBlocker(transport, 2, 3)

    expect(operations.find((entry) => entry.name === 'ticket.dependency.remove')?.request)
      .toMatchObject({ from_ticket: 1, to_ticket: 2 })
    expect(operations.find((entry) => entry.name === 'ticket.blocker.remove')?.request).toMatchObject(
      { ticket_id: 2, blocker_id: 3 },
    )
  })

  it('a refused command reports the message and keeps the open Ticket', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockImplementation((name: string) =>
      name === 'ticket.dependencies'
        ? Promise.resolve(dependencies)
        : Promise.resolve(readiness),
    )
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'the dependency from Ticket 2 to Ticket 1 would close a cycle',
    })
    const editor = useDependencyEditorStore()
    await editor.open(transport, 2)

    const landed = await editor.addDependency(transport, 2, 1)

    expect(landed).toBe(false)
    expect(editor.error).toBe('the dependency from Ticket 2 to Ticket 1 would close a cycle')
    expect(editor.dependencies).toEqual(dependencies)
  })
})
