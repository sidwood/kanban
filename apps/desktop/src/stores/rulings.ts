// Ruling and deferral list state for the embedded timeline surface.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  DeferralRecord,
  RulingRecord,
  TimelineEntityRef,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

export const useRulingsStore = defineStore('rulings', {
  state: () => ({
    projectId: 0 as number,
    entity: null as TimelineEntityRef | null,
    rulings: [] as RulingRecord[],
    deferrals: [] as DeferralRecord[],
    loading: false,
    error: null as string | null,
  }),
  actions: {
    async load(
      transport: ShellTransport,
      projectId: number,
      entity?: TimelineEntityRef | null,
    ): Promise<void> {
      this.projectId = projectId
      this.entity = entity ?? null
      await this.refresh(transport)
    },
    async refresh(transport: ShellTransport): Promise<void> {
      if (this.projectId === 0) {
        return
      }
      this.loading = true
      this.error = null
      try {
        const client = new KanbanClient(transport)
        const [rulings, deferrals] = await Promise.all([
          client.queryRulingList({
            project_id: this.projectId,
            entity: this.entity ?? undefined,
          }),
          client.queryDeferralList({
            project_id: this.projectId,
          }),
        ])
        this.rulings = rulings.rulings
        this.deferrals = deferrals.deferrals
      } catch (failure) {
        this.error = asApiError(failure).message
        this.rulings = []
        this.deferrals = []
      } finally {
        this.loading = false
      }
    },
  },
})
