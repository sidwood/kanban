// The Ticket editor left `/planning/tickets` behind as a dialog
// (KAN-T139-AC1), but the two per-Ticket execution surfaces that page
// also carried are not Ticket editors and keep their home on the
// planning surface: a Task's activation schedule and an assignment's
// review stages, both reading the picked Project's Tickets.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, describe, expect, it } from 'vitest'
import type { ProjectListResponse, TicketRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import ReviewConfigEditor from '../components/ReviewConfigEditor.vue'
import ScheduleEditor from '../components/ScheduleEditor.vue'
import PlanningView from './PlanningView.vue'

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
  counters: { plan: 1, spec: 1, ticket: 1 },
  version: 1,
} satisfies ProjectListResponse['projects'][number]

const tickets: TicketRecord[] = [
  {
    id: 3,
    project_id: 4,
    number: 19,
    kind: 'task',
    priority: 'low',
    state: 'draft',
    spec_id: null,
    title: 'Archive the old register',
    slice: null,
    criteria: [],
    bug: null,
    subtype: 'migration',
    mode: 'agent',
    completion: ['The register moves.'],
    scheduled_for: null,
    due: null,
    profile: null,
    version: 1,
  },
]

function harness() {
  const operations: Array<{ name: string; request: unknown }> = []
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ name, request })
      if (name === 'project.list') return Promise.resolve({ projects: [project] })
      if (name === 'spec.list') return Promise.resolve({ specs: [] })
      if (name === 'plan.list') return Promise.resolve({ plans: [] })
      if (name === 'ticket.list') return Promise.resolve({ tickets })
      if (name === 'ticket.graph.list') return Promise.resolve({ proposals: [] })
      return Promise.resolve({})
    },
    command: () => Promise.resolve({}),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, operations }
}

const mounted: VueWrapper[] = []

afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
})

async function mountView(transport: ShellTransport) {
  await router.push('/planning')
  await router.isReady()
  const wrapper = mount(PlanningView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  mounted.push(wrapper)
  await flushPromises()
  return wrapper
}

describe('the planning surface keeps the per-Ticket execution editors', () => {
  it('offers the schedule editor over the picked Project s Tickets', async () => {
    setActivePinia(createPinia())
    const wrapper = await mountView(harness().transport)

    const schedule = wrapper.findComponent(ScheduleEditor)
    expect(schedule.exists()).toBe(true)
    expect(schedule.props('tickets')).toEqual(tickets)
    expect(schedule.props('projectCode')).toBe('CORE')
  })

  it('re-reads the Project s Tickets after a schedule lands', async () => {
    setActivePinia(createPinia())
    const state = harness()
    const wrapper = await mountView(state.transport)
    const before = state.operations.filter((entry) => entry.name === 'ticket.list').length

    wrapper.findComponent(ScheduleEditor).vm.$emit('saved', tickets[0]!)
    await flushPromises()

    expect(state.operations.filter((entry) => entry.name === 'ticket.list')).toHaveLength(
      before + 1,
    )
  })

  it('offers the review configuration editor over the same Tickets', async () => {
    setActivePinia(createPinia())
    const wrapper = await mountView(harness().transport)

    const reviews = wrapper.findComponent(ReviewConfigEditor)
    expect(reviews.exists()).toBe(true)
    expect(reviews.props('tickets')).toEqual(tickets)
    expect(reviews.props('projectCode')).toBe('CORE')
  })
})
