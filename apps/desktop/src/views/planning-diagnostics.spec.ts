import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it } from 'vitest'
import type {
  PlanDiagnosticsResponse,
  PlanGetResponse,
  PlanListResponse,
  ProjectListResponse,
} from '@kanban/contracts'
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
  spec_numbers: [1, 2],
  edges: [
    { from_spec: 1, to_spec: 2 },
    { from_spec: 2, to_spec: 1 },
  ],
  version: 6,
}

const versions = {
  plan: { ...draft },
  versions: [
    {
      number: 1,
      spec_numbers: [1, 2],
      edges: [
        { from_spec: 1, to_spec: 2 },
        { from_spec: 2, to_spec: 1 },
      ],
    },
  ],
} satisfies PlanGetResponse

// The blocking diagnostics the ring of CORE-S1 and CORE-S2 reports.
const blocked = {
  cycles: [{ spec_numbers: [1, 2] }],
  coverage_gaps: [
    {
      spec_number: 1,
      uncovered: ['CORE-S1-US1', 'CORE-S1-US2'],
      claims_no_stories: false,
    },
    {
      spec_number: 2,
      uncovered: [],
      claims_no_stories: true,
    },
  ],
  invalid_profiles: [] as PlanDiagnosticsResponse['invalid_profiles'],
  blocking: true,
} satisfies PlanDiagnosticsResponse

// A transport steered per operation name, recording every query. The
// Plan's aggregate version moves when a command lands, exactly as the
// core bumps it, so the diagnostics re-read after an edit.
function harness(answers: Record<string, unknown> = {}) {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  let planVersion = 6
  const state = {
    operations,
    answers,
    bumpPlanVersion(): void {
      planVersion += 1
    },
  }
  const reply = (name: string): unknown => {
    if (name === 'project.list') {
      return { projects: [project] } satisfies ProjectListResponse
    }
    if (name === 'plan.list') {
      return { plans: [{ ...draft, version: planVersion }] } satisfies PlanListResponse
    }
    if (name === 'plan.get') {
      return {
        plan: { ...draft, version: planVersion },
        versions: versions.versions,
      } satisfies PlanGetResponse
    }
    if (name in answers) {
      return answers[name]
    }
    return blocked
  }
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      return Promise.resolve(reply(name))
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      state.bumpPlanVersion()
      return Promise.resolve({ ...draft, version: planVersion })
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, ...state }
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

// The view with the first plan open and its diagnostics loaded.
async function mountedWithDiagnostics(answers: Record<string, unknown> = {}) {
  const harnessState = harness(answers)
  const wrapper = await mountView(harnessState.transport)
  await wrapper.find('[data-testid="plan-row-1"]').trigger('click')
  await flushPromises()
  return { wrapper, ...harnessState }
}

// The diagnostics the view issued, one entry per query.
function diagnosticQueries(operations: Array<{ kind: string; name: string; request: unknown }>) {
  return operations.filter((entry) => entry.name === 'plan.diagnostics')
}

describe('PlanningView diagnostics', () => {
  it('renders the blocking cycles and gaps next to the graph', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations } = await mountedWithDiagnostics()

    const editor = wrapper.find('[data-testid="plan-editor"]')
    expect(editor.exists()).toBe(true)
    const panel = editor.find('[data-testid="plan-diagnostics"]')
    expect(panel.exists(), 'the diagnostics sit inside the graph editor').toBe(true)

    expect(panel.find('[data-testid="plan-diagnostics-blocking"]').text()).toContain('blocked')
    expect(panel.find('[data-testid="plan-diagnostics-cycle-0"]').text()).toBe(
      'CORE-S1 → CORE-S2 form a dependency cycle.',
    )
    expect(panel.find('[data-testid="plan-diagnostics-gap-1"]').text()).toBe(
      'CORE-S1: CORE-S1-US1, CORE-S1-US2 uncovered.',
    )
    expect(panel.find('[data-testid="plan-diagnostics-gap-2"]').text()).toBe(
      'CORE-S2 claims no User Stories to cover.',
    )
    expect(diagnosticQueries(operations)[0]?.request).toEqual({
      plan_id: 1,
      version: null,
    })
  })

  it('follows the graph on display when the version switches', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations } = await mountedWithDiagnostics()

    await wrapper.find('[data-testid="plan-version-1"]').trigger('click')
    await flushPromises()

    const queries = diagnosticQueries(operations).map((entry) => entry.request)
    expect(queries).toEqual([
      { plan_id: 1, version: null },
      { plan_id: 1, version: 1 },
    ])
  })

  it('re-reads the working shape after an edit lands', async () => {
    setActivePinia(createPinia())
    const { wrapper, operations } = await mountedWithDiagnostics()

    await wrapper.find('[data-testid="plan-edge-remove-2-1"]').trigger('click')
    await flushPromises()

    const queries = diagnosticQueries(operations).map((entry) => entry.request)
    expect(queries).toEqual([
      { plan_id: 1, version: null },
      { plan_id: 1, version: null },
    ])
  })

  it('renders the invalid profile references the catalogue feeds', async () => {
    setActivePinia(createPinia())
    const { wrapper } = await mountedWithDiagnostics({
      'plan.diagnostics': {
        cycles: [],
        coverage_gaps: [],
        invalid_profiles: [{ reference: 'ghost-profile' }],
        blocking: true,
      } satisfies PlanDiagnosticsResponse,
    })

    expect(wrapper.find('[data-testid="plan-diagnostics-profile-0"]').text()).toBe(
      'Profile reference ghost-profile resolves to no catalogue entry.',
    )
  })

  it('reports a clear graph without blocking diagnostics', async () => {
    setActivePinia(createPinia())
    const { wrapper } = await mountedWithDiagnostics({
      'plan.diagnostics': {
        cycles: [],
        coverage_gaps: [],
        invalid_profiles: [],
        blocking: false,
      } satisfies PlanDiagnosticsResponse,
    })

    const panel = wrapper.find('[data-testid="plan-diagnostics"]')
    expect(panel.find('[data-testid="plan-diagnostics-clear"]').exists()).toBe(true)
    expect(panel.find('[data-testid="plan-diagnostics-blocking"]').exists()).toBe(false)
  })

  it('reports a refused diagnostics query', async () => {
    setActivePinia(createPinia())
    const harnessState = harness()
    harnessState.transport.query = ((name: string, request: unknown) => {
      harnessState.operations.push({ kind: 'query', name, request })
      if (name === 'plan.diagnostics') {
        return Promise.reject({ code: 'invalid_request', message: 'the core refused' })
      }
      return Promise.resolve(
        name === 'project.list'
          ? { projects: [project] }
          : name === 'plan.list'
            ? { plans: [draft] }
            : versions,
      )
    }) as unknown as ShellTransport['query']
    const wrapper = await mountView(harnessState.transport)
    await wrapper.find('[data-testid="plan-row-1"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-testid="plan-diagnostics-error"]').text()).toBe('the core refused')
  })
})
