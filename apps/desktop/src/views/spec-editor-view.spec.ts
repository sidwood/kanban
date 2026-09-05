import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it } from 'vitest'
import type {
  PlanListResponse,
  ProjectListResponse,
  SpecContent,
  SpecGetResponse,
  SpecListResponse,
} from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import SpecEditorView from './SpecEditorView.vue'

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

function content(overrides: Partial<SpecContent> = {}): SpecContent {
  return {
    name: 'Plans and specifications',
    short_description: 'Versioned Plan graphs of Specs',
    problem_statement: 'Planning must survive change.',
    solution: 'Immutable approved versions.',
    user_stories: 'KAN-S3-US4',
    implementation_decisions: 'Supersession is explicit.',
    testing_decisions: 'Domain tests prove immutability.',
    out_of_scope: 'The Ticket graph proposal.',
    further_notes: 'None',
    ...overrides,
  }
}

const unplanned = {
  id: 1,
  project_id: 4,
  number: 1,
  name: 'Plans and specifications',
  execution: 'unplanned' as const,
  plan_id: null,
  version: 3,
}

const planned = {
  id: 2,
  project_id: 4,
  number: 2,
  name: 'Timeline',
  execution: 'planned' as const,
  plan_id: 5,
  version: 4,
}

const finished = {
  id: 3,
  project_id: 4,
  number: 3,
  name: 'Review',
  execution: 'complete' as const,
  plan_id: 5,
  version: 8,
}

const versions = [
  {
    number: 1,
    state: 'superseded' as const,
    content: content({ name: 'Registration', user_stories: 'KAN-S3-US4\nKAN-S3-US5' }),
  },
  {
    number: 2,
    state: 'approved' as const,
    content: content({
      name: 'Plans and specifications',
      solution: 'Immutable approved versions.',
    }),
  },
  {
    number: 3,
    state: 'draft' as const,
    content: content({
      name: 'Plans and specifications',
      solution: 'Immutable approved versions, explicitly superseded.',
    }),
  },
]

const plans = {
  plans: [
    { id: 5, project_id: 4, number: 1, state: 'draft' as const, spec_numbers: [1], edges: [], version: 2 },
  ] satisfies PlanListResponse['plans'],
}

// A transport steered per operation name, recording every command.
function harness() {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  const answers: Record<string, unknown> = {
    'project.list': { projects: [project] } satisfies ProjectListResponse,
    'spec.list': { specs: [unplanned, planned, finished] } satisfies SpecListResponse,
    'spec.get': { spec: planned, versions } satisfies SpecGetResponse,
    'plan.list': plans satisfies PlanListResponse,
  }
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      return Promise.resolve(answers[name])
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      return Promise.resolve(answers[name] ?? planned)
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, operations, answers }
}

async function mountView(transport: ShellTransport) {
  const wrapper = mount(SpecEditorView, {
    global: {
      plugins: [createPinia()],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return wrapper
}

// The editor with the second spec (planned, three versions) open.
async function mountedWithSelection() {
  const harnessState = harness()
  const wrapper = await mountView(harnessState.transport)
  await wrapper.find('[data-testid="spec-row-2"]').trigger('click')
  await flushPromises()
  return { wrapper, ...harnessState }
}

// The text of one PRD section's textarea.
function sectionValue(
  wrapper: Awaited<ReturnType<typeof mountView>>,
  section: string,
): string {
  return (wrapper.find(`[data-testid="spec-section-${section}"]`).element as HTMLTextAreaElement)
    .value
}

describe('SpecEditorView', () => {
  it('lists every project and the specs of the first one', async () => {
    setActivePinia(createPinia())
    const { wrapper } = await mountedWithSelection()

    expect(wrapper.find('[data-testid="spec-project"]').element.textContent).toContain('CORE')
    expect(wrapper.find('[data-testid="spec-index"]').text()).toContain('CORE-S1')
    expect(wrapper.find('[data-testid="spec-index"]').text()).toContain('CORE-S2')
    expect(wrapper.find('[data-testid="spec-index"]').text()).toContain('CORE-S3')
    expect(wrapper.find('[data-testid="spec-index"]').text()).toContain('unplanned')
    expect(wrapper.find('[data-testid="spec-index"]').text()).toContain('complete')
  })

  it('authoring addresses the picked project with a named draft', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations } = await mountedWithSelection()

    await wrapper.find('[data-testid="spec-name-input"]').setValue('Landing')
    await wrapper.find('[data-testid="spec-create"]').trigger('submit')
    await flushPromises()

    const created = operations.find((entry) => entry.name === 'spec.create')
    expect(created?.request).toMatchObject({ project_id: 4 })
    const request = created?.request as { content: SpecContent }
    expect(request.content.name).toBe('Landing')
    expect(request.content.solution).toBe('')
  })

  it('the editor shows the working content of the opened spec', async () => {
    setActivePinia(createPinia())
    const { wrapper } = await mountedWithSelection()

    expect(wrapper.find('[data-testid="spec-title"]').text()).toBe('CORE-S2')
    expect(wrapper.find('[data-testid="spec-execution"]').text()).toBe('planned')
    expect(sectionValue(wrapper, 'name')).toBe('Plans and specifications')
    expect(sectionValue(wrapper, 'solution')).toBe(
      'Immutable approved versions, explicitly superseded.',
    )
    expect(wrapper.find('[data-testid="spec-save"]').attributes('disabled')).toBeUndefined()
  })

  it('saving sends the nine sections of the working draft', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations } = await mountedWithSelection()

    await wrapper.find('[data-testid="spec-section-name"]').setValue('Plans and specs, revised')
    await wrapper.find('[data-testid="spec-save"]').trigger('submit')
    await flushPromises()

    const update = operations.find((entry) => entry.name === 'spec.content.update')
    expect(update?.request).toMatchObject({ spec_id: 2 })
    const request = update?.request as { content: SpecContent; mutation: { optimistic_version: number } }
    expect(Object.keys(request.content)).toHaveLength(9)
    expect(request.content.name).toBe('Plans and specs, revised')
    expect(request.mutation.optimistic_version).toBe(4)
  })

  it('switching to a frozen version shows its content read-only', async () => {
    setActivePinia(createPinia())
    const { wrapper } = await mountedWithSelection()

    await wrapper.find('[data-testid="spec-version-1"]').trigger('click')
    await flushPromises()

    expect(sectionValue(wrapper, 'name')).toBe('Registration')
    expect(sectionValue(wrapper, 'user_stories')).toBe('KAN-S3-US4\nKAN-S3-US5')
    expect(wrapper.find('[data-testid="spec-save"]').attributes('disabled')).toBeDefined()
    expect(wrapper.find('[data-testid="spec-readonly"]').text()).toContain('Viewing v1')

    await wrapper.find('[data-testid="spec-version-current"]').trigger('click')
    expect(sectionValue(wrapper, 'name')).toBe('Plans and specifications')
  })

  it('the diff shows removed and added lines section by section', async () => {
    setActivePinia(createPinia())
    const { wrapper } = await mountedWithSelection()

    // Diff the working content against the superseded version one.
    await wrapper.find('[data-testid="spec-compare-1"]').trigger('click')
    await flushPromises()

    const diff = wrapper.find('[data-testid="spec-diff"]')
    expect(diff.exists()).toBe(true)
    const solution = wrapper.find('[data-testid="spec-diff-section-solution"]')
    expect(solution.text()).toContain('− Immutable approved versions.')
    expect(solution.text()).toContain('+ Immutable approved versions, explicitly superseded.')
    const stories = wrapper.find('[data-testid="spec-diff-section-user_stories"]')
    expect(stories.text()).toContain('− KAN-S3-US5')

    await wrapper.find('[data-testid="spec-compare-1"]').trigger('click')
    expect(wrapper.find('[data-testid="spec-diff"]').exists()).toBe(false)
  })

  it('approval follows the explicit-supersession gate', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations, answers } = await mountedWithSelection()

    expect(wrapper.find('[data-testid="spec-approve"]').attributes('disabled')).toBeDefined()

    await wrapper.find('[data-testid="spec-supersede-2"]').trigger('submit')
    await flushPromises()
    expect(operations.find((entry) => entry.name === 'spec.version.supersede')?.request).toMatchObject(
      { spec_id: 2, version: 2 },
    )

    // With v2 superseded and v3 a draft, approval opens up.
    answers['spec.get'] = {
      spec: planned,
      versions: [
        ...versions.slice(0, 1),
        { number: 2, state: 'superseded' as const, content: versions[1].content },
        versions[2],
      ],
    } satisfies SpecGetResponse
    await wrapper.find('[data-testid="spec-row-2"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-testid="spec-approve"]').attributes('disabled')).toBeUndefined()
    await wrapper.find('[data-testid="spec-approve"]').trigger('submit')
    await flushPromises()
    expect(
      operations.find((entry) => entry.name === 'spec.version.approve')?.request,
    ).toMatchObject({ spec_id: 2 })
  })

  it('joining a plan and moving execution send their targets', async () => {
    setActivePinia(createPinia())
    const unplannedHarness = harness()
    unplannedHarness.answers['spec.get'] = {
      spec: { ...unplanned, version: 3 },
      versions,
    } satisfies SpecGetResponse
    unplannedHarness.answers['spec.plan.join'] = { ...unplanned, execution: 'planned', plan_id: 5, version: 4 }
    unplannedHarness.answers['spec.execution.move'] = { ...unplanned, execution: 'ready', version: 5 }
    const wrapper = await mountView(unplannedHarness.transport)
    await wrapper.find('[data-testid="spec-row-1"]').trigger('click')
    await flushPromises()

    await wrapper.find('[data-testid="spec-plan-select"]').setValue('5')
    await wrapper.find('[data-testid="spec-plan-join"]').trigger('submit')
    await flushPromises()
    expect(unplannedHarness.operations.find((entry) => entry.name === 'spec.plan.join')?.request)
      .toMatchObject({ spec_id: 1, plan_id: 5 })

    await wrapper.find('[data-testid="spec-execution-select"]').setValue('ready')
    await wrapper.find('[data-testid="spec-execution-move"]').trigger('submit')
    await flushPromises()
    expect(unplannedHarness.operations.find((entry) => entry.name === 'spec.execution.move')?.request)
      .toMatchObject({ spec_id: 1, execution: 'ready' })
  })

  it('terminal execution hides the move action', async () => {
    setActivePinia(createPinia())
    const { wrapper } = await mountedWithSelection()

    await wrapper.find('[data-testid="spec-row-3"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-testid="spec-execution-move"]').exists()).toBe(false)
  })

  it('a refused command reports the message', async () => {
    setActivePinia(createPinia())
    const harnessState = harness()
    harnessState.transport.command = (name: string, request: unknown) => {
      harnessState.operations.push({ kind: 'command', name, request })
      return Promise.reject({
        code: 'invalid_request',
        message: 'version 2 is already superseded',
      })
    }
    const wrapper = await mountView(harnessState.transport)
    await wrapper.find('[data-testid="spec-row-2"]').trigger('click')
    await flushPromises()

    await wrapper.find('[data-testid="spec-supersede-2"]').trigger('submit')
    await flushPromises()

    expect(wrapper.find('[data-testid="spec-error"]').text()).toBe(
      'version 2 is already superseded',
    )
  })
})
