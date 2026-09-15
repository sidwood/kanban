// KAN-T139-AC1 (KAN-S4-US1): the Ticket editor reaches the operator
// through context and a shortcut alone — New Ticket on the board, an
// uncovered Story on planning, Edit in the drawer. No route, rail
// item, or palette row opens an editor destination.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import type { Pinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { ProjectListResponse, SpecCoverageMatrixResponse } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { PALETTE_NAVIGATION } from '../stores/palette-navigation'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import { harness, ticket } from '../test/shell-harness'
import App from '../App.vue'
import PlanningView from './PlanningView.vue'

const mounted: VueWrapper[] = []

beforeEach(() => {
  localStorage.clear()
  document.documentElement.classList.remove('dark')
})

afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

async function mountApp(transport: ShellTransport, path: string, pinia: Pinia) {
  await router.push(path)
  await router.isReady()
  const wrapper = mount(App, {
    attachTo: document.body,
    global: {
      plugins: [pinia, router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  mounted.push(wrapper)
  await flushPromises()
  return wrapper
}

describe('the Ticket editor entry points', () => {
  it('exposes no editor route, rail item, or palette row', () => {
    const paths = router.getRoutes().map((entry) => entry.path)
    expect(paths).not.toContain('/planning/tickets')
    expect(router.resolve('/planning/tickets').matched).toHaveLength(0)
    expect(PALETTE_NAVIGATION.map((item) => item.route)).not.toContain('/planning/tickets')
  })

  it('New ticket on the board opens the dialog on the scoped Project', async () => {
    const pinia = createPinia()
    const wrapper = await mountApp(
      harness({ tickets: [ticket()] }).transport,
      '/projects/1/board',
      pinia,
    )

    const button = wrapper.get('[data-testid="new-ticket"]')
    expect(button.attributes('href')).toBeUndefined()
    await button.trigger('click')
    await flushPromises()

    const dialog = useTicketDialogStore(pinia)
    expect(dialog.editorOpen).toBe(true)
    expect(dialog.editor).toMatchObject({ mode: 'create', projectId: 1 })
    expect(wrapper.find('[data-testid="ticket-dialog"]').exists()).toBe(true)
  })

  // KAN-T139-AC3: what the dialog saves is the core's, so a fresh
  // shell reads it back without the dialog's draft surviving anything.
  it('a Ticket created in the dialog survives a reload of the shell', async () => {
    const state = harness({ tickets: [ticket()] })
    const pinia = createPinia()
    const wrapper = await mountApp(state.transport, '/projects/1/board', pinia)

    await wrapper.get('[data-testid="new-ticket"]').trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="ticket-kind"]').setValue('task')
    await wrapper.find('[data-testid="ticket-title"]').setValue('Archive the old exports')
    await wrapper
      .find('[data-testid="ticket-completion-outcome-0"]')
      .setValue('The old exports are archived.')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    expect(useTicketDialogStore(pinia).editorOpen).toBe(false)
    const created = state.tickets[state.tickets.length - 1]!
    expect(created.title).toBe('Archive the old exports')

    // A fresh shell over the same core: the card is there because the
    // record is, not because anything was kept on the client.
    wrapper.unmount()
    mounted.splice(mounted.indexOf(wrapper), 1)
    const reloaded = await mountApp(state.transport, '/projects/1/board', createPinia())
    expect(reloaded.find(`[data-testid="kanban-card-${created.id}"]`).exists()).toBe(true)
  })

  it('the global shortcut opens quick Bug capture, and nothing else does', async () => {
    const pinia = createPinia()
    const wrapper = await mountApp(harness({ tickets: [ticket()] }).transport, '/board', pinia)
    const dialog = useTicketDialogStore(pinia)
    expect(dialog.quickBugOpen).toBe(false)

    window.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'b', metaKey: true, shiftKey: true }),
    )
    await flushPromises()

    expect(dialog.quickBugOpen).toBe(true)
    expect(wrapper.find('[data-testid="quick-bug-dialog"]').exists()).toBe(true)
  })
})

// The planning surface's own entry point: a Story no Ticket covers
// offers the Implementation that would cover it.
const project = {
  id: 4,
  code: 'CORE',
  name: 'Control plane',
  repository: '/repositories/kanban',
  seed_workspace: '/workspaces/kanban.seed',
  default_branch: 'main',
  herdr_session: 'kanban-main',
  herdr_workspace: 'kanban.seed',
  initiative_id: null,
  archived: false,
  counters: { plan: 1, spec: 2, ticket: 2 },
  version: 1,
} satisfies ProjectListResponse['projects'][number]

const matrix: SpecCoverageMatrixResponse = {
  spec_id: 1,
  version: 2,
  stories: [
    {
      story: 'CORE-S1-US1',
      claims: [{ ticket_id: 5, ticket_number: 5, outcome: 'Graphs record completely.' }],
    },
    { story: 'CORE-S1-US3', claims: [] },
  ],
}

function planningHarness(): ShellTransport {
  return {
    query: (name: string) => {
      if (name === 'project.list') return Promise.resolve({ projects: [project] })
      if (name === 'spec.list') {
        return Promise.resolve({
          specs: [
            {
              id: 1,
              project_id: 4,
              number: 1,
              name: 'Registration',
              execution: 'planned',
              plan_id: 1,
              version: 3,
            },
          ],
        })
      }
      if (name === 'spec.coverage.matrix') return Promise.resolve(matrix)
      if (name === 'plan.list') return Promise.resolve({ plans: [] })
      if (name === 'ticket.list') return Promise.resolve({ tickets: [] })
      if (name === 'ticket.graph.list') return Promise.resolve({ proposals: [] })
      return Promise.resolve({})
    },
    command: () => Promise.resolve({}),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
}

describe('an uncovered Story', () => {
  it('opens the dialog preset to an Implementation on that Spec and Story', async () => {
    const pinia = createPinia()
    setActivePinia(pinia)
    await router.push('/planning')
    await router.isReady()
    const wrapper = mount(PlanningView, {
      global: {
        plugins: [pinia, router],
        provide: { [kanbanTransportKey as symbol]: planningHarness() },
      },
    })
    mounted.push(wrapper)
    await flushPromises()

    // A covered Story offers nothing; only the gap does.
    expect(wrapper.find('[data-testid="coverage-cover-CORE-S1-US1"]').exists()).toBe(false)
    await wrapper.find('[data-testid="coverage-cover-CORE-S1-US3"]').trigger('click')
    await flushPromises()

    expect(useTicketDialogStore(pinia).editor).toMatchObject({
      mode: 'create',
      projectId: 4,
      specId: 1,
      kind: 'implementation',
      kindLocked: true,
      story: 'CORE-S1-US3',
    })
  })
})
