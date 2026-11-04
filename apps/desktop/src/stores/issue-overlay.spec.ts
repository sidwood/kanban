import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it } from 'vitest'
import type { IssueRecord } from '@kanban/contracts'
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
  const transport = {
    query: (name: string) => {
      if (name === 'issue.list_milestones') return Promise.resolve({ milestones: [] })
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
  return { transport, pending }
}

const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 0))

function refreshBoard(transport: ShellTransport): Promise<void> {
  return useIssueOverlayStore().refreshForProject(transport, 1, 'CORE', [7], { 7: 12 })
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
})
