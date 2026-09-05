import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { PlanGetResponse, PlanListResponse, PlanRecord } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { usePlanEditorStore } from './plan-editor'

function record(overrides: Partial<PlanRecord> = {}): PlanRecord {
  return {
    id: 1,
    project_id: 4,
    number: 1,
    state: 'draft',
    spec_numbers: [1, 3, 2],
    edges: [
      { from_spec: 1, to_spec: 2 },
      { from_spec: 3, to_spec: 2 },
    ],
    version: 6,
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
  const listing = (...plans: PlanRecord[]) =>
    query.mockImplementation((_name: string, request: unknown) => {
      const asked = request as { project_id: number }
      return Promise.resolve({
        plans: plans.filter((plan) => plan.project_id === asked.project_id),
      } satisfies PlanListResponse)
    })
  return { transport, operations, query, command, listing }
}

describe('plan editor store', () => {
  it('refresh loads every plan of the project through the generated client', async () => {
    setActivePinia(createPinia())
    const { transport, listing } = harness()
    listing(
      record(),
      record({ id: 2, number: 2, state: 'active', version: 9 }),
      record({ id: 3, number: 3, state: 'complete', version: 10 }),
      record({ id: 4, number: 4, state: 'cancelled', version: 7 }),
      record({ id: 5, number: 5, state: 'archived', version: 8 }),
    )
    const editor = usePlanEditorStore()

    await editor.refresh(transport, 4)

    expect(editor.loaded).toBe(true)
    expect(editor.plans.map((plan) => plan.number)).toEqual([1, 2, 3, 4, 5])
    expect(editor.error).toBeNull()
  })

  it('the terminal states sit off the active surface but stay listed', async () => {
    setActivePinia(createPinia())
    const { transport, listing } = harness()
    listing(
      record(),
      record({ id: 2, state: 'active' }),
      record({ id: 3, state: 'complete' }),
      record({ id: 4, state: 'cancelled' }),
      record({ id: 5, state: 'archived' }),
    )
    const editor = usePlanEditorStore()
    await editor.refresh(transport, 4)

    expect(editor.activeSurface.map((plan) => plan.id)).toEqual([1, 2])
    expect(editor.finished.map((plan) => plan.id)).toEqual([3, 4, 5])
  })

  it('creating sends version zero, a fresh idempotency key, and the project', async () => {
    setActivePinia(createPinia())
    const fresh = record({ spec_numbers: [], edges: [], version: 1 })
    const { transport, operations, query, command } = harness()
    query.mockImplementation((name: string) => {
      if (name === 'plan.get') {
        return Promise.resolve({ plan: fresh, versions: [] } satisfies PlanGetResponse)
      }
      return Promise.resolve({ plans: [fresh] } satisfies PlanListResponse)
    })
    command.mockResolvedValue(fresh)
    const editor = usePlanEditorStore()
    await editor.refresh(transport, 4)

    await editor.create(transport, 4)

    const created = operations.find((entry) => entry.name === 'plan.create')
    expect(created?.kind).toBe('command')
    const request = created?.request as {
      mutation: { optimistic_version: number; idempotency_key: string }
      project_id: number
    }
    expect(request.project_id).toBe(4)
    expect(request.mutation.optimistic_version).toBe(0)
    expect(request.mutation.idempotency_key).toMatch(/[\w-]{8,}/)
  })

  it('creating with no plan selected refreshes and opens the created plan', async () => {
    setActivePinia(createPinia())
    const created = record({ id: 7, number: 2, spec_numbers: [], edges: [], version: 1 })
    const { transport, operations, query, command } = harness()
    query.mockImplementation((name: string) => {
      if (name === 'plan.get') {
        return Promise.resolve({ plan: created, versions: [] } satisfies PlanGetResponse)
      }
      return Promise.resolve({
        plans: [record({ id: 1, number: 1, state: 'complete', version: 12 }), created],
      } satisfies PlanListResponse)
    })
    command.mockResolvedValue(created)
    const editor = usePlanEditorStore()

    await editor.create(transport, 4)

    expect(editor.error).toBeNull()
    expect(
      operations.some((entry) => entry.kind === 'query' && entry.name === 'plan.list'),
      'the collection refreshes after creation',
    ).toBe(true)
    expect(editor.plans.map((plan) => plan.id)).toEqual([1, 7])
    expect(editor.selectedPlanId).toBe(7)
    expect(editor.selectedVersion).toBeNull()
    expect(editor.displayed?.spec_numbers).toEqual([])
  })

  it('repeated creation keeps every minted plan visible and opens the newest', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    const minted = [
      record({ id: 7, number: 2, spec_numbers: [], edges: [], version: 1 }),
      record({ id: 8, number: 3, spec_numbers: [], edges: [], version: 1 }),
    ]
    const plans = [record({ id: 1, number: 1, state: 'complete', version: 12 })]
    query.mockImplementation((name: string, request: unknown) => {
      if (name === 'plan.get') {
        const asked = request as { plan_id: number }
        const plan = minted.find((entry) => entry.id === asked.plan_id) ?? minted[0]
        return Promise.resolve({ plan, versions: [] } satisfies PlanGetResponse)
      }
      return Promise.resolve({ plans: [...plans] } satisfies PlanListResponse)
    })
    command.mockImplementation(() => {
      const next = minted[plans.length - 1]
      plans.push(next)
      return Promise.resolve(next)
    })
    const editor = usePlanEditorStore()

    await editor.create(transport, 4)
    expect(editor.selectedPlanId).toBe(7)

    await editor.create(transport, 4)

    expect(editor.selectedPlanId).toBe(8)
    expect(editor.plans.map((plan) => plan.id)).toEqual([1, 7, 8])
    expect(editor.error).toBeNull()
  })

  it('adding a spec carries the stored version and refreshes the record', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    const stored = [record()]
    query.mockImplementation(() => Promise.resolve({ plans: [...stored] } satisfies PlanListResponse))
    command.mockImplementation(async (_name: string, request: unknown) => {
      const asked = request as { spec_number: number }
      const edited = record({ spec_numbers: [...stored[0].spec_numbers, asked.spec_number] })
      stored[0] = edited
      return edited
    })
    const editor = usePlanEditorStore()
    await editor.refresh(transport, 4)
    editor.select(1)

    await editor.addSpec(transport, 4)

    const added = operations.find((entry) => entry.name === 'plan.spec.add')
    const request = added?.request as {
      mutation: { optimistic_version: number }
      plan_id: number
      spec_number: number
    }
    expect(request.plan_id).toBe(1)
    expect(request.spec_number).toBe(4)
    expect(request.mutation.optimistic_version).toBe(6)
    expect(editor.plans[0]?.spec_numbers).toEqual([1, 3, 2, 4])
  })

  it('moving a spec sends the position; removing sends the number', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockImplementation(() => Promise.resolve({ plans: [record()] } satisfies PlanListResponse))
    command.mockResolvedValue(record())
    const editor = usePlanEditorStore()
    await editor.refresh(transport, 4)
    editor.select(1)

    await editor.moveSpec(transport, 2, 0)
    await editor.removeSpec(transport, 3)

    const moved = operations.find((entry) => entry.name === 'plan.spec.move')
    expect((moved?.request as { spec_number: number; position: number }).position).toBe(0)
    const removed = operations.find((entry) => entry.name === 'plan.spec.remove')
    expect((removed?.request as { spec_number: number }).spec_number).toBe(3)
  })

  it('edges add and remove through their own operations', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockImplementation(() => Promise.resolve({ plans: [record()] } satisfies PlanListResponse))
    command.mockResolvedValue(record())
    const editor = usePlanEditorStore()
    await editor.refresh(transport, 4)
    editor.select(1)

    await editor.addEdge(transport, 2, 1)
    await editor.removeEdge(transport, 1, 2)

    const added = operations.find((entry) => entry.name === 'plan.edge.add')
    expect(added?.request).toMatchObject({ plan_id: 1, from_spec: 2, to_spec: 1 })
    const removed = operations.find((entry) => entry.name === 'plan.edge.remove')
    expect(removed?.request).toMatchObject({ plan_id: 1, from_spec: 1, to_spec: 2 })
  })

  it('opening a plan loads its frozen versions', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation((name: string) => {
      if (name === 'plan.get') {
        return Promise.resolve({
          plan: record({ state: 'active' }),
          versions: [
            {
              number: 1,
              spec_numbers: [1, 3, 2],
              edges: [
                { from_spec: 1, to_spec: 2 },
                { from_spec: 3, to_spec: 2 },
              ],
            },
            {
              number: 2,
              spec_numbers: [2, 1, 3],
              edges: [
                { from_spec: 1, to_spec: 2 },
                { from_spec: 3, to_spec: 2 },
              ],
            },
          ],
        } satisfies PlanGetResponse)
      }
      return Promise.resolve({ plans: [record({ state: 'active' })] } satisfies PlanListResponse)
    })
    const editor = usePlanEditorStore()

    await editor.open(transport, 1)

    expect(editor.selectedPlanId).toBe(1)
    expect(editor.versions.map((version) => version.number)).toEqual([1, 2])
    expect(editor.displayed?.spec_numbers).toEqual([1, 3, 2])
  })

  it('version switching shows the frozen shape and the draft on demand', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockImplementation((name: string) => {
      if (name === 'plan.get') {
        return Promise.resolve({
          plan: record({ state: 'active', spec_numbers: [2, 1, 3] }),
          versions: [
            {
              number: 1,
              spec_numbers: [1, 3, 2],
              edges: [
                { from_spec: 1, to_spec: 2 },
                { from_spec: 3, to_spec: 2 },
              ],
            },
          ],
        } satisfies PlanGetResponse)
      }
      return Promise.resolve({ plans: [record({ state: 'active' })] } satisfies PlanListResponse)
    })
    const editor = usePlanEditorStore()
    await editor.open(transport, 1)

    editor.showVersion(1)
    expect(editor.displayed?.spec_numbers).toEqual([1, 3, 2])

    editor.showDraft()
    expect(editor.displayed?.spec_numbers).toEqual([2, 1, 3])
  })

  it('each lifecycle move carries the stored version and its operation', async () => {
    setActivePinia(createPinia())
    const { transport, operations, query, command } = harness()
    query.mockImplementation(() => Promise.resolve({ plans: [record()] } satisfies PlanListResponse))
    command.mockResolvedValue(record())
    const editor = usePlanEditorStore()
    await editor.refresh(transport, 4)
    editor.select(1)

    await editor.activate(transport)
    await editor.replan(transport)
    await editor.complete(transport)
    await editor.cancel(transport)
    await editor.archive(transport)

    for (const name of [
      'plan.activate',
      'plan.replan',
      'plan.complete',
      'plan.cancel',
      'plan.archive',
    ]) {
      const entry = operations.find((operation) => operation.name === name)
      expect(entry, `${name} is sent`).toBeDefined()
      expect((entry?.request as { mutation: { optimistic_version: number } }).mutation)
        .toMatchObject({ optimistic_version: 6 })
    }
  })

  it('a refused command reports the message and keeps the records', async () => {
    setActivePinia(createPinia())
    const { transport, query, command } = harness()
    query.mockImplementation(() => Promise.resolve({ plans: [record()] } satisfies PlanListResponse))
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'only a draft Plan accepts this change',
    })
    const editor = usePlanEditorStore()
    await editor.refresh(transport, 4)
    editor.select(1)

    await editor.addSpec(transport, 4)

    expect(editor.error).toBe('only a draft Plan accepts this change')
    expect(editor.plans).toHaveLength(1)
  })

  it('a failing refresh reports the unreachable core', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockRejectedValue({ code: 'internal', message: 'the core connection is not writable' })
    const editor = usePlanEditorStore()

    await editor.refresh(transport, 4)

    expect(editor.loaded).toBe(false)
    expect(editor.error).toBe('the core connection is not writable')
  })
})
