// Drawer footer: edit, park, and workflow moves (Linear issue model).
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { TicketRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import { harness, ticket } from '../test/shell-harness'
import BoardView from './BoardView.vue'

const implementation = (overrides: Partial<TicketRecord> = {}): TicketRecord =>
  ticket({
    id: 8,
    number: 13,
    kind: 'implementation',
    state: 'in_review',
    spec_id: 4,
    title: 'Serve the lifecycle command surface',
    slice: 'Serve the lifecycle command surface',
    criteria: [{ outcome: 'The commands are served.', stories: ['CORE-S4-US2'] }],
    subtype: null,
    mode: null,
    completion: [],
    profile: 'glm-implementer',
    version: 5,
    ...overrides,
  })

const mountedBoards: VueWrapper[] = []

async function openDrawer(state: ReturnType<typeof harness>, ticketId = 8) {
  await router.push('/projects/1/board')
  await router.isReady()
  const wrapper = mount(BoardView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: state.transport },
    },
    attachTo: document.body,
  })
  mountedBoards.push(wrapper)
  await flushPromises()
  await wrapper.find(`[data-testid="open-ticket-${ticketId}"]`).trigger('click')
  await flushPromises()
  return wrapper
}

async function click(testid: string): Promise<void> {
  const element = document.querySelector(`[data-testid="${testid}"]`)
  if (!(element instanceof HTMLElement)) throw new Error(`no ${testid}`)
  element.click()
  await flushPromises()
}

async function type(testid: string, value: string): Promise<void> {
  const element = document.querySelector(`[data-testid="${testid}"]`)
  if (
    !(element instanceof HTMLInputElement) &&
    !(element instanceof HTMLTextAreaElement) &&
    !(element instanceof HTMLSelectElement)
  ) {
    throw new Error(`no field ${testid}`)
  }
  element.value = value
  element.dispatchEvent(new Event('input'))
  element.dispatchEvent(new Event('change'))
  await flushPromises()
}

const issued = (state: ReturnType<typeof harness>, name: string): unknown[] =>
  state.command.mock.calls.filter((call) => call[0] === name).map((call) => call[1])

beforeEach(() => {
  localStorage.clear()
})

afterEach(() => {
  for (const wrapper of mountedBoards.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

describe('the drawer footer actions', () => {
  it('Edit opens the issue editor on the open Ticket', async () => {
    const state = harness({ tickets: [implementation()] })
    const pinia = createPinia()
    await router.push('/projects/1/board')
    await router.isReady()
    const wrapper = mount(BoardView, {
      global: {
        plugins: [pinia, router],
        provide: { [kanbanTransportKey as symbol]: state.transport },
      },
      attachTo: document.body,
    })
    mountedBoards.push(wrapper)
    await flushPromises()
    await wrapper.find('[data-testid="open-ticket-8"]').trigger('click')
    await flushPromises()

    await click('drawer-edit')

    const dialog = useTicketDialogStore(pinia)
    expect(dialog.editorOpen).toBe(true)
    expect(dialog.editor).toMatchObject({ mode: 'edit', ticketId: 8, projectId: 1 })
  })

  it('Park runs ticket.park at the open version and follows the record back', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)

    await click('drawer-park')

    expect(issued(state, 'ticket.park')[0]).toEqual({
      mutation: { optimistic_version: 5, idempotency_key: expect.any(String) },
      ticket_id: 8,
    })
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Parked')
    expect(document.querySelector('[data-testid="drawer-unpark"]')).not.toBeNull()
  })

  it.each([
    {
      command: 'Park',
      from: 'ready' as const,
      to: 'Parked',
      act: () => click('drawer-park'),
    },
    {
      command: 'Unpark',
      from: 'parked' as const,
      to: 'Ready',
      act: () => click('drawer-unpark'),
    },
    {
      command: 'a workflow move',
      from: 'ready' as const,
      to: 'Active',
      act: () => type('drawer-move', 'Active'),
    },
    {
      command: 'emergency recovery',
      from: 'active' as const,
      to: 'Ready',
      act: async () => {
        await click('drawer-recover')
        await type('recovery-to', 'ready')
        await type('recovery-who', 'Sid')
        await type('recovery-why', 'The run died holding the lane.')
        await click('recovery-submit')
      },
    },
  ])('after $command the issue panel shows the new state and saves at the new version', async ({ from, to, act }) => {
    const state = harness({ tickets: [implementation({ state: from })] })
    await openDrawer(state)

    await act()

    expect(
      document.querySelector('[data-testid="issue-history-row-0"] td')?.textContent?.trim(),
    ).toBe(to)
    await click('issue-description-edit')
    await type('issue-description-input', 'The commands are served over the socket.')
    await click('issue-description-save')

    expect(issued(state, 'issue.save').at(-1)).toEqual({
      mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
      identifier: 'CORE-13',
      description: 'The commands are served over the socket.',
    })
    expect(document.querySelector('[data-testid="issue-description-input"]')).toBeNull()
  })

  it('offers every workflow state except the current one in the move select', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)

    const move = document.querySelector('[data-testid="drawer-move"]') as HTMLSelectElement
    expect(move).not.toBeNull()
    expect([...move.options].map((option) => option.value)).toContain('Active')
    expect([...move.options].map((option) => option.value)).not.toContain('Ready')
  })

  it('moves through issue.save with a workflow state name', async () => {
    const state = harness({ tickets: [ticket({ id: 7, state: 'ready' })] })
    await openDrawer(state, 7)

    await type('drawer-move', 'Active')

    expect(issued(state, 'issue.save')[0]).toMatchObject({
      identifier: 'CORE-12',
      state: 'Active',
    })
  })
})
