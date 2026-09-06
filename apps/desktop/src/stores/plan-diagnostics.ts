// The planning diagnostics of the graph on display: the blocking
// cycles, coverage gaps, and invalid profile references the core
// reports for one Plan — the working shape, or the frozen version the
// editor switched to (KAN-S3-US7). The invalid-profile list carries
// the references the stored execution profile catalogue resolves to
// no assignable entry (KAN-S7, T38).
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
    // The refreshes issued so far, so only the latest one — the
    // graph actually on display — ever writes state.
    issued: 0,
  }),
  actions: {
    // Read the diagnostics of one Plan's graph: a null version reads
    // the working shape, a number the frozen version on display. A
    // refresh another refresh or clear() has superseded writes
    // nothing; a refused one leaves no stale report on display.
    async refresh(
      transport: ShellTransport,
      planId: number,
      version: number | null,
    ): Promise<void> {
      const attempt = ++this.issued
      try {
        const report = await new KanbanClient(transport).queryPlanDiagnostics({
          plan_id: planId,
          version,
        })
        if (attempt !== this.issued) {
          return
        }
        this.report = report
        this.loaded = true
        this.error = null
      } catch (failure) {
        if (attempt !== this.issued) {
          return
        }
        this.report = null
        this.loaded = false
        this.error = asApiError(failure).message
      }
    },
    // Forget the diagnostics when no graph is on display; anything
    // still in flight is superseded.
    clear(): void {
      this.issued += 1
      this.report = null
      this.loaded = false
      this.error = null
    },
  },
})
