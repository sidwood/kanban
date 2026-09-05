// The planning diagnostics of the graph on display: the blocking
// cycles, coverage gaps, and invalid profile references the core
// reports for one Plan — the working shape, or the frozen version the
// editor switched to (KAN-S3-US7). The invalid-profile list arrives
// empty until the execution profile catalogue feeds it (KAN-S7).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { PlanDiagnosticsResponse } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

export const usePlanDiagnosticsStore = defineStore('plan-diagnostics', {
  state: () => ({
    report: null as PlanDiagnosticsResponse | null,
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    // Read the diagnostics of one Plan's graph: a null version reads
    // the working shape, a number the frozen version on display.
    async refresh(
      transport: ShellTransport,
      planId: number,
      version: number | null,
    ): Promise<void> {
      try {
        this.report = await new KanbanClient(transport).queryPlanDiagnostics({
          plan_id: planId,
          version,
        })
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Forget the diagnostics when no graph is on display.
    clear(): void {
      this.report = null
      this.loaded = false
      this.error = null
    },
  },
})
