import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { TicketRecord } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useReassignmentStore } from './reassignment'

const openTask = {
  id: 2,
  project_id: 1,
  number: 3,
  kind: 'task' as const,
  priority: 'normal' as const,
  state: 'active' as const,
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

// The replacement a successful reassignment returns: a fresh row,
// minted past the original's number, referencing its predecessor
// (DR-DE-07).
const replacement = {
  ...openTask,
  id: 9,
  number: 4,
  title: 'Replan the register archive',
  subtype: 'migration' as const,
  state: 'draft' as const,
  predecessor_id: 2,
  version: 1,
} satisfies TicketRecord

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

// The mutation context the reassignment must carry against the open
// Ticket's version.
const mutationOf = (version: number) => ({
  optimistic_version: version,
  idempotency_key: expect.stringMatching(/[\w-]{8,}/),
})

describe('reassignment store', () => {
  it('open loads the Ticket being replaced through the generated client', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockResolvedValue(openTask satisfies TicketRecord)
    const reassignment = useReassignmentStore()

    await reassignment.open(transport, 2)

    expect(reassignment.ticket).toEqual(openTask)
    expect(reassignment.error).toBeNull()
    expect(query).toHaveBeenCalledWith('ticket.get', { ticket_id: 2 })
  })

  it('reassignment states the replacement whole against the open version', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockResolvedValue(openTask satisfies TicketRecord)
    command.mockResolvedValue(replacement satisfies TicketRecord)
    const reassignment = useReassignmentStore()
    await reassignment.open(transport, 2)

    const landed = await reassignment.reassign(transport, {
      kind: 'task',
      priority: 'high',
      title: 'Replan the register archive',
      subtype: 'migration',
      mode: 'agent',
      completion: ['The register moves and restores.'],
    })

    expect(landed).toBe(true)
    const sent = operations.find((entry) => entry.name === 'ticket.reassign')
    expect(sent?.kind).toBe('command')
    expect(sent?.request).toEqual({
      mutation: mutationOf(4),
      ticket_id: 2,
      kind: 'task',
      priority: 'high',
      title: 'Replan the register archive',
      subtype: 'migration',
      mode: 'agent',
      completion: ['The register moves and restores.'],
    })
    // The open Ticket becomes the replacement the command returned,
    // predecessor reference and all.
    expect(reassignment.ticket).toEqual(replacement)
    expect(reassignment.error).toBeNull()
  })

  it('fields the kind does not carry are never sent', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockResolvedValue(openTask satisfies TicketRecord)
    command.mockResolvedValue(replacement satisfies TicketRecord)
    const reassignment = useReassignmentStore()
    await reassignment.open(transport, 2)

    await reassignment.reassign(transport, {
      kind: 'implementation',
      priority: 'urgent',
      spec_id: 7,
      slice: 'Spec authoring creates content versions end to end',
      criteria: [{ outcome: 'Specs mint unique numbers.', stories: ['CORE-S1-US1'] }],
    })

    expect(operations.find((entry) => entry.name === 'ticket.reassign')?.request).toEqual({
      mutation: mutationOf(4),
      ticket_id: 2,
      kind: 'implementation',
      priority: 'urgent',
      spec_id: 7,
      slice: 'Spec authoring creates content versions end to end',
      criteria: [{ outcome: 'Specs mint unique numbers.', stories: ['CORE-S1-US1'] }],
    })
  })

  it('a refused reassignment reports the explanation and keeps the original', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockResolvedValue(openTask satisfies TicketRecord)
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'done is final; landed work is not reassigned',
    })
    const reassignment = useReassignmentStore()
    await reassignment.open(transport, 2)

    const landed = await reassignment.reassign(transport, {
      kind: 'task',
      priority: 'high',
      title: 'Replan the register archive',
      subtype: 'migration',
      mode: 'agent',
      completion: ['The register moves and restores.'],
    })

    expect(landed).toBe(false)
    expect(reassignment.error).toBe('done is final; landed work is not reassigned')
    expect(reassignment.ticket).toEqual(openTask)
  })

  it('no reassignment runs before a Ticket is opened', async () => {
    setActivePinia(createPinia())
    const { transport } = harness()
    const reassignment = useReassignmentStore()

    const landed = await reassignment.reassign(transport, {
      kind: 'task',
      priority: 'high',
      title: 'Replan the register archive',
      subtype: 'migration',
      mode: 'agent',
      completion: ['The register moves and restores.'],
    })

    expect(landed).toBe(false)
    expect(reassignment.error).toBe('open a Ticket before reassigning it')
  })
})
