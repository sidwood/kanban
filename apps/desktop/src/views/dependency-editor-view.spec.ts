import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it } from 'vitest'
import type {
  ProjectListResponse,
  TicketDependenciesResponse,
  TicketListResponse,
  TicketReadinessResponse,
} from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import DependencyEditorView from './DependencyEditorView.vue'

const coreProject = {
  id: 1,
  code: 'CORE',
  name: 'Control plane',
  repository: '/repositories/kanban',
  seed_workspace: '/workspaces/kanban.seed',
  default_branch: 'main',
  herdr_session: 'kanban-main',
  herdr_workspace: 'kanban.seed',
  initiative_id: null,
  archived: false,
  counters: { plan: 0, spec: 0, ticket: 1 },
  version: 1,
}

const edgeProject = {
  ...coreProject,
  id: 2,
  code: 'EDGE',
  name: 'Edge work',
  counters: { plan: 0, spec: 0, ticket: 1 },
}

const coreTicket = {
  id: 1,
  project_id: 1,
  number: 1,
  kind: 'bug' as const,
  priority: 'normal' as const,
  state: 'active' as const,
  spec_id: null,
  title: 'Landing drops the integration branch',
  slice: null,
  criteria: [],
  version: 4,
}

const edgeTicket = {
  ...coreTicket,
  id: 2,
  project_id: 2,
  state: 'draft' as const,
  title: 'Archive the old register',
  version: 5,
}

const dependencies: TicketDependenciesResponse = {
  ticket_id: 2,
  version: 5,
  dependencies: [
    { from_ticket_id: 1, from_project_id: 1, from_number: 1, from_state: 'active' },
  ],
  blockers: [{ id: 3, ticket_id: 2, description: 'The vendor SDK 4 upgrade' }],
}

const readiness: TicketReadinessResponse = {
  ticket_id: 2,
  state: 'draft',
  ready: false,
  blocked_by: [
    { Ticket: { from_ticket_id: 1, from_project_id: 1, from_number: 1, from_state: 'active' } },
    { External: { blocker_id: 3, description: 'The vendor SDK 4 upgrade' } },
  ],
}

// A transport steered per operation name, recording every command.
function harness() {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  const answers: Record<string, unknown> = {
    'project.list': { projects: [coreProject, edgeProject] } satisfies ProjectListResponse,
    'ticket.list': {
      tickets: [coreTicket, edgeTicket],
    } satisfies TicketListResponse,
    'ticket.dependencies': dependencies satisfies TicketDependenciesResponse,
    'ticket.readiness': readiness satisfies TicketReadinessResponse,
  }
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      return Promise.resolve(answers[name])
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      return Promise.resolve(answers[name] ?? dependencies)
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, operations, answers }
}

async function mountView(transport: ShellTransport) {
  const wrapper = mount(DependencyEditorView, {
    global: {
      plugins: [createPinia()],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return wrapper
}

// The editor with the EDGE Ticket open.
async function mountedWithTicket() {
  const harnessState = harness()
  const wrapper = await mountView(harnessState.transport)
  await wrapper.find('[data-testid="dependency-project"]').setValue(2)
  await flushPromises()
  await wrapper.find('[data-testid="dependency-ticket"]').setValue(2)
  await wrapper.find('[data-testid="dependency-ticket"]').trigger('change')
  await flushPromises()
  return { wrapper, ...harnessState }
}

describe('DependencyEditorView', () => {
  it('opens a ticket and shows the computed readiness and both wait lists', async () => {
    setActivePinia(createPinia())
    const { wrapper } = await mountedWithTicket()

    expect(wrapper.find('[data-testid="dependency-readiness-state"]').text()).toContain(
      'Waiting on',
    )
    const blockers = wrapper.findAll('[data-testid^="dependency-blocker-"]')
    expect(blockers.map((row) => row.text())).toEqual([
      'CORE-T1 — active',
      'The vendor SDK 4 upgrade',
    ])
    expect(wrapper.find('[data-testid="dependency-row-1"]').text()).toContain('CORE-T1')
    expect(wrapper.find('[data-testid="dependency-row-1"]').text()).toContain('must land first')
    expect(wrapper.find('[data-testid="blocker-row-3"]').text()).toContain(
      'The vendor SDK 4 upgrade',
    )
  })

  it('adding a cross-project dependency names both tickets', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations } = await mountedWithTicket()

    await wrapper.find('[data-testid="dependency-source-project"]').setValue(1)
    await flushPromises()
    await wrapper.find('[data-testid="dependency-source-ticket"]').setValue(1)
    await wrapper.find('[data-testid="dependency-add"]').trigger('submit')
    await flushPromises()

    const added = operations.find((entry) => entry.name === 'ticket.dependency.add')
    expect(added?.request).toMatchObject({
      mutation: { optimistic_version: 5 },
      from_ticket: 1,
      to_ticket: 2,
    })
  })

  it('adding an external blocker sends the description', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations } = await mountedWithTicket()

    await wrapper.find('[data-testid="blocker-description"]').setValue('Design sign-off')
    await wrapper.find('[data-testid="blocker-add"]').trigger('submit')
    await flushPromises()

    const added = operations.find((entry) => entry.name === 'ticket.blocker.add')
    expect(added?.request).toMatchObject({
      ticket_id: 2,
      description: 'Design sign-off',
    })
  })

  it('removing a dependency and a blocker names their identities', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations } = await mountedWithTicket()

    await wrapper.find('[data-testid="dependency-remove-1"]').trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="blocker-remove-3"]').trigger('click')
    await flushPromises()

    expect(
      operations.find((entry) => entry.name === 'ticket.dependency.remove')?.request,
    ).toMatchObject({ from_ticket: 1, to_ticket: 2 })
    expect(operations.find((entry) => entry.name === 'ticket.blocker.remove')?.request).toMatchObject(
      { ticket_id: 2, blocker_id: 3 },
    )
  })

  it('a refused command reports the message', async () => {
    setActivePinia(createPinia())
    const harnessState = harness()
    harnessState.transport.command = (name: string, request: unknown) => {
      harnessState.operations.push({ kind: 'command', name, request })
      return Promise.reject({
        code: 'invalid_request',
        message: 'the dependency from Ticket 2 to Ticket 1 would close a cycle',
      })
    }
    const wrapper = await mountView(harnessState.transport)
    await wrapper.find('[data-testid="dependency-project"]').setValue(2)
    await flushPromises()
    await wrapper.find('[data-testid="dependency-ticket"]').setValue(2)
    await wrapper.find('[data-testid="dependency-ticket"]').trigger('change')
    await flushPromises()

    await wrapper.find('[data-testid="dependency-source-project"]').setValue(1)
    await flushPromises()
    await wrapper.find('[data-testid="dependency-source-ticket"]').setValue(1)
    await wrapper.find('[data-testid="dependency-add"]').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[data-testid="dependency-error"]').text()).toBe(
      'the dependency from Ticket 2 to Ticket 1 would close a cycle',
    )
  })
})
