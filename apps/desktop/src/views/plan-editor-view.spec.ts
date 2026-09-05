import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it } from 'vitest'
import type { PlanGetResponse, PlanListResponse, ProjectListResponse } from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
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
  counters: { plan: 2, spec: 3, ticket: 0 },
  version: 1,
}

const draft = {
  id: 1,
  project_id: 4,
  number: 1,
  state: 'draft' as const,
  spec_numbers: [1, 3, 2],
  edges: [
    { from_spec: 1, to_spec: 2 },
    { from_spec: 3, to_spec: 2 },
  ],
  version: 6,
}

const active = {
  id: 2,
  project_id: 4,
  number: 2,
  state: 'active' as const,
  spec_numbers: [1],
  edges: [],
  version: 3,
}

const finished = {
  id: 3,
  project_id: 4,
  number: 3,
  state: 'complete' as const,
  spec_numbers: [1],
  edges: [],
  version: 4,
}

const versions = {
  plan: { ...active },
  versions: [
    {
      number: 1,
      spec_numbers: [1, 3, 2],
      edges: [
        { from_spec: 1, to_spec: 2 },
        { from_spec: 3, to_spec: 2 },
      ],
    },
  ],
}

// A transport steered per operation name, recording every command.
function harness() {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  const answers: Record<string, unknown> = {
    'project.list': { projects: [project] } satisfies ProjectListResponse,
    'plan.list': { plans: [draft, active, finished] } satisfies PlanListResponse,
    'plan.get': versions satisfies PlanGetResponse,
  }
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      return Promise.resolve(answers[name])
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      return Promise.resolve(answers[name] ?? draft)
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, operations, answers }
}

async function mountView(transport: ShellTransport) {
  const wrapper = mount(PlanningView, {
    global: {
      plugins: [createPinia()],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return wrapper
}

// The editor with the first plan open.
async function mountedWithSelection() {
  const harnessState = harness()
  const wrapper = await mountView(harnessState.transport)
  await wrapper.find('[data-testid="plan-row-1"]').trigger('click')
  await flushPromises()
  return { wrapper, ...harnessState }
}

// The rendered Spec identities of the editor's spec rows.
function specIds(wrapper: Awaited<ReturnType<typeof mountView>>) {
  return wrapper
    .findAll('[data-testid^="plan-spec-row-"]')
    .map((row) => row.find('span.font-mono').text())
}

describe('PlanningView', () => {
  it('lists every project and the plans of the first one', async () => {
    setActivePinia(createPinia())
    const { wrapper } = await mountedWithSelection()

    expect(wrapper.find('[data-testid="planning-project"]').element.textContent).toContain('CORE')
    expect(wrapper.find('[data-testid="plan-active"]').text()).toContain('CORE-P1')
    expect(wrapper.find('[data-testid="plan-active"]').text()).toContain('CORE-P2')
    expect(wrapper.find('[data-testid="plan-finished"]').text()).toContain('CORE-P3')
    expect(
      wrapper.find('[data-testid="plan-finished"]').text(),
      'the terminal states sit off the active surface',
    ).not.toContain('CORE-P1')
  })

  it('creating a plan addresses the picked project', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations } = await mountedWithSelection()

    await wrapper.find('[data-testid="plan-create"]').trigger('submit')
    await flushPromises()

    const created = operations.find((entry) => entry.name === 'plan.create')
    expect(created?.request).toMatchObject({ project_id: 4 })
  })

  it('the editor shows the selected plan with both relations', async () => {
    setActivePinia(createPinia())
    const { wrapper } = await mountedWithSelection()

    expect(wrapper.find('[data-testid="plan-title"]').text()).toBe('CORE-P1')
    expect(wrapper.find('[data-testid="plan-state"]').text()).toBe('draft')
    expect(specIds(wrapper)).toEqual(['CORE-S1', 'CORE-S3', 'CORE-S2'])
    const edges = wrapper.findAll('[data-testid^="plan-edge-row-"]')
    expect(edges.map((row) => row.find('span.font-mono').text())).toEqual([
      'CORE-S1 → CORE-S2',
      'CORE-S3 → CORE-S2',
    ])
  })

  it('adding a spec sends its number', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations } = await mountedWithSelection()

    await wrapper.find('[data-testid="plan-spec-number"]').setValue('4')
    await wrapper.find('[data-testid="plan-spec-add"]').trigger('submit')
    await flushPromises()

    const added = operations.find((entry) => entry.name === 'plan.spec.add')
    expect(added?.request).toMatchObject({ plan_id: 1, spec_number: 4 })
  })

  it('moving a spec sends the target position', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations } = await mountedWithSelection()

    await wrapper.find('[data-testid="plan-spec-up-2"]').trigger('click')
    await flushPromises()

    const moved = operations.find((entry) => entry.name === 'plan.spec.move')
    expect(moved?.request).toMatchObject({ plan_id: 1, spec_number: 2, position: 1 })
  })

  it('adding and removing edges send both endpoints', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations } = await mountedWithSelection()

    await wrapper.find('[data-testid="plan-edge-from"]').setValue('2')
    await wrapper.find('[data-testid="plan-edge-to"]').setValue('1')
    await wrapper.find('[data-testid="plan-edge-add"]').trigger('submit')
    await flushPromises()
    await wrapper.find('[data-testid="plan-edge-remove-1-2"]').trigger('click')
    await flushPromises()

    expect(operations.find((entry) => entry.name === 'plan.edge.add')?.request).toMatchObject({
      plan_id: 1,
      from_spec: 2,
      to_spec: 1,
    })
    expect(operations.find((entry) => entry.name === 'plan.edge.remove')?.request).toMatchObject({
      plan_id: 1,
      from_spec: 1,
      to_spec: 2,
    })
  })

  it('the lifecycle actions follow the state', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations } = await mountedWithSelection()

    expect(wrapper.find('[data-testid="plan-activate"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="plan-replan"]').exists()).toBe(false)
    await wrapper.find('[data-testid="plan-activate"]').trigger('submit')
    await flushPromises()
    expect(operations.find((entry) => entry.name === 'plan.activate')).toBeDefined()

    const activeWrapper = wrapper
    await activeWrapper.find('[data-testid="plan-row-2"]').trigger('click')
    await flushPromises()
    expect(activeWrapper.find('[data-testid="plan-replan"]').exists()).toBe(true)
    expect(activeWrapper.find('[data-testid="plan-activate"]').exists()).toBe(false)

    await activeWrapper.find('[data-testid="plan-row-3"]').trigger('click')
    await flushPromises()
    expect(activeWrapper.find('[data-testid="plan-replan"]').exists()).toBe(false)
    expect(activeWrapper.find('[data-testid="plan-archive"]').exists()).toBe(true)
  })

  it('version switching shows the frozen shape beside the working one', async () => {
    setActivePinia(createPinia())
    const { wrapper } = await mountedWithSelection()

    await wrapper.find('[data-testid="plan-row-2"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="plan-version-1"]').exists()).toBe(true)

    await wrapper.find('[data-testid="plan-version-1"]').trigger('click')
    // The frozen order shows.
    expect(specIds(wrapper)).toEqual(['CORE-S1', 'CORE-S3', 'CORE-S2'])

    await wrapper.find('[data-testid="plan-version-draft"]').trigger('click')
    // The working order returns.
    expect(specIds(wrapper)).toEqual(['CORE-S1'])
  })

  it('a refused command reports the message', async () => {
    setActivePinia(createPinia())
    const harnessState = harness()
    harnessState.transport.command = (name: string, request: unknown) => {
      harnessState.operations.push({ kind: 'command', name, request })
      return Promise.reject({ code: 'invalid_request', message: 'only a draft Plan accepts this change' })
    }
    const wrapper = await mountView(harnessState.transport)
    await wrapper.find('[data-testid="plan-row-1"]').trigger('click')
    await flushPromises()

    await wrapper.find('[data-testid="plan-spec-number"]').setValue('4')
    await wrapper.find('[data-testid="plan-spec-add"]').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[data-testid="plan-error"]').text()).toBe(
      'only a draft Plan accepts this change',
    )
  })
})
