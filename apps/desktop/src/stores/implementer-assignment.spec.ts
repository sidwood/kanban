// The assignment store is bound to one Project scope (KAN-T140-AC5,
// DR-EP-03): a load for the Project the operator has left cannot
// answer into the one they are in, a Ticket that Project holds cannot
// be assigned from here, and an answer that arrives after the scope
// moved on is dropped rather than applied.
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it } from 'vitest'
import type { TicketRecord } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useImplementerAssignmentStore } from './implementer-assignment'

function ticket(id: number, projectId: number): TicketRecord {
  return {
    id,
    project_id: projectId,
    number: id,
    kind: 'task',
    priority: 'normal',
    state: 'draft',
    title: `Ticket ${id}`,
    subtype: 'operational',
    mode: 'agent',
    completion: ['Done'],
    criteria: [],
    bug: null,
    pinned_spec_version: null,
    profile: null,
    version: 1,
  } as unknown as TicketRecord
}

function deferred<T>() {
  let settle!: (value: T) => void
  const promise = new Promise<T>((resolve) => {
    settle = resolve
  })
  return { promise, settle }
}

function harness(answers: {
  query: (request: Record<string, unknown>) => unknown
  command?: (request: Record<string, unknown>) => unknown
}) {
  const commands: Array<Record<string, unknown>> = []
  const transport = {
    query: (_name: string, request: Record<string, unknown>) => {
      const value = answers.query(request)
      return value instanceof Promise ? value : Promise.resolve(value)
    },
    command: (_name: string, request: Record<string, unknown>) => {
      commands.push(request)
      const value = answers.command?.(request) ?? {}
      return value instanceof Promise ? value : Promise.resolve(value)
    },
  } as unknown as ShellTransport
  return { transport, commands }
}

beforeEach(() => {
  setActivePinia(createPinia())
})

describe('implementer assignment scope', () => {
  it('drops a load the operator has already moved on from', async () => {
    const slow = deferred<unknown>()
    const { transport } = harness({
      query: (request) =>
        request.project_id === 1 ? slow.promise : { tickets: [ticket(20, 2)] },
    })
    const store = useImplementerAssignmentStore()

    const first = store.load(transport, 1)
    const second = store.load(transport, 2)
    await second
    slow.settle({ tickets: [ticket(10, 1)] })
    await first

    expect(store.projectId).toBe(2)
    expect(store.tickets.map((entry) => entry.id)).toEqual([20])
  })

  it('shows no Ticket at all while the Project arrived at is still loading', async () => {
    const pending = deferred<unknown>()
    const { transport } = harness({
      query: (request) =>
        request.project_id === 1 ? { tickets: [ticket(10, 1)] } : pending.promise,
    })
    const store = useImplementerAssignmentStore()

    await store.load(transport, 1)
    const second = store.load(transport, 2)

    expect(store.tickets).toEqual([])
    expect(store.loaded).toBe(false)

    pending.settle({ tickets: [ticket(20, 2)] })
    await second
    expect(store.tickets.map((entry) => entry.id)).toEqual([20])
  })

  it('refuses to command a Ticket the Project on display does not hold', async () => {
    const { transport, commands } = harness({
      query: () => ({ tickets: [ticket(20, 2)] }),
    })
    const store = useImplementerAssignmentStore()
    await store.load(transport, 2)
    store.tickets = [...store.tickets, ticket(10, 1)]

    expect(await store.assign(transport, 10, 'deep')).toBe(false)
    expect(commands).toEqual([])
    expect(store.error).toContain('not in the Project on display')
  })

  it('never applies an assignment answered after the Project changed', async () => {
    const slow = deferred<unknown>()
    const { transport, commands } = harness({
      query: (request) =>
        request.project_id === 1 ? { tickets: [ticket(10, 1)] } : { tickets: [ticket(20, 2)] },
      command: () => slow.promise,
    })
    const store = useImplementerAssignmentStore()
    await store.load(transport, 1)

    const assigned = store.assign(transport, 10, 'deep')
    await store.load(transport, 2)
    slow.settle({ ...ticket(10, 1), profile: 'deep', version: 2 })

    expect(await assigned).toBe(false)
    expect(commands).toHaveLength(1)
    expect(store.projectId).toBe(2)
    expect(store.tickets.map((entry) => entry.id)).toEqual([20])
    expect(store.error).toBeNull()
  })

  it('assigns through the production command inside its own scope', async () => {
    const { transport, commands } = harness({
      query: () => ({ tickets: [ticket(10, 1)] }),
      command: (request) => ({ ...ticket(10, 1), profile: request.profile, version: 2 }),
    })
    const store = useImplementerAssignmentStore()
    await store.load(transport, 1)

    expect(await store.assign(transport, 10, 'deep')).toBe(true)
    expect(commands).toEqual([
      {
        mutation: { optimistic_version: 1, idempotency_key: expect.any(String) },
        ticket_id: 10,
        profile: 'deep',
      },
    ])
    expect(store.tickets[0]!.profile).toBe('deep')
  })
})
