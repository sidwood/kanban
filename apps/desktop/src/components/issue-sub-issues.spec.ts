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

function transportFor(issue: IssueRecord): ShellTransport {
  return {
    query: ((name, request) => {
      if (name === 'ticket.get') {
        return Promise.resolve({
          id: issue.ticket_id,
          project_id: issue.project_id,
          number: 1,
          kind: 'task',
          priority: 'normal',
          state: 'active',
          spec_id: null,
          title: issue.title,
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
        if (body.identifier === issue.identifier) {
          return Promise.resolve(issue)
        }
        return Promise.reject({ code: 'not_found', message: 'issue' })
      }
      return Promise.reject({ code: 'not_found', message: name })
    }) as ShellTransport['query'],
    command: (() => Promise.reject({ code: 'not_found', message: 'command' })) as ShellTransport['command'],
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
})
