// KAN-T30-AC2: the detail drawer shows Linear issue detail (U2).
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { TicketRecord } from '@kanban/contracts'
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

describe('issue drawer', () => {
  it('loads issue detail through ticket.get and issue.get when it opens', async () => {
    const held = ticket({
      id: 8,
      number: 13,
      kind: 'implementation',
      state: 'active',
      title: 'Serve the lifecycle command surface',
      slice: 'Serve the lifecycle command surface',
    })
    const { query } = await openDrawer([held], 8)

    expect(query).toHaveBeenCalledWith('ticket.get', { ticket_id: 8 })
    expect(query).toHaveBeenCalledWith('issue.get', { identifier: 'CORE-13' })
    expect(document.querySelector('[data-testid="issue-detail-panel"]')).toBeTruthy()
    expect(document.querySelector('[data-testid="drawer-criteria"]')).toBeFalsy()
  })
})
