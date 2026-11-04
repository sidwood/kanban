// Cached Linear issue records keyed by ticket id for board cards and
// the detail drawer.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { IssueMilestoneRecord, IssueRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { issueDisplayKey } from '../views/board-card'

interface OverlayCache {
  byTicketId: Record<number, IssueRecord>
  refreshesIssued: number
  clearedAt: number
  cachedAt: Record<number, number>
}

function olderThanCached(cache: OverlayCache, ticketId: number, issue: IssueRecord): boolean {
  const cached = cache.byTicketId[ticketId]
  return cached !== undefined && issue.version < cached.version
}

function cacheAnswer(
  cache: OverlayCache,
  ticketId: number,
  issue: IssueRecord,
  generation: number,
): void {
  const supersededAt = Math.max(cache.clearedAt, cache.cachedAt[ticketId] ?? 0)
  if (generation <= supersededAt || olderThanCached(cache, ticketId, issue)) return
  cache.byTicketId[ticketId] = issue
  cache.cachedAt[ticketId] = generation
}

export const useIssueOverlayStore = defineStore('issue-overlay', {
  state: () => ({
    byTicketId: {} as Record<number, IssueRecord>,
    milestonesByProject: {} as Record<number, Record<number, string>>,
    loading: false,
    error: null as string | null,
    refreshesIssued: 0,
    clearedAt: 0,
    cachedAt: {} as Record<number, number>,
  }),
  getters: {
    issueFor: (state) => (ticketId: number): IssueRecord | null =>
      state.byTicketId[ticketId] ?? null,
    milestoneName:
      (state) =>
      (projectId: number, milestoneId: number | null | undefined): string | null => {
        if (milestoneId == null) return null
        return state.milestonesByProject[projectId]?.[milestoneId] ?? null
      },
  },
  actions: {
    clear(): void {
      this.byTicketId = {}
      this.milestonesByProject = {}
      this.error = null
      this.clearedAt = this.refreshesIssued
      this.cachedAt = {}
    },
    put(issue: IssueRecord): void {
      if (olderThanCached(this, issue.ticket_id, issue)) return
      this.byTicketId[issue.ticket_id] = issue
      this.cachedAt[issue.ticket_id] = ++this.refreshesIssued
    },
    async refreshForProject(
      transport: ShellTransport,
      projectId: number,
      projectCode: string,
      ticketIds: readonly number[],
      ticketNumbers: Readonly<Record<number, number>>,
    ): Promise<void> {
      this.loading = true
      this.error = null
      try {
        const client = new KanbanClient(transport)
        const milestones = await client.queryIssueList_milestones({ project_id: projectId })
        this.milestonesByProject[projectId] = Object.fromEntries(
          milestones.milestones.map((entry: IssueMilestoneRecord) => [entry.id, entry.name]),
        )
        await Promise.all(
          ticketIds.map(async (ticketId) => {
            const number = ticketNumbers[ticketId]
            if (number === undefined) return
            const identifier = issueDisplayKey(projectCode, number)
            const generation = ++this.refreshesIssued
            const issue = await client.queryIssueGet({ identifier })
            cacheAnswer(this, ticketId, issue, generation)
          }),
        )
      } catch (failure) {
        this.error = asApiError(failure).message
      } finally {
        this.loading = false
      }
    },
    async refreshOne(
      transport: ShellTransport,
      _projectId: number,
      projectCode: string,
      ticketId: number,
      ticketNumber: number,
    ): Promise<IssueRecord | null> {
      try {
        const client = new KanbanClient(transport)
        const identifier = issueDisplayKey(projectCode, ticketNumber)
        const generation = ++this.refreshesIssued
        const issue = await client.queryIssueGet({ identifier })
        cacheAnswer(this, ticketId, issue, generation)
        this.error = null
        return this.byTicketId[ticketId] ?? issue
      } catch (failure) {
        this.error = asApiError(failure).message
        return null
      }
    },
  },
})
