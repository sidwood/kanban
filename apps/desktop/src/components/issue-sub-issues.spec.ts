import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { IssueRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import IssueSubIssues from './IssueSubIssues.vue'

const mounted: ReturnType<typeof mount>[] = []

function issueRecord(overrides: Partial<IssueRecord> = {}): IssueRecord {
  return {
    identifier: 'CORE-1',
    url: 'kanban://issue/CORE-1',
    ticket_id: 1,
    project_id: 4,
    title: 'Parent',
    description: '',
    state: 'Active',
    state_type: 'started',
    priority: 3,
    labels: [],
    relations: [],
    state_history: [],
    version: 1,
    created_at: '2026-01-01T00:00:00.000000Z',
    updated_at: '2026-01-01T00:00:00.000000Z',
    children: [
      {
        identifier: 'CORE-2',
        title: 'Done child',
        state: 'Done',
        sort_order: 0,
      },
      {
        identifier: 'CORE-3',
        title: 'Open child',
        state: 'Active',
        sort_order: 1,
      },
    ],
    ...overrides,
  }
}

function transportFor(issue: IssueRecord, onSave?: (body: Record<string, unknown>) => void): ShellTransport {
  let current = issue
  return {
    query: ((name, request) => {
      if (name === 'ticket.get') {
        return Promise.resolve({
          id: current.ticket_id,
          project_id: current.project_id,
          number: 1,
          kind: 'task',
          priority: 'normal',
          state: 'active',
          spec_id: null,
          title: current.title,
          slice: null,
          criteria: [],
          bug: null,
          subtype: null,
          mode: null,
          completion: [],
          scheduled_for: null,
          due: null,
          profile: null,
          version: 1,
        })
      }
      if (name === 'issue.get') {
        const body = request as { identifier?: string }
        if (body.identifier === current.identifier) {
          return Promise.resolve(current)
        }
        const child = current.children?.find((entry) => entry.identifier === body.identifier)
        if (child) {
          return Promise.resolve({
            ...current,
            identifier: child.identifier,
            title: child.title,
            state: child.state,
            children: [],
            version: 1,
          })
        }
        return Promise.reject({ code: 'not_found', message: 'issue' })
      }
      return Promise.reject({ code: 'not_found', message: name })
    }) as ShellTransport['query'],
    command: ((name, request) => {
      if (name === 'issue.save') {
        const body = request as Record<string, unknown>
        onSave?.(body)
        const identifier = body.identifier as string
        const sortOrder = body.sort_order as number | undefined
        if (identifier != null && sortOrder != null && current.children) {
          const next = current.children.map((child) =>
            child.identifier === identifier ? { ...child, sort_order: sortOrder } : child,
          )
          next.sort((a, b) => a.sort_order - b.sort_order)
          current = { ...current, children: next }
        }
        return Promise.resolve({ identifier, ticket_id: 1, url: `kanban://issue/${identifier}` })
      }
      return Promise.reject({ code: 'not_found', message: name })
    }) as ShellTransport['command'],
    subscribe: (() => () => {}) as ShellTransport['subscribe'],
    onConnectionChange: () => () => {},
  }
}

beforeEach(() => {
  setActivePinia(createPinia())
})

afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
})

function mountWith(issue: IssueRecord, onSave?: (body: Record<string, unknown>) => void) {
  const wrapper = mount(IssueSubIssues, {
    props: {
      projectCode: 'CORE',
      ticketId: 1,
      projectId: 4,
    },
    attachTo: document.body,
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transportFor(issue, onSave) },
    },
  })
  mounted.push(wrapper)
  return wrapper
}

// Stored sort values are not positions: other writers set them freely
// and a child moving to another parent leaves a gap.
function spacedChildren(): IssueRecord {
  return issueRecord({
    children: [
      { identifier: 'CORE-2', title: 'First', state: 'Active', sort_order: 10 },
      { identifier: 'CORE-3', title: 'Second', state: 'Active', sort_order: 20 },
      { identifier: 'CORE-4', title: 'Third', state: 'Active', sort_order: 30 },
      { identifier: 'CORE-5', title: 'Fourth', state: 'Active', sort_order: 40 },
    ],
  })
}

function rowOrder(wrapper: ReturnType<typeof mount>): string[] {
  return wrapper
    .findAll('[data-testid^="drawer-sub-issue-row-"]')
    .map((row) => (row.attributes('data-testid') ?? '').replace('drawer-sub-issue-row-', ''))
}

describe('IssueSubIssues progress', () => {
  it('shows how many children are done (Done, Cancelled, Superseded, Duplicate)', async () => {
    const wrapper = mount(IssueSubIssues, {
      props: {
        projectCode: 'CORE',
        ticketId: 1,
        projectId: 4,
      },
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: transportFor(issueRecord()) },
      },
    })
    mounted.push(wrapper)
    await flushPromises()

    expect(wrapper.find('[data-testid="drawer-sub-issue-progress"]').text()).toBe('1 of 2 done')
  })

  it('swaps the stored sort values of a child moved down, gaps and all', async () => {
    const saves: Record<string, unknown>[] = []
    const wrapper = mountWith(spacedChildren(), (body) => saves.push(body))
    await flushPromises()

    await wrapper.find('[data-testid="drawer-sub-issue-down-CORE-3"]').trigger('click')
    await flushPromises()

    expect(saves.map(({ identifier, sort_order }) => ({ identifier, sort_order }))).toEqual([
      { identifier: 'CORE-3', sort_order: 30 },
      { identifier: 'CORE-4', sort_order: 20 },
    ])
    expect(rowOrder(wrapper)).toEqual(['CORE-2', 'CORE-4', 'CORE-3', 'CORE-5'])
  })

  it('swaps the stored sort values of a child moved up, gaps and all', async () => {
    const saves: Record<string, unknown>[] = []
    const wrapper = mountWith(spacedChildren(), (body) => saves.push(body))
    await flushPromises()

    await wrapper.find('[data-testid="drawer-sub-issue-up-CORE-5"]').trigger('click')
    await flushPromises()

    expect(saves.map(({ identifier, sort_order }) => ({ identifier, sort_order }))).toEqual([
      { identifier: 'CORE-5', sort_order: 30 },
      { identifier: 'CORE-4', sort_order: 40 },
    ])
    expect(rowOrder(wrapper)).toEqual(['CORE-2', 'CORE-3', 'CORE-5', 'CORE-4'])
  })
})
