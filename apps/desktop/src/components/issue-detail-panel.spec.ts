import { flushPromises, mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, describe, expect, it } from 'vitest'
import type { CommentListItem, IssueRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import IssueDetailPanel from './IssueDetailPanel.vue'

const mounted: ReturnType<typeof mount>[] = []

function issue(overrides: Partial<IssueRecord> = {}): IssueRecord {
  return {
    identifier: 'CORE-8',
    url: 'kanban://issue/CORE-8',
    ticket_id: 8,
    project_id: 1,
    title: 'Host',
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
    children: [],
    ...overrides,
  }
}

function transport(comments: CommentListItem[]): ShellTransport {
  let list = [...comments]
  return {
    query: ((name, request) => {
      if (name === 'comment.list') {
        return Promise.resolve({ comments: list, next_cursor: null })
      }
      if (name === 'comment.revisions') {
        const { comment_id } = request as { comment_id: number }
        return Promise.resolve({
          comment: {
            id: comment_id,
            project_id: 1,
            target: { kind: 'ticket', id: '8' },
            text: 'Root',
            version: 2,
          },
          revisions: [],
        })
      }
      return Promise.reject({ code: 'not_found', message: name })
    }) as ShellTransport['query'],
    command: ((name, request) => {
      if (name === 'comment.create') {
        const body = request as {
          text: string
          parent_comment_id?: number | null
        }
        const created: CommentListItem = {
          id: list.length + 1,
          author: 'operator',
          body: body.text,
          created_at: '2026-01-02T00:00:00.000000Z',
          parent_comment_id: body.parent_comment_id ?? null,
          version: 1,
        }
        list = [...list, created]
        return Promise.resolve(created)
      }
      if (name === 'comment.resolve') {
        const body = request as { comment_id: number; resolved: boolean }
        list = list.map((entry) =>
          entry.id === body.comment_id
            ? {
                ...entry,
                resolved_at: body.resolved ? '2026-01-03T00:00:00.000000Z' : null,
              }
            : entry,
        )
        return Promise.resolve(body)
      }
      return Promise.reject({ code: 'not_found', message: name })
    }) as ShellTransport['command'],
    subscribe: (() => () => {}) as ShellTransport['subscribe'],
    onConnectionChange: () => () => {},
  }
}

afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
})

describe('IssueDetailPanel comments', () => {
  it('threads replies and resolves root comments', async () => {
    const root: CommentListItem = {
      id: 1,
      author: 'lin',
      body: 'Root note',
      created_at: '2026-01-01T00:00:00.000000Z',
      version: 2,
    }
    const wrapper = mount(IssueDetailPanel, {
      props: { issue: issue(), projectId: 1, projectCode: 'CORE' },
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: transport([root]) },
      },
    })
    mounted.push(wrapper)
    await flushPromises()

    await wrapper.find('[data-testid="issue-comment-reply-1"]').trigger('click')
    await wrapper.find('[data-testid="issue-comment-input"]').setValue('Nested reply')
    await wrapper.find('[data-testid="issue-comment-post"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-testid="issue-comment-replies-1"]').text()).toContain('Nested reply')

    await wrapper.find('[data-testid="issue-comment-resolve-1"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-testid="issue-comment-resolved-badge"]').exists()).toBe(true)
  })

  it('shows an imported comment under the author the core lists for it', async () => {
    // The core lists a comment whose author was never recorded under
    // the `Unknown author` placeholder; the panel shows that name like
    // any other, with no special case of its own.
    const imported: CommentListItem = {
      id: 1,
      author: 'Unknown author',
      body: 'Imported note',
      created_at: '2020-06-03T09:00:00.000000Z',
      version: 1,
    }
    const wrapper = mount(IssueDetailPanel, {
      props: { issue: issue(), projectId: 1, projectCode: 'CORE' },
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: transport([imported]) },
      },
    })
    mounted.push(wrapper)
    await flushPromises()

    const card = wrapper.find('[data-testid="issue-comment-1"]')
    expect(card.find('span').text()).toBe('Unknown author')
    expect(card.text()).toContain('Imported note')

    await wrapper.find('[data-testid="issue-comment-reply-1"]').trigger('click')
    await wrapper.find('[data-testid="issue-comment-input"]').setValue('Answering the import')
    await wrapper.find('[data-testid="issue-comment-post"]').trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="issue-comment-resolve-1"]').trigger('click')
    await flushPromises()

    const replies = wrapper.find('[data-testid="issue-comment-replies-1"]')
    expect(replies.text()).toContain('operator')
    expect(replies.text()).toContain('Answering the import')
    expect(wrapper.find('[data-testid="issue-comment-resolved-badge"]').exists()).toBe(true)
  })
})
