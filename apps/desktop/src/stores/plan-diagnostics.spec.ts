// The refresh discipline of the planning diagnostics store: a refused
// read leaves no stale report on display (KAN-S3-US7).
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

// A transport whose diagnostics query stays pending until the test
// settles it, so a read in flight is steered on demand.
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
    settle(report: PlanDiagnosticsResponse): void {
      pending.shift()?.resolve(report)
    },
    refuse(failure: unknown): void {
      pending.shift()?.reject(failure)
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
})
