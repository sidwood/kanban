import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { TicketRecord } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useLifecycleActionsStore } from './lifecycle-actions'

const openTask = {
  id: 2,
  project_id: 1,
  number: 3,
  kind: 'task' as const,
  priority: 'normal' as const,
  state: 'draft' as const,
  spec_id: null,
  title: 'Archive the old register',
  slice: null,
  criteria: [],
  bug: null,
  subtype: 'administrative' as const,
  mode: 'human' as const,
  completion: ['The old register is archived.'],
  scheduled_for: null,
  due: null,
  profile: null,
  version: 4,
} satisfies TicketRecord

// The record one successful command returns: the Ticket moved and the
// version counted the change.
const moved = (state: string, version: number): TicketRecord => ({
  ...openTask,
  state: state as TicketRecord['state'],
  version,
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

// The mutation context every command must carry against the open
// Ticket's version.
const mutationOf = (version: number) => ({
  optimistic_version: version,
  idempotency_key: expect.stringMatching(/[\w-]{8,}/),
})

describe('lifecycle actions store', () => {
  it('open loads the Ticket through the generated client', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockResolvedValue(openTask satisfies TicketRecord)
    const actions = useLifecycleActionsStore()

    await actions.open(transport, 2)

    expect(actions.ticket).toEqual(openTask)
    expect(actions.error).toBeNull()
    expect(query).toHaveBeenCalledWith('ticket.get', { ticket_id: 2 })
  })

  it('a drag sends the target against the open version', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockResolvedValue(openTask satisfies TicketRecord)
    command.mockResolvedValue(moved('ready', 5))
    const actions = useLifecycleActionsStore()
    await actions.open(transport, 2)

    const landed = await actions.transition(transport, 'ready')

    expect(landed).toBe(true)
    const dragged = operations.find((entry) => entry.name === 'ticket.transition')
    expect(dragged?.kind).toBe('command')
    expect(dragged?.request).toEqual({
      mutation: mutationOf(4),
      ticket_id: 2,
      to: 'ready',
    })
    expect(actions.ticket?.state).toBe('ready')
    expect(actions.ticket?.version).toBe(5)
  })

  it('a drag refused as agent-owned reports the explanation and keeps the Ticket', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockResolvedValue(openTask satisfies TicketRecord)
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'bug transitions are agent-owned; a human may drag only Task Tickets',
    })
    const actions = useLifecycleActionsStore()
    await actions.open(transport, 2)

    const landed = await actions.transition(transport, 'ready')

    expect(landed).toBe(false)
    expect(actions.error).toBe(
      'bug transitions are agent-owned; a human may drag only Task Tickets',
    )
    expect(actions.ticket).toEqual(openTask)
  })

  it('the named commands send their Ticket against the open version', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockResolvedValue(openTask satisfies TicketRecord)
    // Each command returns the Ticket unchanged, so every assertion
    // sees the same open version; the drag test above proves the
    // returned record replaces the open one.
    command.mockResolvedValue(openTask satisfies TicketRecord)
    const actions = useLifecycleActionsStore()
    await actions.open(transport, 2)

    await actions.park(transport)
    await actions.unpark(transport)
    await actions.schedule(transport)
    await actions.cancel(transport)

    for (const name of ['ticket.park', 'ticket.unpark', 'ticket.schedule', 'ticket.cancel']) {
      const sent = operations.find((entry) => entry.name === name)
      expect(sent?.kind).toBe('command')
      expect(sent?.request).toEqual({
        mutation: mutationOf(4),
        ticket_id: 2,
      })
    }
  })

  it('a review decision, a priority, and an edit send their payloads', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockResolvedValue(openTask satisfies TicketRecord)
    command.mockResolvedValue(openTask satisfies TicketRecord)
    const actions = useLifecycleActionsStore()
    await actions.open(transport, 2)

    await actions.review(transport, 'approve')
    await actions.prioritise(transport, 'urgent')
    await actions.edit(transport, { title: 'Archive the newer register' })

    expect(operations.find((entry) => entry.name === 'ticket.review')?.request).toEqual({
      mutation: mutationOf(4),
      ticket_id: 2,
      decision: 'approve',
    })
    expect(operations.find((entry) => entry.name === 'ticket.prioritise')?.request).toEqual({
      mutation: mutationOf(4),
      ticket_id: 2,
      priority: 'urgent',
    })
    expect(operations.find((entry) => entry.name === 'ticket.edit')?.request).toEqual({
      mutation: mutationOf(4),
      ticket_id: 2,
      title: 'Archive the newer register',
    })
  })

  it('the override carries who, what, and why', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockResolvedValue(openTask satisfies TicketRecord)
    command.mockResolvedValue(moved('ready', 5))
    const actions = useLifecycleActionsStore()
    await actions.open(transport, 2)

    const landed = await actions.override(
      transport,
      'ready',
      'Sid Wood',
      'Recovery after the core crashed mid move',
    )

    expect(landed).toBe(true)
    expect(
      operations.find((entry) => entry.name === 'ticket.emergency.override')?.request,
    ).toEqual({
      mutation: mutationOf(4),
      ticket_id: 2,
      to: 'ready',
      who: 'Sid Wood',
      why: 'Recovery after the core crashed mid move',
    })
  })

  it('a blank override is refused by the core and reported', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockResolvedValue(openTask satisfies TicketRecord)
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'an emergency override reason cannot be blank',
    })
    const actions = useLifecycleActionsStore()
    await actions.open(transport, 2)

    const landed = await actions.override(transport, 'ready', 'Sid Wood', '  ')

    expect(landed).toBe(false)
    expect(actions.error).toBe('an emergency override reason cannot be blank')
    expect(actions.ticket).toEqual(openTask)
  })

  it('no action runs before a Ticket is opened', async () => {
    setActivePinia(createPinia())
    const { transport } = harness()
    const actions = useLifecycleActionsStore()

    const landed = await actions.park(transport)

    expect(landed).toBe(false)
    expect(actions.error).toBe('open a Ticket before acting on it')
  })
})
