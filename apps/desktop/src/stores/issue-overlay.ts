// Cached Linear issue records keyed by ticket id for board cards and
// the detail drawer.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { IssueMilestoneRecord, IssueRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { issueDisplayKey } from '../views/board-card'

const ISSUE_PAGE = 100
const ISSUE_READS_AT_ONCE = 4

export interface IssueReference {
  ticketId: number
  projectId: number
  projectCode: string
  number: number
}

interface OverlayCache {
  byTicketId: Record<number, IssueRecord>
  refreshesIssued: number
  clearedAt: number
  cachedAt: Record<number, number>
}

interface ReadSlots {
  open: number
  waiting: Array<() => void>
}

const readSlots = new WeakMap<object, ReadSlots>()

async function oneIssueRead<T>(owner: object, read: () => Promise<T>): Promise<T> {
  let slots = readSlots.get(owner)
  if (!slots) {
    slots = { open: ISSUE_READS_AT_ONCE, waiting: [] }
    readSlots.set(owner, slots)
  }
  const held = slots
  if (held.open > 0) {
    held.open -= 1
  } else {
    await new Promise<void>((resolve) => held.waiting.push(resolve))
  }
  try {
    return await read()
  } finally {
    const next = held.waiting.shift()
    if (next) next()
    else held.open += 1
  }
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

async function milestoneNames(
  client: KanbanClient,
  projectId: number,
): Promise<Record<number, string>> {
  const milestones = await client.queryIssueList_milestones({ project_id: projectId })
  return Object.fromEntries(
    milestones.milestones.map((entry: IssueMilestoneRecord) => [entry.id, entry.name]),
  )
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
      ticketIds: readonly number[],
    ): Promise<void> {
      this.loading = true
      this.error = null
      try {
        const client = new KanbanClient(transport)
        this.milestonesByProject[projectId] = await milestoneNames(client, projectId)
        const unread = new Set(ticketIds)
        let cursor: string | null = null
        do {
          const generation = ++this.refreshesIssued
          const page = await client.queryIssueList({
            project_id: projectId,
            limit: ISSUE_PAGE,
            ...(cursor === null ? {} : { cursor }),
          })
          for (const issue of page.issues) {
            if (!unread.delete(issue.ticket_id)) continue
            cacheAnswer(this, issue.ticket_id, issue, generation)
          }
          cursor = page.next_cursor ?? null
        } while (cursor !== null && unread.size > 0)
      } catch (failure) {
        this.error = asApiError(failure).message
      } finally {
        this.loading = false
      }
    },
    async refreshIssues(
      transport: ShellTransport,
      issues: readonly IssueReference[],
    ): Promise<void> {
      this.loading = true
      this.error = null
      try {
        const client = new KanbanClient(transport)
        const projects = [...new Set(issues.map((entry) => entry.projectId))]
        await Promise.all(
          projects.map(async (projectId) => {
            this.milestonesByProject[projectId] = await milestoneNames(client, projectId)
          }),
        )
        await Promise.all(
          issues.map((entry) =>
            oneIssueRead(this, async () => {
              const generation = ++this.refreshesIssued
              const issue = await client.queryIssueGet({
                identifier: issueDisplayKey(entry.projectCode, entry.number),
              })
              cacheAnswer(this, entry.ticketId, issue, generation)
            }),
          ),
        )
      } catch (failure) {
        this.error = asApiError(failure).message
      } finally {
        this.loading = false
      }
    },
    readIssue(transport: ShellTransport, identifier: string): Promise<IssueRecord> {
      return oneIssueRead(this, () => new KanbanClient(transport).queryIssueGet({ identifier }))
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
        const issue = await oneIssueRead(this, async () => {
          const generation = ++this.refreshesIssued
          const answer = await client.queryIssueGet({ identifier })
          cacheAnswer(this, ticketId, answer, generation)
          return answer
        })
        this.error = null
        return this.byTicketId[ticketId] ?? issue
      } catch (failure) {
        this.error = asApiError(failure).message
        return null
      }
    },
  },
})
