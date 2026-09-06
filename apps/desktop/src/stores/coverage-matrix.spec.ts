// The refresh discipline of the coverage matrix store: the Spec list
// picks the Spec the matrix reads, only the latest read writes state,
// a refused read leaves no stale matrix behind, and clearing the
// display supersedes whatever is still in flight (DR-PS-18).
import { createPinia, setActivePinia } from 'pinia'
import { flushPromises } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import type { SpecCoverageMatrixResponse, SpecListResponse } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useCoverageMatrixStore } from './coverage-matrix'

// The Spec list of one Project, varied by identity.
function specList(...ids: number[]): SpecListResponse {
  return {
    specs: ids.map((id) => ({
      id,
      project_id: 4,
      number: id,
      name: `Spec ${id}`,
      execution: 'planned' as const,
      plan_id: 1,
      version: 1,
    })),
  }
}

// The matrix of one Spec version, varied by the story it claims.
function matrix(specId: number, story: string): SpecCoverageMatrixResponse {
  return {
    spec_id: specId,
    version: 2,
    stories: [
      {
        story,
        claims: [
          { ticket_id: 17, ticket_number: 17, outcome: `${story} is claimed.` },
        ],
      },
    ],
  }
}

// A transport whose spec list and matrix reads stay pending until the
// test settles them, so loads overlap on demand.
function harness() {
  const pending: Array<{
    resolve: (report: SpecListResponse | SpecCoverageMatrixResponse) => void
    reject: (failure: unknown) => void
  }> = []
  const transport = {
    query: () =>
      new Promise<SpecListResponse | SpecCoverageMatrixResponse>((resolve, reject) => {
        pending.push({ resolve, reject })
      }),
    command: () => Promise.resolve({}),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return {
    transport,
    settle(
      report: SpecListResponse | SpecCoverageMatrixResponse,
      index = 0,
    ): void {
      pending.splice(index, 1)[0]?.resolve(report)
    },
    refuse(failure: unknown, index = 0): void {
      pending.splice(index, 1)[0]?.reject(failure)
    },
  }
}

describe('coverage matrix store', () => {
  it('loads the specs and reads the first matrix', async () => {
    setActivePinia(createPinia())
    const { transport, settle } = harness()
    const store = useCoverageMatrixStore()

    const loading = store.loadSpecs(transport, 4)
    settle(specList(1, 2))
    await flushPromises()
    settle(matrix(1, 'CORE-S1-US1'))
    await loading

    expect(store.specs.map((spec) => spec.id)).toEqual([1, 2])
    expect(store.pickedSpecId).toBe(1)
    expect(store.report?.spec_id).toBe(1)
    expect(store.report?.stories[0].claims[0].outcome).toBe('CORE-S1-US1 is claimed.')
    expect(store.loaded).toBe(true)
    expect(store.error).toBe(null)
  })

  it('keeps the picked spec across a reload and picks the first when it left', async () => {
    setActivePinia(createPinia())
    const { transport, settle } = harness()
    const store = useCoverageMatrixStore()

    const first = store.loadSpecs(transport, 4)
    settle(specList(1, 2))
    await flushPromises()
    settle(matrix(1, 'CORE-S1-US1'))
    await first
    const second = store.loadSpecs(transport, 4)
    settle(specList(2, 3))
    await flushPromises()
    settle(matrix(2, 'CORE-S2-US1'))
    await second

    expect(store.pickedSpecId).toBe(2)

    const third = store.loadSpecs(transport, 4)
    settle(specList(5))
    await flushPromises()
    settle(matrix(5, 'CORE-S5-US1'))
    await third

    expect(store.pickedSpecId).toBe(5)
    expect(store.report?.spec_id).toBe(5)
  })

  it('drops the matrix a refused read leaves behind', async () => {
    setActivePinia(createPinia())
    const { transport, settle, refuse } = harness()
    const store = useCoverageMatrixStore()

    const first = store.loadSpecs(transport, 4)
    settle(specList(1))
    await flushPromises()
    settle(matrix(1, 'CORE-S1-US1'))
    await first

    const second = store.read(transport, 1)
    refuse({ code: 'internal', message: 'the core refused' })
    await second

    expect(store.report).toBe(null)
    expect(store.loaded).toBe(false)
    expect(store.error).toBe('the core refused')
  })

  it('lets only the latest read write the matrix', async () => {
    setActivePinia(createPinia())
    const { transport, settle } = harness()
    const store = useCoverageMatrixStore()

    const older = store.read(transport, 1)
    const newer = store.read(transport, 2)
    settle(matrix(2, 'CORE-S2-US1'), 1)
    await newer
    settle(matrix(1, 'CORE-S1-US1'))
    await older

    expect(store.report?.spec_id).toBe(2)
    expect(store.loaded).toBe(true)
    expect(store.error).toBe(null)
  })

  it('forgets an in-flight read when the display clears', async () => {
    setActivePinia(createPinia())
    const { transport, settle } = harness()
    const store = useCoverageMatrixStore()

    const flight = store.read(transport, 1)
    store.clear()
    settle(matrix(1, 'CORE-S1-US1'))
    await flight

    expect(store.specs).toEqual([])
    expect(store.pickedSpecId).toBe(null)
    expect(store.report).toBe(null)
    expect(store.loaded).toBe(false)
    expect(store.error).toBe(null)
  })

  it('keeps a spec list a refused load leaves empty', async () => {
    setActivePinia(createPinia())
    const { transport, refuse } = harness()
    const store = useCoverageMatrixStore()

    const loading = store.loadSpecs(transport, 9)
    refuse({ code: 'not_found', message: 'spec 9 was not found' })
    await loading

    expect(store.specs).toEqual([])
    expect(store.pickedSpecId).toBe(null)
    expect(store.loaded).toBe(false)
    expect(store.error).toBe('spec 9 was not found')
  })
})
