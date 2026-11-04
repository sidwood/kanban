// Issue drawer detail (U2): authoritative issue.get, not legacy ticket sections.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { IssueRecord, ReviewExecutionRecord, TicketRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import { useTicketDetailStore } from '../stores/ticket-detail'
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
    const answers: Array<(issue: IssueRecord) => void> = []
    const shell = harness({
      tickets: [ticket({ id: 7, number: 12, state: 'ready' })],
      override: (name) => {
        if (name === 'issue.list') {
          return Promise.resolve({ issues: [record({ labels: ['Queued'] })], next_cursor: null })
        }
        if (name !== 'issue.get') return undefined
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

  it('waits its turn behind the board refresh reads, keeping four issue reads on the wire', async () => {
    const answers: Array<() => void> = []
    let onWire = 0
    let most = 0
    const shell = harness({
      tickets: [7, 8, 9, 10, 11].map((id) =>
        ticket({ id, number: id + 5, state: 'ready', title: `Issue ${id + 5}` }),
      ),
      override: (name, request) => {
        if (name !== 'issue.get') return undefined
        const { identifier } = request as { identifier: string }
        const number = Number(identifier.split('-')[1])
        onWire += 1
        most = Math.max(most, onWire)
        return new Promise<IssueRecord>((resolve) =>
          answers.push(() => {
            onWire -= 1
            resolve({
              identifier,
              url: `kanban://issue/${identifier}`,
              ticket_id: number - 5,
              project_id: 1,
              title: `Issue ${number}`,
              description: '',
              state: 'Ready',
              state_type: 'backlog',
              priority: 3,
              labels: [identifier],
              relations: [],
              state_history: [],
              version: 4,
              created_at: '2026-01-01T00:00:00Z',
              updated_at: '2026-01-01T00:00:00Z',
            })
          }),
        )
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

    for (const [index, held] of shell.tickets.slice(0, 4).entries()) {
      shell.emit({ sequence: 4 + index, event_type: 'ticket.edited', payload: held })
    }
    await flushPromises()
    expect(answers).toHaveLength(4)

    await wrapper.find('[data-testid="open-ticket-11"]').trigger('click')
    await flushPromises()
    expect(answers).toHaveLength(4)

    while (answers.length > 0) {
      answers.shift()!()
      await flushPromises()
    }

    expect(most).toBe(4)
    expect(shell.query).toHaveBeenCalledWith('issue.get', { identifier: 'CORE-16' })
    expect(
      document.querySelector('[data-testid="issue-detail-labels"]')?.textContent?.trim(),
    ).toBe('CORE-16')
  })

  it.each([
    ['answers', true],
    ['fails', false],
  ])('keeps a late issue read that %s for the Ticket the operator left off the Ticket now on show', async (_outcome, answers) => {
    const left = ticket({
      id: 8,
      number: 13,
      kind: 'implementation',
      state: 'in_review',
      title: null,
      slice: 'Serve the lifecycle command surface',
      version: 5,
    })
    const shown = ticket({ id: 9, number: 14, title: 'Clone guard misses a dirty tree' })
    // The Ticket the operator leaves has a human review slot waiting,
    // so a detail read for it would offer that slot a decision.
    const waiting: ReviewExecutionRecord = {
      id: 55,
      project_id: 1,
      ticket_id: 8,
      submission_id: 9,
      configuration_version: 2,
      tip: 'f00dcafe',
      status: 'in_progress',
      version: 6,
      bounce: null,
      stages: [
        {
          index: 0,
          status: 'waiting',
          slots: [
            {
              id: 72,
              occupant: { kind: 'human' },
              requirement: 'required',
              requested: null,
              effective: null,
              fallback_path: [],
              dispatch_request_id: null,
              verdict: null,
            },
          ],
        },
      ],
    }
    let holding = false
    let releaseLeft: (() => void) | null = null
    const shell = harness({
      tickets: [left, shown],
      reviews: [waiting],
      override: (name, request) => {
        if (name !== 'issue.get' || !holding) return undefined
        if ((request as { identifier: string }).identifier !== 'CORE-13') return undefined
        return new Promise<IssueRecord>((resolve, reject) => {
          releaseLeft = () =>
            answers
              ? resolve({
                  identifier: 'CORE-13',
                  url: 'kanban://issue/CORE-13',
                  ticket_id: 8,
                  project_id: 1,
                  title: 'Serve the lifecycle command surface',
                  description: 'Serve the lifecycle command surface',
                  state: 'In Review',
                  state_type: 'started',
                  priority: 3,
                  labels: [],
                  relations: [],
                  state_history: [],
                  version: 5,
                  created_at: '2026-01-01T00:00:00Z',
                  updated_at: '2026-01-01T00:00:00Z',
                })
              : reject({ code: 'unavailable', message: 'the issue read failed' })
        })
      },
    })
    const pinia = createPinia()
    await router.push('/projects/1/board')
    await router.isReady()
    const wrapper = mount(BoardView, {
      global: {
        plugins: [pinia, router],
        provide: { [kanbanTransportKey as symbol]: shell.transport },
      },
      attachTo: document.body,
    })
    mountedBoards.push(wrapper)
    await flushPromises()

    holding = true
    await wrapper.find('[data-testid="open-ticket-8"]').trigger('click')
    await flushPromises()
    expect(releaseLeft).not.toBeNull()
    await wrapper.find('[data-testid="open-ticket-9"]').trigger('click')
    await flushPromises()
    releaseLeft!()
    await flushPromises()

    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-14')
    expect(
      document.querySelector('[data-testid="issue-description-body"]')?.textContent?.trim(),
    ).toBe('Clone guard misses a dirty tree')
    expect(useTicketDetailStore(pinia).ticketId).toBe(9)
    expect(useTicketDetailStore(pinia).review).toBeNull()
    expect(document.querySelector('[data-testid="drawer-review-decision"]')).toBeNull()
  })
})
