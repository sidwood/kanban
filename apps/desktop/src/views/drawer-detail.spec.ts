// Issue drawer detail (U2): authoritative issue.get, not legacy ticket sections.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { IssueRecord, TicketRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import { harness, ticket } from '../test/shell-harness'
import BoardView from './BoardView.vue'

const mountedBoards: VueWrapper[] = []

async function openDrawer(tickets: TicketRecord[], ticketId: number) {
  const shell = harness({ tickets })
  await router.push('/projects/1/board')
  await router.isReady()
  const wrapper = mount(BoardView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: shell.transport },
    },
    attachTo: document.body,
  })
  mountedBoards.push(wrapper)
  await flushPromises()
  await wrapper.find(`[data-testid="open-ticket-${ticketId}"]`).trigger('click')
  await flushPromises()
  return { wrapper, query: shell.query }
}

beforeEach(() => {
  localStorage.clear()
})

afterEach(() => {
  for (const wrapper of mountedBoards.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

describe('the issue drawer detail', () => {
  it('loads issue.get and shows the issue panel, not legacy criteria', async () => {
    const held = ticket({
      id: 8,
      number: 13,
      kind: 'implementation',
      state: 'active',
      title: 'Serve the lifecycle command surface',
      slice: 'Serve the lifecycle command surface',
    })
    const { query } = await openDrawer([held], 8)

    expect(query).toHaveBeenCalledWith('issue.get', { identifier: 'CORE-13' })
    expect(document.querySelector('[data-testid="issue-detail-panel"]')).toBeTruthy()
    expect(document.querySelector('[data-testid="drawer-criteria"]')).toBeFalsy()
    expect(document.querySelector('[data-testid="drawer-timeline"]')).toBeFalsy()
  })

  it('shows state history from the issue record', async () => {
    await openDrawer([ticket({ id: 7, number: 12, state: 'ready' })], 7)
    expect(document.querySelector('[data-testid="issue-state-history"]')).toBeTruthy()
  })

  it('keeps showing the newer cached issue when the drawer read answers after it', async () => {
    const record = (overrides: Partial<IssueRecord>): IssueRecord => ({
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
    })
    let loaded = false
    const answers: Array<(issue: IssueRecord) => void> = []
    const shell = harness({
      tickets: [ticket({ id: 7, number: 12, state: 'ready' })],
      override: (name) => {
        if (name !== 'issue.get') return undefined
        if (!loaded) {
          loaded = true
          return Promise.resolve(record({ labels: ['Queued'] }))
        }
        return new Promise<IssueRecord>((resolve) => answers.push(resolve))
      },
    })
    await router.push('/projects/1/board')
    await router.isReady()
    const wrapper = mount(BoardView, {
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: shell.transport },
      },
      attachTo: document.body,
    })
    mountedBoards.push(wrapper)
    await flushPromises()

    await wrapper.find('[data-testid="open-ticket-7"]').trigger('click')
    await flushPromises()
    shell.emit({ sequence: 4, event_type: 'ticket.edited', payload: shell.tickets[0] })
    await flushPromises()
    expect(answers).toHaveLength(2)

    answers[1](record({ version: 5, labels: ['Shipped'] }))
    await flushPromises()
    answers[0](record({ version: 4, labels: ['Stale'] }))
    await flushPromises()

    expect(
      document.querySelector('[data-testid="issue-detail-labels"]')?.textContent?.trim(),
    ).toBe('Shipped')
  })
})
