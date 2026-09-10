import { nextTick } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, describe, expect, it } from 'vitest'
import type {
  InitiativeRecord,
  PlanRecord,
  ProjectRecord,
  SpecRecord,
  TicketGraphRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useCoverageMatrixStore } from '../stores/coverage-matrix'
import { useGraphProposalsStore } from '../stores/graph-proposals'
import { usePlanEditorStore } from '../stores/plan-editor'
import { useProjectRegisterStore } from '../stores/project-register'
import { useTimelineStore } from '../stores/timeline'
import HomeView from './HomeView.vue'
import PlanningView from './PlanningView.vue'
import ProjectSettingsView from './ProjectSettingsView.vue'

function project(id: number, code: string, name: string): ProjectRecord {
  return {
    id,
    code,
    name,
    repository: `/repositories/${code.toLowerCase()}`,
    seed_workspace: `/workspaces/${code.toLowerCase()}.seed`,
    default_branch: 'main',
    herdr_session: `${code.toLowerCase()}-main`,
    herdr_workspace: `${code.toLowerCase()}.seed`,
    initiative_id: null,
    archived: false,
    counters: { plan: 1, spec: 1, ticket: 1 },
    version: 1,
  }
}

function plan(id: number, projectId: number, number = 1): PlanRecord {
  return {
    id,
    project_id: projectId,
    number,
    state: 'draft',
    spec_numbers: [],
    edges: [],
    version: 1,
  }
}

function spec(id: number, projectId: number, number = 1): SpecRecord {
  return {
    id,
    project_id: projectId,
    number,
    name: `Spec ${number} of ${projectId}`,
    execution: 'planned',
    plan_id: null,
    version: 1,
  } as unknown as SpecRecord
}

function proposal(id: number, specId: number): TicketGraphRecord {
  return {
    id,
    spec_id: specId,
    spec_version: 1,
    tickets: [],
    edges: [],
    state: 'proposed',
    version: 1,
  }
}

const initiatives: InitiativeRecord[] = []

function deferred<T>() {
  let settle!: (value: T) => void
  const promise = new Promise<T>((resolve) => {
    settle = resolve
  })
  return { promise, settle }
}

function harness(answers: Record<string, (request: Record<string, unknown>) => unknown>) {
  const queries: Array<{ name: string; request: Record<string, unknown> }> = []
  const commands: Array<{ name: string; request: Record<string, unknown> }> = []
  const transport = {
    query(name: string, request: Record<string, unknown>) {
      queries.push({ name, request })
      const answer = answers[name]
      if (!answer) return Promise.resolve({})
      const value = answer(request)
      return value instanceof Promise ? value : Promise.resolve(value)
    },
    command(name: string, request: Record<string, unknown>) {
      commands.push({ name, request })
      const answer = answers[name]
      if (!answer) return Promise.resolve({})
      const value = answer(request)
      return value instanceof Promise ? value : Promise.resolve(value)
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, queries, commands }
}

const mounted: VueWrapper[] = []
afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
})

function mountWith(component: unknown, transport: ShellTransport, pinia = createPinia()) {
  const wrapper = mount(component as never, {
    global: {
      plugins: [pinia, router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  mounted.push(wrapper)
  return wrapper
}

async function mountAt(
  component: unknown,
  path: string,
  transport: ShellTransport,
  pinia = createPinia(),
) {
  await router.push(path)
  await router.isReady()
  const wrapper = mountWith(component, transport, pinia)
  await flushPromises()
  return wrapper
}

describe('Project settings never carry a draft across a route change', () => {
  const projects = [project(1, 'CORE', 'Control plane'), project(5, 'EDGE', 'Edge tooling')]

  function settingsHarness(hold?: Promise<{ projects: ProjectRecord[] }>) {
    let lists = 0
    return harness({
      'project.list': () => {
        lists += 1
        return lists === 2 && hold ? hold : { projects }
      },
      'initiative.list': () => ({ initiatives }),
      'project.update': (request) => ({ ...projects[1], ...request, version: 2 }),
    })
  }

  it('submits the arrived Project’s own values while its refresh is still held', async () => {
    const held = deferred<{ projects: ProjectRecord[] }>()
    const state = settingsHarness(held.promise)
    const wrapper = await mountAt(ProjectSettingsView, '/projects/1/settings', state.transport)

    await router.push('/projects/5/settings')
    await nextTick()
    expect(wrapper.get('[data-testid="settings-code"]').text()).toBe('EDGE')
    await wrapper.get('[data-testid="settings-save"]').trigger('submit')
    await flushPromises()
    held.settle({ projects })
    await flushPromises()

    expect(state.commands).toHaveLength(1)
    expect(state.commands[0]?.request).toMatchObject({ project_id: 5, name: 'Edge tooling' })
  })

  it('offers no settings form until the arrived Project’s own draft is taken', async () => {
    const held = deferred<{ projects: ProjectRecord[] }>()
    const state = harness({
      'project.list': () => held.promise,
      'initiative.list': () => ({ initiatives }),
    })
    await router.push('/projects/1/settings')
    await router.isReady()
    const wrapper = mountWith(ProjectSettingsView, state.transport)
    await nextTick()

    expect(wrapper.find('[data-testid="settings-save"]').exists()).toBe(false)

    held.settle({ projects })
    await flushPromises()
    expect(wrapper.find('[data-testid="settings-save"]').exists()).toBe(true)
  })

  it('drops a settings answer that lands after the route named another Project', async () => {
    const held = deferred<ProjectRecord>()
    const state = harness({
      'project.list': () => ({ projects }),
      'initiative.list': () => ({ initiatives }),
      'project.update': () => held.promise,
    })
    const wrapper = await mountAt(ProjectSettingsView, '/projects/1/settings', state.transport)

    await wrapper.get('[data-testid="settings-name"]').setValue('Renamed control plane')
    await wrapper.get('[data-testid="settings-save"]').trigger('submit')
    await router.push('/projects/5/settings')
    await flushPromises()
    held.settle({ ...projects[0], name: 'Renamed control plane', version: 5 })
    await flushPromises()

    expect(state.commands.map((entry) => entry.name)).toEqual(['project.update'])
    expect(state.commands[0]?.request).toMatchObject({ project_id: 1 })
    expect(wrapper.find('[data-testid="settings-saved"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="settings-code"]').text()).toBe('EDGE')
  })

  it('never sends the Project the route left, whatever the draft still holds', async () => {
    const state = settingsHarness()
    const wrapper = await mountAt(ProjectSettingsView, '/projects/1/settings', state.transport)

    await wrapper.get('[data-testid="settings-name"]').setValue('Control plane edited')
    await router.push('/projects/5/settings')
    await flushPromises()
    await wrapper.get('[data-testid="settings-save"]').trigger('submit')
    await flushPromises()

    expect(state.commands).toHaveLength(1)
    expect(state.commands[0]?.request).toMatchObject({
      project_id: 5,
      name: 'Edge tooling',
      default_branch: 'main',
    })
  })
})

describe('Planning takes its scope from the route, never from what was retained', () => {
  const projects = [project(1, 'ONE', 'First project'), project(2, 'TWO', 'Second project')]

  function planningHarness(
    overrides: Record<string, (request: Record<string, unknown>) => unknown> = {},
  ) {
    return harness({
      'project.list': () => ({ projects }),
      'plan.list': (request) => ({
        plans: request.project_id === 1 ? [plan(100, 1)] : [plan(200, 2)],
      }),
      'plan.get': (request) => ({
        plan: request.plan_id === 100 ? plan(100, 1) : plan(200, 2),
        versions: [],
      }),
      'plan.diagnostics': () => ({
        blocking: false,
        cycles: [],
        coverage_gaps: [],
        invalid_profiles: [],
      }),
      'spec.list': (request) => ({
        specs: request.project_id === 1 ? [spec(11, 1)] : [spec(22, 2)],
      }),
      'spec.coverage.matrix': (request) => ({
        spec_id: request.spec_id,
        version: 1,
        stories: [],
      }),
      'ticket.list': () => ({ tickets: [] }),
      'ticket.graph.list': (request) => ({
        proposals: request.spec_id === 11 ? [proposal(1, 11)] : [proposal(9, 22)],
      }),
      ...overrides,
    })
  }

  function retainedProjectOne() {
    const pinia = createPinia()
    setActivePinia(pinia)
    const register = useProjectRegisterStore()
    register.projects = projects
    register.loaded = true
    const editor = usePlanEditorStore()
    editor.plans = [plan(100, 1)]
    editor.projectId = 1
    editor.selectedPlanId = 100
    editor.loaded = true
    const matrix = useCoverageMatrixStore()
    matrix.specs = [spec(11, 1)]
    matrix.projectId = 1
    matrix.pickedSpecId = 11
    const graphs = useGraphProposalsStore()
    graphs.specId = 11
    graphs.proposals = [proposal(1, 11)]
    graphs.loaded = true
    return pinia
  }

  it('exposes no retained Plan on a fresh mount whose route names another Project', async () => {
    const held = deferred<{ projects: ProjectRecord[] }>()
    const state = planningHarness({ 'project.list': () => held.promise })
    const pinia = retainedProjectOne()
    await router.push('/planning?project=2')
    await router.isReady()
    const wrapper = mountWith(PlanningView, state.transport, pinia)
    await nextTick()

    expect(wrapper.find('[data-testid="plan-editor"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="plan-activate"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="graph-approve-1"]').exists()).toBe(false)
    expect(state.commands).toEqual([])

    held.settle({ projects })
    await flushPromises()
    expect(state.commands).toEqual([])
  })

  it('exposes no retained Spec proposal when the route names another Spec', async () => {
    const held = deferred<{ projects: ProjectRecord[] }>()
    const state = planningHarness({ 'project.list': () => held.promise })
    const pinia = retainedProjectOne()
    await router.push('/planning?project=1&spec=22')
    await router.isReady()
    const wrapper = mountWith(PlanningView, state.transport, pinia)
    await nextTick()

    expect(wrapper.find('[data-testid="graph-proposal-1"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="graph-approve-1"]').exists()).toBe(false)

    held.settle({ projects })
    await flushPromises()
    expect(state.commands).toEqual([])
  })

  it('exposes no retained Plan when a remount keeps the shared stores', async () => {
    const state = planningHarness()
    const pinia = retainedProjectOne()
    const first = await mountAt(PlanningView, '/planning?project=1', state.transport, pinia)
    expect(first.find('[data-testid="plan-editor"]').exists()).toBe(true)
    first.unmount()
    mounted.splice(mounted.indexOf(first), 1)

    const held = deferred<{ projects: ProjectRecord[] }>()
    const later = planningHarness({ 'project.list': () => held.promise })
    await router.push('/planning?project=2')
    await router.isReady()
    const wrapper = mountWith(PlanningView, later.transport, pinia)
    await nextTick()

    expect(wrapper.find('[data-testid="plan-editor"]').exists()).toBe(false)
    expect(later.commands).toEqual([])
    held.settle({ projects })
    await flushPromises()
  })

  it('drops a held Project read once a later link names a different Project', async () => {
    const held = deferred<{ plans: PlanRecord[] }>()
    const state = planningHarness({
      'plan.list': (request) =>
        request.project_id === 1 ? held.promise : { plans: [plan(200, 2)] },
    })
    const wrapper = await mountAt(PlanningView, '/planning?project=1', state.transport)

    await router.push('/planning?project=2')
    await flushPromises()
    held.settle({ plans: [plan(100, 1)] })
    await flushPromises()

    expect(wrapper.findAll('[data-testid^="plan-row-"]').map((row) => row.attributes('data-testid')))
      .toEqual(['plan-row-200'])
    expect(state.commands).toEqual([])
  })

  it('keeps the latest shared Project list after an older route read answers', async () => {
    const older = deferred<{ projects: ProjectRecord[] }>()
    let lists = 0
    const state = planningHarness({
      'project.list': () => {
        lists += 1
        return lists === 1 ? older.promise : { projects: [projects[1]!] }
      },
    })
    const pinia = createPinia()
    setActivePinia(pinia)
    await router.push('/planning?project=1')
    await router.isReady()
    const wrapper = mountWith(PlanningView, state.transport, pinia)
    await nextTick()

    await router.push('/planning?project=2')
    await flushPromises()
    older.settle({ projects: [projects[0]!] })
    await flushPromises()

    expect(useProjectRegisterStore().projects.map((entry) => entry.id)).toEqual([2])
    expect((wrapper.get('[data-testid="planning-project"]').element as HTMLSelectElement).value)
      .toBe('2')
  })

  it('drops the retained Plan when a link names another Plan on the same route', async () => {
    const state = planningHarness({
      'plan.list': () => ({ plans: [plan(100, 1), plan(101, 1, 2)] }),
      'plan.get': (request) => ({
        plan: request.plan_id === 100 ? plan(100, 1) : plan(101, 1, 2),
        versions: [],
      }),
    })
    const wrapper = await mountAt(PlanningView, '/planning?project=1&plan=100', state.transport)
    expect(wrapper.get('[data-testid="plan-title"]').text()).toBe('ONE-P1')

    await router.push('/planning?project=1&plan=101')
    await nextTick()
    expect(wrapper.find('[data-testid="plan-editor"]').exists()).toBe(false)

    await flushPromises()
    expect(wrapper.get('[data-testid="plan-title"]').text()).toBe('ONE-P2')
    expect(state.commands).toEqual([])
  })

  it('keeps same-Project Plan navigation after an older Plan command answers', async () => {
    const held = deferred<PlanRecord>()
    const state = planningHarness({
      'plan.list': () => ({ plans: [plan(100, 1), plan(101, 1, 2)] }),
      'plan.get': (request) => ({
        plan: request.plan_id === 100 ? plan(100, 1) : plan(101, 1, 2),
        versions: [],
      }),
      'plan.activate': () => held.promise,
    })
    const wrapper = await mountAt(PlanningView, '/planning?project=1&plan=100', state.transport)

    await wrapper.get('[data-testid="plan-activate"]').trigger('submit')
    await router.push('/planning?project=1&plan=101')
    await flushPromises()
    held.settle(plan(100, 1))
    await flushPromises()

    expect(state.commands).toEqual([
      expect.objectContaining({ name: 'plan.activate', request: expect.objectContaining({ plan_id: 100 }) }),
    ])
    expect(wrapper.get('[data-testid="plan-title"]').text()).toBe('ONE-P2')
    expect(
      state.queries
        .filter((entry) => entry.name === 'plan.get')
        .map((entry) => entry.request.plan_id),
    ).toEqual([100, 101])
  })

  it('drops a held Plan command once the route names another Project', async () => {
    const held = deferred<PlanRecord>()
    const state = planningHarness({ 'plan.activate': () => held.promise })
    const wrapper = await mountAt(PlanningView, '/planning?project=1&plan=100', state.transport)

    await wrapper.get('[data-testid="plan-activate"]').trigger('submit')
    await router.push('/planning?project=2')
    await flushPromises()
    held.settle(plan(100, 1))
    await flushPromises()

    expect(state.commands.map((entry) => entry.name)).toEqual(['plan.activate'])
    expect((wrapper.get('[data-testid="planning-project"]').element as HTMLSelectElement).value)
      .toBe('2')
    expect(wrapper.findAll('[data-testid^="plan-row-"]').map((row) => row.attributes('data-testid')))
      .toEqual(['plan-row-200'])
  })

  it('drops a graph approval answered after the route named another Spec', async () => {
    const held = deferred<TicketGraphRecord>()
    const state = planningHarness({
      'ticket.graph.approve': () => held.promise,
    })
    const wrapper = await mountAt(PlanningView, '/planning?project=1&spec=11', state.transport)

    await wrapper.get('[data-testid="graph-approve-1"]').trigger('click')
    await router.push('/planning?project=2&spec=22')
    await flushPromises()
    held.settle({ ...proposal(1, 11), state: 'approved', version: 2 })
    await flushPromises()

    expect(state.commands.map((entry) => entry.name)).toEqual(['ticket.graph.approve'])
    expect((wrapper.get('[data-testid="coverage-spec"]').element as HTMLSelectElement).value)
      .toBe('22')
    expect(wrapper.find('[data-testid="graph-proposal-9"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="graph-proposal-1"]').exists()).toBe(false)
  })

  it('refuses a Plan command the arrived Project does not own', async () => {
    setActivePinia(createPinia())
    const state = planningHarness()
    const editor = usePlanEditorStore()
    await editor.refresh(state.transport, 1)
    editor.select(100)
    await editor.refresh(state.transport, 2)

    await editor.activate(state.transport)

    expect(state.commands).toEqual([])
  })

  it('drops a Plan command answered after the store took another Project', async () => {
    setActivePinia(createPinia())
    const held = deferred<PlanRecord>()
    const state = planningHarness({ 'plan.activate': () => held.promise })
    const editor = usePlanEditorStore()
    await editor.refresh(state.transport, 1)
    editor.select(100)

    const submitted = editor.activate(state.transport)
    editor.clear()
    await editor.refresh(state.transport, 2)
    held.settle(plan(100, 1))
    await submitted

    expect(editor.plans.map((entry) => entry.project_id)).toEqual([2])
    expect(editor.selectedPlanId).not.toBe(100)
  })

  it('drops a graph approval answered after the store took another Spec', async () => {
    setActivePinia(createPinia())
    const held = deferred<TicketGraphRecord>()
    const state = planningHarness({ 'ticket.graph.approve': () => held.promise })
    const graphs = useGraphProposalsStore()
    await graphs.load(state.transport, 11)

    const submitted = graphs.approve(state.transport, proposal(1, 11))
    graphs.clear()
    await graphs.load(state.transport, 22)
    held.settle({ ...proposal(1, 11), state: 'approved', version: 2 })
    await submitted

    expect(graphs.specId).toBe(22)
    expect(graphs.proposals.map((entry) => entry.id)).toEqual([9])
  })

  it('refuses a graph approval the Spec on display does not own', async () => {
    setActivePinia(createPinia())
    const state = planningHarness()
    const graphs = useGraphProposalsStore()
    await graphs.load(state.transport, 22)

    const landed = await graphs.approve(state.transport, proposal(1, 11))

    expect(landed).toBe(false)
    expect(state.commands).toEqual([])
  })
})

describe('Activity derives its Role filter from the route in both directions', () => {
  const projects = [project(3, 'CORE', 'Control plane'), project(4, 'EDGE', 'Edge tooling')]

  const events = [
    {
      id: 1,
      scope: { project: 3 },
      kind: 'telemetry',
      entity: null,
      recorded_at: '2026-09-14T06:00:00Z',
      detail: { role: 'reviewer' },
    },
    {
      id: 2,
      scope: { project: 3 },
      kind: 'telemetry',
      entity: null,
      recorded_at: '2026-09-14T06:01:00Z',
      detail: { role: 'implementer' },
    },
  ]

  function activityHarness(
    overrides: Record<string, (request: Record<string, unknown>) => unknown> = {},
  ) {
    return harness({
      'health.get': () => ({
        connected: true,
        service_version: '0.1.0',
        service: { started_at: '2026-09-14T06:00:00Z' },
        database: { journal_mode: 'wal', last_change_at: null, schema_version: 1 },
        scheduler: { last_backup_success_at: null },
        mcp: { exposed_tools: 0 },
        herdr: { connection_diagnostic: null, sessions: [] },
        workspaces: {
          by_health: { assigned: 0, available: 0, dirty: 0, missing: 0, retired: 0, unobserved: 0 },
          last_change_at: null,
        },
      }),
      'project.list': () => ({ projects }),
      'timeline.query': () => ({ events }),
      'ruling.list': () => ({ rulings: [] }),
      'deferral.list': () => ({ deferrals: [] }),
      ...overrides,
    })
  }

  function timelineRequests(queries: Array<{ name: string; request: Record<string, unknown> }>) {
    return queries.filter((entry) => entry.name === 'timeline.query').map((entry) => entry.request)
  }

  it('clears the Role filter when the same surface leaves a role link', async () => {
    const state = activityHarness()
    const wrapper = await mountAt(HomeView, '/activity?project=3&role=reviewer', state.transport)
    expect(timelineRequests(state.queries).at(-1)?.kinds).toEqual(['telemetry'])

    await router.push('/activity?project=3')
    await flushPromises()

    expect(timelineRequests(state.queries).at(-1)?.kinds).toBeUndefined()
    expect(wrapper.find('[data-testid="activity-role"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="timeline-event-1"]').attributes('data-linked')).toBeUndefined()
    expect(state.commands).toEqual([])
  })

  it('follows a role replaced on the mounted surface, marking only its rows', async () => {
    const state = activityHarness()
    const wrapper = await mountAt(HomeView, '/activity?project=3&role=reviewer', state.transport)
    expect(wrapper.get('[data-testid="timeline-event-1"]').attributes('data-linked')).toBe('true')

    await router.push('/activity?project=3&role=implementer')
    await flushPromises()

    expect(timelineRequests(state.queries).at(-1)?.kinds).toEqual(['telemetry'])
    expect(wrapper.get('[data-testid="timeline-event-1"]').attributes('data-linked')).toBeUndefined()
    expect(wrapper.get('[data-testid="timeline-event-2"]').attributes('data-linked')).toBe('true')
    expect(state.commands).toEqual([])
  })

  it('adds the Role filter when a link names a role on a surface already open', async () => {
    const state = activityHarness()
    const wrapper = await mountAt(HomeView, '/activity?project=3', state.transport)
    expect(timelineRequests(state.queries).at(-1)?.kinds).toBeUndefined()

    await router.push('/activity?project=3&role=reviewer')
    await flushPromises()

    expect(timelineRequests(state.queries).at(-1)?.kinds).toEqual(['telemetry'])
    expect(wrapper.get('[data-testid="activity-role"]').text()).toContain('reviewer')
    expect(state.commands).toEqual([])
  })

  it('keeps a kind filter the operator chose while a role was traced', async () => {
    const state = activityHarness()
    const wrapper = await mountAt(HomeView, '/activity?project=3&role=reviewer', state.transport)

    const kinds = wrapper.get('[data-testid="timeline-filter-kinds"]')
    await kinds.setValue(['run', 'review'])
    await wrapper.get('[data-testid="timeline-apply-filters"]').trigger('submit')
    await flushPromises()
    await router.push('/activity?project=3')
    await flushPromises()

    expect(timelineRequests(state.queries).at(-1)?.kinds).toEqual(['run', 'review'])
    expect(state.commands).toEqual([])
  })

  it('never shows the Project the route left, even on a remount', async () => {
    const pinia = createPinia()
    const state = activityHarness({
      'timeline.query': (request) => ({
        events: (request.scope as { project: number }).project === 3 ? events : [],
      }),
    })
    const first = await mountAt(HomeView, '/activity?project=3', state.transport, pinia)
    expect(first.find('[data-testid="timeline-event-1"]').exists()).toBe(true)
    first.unmount()
    mounted.splice(mounted.indexOf(first), 1)

    const wrapper = await mountAt(HomeView, '/activity?project=4', state.transport, pinia)

    expect(wrapper.find('[data-testid="timeline-event-1"]').exists()).toBe(false)
    expect(timelineRequests(state.queries).at(-1)?.scope).toEqual({ project: 4 })
    expect(state.commands).toEqual([])
  })

  it('rejects a slower timeline answer from the Project the store left', async () => {
    setActivePinia(createPinia())
    const one = deferred<{ events: typeof events }>()
    const two = deferred<{ events: typeof events }>()
    const transport = {
      query(_name: string, request: { scope: { project?: number } }) {
        return request.scope.project === 3 ? one.promise : two.promise
      },
    } as unknown as ShellTransport
    const timeline = useTimelineStore()
    const newer = [{ ...events[0], id: 7, scope: { project: 4 } }]

    const older = timeline.load(transport, { project: 3 })
    const later = timeline.load(transport, { project: 4 })
    two.settle({ events: newer })
    await later
    one.settle({ events })
    await older

    expect(timeline.scope).toEqual({ project: 4 })
    expect(timeline.events.map((event) => event.id)).toEqual([7])
  })
})
