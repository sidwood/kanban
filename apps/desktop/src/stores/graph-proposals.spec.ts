import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it } from 'vitest'
import type { TicketGraphRecord } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useGraphProposalsStore } from './graph-proposals'

function proposal(overrides: Partial<TicketGraphRecord> = {}): TicketGraphRecord {
  return {
    id: 1,
    spec_id: 4,
    spec_version: 1,
    tickets: [7, 8],
    edges: [{ from_ticket: 7, to_ticket: 8 }],
    state: 'proposed',
    version: 1,
    ...overrides,
  }
}

function harness(options: { refuse?: string; proposals?: TicketGraphRecord[] } = {}) {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  let held = options.proposals ?? [proposal()]
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      if (name === 'ticket.graph.list') return Promise.resolve({ proposals: held })
      return Promise.reject(new Error(`unexpected query ${name}`))
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      if (options.refuse) {
        return Promise.reject({ code: 'invalid_request', message: options.refuse })
      }
      const body = request as { proposal_id: number }
      held = held.map((entry) =>
        entry.id === body.proposal_id
          ? { ...entry, state: 'approved' as const, version: entry.version + 1 }
          : entry,
      )
      return Promise.resolve(held.find((entry) => entry.id === body.proposal_id))
    },
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, operations }
}

describe('graph proposals store', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('reads one Spec’s proposals through the production query', async () => {
    const store = useGraphProposalsStore()
    const { transport, operations } = harness()

    await store.load(transport, 4)

    expect(operations).toEqual([{ kind: 'query', name: 'ticket.graph.list', request: { spec_id: 4 } }])
    expect(store.proposals.map((entry) => entry.id)).toEqual([1])
    expect(store.loaded).toBe(true)
  })

  it('approves one proposal against its own version and re-reads the Spec', async () => {
    const store = useGraphProposalsStore()
    const { transport, operations } = harness()
    await store.load(transport, 4)

    expect(await store.approve(transport, store.proposals[0]!)).toBe(true)

    expect(operations[1]).toEqual({
      kind: 'command',
      name: 'ticket.graph.approve',
      request: {
        mutation: { optimistic_version: 1, idempotency_key: expect.any(String) },
        proposal_id: 1,
      },
    })
    expect(store.proposals[0]!.state).toBe('approved')
    expect(store.error).toBeNull()
  })

  it('reports the gate’s refusal and leaves the proposal standing', async () => {
    const store = useGraphProposalsStore()
    const { transport } = harness({
      refuse: 'the graph leaves CORE-S1-US2 uncovered',
    })
    await store.load(transport, 4)

    expect(await store.approve(transport, store.proposals[0]!)).toBe(false)

    expect(store.refusal).toEqual({
      proposalId: store.proposals[0]!.id,
      message: 'the graph leaves CORE-S1-US2 uncovered',
    })
    expect(store.proposals[0]!.state).toBe('proposed')
  })

  it('forgets the proposals when no Spec is on display', async () => {
    const store = useGraphProposalsStore()
    const { transport } = harness()
    await store.load(transport, 4)

    store.clear()

    expect(store.proposals).toEqual([])
    expect(store.loaded).toBe(false)
  })

  it('lets only the latest read write state', async () => {
    const store = useGraphProposalsStore()
    let release!: (value: { proposals: TicketGraphRecord[] }) => void
    const pending = new Promise<{ proposals: TicketGraphRecord[] }>((resolve) => {
      release = resolve
    })
    let call = 0
    const transport = {
      query: () => {
        call += 1
        return call === 1 ? pending : Promise.resolve({ proposals: [proposal({ id: 9 })] })
      },
      command: () => Promise.reject(new Error('no command here')),
      subscribe: () => () => undefined,
    } as unknown as ShellTransport

    const first = store.load(transport, 4)
    const second = store.load(transport, 5)
    await second
    release({ proposals: [proposal({ id: 1 })] })
    await first

    expect(store.proposals.map((entry) => entry.id)).toEqual([9])
  })
})
