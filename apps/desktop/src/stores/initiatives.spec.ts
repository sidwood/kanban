import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { InitiativeListResponse, InitiativeRecord } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useInitiativesStore } from './initiatives'

function record(overrides: Partial<InitiativeRecord> = {}): InitiativeRecord {
  return {
    id: 1,
    name: 'Alpha',
    archived: false,
    version: 1,
    ...overrides,
  }
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
  const listing = (...initiatives: InitiativeRecord[]) =>
    query.mockImplementation(() =>
      Promise.resolve({ initiatives } satisfies InitiativeListResponse),
    )
  return { transport, operations, query, command, listing }
}

describe('initiatives store', () => {
  it('refresh loads every Initiative through the generated client', async () => {
    setActivePinia(createPinia())
    const { transport, listing } = harness()
    listing(record(), record({ id: 2, name: 'Beta', archived: true, version: 2 }))
    const initiatives = useInitiativesStore()

    await initiatives.refresh(transport)

    expect(initiatives.loaded).toBe(true)
    expect(initiatives.initiatives.map((entry) => entry.name)).toEqual(['Alpha', 'Beta'])
    expect(initiatives.error).toBeNull()
  })

  it('creating sends version zero and a fresh idempotency key', async () => {
    setActivePinia(createPinia())
    const { transport, operations, command, listing } = harness()
    listing()
    command.mockResolvedValue(record())
    const initiatives = useInitiativesStore()

    await initiatives.create(transport, 'Alpha')

    const create = operations.find((entry) => entry.name === 'initiative.create')
    expect(create?.kind).toBe('command')
    const request = create?.request as { mutation: { optimistic_version: number; idempotency_key: string }; name: string }
    expect(request.name).toBe('Alpha')
    expect(request.mutation.optimistic_version).toBe(0)
    expect(request.mutation.idempotency_key).toMatch(/[\w-]{8,}/)
    expect(initiatives.error).toBeNull()
  })

  it('renaming carries the stored version for that Initiative', async () => {
    setActivePinia(createPinia())
    const { transport, operations, command, listing } = harness()
    listing(record({ id: 3, name: 'Alpha', version: 4 }))
    command.mockResolvedValue(record({ id: 3, name: 'Beta', version: 5 }))
    const initiatives = useInitiativesStore()
    await initiatives.refresh(transport)

    await initiatives.rename(transport, 3, 'Beta')

    const rename = operations.find((entry) => entry.name === 'initiative.rename')
    const request = rename?.request as { mutation: { optimistic_version: number }; initiative_id: number; name: string }
    expect(request.initiative_id).toBe(3)
    expect(request.name).toBe('Beta')
    expect(request.mutation.optimistic_version).toBe(4)
  })

  it('archiving carries the stored version and refreshes', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    const stored = [record({ id: 7, name: 'Alpha', version: 2 })]
    query.mockImplementation(() => Promise.resolve({ initiatives: [...stored] }))
    command.mockImplementation(async () => {
      // The core's recorded fact changed; the next listing shows it.
      stored[0] = record({ id: 7, archived: true, version: 3 })
      return stored[0]
    })
    const initiatives = useInitiativesStore()
    await initiatives.refresh(transport)

    await initiatives.archive(transport, 7)

    const archive = operations.find((entry) => entry.name === 'initiative.archive')
    const request = archive?.request as { mutation: { optimistic_version: number }; initiative_id: number }
    expect(request.initiative_id).toBe(7)
    expect(request.mutation.optimistic_version).toBe(2)
    expect(initiatives.initiatives[0]?.archived).toBe(true)
  })

  it('a refused command reports the message and keeps the records', async () => {
    setActivePinia(createPinia())
    const { transport, command, listing } = harness()
    listing(record())
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'an Initiative name cannot be blank',
    })
    const initiatives = useInitiativesStore()
    await initiatives.refresh(transport)

    await initiatives.create(transport, '   ')

    expect(initiatives.error).toBe('an Initiative name cannot be blank')
    expect(initiatives.initiatives).toHaveLength(1)
  })

  it('a failing refresh reports the unreachable core', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockRejectedValue({ code: 'internal', message: 'the core connection is not writable' })
    const initiatives = useInitiativesStore()

    await initiatives.refresh(transport)

    expect(initiatives.loaded).toBe(false)
    expect(initiatives.error).toBe('the core connection is not writable')
  })
})
