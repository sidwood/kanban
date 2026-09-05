import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it } from 'vitest'
import type {
  ProjectListResponse,
  SpecListResponse,
  TicketListResponse,
  TicketRecord,
} from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import TicketEditorView from './TicketEditorView.vue'

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
  counters: { plan: 2, spec: 3, ticket: 0 },
  version: 1,
}

const spec = {
  id: 7,
  project_id: 4,
  number: 1,
  name: 'Registration',
  execution: 'planned' as const,
  plan_id: null,
  version: 3,
}

const tickets = [
  {
    id: 1,
    project_id: 4,
    number: 17,
    kind: 'implementation' as const,
    priority: 'high' as const,
    state: 'draft' as const,
    spec_id: 7,
    title: null,
    slice: 'Spec authoring creates content versions end to end',
    criteria: [{ outcome: 'Specs mint unique numbers.', stories: ['CORE-S1-US1'] }],
    version: 1,
  },
  {
    id: 2,
    project_id: 4,
    number: 18,
    kind: 'bug' as const,
    priority: 'urgent' as const,
    state: 'draft' as const,
    spec_id: null,
    title: 'Landing drops the integration branch',
    slice: null,
    criteria: [],
    version: 1,
  },
  {
    id: 3,
    project_id: 4,
    number: 19,
    kind: 'task' as const,
    priority: 'low' as const,
    state: 'draft' as const,
    spec_id: 7,
    title: 'Archive the old register',
    slice: null,
    criteria: [],
    version: 1,
  },
] satisfies TicketRecord[]

// A transport steered per operation name, recording every command.
function harness() {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  const answers: Record<string, unknown> = {
    'project.list': { projects: [project] } satisfies ProjectListResponse,
    'spec.list': { specs: [spec] } satisfies SpecListResponse,
    'ticket.list': { tickets } satisfies TicketListResponse,
  }
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      return Promise.resolve(answers[name])
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      return Promise.resolve(answers[name] ?? tickets[0])
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, operations, answers }
}

async function mountView(transport: ShellTransport) {
  const wrapper = mount(TicketEditorView, {
    global: {
      plugins: [createPinia()],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return wrapper
}

describe('TicketEditorView', () => {
  it('lists every ticket of the first project with its minted identity', async () => {
    setActivePinia(createPinia())
    const wrapper = await mountView(harness().transport)

    expect(wrapper.find('[data-testid="ticket-project"]').element.textContent).toContain('CORE')
    const list = wrapper.find('[data-testid="ticket-list"]')
    expect(list.text()).toContain('CORE-T17')
    expect(list.text()).toContain('implementation')
    expect(list.text()).toContain('Spec authoring creates content versions end to end')
    expect(list.text()).toContain('CORE-T18')
    expect(list.text()).toContain('bug')
    expect(list.text()).toContain('Landing drops the integration branch')
    expect(list.text()).toContain('urgent')
    expect(list.text()).toContain('CORE-T19')
    expect(list.text()).toContain('task')
  })

  it('the form follows the picked kind', async () => {
    setActivePinia(createPinia())
    const wrapper = await mountView(harness().transport)

    // The blank draft is a Bug: title and optional attachment only.
    expect(wrapper.find('[data-testid="ticket-title"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-slice"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="ticket-criteria"]').exists()).toBe(false)

    await wrapper.find('[data-testid="ticket-kind"]').setValue('implementation')
    expect(wrapper.find('[data-testid="ticket-title"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="ticket-slice"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-criteria"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="ticket-spec"]').exists()).toBe(true)
  })

  it('creating an implementation addresses the picked project with the kind fields', async () => {
    setActivePinia(createPinia())
    const harnessState = harness()
    const wrapper = await mountView(harnessState.transport)

    await wrapper.find('[data-testid="ticket-kind"]').setValue('implementation')
    await wrapper.find('[data-testid="ticket-spec"]').setValue('7')
    await wrapper.find('[data-testid="ticket-slice"]').setValue('Registration lands end to end')
    await wrapper
      .find('[data-testid="ticket-criterion-outcome-0"]')
      .setValue('Projects register with unique codes.')
    await wrapper.find('[data-testid="ticket-criterion-stories-0"]').setValue('CORE-S1-US1')
    await wrapper.find('[data-testid="ticket-priority"]').setValue('high')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    const created = harnessState.operations.find((entry) => entry.name === 'ticket.create')
    expect(created?.request).toMatchObject({
      project_id: 4,
      kind: 'implementation',
      priority: 'high',
      spec_id: 7,
      slice: 'Registration lands end to end',
      criteria: [
        { outcome: 'Projects register with unique codes.', stories: ['CORE-S1-US1'] },
      ],
    })
  })

  it('creating a bug sends the title and the priority', async () => {
    setActivePinia(createPinia())
    const harnessState = harness()
    const wrapper = await mountView(harnessState.transport)

    await wrapper
      .find('[data-testid="ticket-title"]')
      .setValue('Landing drops the integration branch')
    await wrapper.find('[data-testid="ticket-priority"]').setValue('urgent')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    const created = harnessState.operations.find((entry) => entry.name === 'ticket.create')
    expect(created?.request).toMatchObject({
      project_id: 4,
      kind: 'bug',
      priority: 'urgent',
      title: 'Landing drops the integration branch',
    })
    expect(created?.request).not.toHaveProperty('slice')
  })

  it('criteria rows add and remove', async () => {
    setActivePinia(createPinia())
    const harnessState = harness()
    const wrapper = await mountView(harnessState.transport)

    await wrapper.find('[data-testid="ticket-kind"]').setValue('implementation')
    await wrapper.find('[data-testid="ticket-spec"]').setValue('7')
    await wrapper.find('[data-testid="ticket-slice"]').setValue('A slice')
    await wrapper.find('[data-testid="ticket-criterion-add"]').trigger('click')
    await wrapper
      .find('[data-testid="ticket-criterion-outcome-0"]')
      .setValue('First outcome.')
    await wrapper.find('[data-testid="ticket-criterion-stories-0"]').setValue('CORE-S1-US1')
    await wrapper
      .find('[data-testid="ticket-criterion-outcome-1"]')
      .setValue('Second outcome.')
    await wrapper.find('[data-testid="ticket-criterion-stories-1"]').setValue('CORE-S1-US2')
    await wrapper.find('[data-testid="ticket-criterion-remove-1"]').trigger('click')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    const created = harnessState.operations.find((entry) => entry.name === 'ticket.create')
    const request = created?.request as { criteria: Array<{ outcome: string }> }
    expect(request.criteria).toEqual([{ outcome: 'First outcome.', stories: ['CORE-S1-US1'] }])
  })

  it('a refused creation reports the message', async () => {
    setActivePinia(createPinia())
    const harnessState = harness()
    harnessState.transport.command = (name: string, request: unknown) => {
      harnessState.operations.push({ kind: 'command', name, request })
      return Promise.reject({
        code: 'invalid_request',
        message: 'an Implementation Ticket carries story-linked criteria',
      })
    }
    const wrapper = await mountView(harnessState.transport)

    await wrapper.find('[data-testid="ticket-kind"]').setValue('implementation')
    await wrapper.find('[data-testid="ticket-spec"]').setValue('7')
    await wrapper.find('[data-testid="ticket-slice"]').setValue('A slice')
    await wrapper.find('[data-testid="ticket-criterion-outcome-0"]').setValue('  ')
    await wrapper.find('[data-testid="ticket-create"]').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[data-testid="ticket-error"]').text()).toBe(
      'an Implementation Ticket carries story-linked criteria',
    )
  })
})
