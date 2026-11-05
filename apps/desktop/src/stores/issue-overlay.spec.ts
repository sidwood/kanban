import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it } from 'vitest'
import type { IssueListResponse, IssueRecord } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useIssueOverlayStore } from './issue-overlay'

function issue(overrides: Partial<IssueRecord> = {}): IssueRecord {
  return {
    identifier: 'CORE-12',
    url: 'kanban://issue/CORE-12',
    ticket_id: 7,
    project_id: 1,
    title: 'Archive the old exports',
    description: '',
    state: 'Ready',
    state_type: 'backlog',
    priority: 3,
    labels: [],
    relations: [],
    state_history: [],
    version: 3,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  }
}

interface Pending {
  resolve: (record: IssueRecord) => void
}

function wire() {
  const pending: Pending[] = []
  const reads: Array<{ name: string; request: unknown }> = []
  const transport = {
    query: (name: string, request: unknown) => {
      if (name === 'issue.list_milestones') return Promise.resolve({ milestones: [] })
      reads.push({ name, request })
      if (name === 'issue.list') {
        return new Promise<IssueListResponse>((resolve) => {
          pending.push({ resolve: (record) => resolve({ issues: [record], next_cursor: null }) })
        })
      }
      if (name === 'issue.get') {
        return new Promise<IssueRecord>((resolve) => {
          pending.push({ resolve })
        })
      }
      return Promise.reject(new Error(`unexpected query ${name}`))
    },
    command: () => Promise.reject(new Error('commands are forbidden here')),
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, pending, reads }
}

const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 0))

function refreshBoard(transport: ShellTransport): Promise<void> {
  return useIssueOverlayStore().refreshForProject(transport, 1, [7])
}

function refreshDrawer(transport: ShellTransport): Promise<IssueRecord | null> {
  return useIssueOverlayStore().refreshOne(transport, 1, 'CORE', 7, 12)
}

describe('issue overlay refreshes', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('keeps the newer issue when an earlier board refresh answers last', async () => {
    const { transport, pending } = wire()
    const overlay = useIssueOverlayStore()

    const first = refreshBoard(transport)
    await settle()
    const second = refreshBoard(transport)
    await settle()
    expect(pending).toHaveLength(2)

    pending[1].resolve(issue({ version: 5, labels: ['Shipped'] }))
    await second
    pending[0].resolve(issue({ version: 4, labels: ['Stale'] }))
    await first

    expect(overlay.issueFor(7)?.version).toBe(5)
    expect(overlay.issueFor(7)?.labels).toEqual(['Shipped'])
  })

  it('keeps the drawer refresh when a board refresh issued before it answers last', async () => {
    const { transport, pending } = wire()
    const overlay = useIssueOverlayStore()

    const board = refreshBoard(transport)
    await settle()
    const drawer = refreshDrawer(transport)
    await settle()

    pending[1].resolve(issue({ version: 5, assignee: 'ana' }))
    expect((await drawer)?.version).toBe(5)
    pending[0].resolve(issue({ version: 4, assignee: null }))
    await board

    expect(overlay.issueFor(7)?.version).toBe(5)
    expect(overlay.issueFor(7)?.assignee).toBe('ana')
  })

  it('hands the drawer the newer cached issue when its own answer is older', async () => {
    const { transport, pending } = wire()
    const overlay = useIssueOverlayStore()

    const drawer = refreshDrawer(transport)
    await settle()
    const board = refreshBoard(transport)
    await settle()

    pending[1].resolve(issue({ version: 6, description: 'Saved text' }))
    await board
    pending[0].resolve(issue({ version: 5, description: 'Earlier text' }))
    const refreshed = await drawer

    expect(refreshed?.version).toBe(6)
    expect(refreshed?.description).toBe('Saved text')
    expect(overlay.issueFor(7)?.version).toBe(6)
  })

  it('takes the newer answer to a drawer refresh issued before an older issue was put', async () => {
    const { transport, pending } = wire()
    const overlay = useIssueOverlayStore()

    const drawer = refreshDrawer(transport)
    await settle()
    overlay.put(issue({ version: 4, description: 'Earlier text' }))
    pending[0].resolve(issue({ version: 5, description: 'Saved text' }))
    const refreshed = await drawer

    expect(refreshed?.version).toBe(5)
    expect(refreshed?.description).toBe('Saved text')
    expect(overlay.issueFor(7)?.version).toBe(5)
  })

  it('keeps the later-issued answer when an earlier one of the same version arrives after it', async () => {
    const { transport, pending } = wire()
    const overlay = useIssueOverlayStore()

    const first = refreshBoard(transport)
    await settle()
    const second = refreshBoard(transport)
    await settle()

    pending[1].resolve(issue({ version: 4, sub_issues: { done: 2, total: 3 } }))
    await second
    pending[0].resolve(issue({ version: 4, sub_issues: { done: 1, total: 3 } }))
    await first

    expect(overlay.issueFor(7)?.sub_issues).toEqual({ done: 2, total: 3 })
  })

  it('writes nothing from a refresh still on the wire when the overlay clears', async () => {
    const { transport, pending } = wire()
    const overlay = useIssueOverlayStore()

    const board = refreshBoard(transport)
    await settle()
    overlay.clear()
    pending[0].resolve(issue({ version: 4 }))
    await board

    expect(overlay.issueFor(7)).toBeNull()
  })

  it('keeps the newer cached issue when the drawer puts an older read', async () => {
    const { transport, pending } = wire()
    const overlay = useIssueOverlayStore()

    const drawer = refreshDrawer(transport)
    await settle()
    pending[0].resolve(issue({ version: 5, labels: ['Shipped'] }))
    await drawer

    overlay.put(issue({ version: 4, labels: ['Stale'] }))

    expect(overlay.issueFor(7)?.version).toBe(5)
    expect(overlay.issueFor(7)?.labels).toEqual(['Shipped'])
  })

  it('keeps a newer issue the drawer put when an earlier board refresh answers last', async () => {
    const { transport, pending } = wire()
    const overlay = useIssueOverlayStore()

    const board = refreshBoard(transport)
    await settle()
    overlay.put(issue({ version: 5, labels: ['Shipped'] }))
    pending[0].resolve(issue({ version: 4, labels: ['Stale'] }))
    await board

    expect(overlay.issueFor(7)?.version).toBe(5)
    expect(overlay.issueFor(7)?.labels).toEqual(['Shipped'])
  })

  it('keeps a same-version issue the drawer put when an earlier board refresh answers last', async () => {
    const { transport, pending } = wire()
    const overlay = useIssueOverlayStore()

    const board = refreshBoard(transport)
    await settle()
    overlay.put(issue({ version: 4, sub_issues: { done: 2, total: 3 } }))
    pending[0].resolve(issue({ version: 4, sub_issues: { done: 1, total: 3 } }))
    await board

    expect(overlay.issueFor(7)?.sub_issues).toEqual({ done: 2, total: 3 })
  })

  it('hands the drawer the same-version issue put while its own refresh was on the wire', async () => {
    const { transport, pending } = wire()
    const overlay = useIssueOverlayStore()

    const drawer = refreshDrawer(transport)
    await settle()
    overlay.put(issue({ version: 4, sub_issues: { done: 2, total: 3 } }))
    pending[0].resolve(issue({ version: 4, sub_issues: { done: 1, total: 3 } }))
    const refreshed = await drawer

    expect(refreshed?.sub_issues).toEqual({ done: 2, total: 3 })
    expect(overlay.issueFor(7)?.sub_issues).toEqual({ done: 2, total: 3 })
  })

  it('takes a same-version answer to a refresh sent after the drawer put', async () => {
    const { transport, pending } = wire()
    const overlay = useIssueOverlayStore()

    overlay.put(issue({ version: 4, sub_issues: { done: 1, total: 3 } }))
    const board = refreshBoard(transport)
    await settle()
    const drawer = refreshDrawer(transport)
    await settle()
    pending[0].resolve(issue({ version: 4, sub_issues: { done: 2, total: 3 } }))
    await board
    expect(overlay.issueFor(7)?.sub_issues).toEqual({ done: 2, total: 3 })
    pending[1].resolve(issue({ version: 4, sub_issues: { done: 3, total: 3 } }))

    expect((await drawer)?.sub_issues).toEqual({ done: 3, total: 3 })
    expect(overlay.issueFor(7)?.sub_issues).toEqual({ done: 3, total: 3 })
  })

  it('takes every answer that is newer than the one cached', async () => {
    const { transport, pending } = wire()
    const overlay = useIssueOverlayStore()

    overlay.put(issue({ version: 3 }))
    const board = refreshBoard(transport)
    await settle()
    pending[0].resolve(issue({ version: 4, labels: ['Shipped'] }))
    await board
    overlay.put(issue({ version: 5, labels: ['Released'] }))

    expect(overlay.issueFor(7)?.version).toBe(5)
    expect(overlay.issueFor(7)?.labels).toEqual(['Released'])
  })

  it('keeps a cleared overlay empty when the remaining pages of an earlier refresh answer', async () => {
    const { transport, pages, reads } = pagedWire()
    const overlay = useIssueOverlayStore()

    const board = overlay.refreshForProject(transport, 1, [7, 8])
    await settle()
    overlay.clear()
    pages.shift()!({ issues: [held(7, 12)], next_cursor: '7' })
    await settle()
    pages.shift()?.({ issues: [held(8, 13)], next_cursor: null })
    await board

    expect(overlay.byTicketId).toEqual({})
    expect(reads).toHaveLength(1)
  })

  it('keeps a cleared overlay empty when the queued reads of an earlier refresh answer', async () => {
    const { transport, answers } = heldReads()
    const overlay = useIssueOverlayStore()

    const event = overlay.refreshIssues(transport, [7, 8, 9, 10, 11].map(named))
    await settle()
    expect(answers).toHaveLength(4)
    overlay.clear()
    while (answers.length > 0) {
      answers.shift()!()
      await settle()
    }
    await event

    expect(overlay.byTicketId).toEqual({})
  })

  it('keeps an issue put while a refresh waits for a read slot over that refresh answer', async () => {
    const { transport, answers } = heldReads({ labels: ['Stale'] })
    const overlay = useIssueOverlayStore()

    const event = overlay.refreshIssues(transport, [7, 8, 9, 10, 11].map(named))
    await settle()
    expect(answers).toHaveLength(4)
    overlay.put(held(11, 16, { version: 3, labels: ['Fresh'] }))
    while (answers.length > 0) {
      answers.shift()!()
      await settle()
    }
    await event

    expect(overlay.issueFor(11)?.labels).toEqual(['Fresh'])
  })

  it('keeps a later refresh answer when a later page of an earlier refresh answers after it', async () => {
    const { transport, pages } = pagedWire()
    const overlay = useIssueOverlayStore()

    const earlier = overlay.refreshForProject(transport, 1, [7, 8])
    await settle()
    const later = overlay.refreshForProject(transport, 1, [8])
    await settle()
    expect(pages).toHaveLength(2)

    pages[1]!({ issues: [held(8, 13, { labels: ['Newer'] })], next_cursor: null })
    await later
    pages[0]!({ issues: [held(7, 12)], next_cursor: '7' })
    await settle()
    expect(pages).toHaveLength(3)
    pages[2]!({ issues: [held(8, 13, { labels: ['Older'] })], next_cursor: null })
    await earlier

    expect(overlay.issueFor(8)?.labels).toEqual(['Newer'])
  })
})

function listing(records: readonly IssueRecord[], pageSize: number) {
  const reads: Array<{ name: string; request: unknown }> = []
  const transport = {
    query: (name: string, request: unknown) => {
      if (name === 'issue.list_milestones') return Promise.resolve({ milestones: [] })
      reads.push({ name, request })
      if (name !== 'issue.list') return Promise.reject(new Error(`unexpected query ${name}`))
      const { cursor } = request as { cursor?: string | null }
      const start = cursor
        ? records.findIndex((record) => String(record.ticket_id) === cursor) + 1
        : 0
      const page = records.slice(start, start + pageSize)
      const more = start + pageSize < records.length
      return Promise.resolve({
        issues: page,
        next_cursor: more ? String(page[page.length - 1]!.ticket_id) : null,
      } satisfies IssueListResponse)
    },
    command: () => Promise.reject(new Error('commands are forbidden here')),
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, reads }
}

function held(ticketId: number, number: number, overrides: Partial<IssueRecord> = {}): IssueRecord {
  return issue({
    ticket_id: ticketId,
    identifier: `CORE-${number}`,
    title: `Issue ${number}`,
    ...overrides,
  })
}

function named(ticketId: number) {
  return { ticketId, projectId: 1, projectCode: 'CORE', number: ticketId + 5 }
}

function pagedWire() {
  const pages: Array<(page: IssueListResponse) => void> = []
  const reads: unknown[] = []
  const transport = {
    query: (name: string, request: unknown) => {
      if (name === 'issue.list_milestones') return Promise.resolve({ milestones: [] })
      if (name !== 'issue.list') return Promise.reject(new Error(`unexpected query ${name}`))
      reads.push(request)
      return new Promise<IssueListResponse>((resolve) => pages.push(resolve))
    },
    command: () => Promise.reject(new Error('commands are forbidden here')),
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, pages, reads }
}

function heldReads(overrides: Partial<IssueRecord> = {}) {
  const answers: Array<() => void> = []
  const transport = {
    query: (name: string, request: unknown) => {
      if (name === 'issue.list_milestones') return Promise.resolve({ milestones: [] })
      if (name !== 'issue.get') return Promise.reject(new Error(`unexpected query ${name}`))
      const number = Number((request as { identifier: string }).identifier.split('-')[1])
      return new Promise<IssueRecord>((resolve) =>
        answers.push(() => resolve(held(number - 5, number, overrides))),
      )
    },
    command: () => Promise.reject(new Error('commands are forbidden here')),
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, answers }
}

describe('issue overlay reads', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('reads a project board through its paged issue list, never one issue at a time', async () => {
    const { transport, reads } = listing([held(9, 14), held(8, 13), held(7, 12)], 2)
    const overlay = useIssueOverlayStore()

    await overlay.refreshForProject(transport, 1, [7, 8, 9])

    expect(reads).toEqual([
      { name: 'issue.list', request: { project_id: 1, limit: 100 } },
      { name: 'issue.list', request: { project_id: 1, limit: 100, cursor: '8' } },
    ])
    expect([7, 8, 9].map((ticketId) => overlay.issueFor(ticketId)?.identifier)).toEqual([
      'CORE-12',
      'CORE-13',
      'CORE-14',
    ])
  })

  it('stops reading pages once every card it refreshes has been read', async () => {
    const { transport, reads } = listing([held(9, 14), held(8, 13), held(7, 12), held(6, 11)], 2)
    const overlay = useIssueOverlayStore()

    await overlay.refreshForProject(transport, 1, [8, 9])

    expect(reads).toHaveLength(1)
    expect(overlay.issueFor(9)?.identifier).toBe('CORE-14')
    expect(overlay.issueFor(8)?.identifier).toBe('CORE-13')
  })

  it('keeps at most four issue reads on the wire, across concurrent refreshes', async () => {
    const waiting: Array<() => void> = []
    const identifiers: string[] = []
    let onWire = 0
    let most = 0
    const transport = {
      query: (name: string, request: unknown) => {
        if (name === 'issue.list_milestones') return Promise.resolve({ milestones: [] })
        if (name !== 'issue.get') return Promise.reject(new Error(`unexpected query ${name}`))
        const { identifier } = request as { identifier: string }
        identifiers.push(identifier)
        onWire += 1
        most = Math.max(most, onWire)
        const number = Number(identifier.split('-')[1])
        return new Promise<IssueRecord>((resolve) => {
          waiting.push(() => {
            onWire -= 1
            resolve(held(number - 5, number))
          })
        })
      },
      command: () => Promise.reject(new Error('commands are forbidden here')),
      subscribe: () => () => undefined,
    } as unknown as ShellTransport
    const overlay = useIssueOverlayStore()
    const named = [7, 8, 9, 10, 11, 12].map((ticketId) => ({
      ticketId,
      projectId: 1,
      projectCode: 'CORE',
      number: ticketId + 5,
    }))

    const event = overlay.refreshIssues(transport, named)
    const drawer = overlay.refreshOne(transport, 1, 'CORE', 13, 18)
    await settle()
    expect(identifiers).toHaveLength(4)

    while (waiting.length > 0) {
      waiting.shift()!()
      await settle()
    }
    await event
    await drawer

    expect(most).toBe(4)
    expect(identifiers).toHaveLength(7)
    expect([7, 8, 9, 10, 11, 12, 13].map((ticketId) => overlay.issueFor(ticketId)?.ticket_id)).toEqual([
      7, 8, 9, 10, 11, 12, 13,
    ])
  })
})
