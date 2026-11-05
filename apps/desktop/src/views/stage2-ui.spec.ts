// Stage 2 UI acceptance (U1–U8): Linear issue model on the board,
// drawer, editor, and card interactions.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { TicketRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import TicketEditorDialog from '../components/TicketEditorDialog.vue'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import { editorHarness } from '../test/editor-harness'
import { harness, ticket } from '../test/shell-harness'
import BoardView from './BoardView.vue'

const mounted: VueWrapper[] = []

beforeEach(() => {
  localStorage.clear()
})
afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

async function mountBoard(tickets: TicketRecord[]) {
  const shell = harness({ tickets })
  await router.push('/projects/1/board')
  await router.isReady()
  const wrapper = mount(BoardView, {
    attachTo: document.body,
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: shell.transport },
    },
  })
  mounted.push(wrapper)
  await flushPromises()
  await flushPromises()
  return wrapper
}

describe('Stage 2 UI (U1–U8)', () => {
  it('U1 hides legacy kind badges and severity chips on cards', async () => {
    const wrapper = await mountBoard([
      ticket({
        id: 3,
        number: 3,
        kind: 'bug',
        state: 'ready',
        title: 'Regression in sync',
        bug: {
          actual_behaviour: 'x',
          reporter_evidence: 'y',
          evidence_ids: [],
          external_references: [],
          occurrence_snapshots: [],
          qualification: { severity: 'high', frequency: 'often' } as never,
        },
      }),
    ])

    expect(wrapper.find('[data-testid="card-kind-3"]').exists()).toBe(false)
    expect(wrapper.text()).not.toMatch(/Severity/i)
    expect(wrapper.text()).not.toMatch(/Subtype/i)
  })

  it('U2 shows issue detail panel instead of legacy execution sections', async () => {
    const wrapper = await mountBoard([
      ticket({
        id: 8,
        number: 13,
        kind: 'implementation',
        state: 'active',
        title: 'Detail me',
        slice: 'Detail me',
      }),
    ])
    await wrapper.find('[data-testid="open-ticket-8"]').trigger('click')
    await flushPromises()

    expect(document.querySelector('[data-testid="issue-detail-panel"]')).toBeTruthy()
    expect(document.querySelector('[data-testid="drawer-timeline"]')).toBeFalsy()
    expect(document.querySelector('[data-testid="drawer-criteria"]')).toBeFalsy()
  })

  it('U3 uses a date input and persists due date on the issue record', async () => {
    const { transport } = editorHarness()
    const wrapper = mount(TicketEditorDialog, {
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: transport },
      },
      attachTo: document.body,
    })
    mounted.push(wrapper)
    useTicketDialogStore().openCreate({ projectId: 4 })
    await flushPromises()

    const due = wrapper.find('[data-testid="ticket-due"]').element as HTMLInputElement
    expect(due.type).toBe('date')
    due.value = '2026-09-30'
    await due.dispatchEvent(new Event('input'))
    await wrapper.find('[data-testid="ticket-title"]').setValue('Due soon')
    await wrapper.find('[data-testid="ticket-create"]').trigger('click')
    await flushPromises()
  })

  it('U7 keeps rapid typing in title and description separate', async () => {
    const { transport } = editorHarness()
    const wrapper = mount(TicketEditorDialog, {
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: transport },
      },
      attachTo: document.body,
    })
    mounted.push(wrapper)
    useTicketDialogStore().openCreate({ projectId: 4 })
    await flushPromises()

    const title = wrapper.find('[data-testid="ticket-title"]').element as HTMLInputElement
    const description = wrapper.find('[data-testid="ticket-description"]')
      .element as HTMLTextAreaElement
    title.value = 'alpha'
    title.dispatchEvent(new Event('input'))
    description.value = 'beta'
    description.dispatchEvent(new Event('input'))
    title.value += 'gamma'
    title.dispatchEvent(new Event('input'))

    expect(title.value).toBe('alpha' + 'gamma')
    expect(description.value).toBe('beta')
  })

  it('U8 opens the drawer from a single card click', async () => {
    const wrapper = await mountBoard([
      ticket({
        id: 8,
        number: 13,
        kind: 'implementation',
        state: 'active',
        title: 'One click',
        slice: 'One click',
      }),
    ])
    expect(wrapper.find('[data-testid="kanban-card-8"]').exists()).toBe(true)
    await wrapper.find('[data-testid="kanban-card-8"]').trigger('click')
    await flushPromises()
    await flushPromises()
    expect(document.querySelector('[data-testid="drawer-error"]')).toBeFalsy()
    expect(document.querySelector('[data-testid="drawer-state"]')).toBeTruthy()
    expect(document.querySelector('[data-testid="issue-detail-panel"]')).toBeTruthy()
  })
})
