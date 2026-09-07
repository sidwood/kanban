import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it } from 'vitest'
import type {
  TicketReviewConfigRecord,
  TicketReviewConfigResponse,
  TicketReviewStage,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useReviewConfigStore } from './review-config'

function standing(overrides: Partial<TicketReviewConfigRecord> = {}): TicketReviewConfigRecord {
  return {
    ticket_id: 1,
    stages: [
      {
        slots: [
          { occupant: { kind: 'profile', name: 'outsider' }, requirement: 'required' },
          { occupant: { kind: 'human' }, requirement: 'optional' },
        ],
      },
    ],
    version: 2,
    ...overrides,
  }
}

function stages(): TicketReviewStage[] {
  return [
    {
      slots: [
        { occupant: { kind: 'profile', name: 'outsider' }, requirement: 'required' },
        { occupant: { kind: 'human' }, requirement: 'optional' },
      ],
    },
  ]
}

function transportWith(
  answers: Record<string, unknown>,
  failures: Record<string, unknown> = {},
): ShellTransport & { commands: Array<[string, unknown]> } {
  const commands: Array<[string, unknown]> = []
  const transport = {
    command: (name: string, request: unknown) => {
      commands.push([name, request])
      if (name in failures) {
        return Promise.reject(failures[name])
      }
      return Promise.resolve(answers[name])
    },
    query: (name: string, request: unknown) => {
      commands.push([name, request])
      if (name in failures) {
        return Promise.reject(failures[name])
      }
      return Promise.resolve(answers[name])
    },
  }
  return Object.assign(transport as ShellTransport, { commands })
}

describe('the review configuration store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('loads the stored configuration of one Ticket', async () => {
    const store = useReviewConfigStore()
    const transport = transportWith({
      'ticket.review.config': { config: standing() } satisfies TicketReviewConfigResponse,
    })

    await store.refresh(transport, 1)

    expect(store.config).toEqual(standing())
    expect(store.loaded).toBe(true)
    expect(store.error).toBeNull()
  })

  it('serves nothing while the Ticket carries no configuration', async () => {
    const store = useReviewConfigStore()
    const transport = transportWith({
      'ticket.review.config': { config: null } satisfies TicketReviewConfigResponse,
    })

    await store.refresh(transport, 1)

    expect(store.config).toBeNull()
    expect(store.loaded).toBe(true)
  })

  it('reports a failed read instead of guessing a configuration', async () => {
    const store = useReviewConfigStore()
    const transport = transportWith(
      {},
      { 'ticket.review.config': { code: 'internal', message: 'core offline' } },
    )

    await store.refresh(transport, 1)

    expect(store.config).toBeNull()
    expect(store.loaded).toBe(false)
    expect(store.error).toBe('core offline')
  })

  it('configures from zero while nothing stands and holds the record', async () => {
    const store = useReviewConfigStore()
    const transport = transportWith({
      'ticket.review.configure': standing({ version: 1 }),
    })

    const landed = await store.configure(transport, 1, stages())

    expect(landed).toBe(true)
    expect(store.config?.version).toBe(1)
    expect(store.error).toBeNull()
    expect(transport.commands).toEqual([
      [
        'ticket.review.configure',
        {
          mutation: expect.objectContaining({ optimistic_version: 0 }),
          ticket_id: 1,
          stages: stages(),
        },
      ],
    ])
  })

  it('configures from the standing version when one exists', async () => {
    const store = useReviewConfigStore()
    const transport = transportWith({
      'ticket.review.config': { config: standing() } satisfies TicketReviewConfigResponse,
      'ticket.review.configure': standing({ version: 3 }),
    })
    await store.refresh(transport, 1)

    await store.configure(transport, 1, stages())

    const configure = transport.commands.filter(([name]) => name === 'ticket.review.configure')
    expect(configure[0]?.[1]).toMatchObject({ mutation: { optimistic_version: 2 } })
    expect(store.config?.version).toBe(3)
  })

  it('reports a separation refusal and keeps the standing configuration', async () => {
    const store = useReviewConfigStore()
    const refusal = asApiError(
      new Error('the reviewer profile `same-model` shares the implementer model family'),
    )
    const transport = transportWith(
      {
        'ticket.review.config': { config: standing() } satisfies TicketReviewConfigResponse,
      },
      { 'ticket.review.configure': refusal },
    )
    await store.refresh(transport, 1)

    const landed = await store.configure(transport, 1, stages())

    expect(landed).toBe(false)
    expect(store.error).toContain('model family')
    expect(store.config).toEqual(standing())
  })

  it('mints a fresh idempotency key for every configure attempt', async () => {
    const store = useReviewConfigStore()
    const transport = transportWith({
      'ticket.review.configure': standing({ version: 1 }),
    })

    await store.configure(transport, 1, stages())
    await store.configure(transport, 1, stages())

    const keys = transport.commands.map(
      ([, request]) => (request as { mutation: { idempotency_key: string } }).mutation
        .idempotency_key,
    )
    expect(keys).toHaveLength(2)
    expect(keys[0]).not.toBe(keys[1])
  })
})
