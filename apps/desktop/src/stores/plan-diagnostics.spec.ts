// The refresh discipline of the planning diagnostics store: a refused
// read leaves no stale report on display, only the latest refresh
// writes state, and clearing the display supersedes whatever is still
// in flight (KAN-S3-US7).
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it } from 'vitest'
import type { PlanDiagnosticsResponse } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { usePlanDiagnosticsStore } from './plan-diagnostics'

// The report of a blocked graph.
const blocked = {
  cycles: [{ spec_numbers: [1, 2] }],
  coverage_gaps: [
    { spec_number: 1, uncovered: ['CORE-S1-US1'], claims_no_stories: false },
  ],
  invalid_profiles: [],
  blocking: true,
} satisfies PlanDiagnosticsResponse

// The report of a clear graph.
const clear = {
  cycles: [],
  coverage_gaps: [],
  invalid_profiles: [],
  blocking: false,
} satisfies PlanDiagnosticsResponse

// A transport whose every diagnostics query stays pending until the
// test settles it, so refreshes overlap on demand.
function harness() {
  const pending: Array<{
    resolve: (report: PlanDiagnosticsResponse) => void
    reject: (failure: unknown) => void
  }> = []
  const transport = {
    query: () =>
      new Promise<PlanDiagnosticsResponse>((resolve, reject) => {
        pending.push({ resolve, reject })
      }),
    command: () => Promise.resolve({}),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return {
    transport,
    settle(report: PlanDiagnosticsResponse, index = 0): void {
      pending.splice(index, 1)[0]?.resolve(report)
    },
    refuse(failure: unknown, index = 0): void {
      pending.splice(index, 1)[0]?.reject(failure)
    },
  }
}

describe('plan diagnostics store', () => {
  it('drops the report a refused refresh leaves behind', async () => {
    setActivePinia(createPinia())
    const { transport, settle, refuse } = harness()
    const diagnostics = usePlanDiagnosticsStore()
    const first = diagnostics.refresh(transport, 1, null)
    settle(blocked)
    await first
    expect(diagnostics.report).toEqual(blocked)

    const second = diagnostics.refresh(transport, 1, null)
    refuse({ code: 'internal', message: 'the core refused' })
    await second

    expect(diagnostics.report).toBe(null)
    expect(diagnostics.loaded).toBe(false)
    expect(diagnostics.error).toBe('the core refused')
  })

  it('lets only the latest refresh write the report', async () => {
    setActivePinia(createPinia())
    const { transport, settle } = harness()
    const diagnostics = usePlanDiagnosticsStore()
    const older = diagnostics.refresh(transport, 1, null)
    const newer = diagnostics.refresh(transport, 1, 2)
    settle(clear, 1)
    await newer
    expect(diagnostics.report).toEqual(clear)

    settle(blocked)
    await older

    expect(diagnostics.report).toEqual(clear)
    expect(diagnostics.loaded).toBe(true)
    expect(diagnostics.error).toBe(null)
  })

  it('forgets an in-flight refresh when the display clears', async () => {
    setActivePinia(createPinia())
    const { transport, settle } = harness()
    const diagnostics = usePlanDiagnosticsStore()
    const flight = diagnostics.refresh(transport, 1, null)

    diagnostics.clear()
    settle(blocked)
    await flight

    expect(diagnostics.report).toBe(null)
    expect(diagnostics.loaded).toBe(false)
    expect(diagnostics.error).toBe(null)
  })
})
