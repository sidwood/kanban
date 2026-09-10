// One asynchronous scope class (KAN-T140-AC1 to KAN-T140-AC6): when a
// link changes the Project, Plan, Spec, Run, or Activity context of a
// surface that is already mounted, the surface must follow it, and no
// request issued for the scope the operator left may write over the
// scope they are in. Navigating is a read throughout: none of these
// cases issues a command.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, describe, expect, it } from 'vitest'
import type { ProjectRecord, RunRecord, TicketRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import HomeView from './HomeView.vue'
import PlanningView from './PlanningView.vue'
import ProfilesView from './ProfilesView.vue'
import WorkspacesView from './WorkspacesView.vue'

function project(id: number, code: string): ProjectRecord {
  return {
    id,
    code,
    name: `${code} project`,
    repository: `/repositories/${code.toLowerCase()}`,
    seed_workspace: `/workspaces/${code.toLowerCase()}.seed`,
    default_branch: 'main',
    herdr_session: null,
    herdr_workspace: `${code.toLowerCase()}.seed`,
    initiative_id: null,
    archived: false,
    counters: { plan: 1, spec: 1, ticket: 1 },
    version: 1,
  }
}

const projects = [project(1, 'ONE'), project(2, 'TWO')]

function ticket(id: number, projectId: number): TicketRecord {
  return {
    id,
    project_id: projectId,
    number: id,
    kind: 'task',
    priority: 'normal',
    state: 'draft',
    title: `Ticket ${id}`,
    subtype: 'operational',
    mode: 'agent',
    completion: ['Done'],
    criteria: [],
    bug: null,
    pinned_spec_version: null,
    profile: null,
    version: 1,
  } as unknown as TicketRecord
}

function snapshot(name: string) {
  return { name, harness: 'claude-code', model: 'opus', effort: 'high', usage_pool: 'operator' }
}

function run(id: number, projectId: number, ticketId: number): RunRecord {
  return {
    id,
    project_id: projectId,
    ticket_id: ticketId,
    dispatch_request_id: id,
    requested: snapshot('deep'),
    effective: snapshot('standard'),
    fallback: true,
    fallback_path: ['deep', 'standard'],
    status: 'superseded',
    created_at: 1789000000,
    version: 1,
  }
}

/** One transport whose answers are steered per operation, recording
 * every call so a test can prove which scope was asked for and that
 * nothing was mutated. */
function harness(answers: Record<string, (request: Record<string, unknown>) => unknown>) {
  const queries: Array<{ name: string; request: Record<string, unknown> }> = []
  const commands: Array<{ name: string; request: Record<string, unknown> }> = []
  const transport = {
    query(name: string, request: Record<string, unknown>) {
      queries.push({ name, request })
      const answer = answers[name]
      if (!answer) return Promise.reject(new Error(`unexpected query ${name}`))
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

/** A promise a test resolves by hand, so one scope's answer can be
 * held back until a later scope has already landed. */
function deferred<T>() {
  let settle!: (value: T) => void
  const promise = new Promise<T>((resolve) => {
    settle = resolve
  })
  return { promise, settle }
}

const mounted: VueWrapper[] = []
afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
})

async function mountAt(component: unknown, path: string, transport: ShellTransport) {
  await router.push(path)
  await router.isReady()
  const wrapper = mount(component as never, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  mounted.push(wrapper)
  await flushPromises()
  return wrapper
}

function planningAnswers(overrides: Record<string, (request: Record<string, unknown>) => unknown> = {}) {
  return {
    'project.list': () => ({ projects }),
    'plan.list': (request: Record<string, unknown>) => ({
      plans: [
        {
          id: (request.project_id as number) * 100,
          project_id: request.project_id,
          number: 1,
          state: 'draft',
          spec_numbers: [],
          edges: [],
          version: 1,
        },
      ],
    }),
    'plan.get': (request: Record<string, unknown>) => ({
      plan: {
        id: request.plan_id,
        project_id: Math.floor((request.plan_id as number) / 100),
        number: 1,
        state: 'draft',
        spec_numbers: [],
        edges: [],
        version: 1,
      },
      versions: [],
    }),
    'plan.diagnostics': () => ({
      blocking: false,
      cycles: [],
      coverage_gaps: [],
      invalid_profiles: [],
    }),
    'spec.list': (request: Record<string, unknown>) => ({
      specs: [
        {
          id: (request.project_id as number) * 11,
          project_id: request.project_id,
          number: 1,
          name: `Spec of ${request.project_id}`,
          execution: 'planned',
          plan_id: null,
          version: 1,
        },
      ],
    }),
    'spec.coverage.matrix': (request: Record<string, unknown>) => ({
      spec_id: request.spec_id,
      version: 1,
      stories: [],
    }),
    'ticket.list': (request: Record<string, unknown>) => ({
      tickets: [ticket((request.project_id as number) * 10, request.project_id as number)],
    }),
    'ticket.graph.list': () => ({ proposals: [] }),
    ...overrides,
  }
}

describe('Planning follows a link that changes its scope', () => {
  it('adopts the exact Project and Spec a same-component link names', async () => {
    const state = harness(planningAnswers())
    const wrapper = await mountAt(PlanningView, '/planning?project=1&spec=11', state.transport)

    expect((wrapper.get('[data-testid="planning-project"]').element as HTMLSelectElement).value).toBe('1')

    await router.push('/planning?project=2&spec=22')
    await flushPromises()

    expect((wrapper.get('[data-testid="planning-project"]').element as HTMLSelectElement).value).toBe('2')
    expect(
      state.queries.some((entry) => entry.name === 'spec.list' && entry.request.project_id === 2),
    ).toBe(true)
    expect((wrapper.get('[data-testid="coverage-spec"]').element as HTMLSelectElement).value).toBe('22')
    expect(state.commands).toEqual([])
  })

  it('opens the exact Plan a same-component link names', async () => {
    const state = harness(planningAnswers())
    const wrapper = await mountAt(PlanningView, '/planning?project=1', state.transport)

    await router.push('/planning?project=1&plan=100')
    await flushPromises()

    expect(wrapper.get('[data-testid="plan-title"]').text()).toBe('ONE-P1')
    expect(state.commands).toEqual([])
  })

  it('never lets the Project left behind write over the Project arrived at', async () => {
    const slow = deferred<unknown>()
    const state = harness(
      planningAnswers({
        'ticket.list': (request: Record<string, unknown>) =>
          request.project_id === 1
            ? slow.promise
            : { tickets: [ticket(20, 2)] },
      }),
    )
    const wrapper = await mountAt(PlanningView, '/planning?project=1&spec=11', state.transport)

    await router.push('/planning?project=2&spec=22')
    await flushPromises()
    slow.settle({ tickets: [ticket(10, 1)] })
    await flushPromises()

    expect((wrapper.get('[data-testid="planning-project"]').element as HTMLSelectElement).value).toBe('2')
    expect(wrapper.get('[data-testid="coverage-spec"]').findAll('option').map((option) => option.text())).toEqual([
      'TWO-S1 — Spec of 2',
    ])
    expect(state.commands).toEqual([])
  })
})

function workspaceAnswers(overrides: Record<string, (request: Record<string, unknown>) => unknown> = {}) {
  return {
    'project.list': () => ({ projects }),
    'workspace.list': (request: Record<string, unknown>) => ({
      workspaces: [
        {
          id: (request.project_id as number) * 5,
          project_id: request.project_id,
          path: `/workspaces/${request.project_id}`,
          is_seed: false,
          health: 'available',
          observation: {
            repository_identity: 'identity',
            checkout: 'branch',
            branch: 'main',
            head: 'abc123',
            working_tree_clean: true,
            unique_unlanded_commits: false,
            lane_assignment: null,
          },
          reuse: {
            reusable: true,
            clean: true,
            unassigned: true,
            free_of_unlanded_commits: true,
          },
          version: 1,
        },
      ],
    }),
    'lane.list': (request: Record<string, unknown>) => ({
      lanes: [
        {
          id: (request.project_id as number) * 7,
          project_id: request.project_id,
          workspace_id: null,
          ticket_id: (request.project_id as number) * 10,
          version: 1,
        },
      ],
    }),
    'run.list': (request: Record<string, unknown>) => ({
      project_id: request.project_id,
      runs: [run((request.project_id as number) * 3, request.project_id as number, (request.project_id as number) * 10)],
    }),
    'ticket.list': (request: Record<string, unknown>) => ({
      tickets: [ticket((request.project_id as number) * 10, request.project_id as number)],
    }),
    'capacity.defaults.get': () => ({
      defaults: {
        max_active_per_harness: 4,
        max_active_per_model: 3,
        max_active_per_usage_pool: 2,
        version: 1,
      },
    }),
    'capacity.settings.get': (request: Record<string, unknown>) => ({
      project_id: request.project_id,
      caps: {
        max_active_lanes: 2,
        max_active_per_harness: null,
        max_active_per_model: null,
        max_active_per_usage_pool: null,
        version: 1,
      },
    }),
    ...overrides,
  }
}

describe('Workspaces & Lanes follows its Project route parameter', () => {
  it('loads the Project the route now names', async () => {
    const state = harness(workspaceAnswers())
    const wrapper = await mountAt(WorkspacesView, '/projects/1/workspaces', state.transport)

    expect(wrapper.text()).toContain('Where ONE runs')

    await router.push('/projects/2/workspaces')
    await flushPromises()

    expect(wrapper.text()).toContain('Where TWO runs')
    for (const operation of ['workspace.list', 'lane.list', 'run.list', 'ticket.list']) {
      expect(
        state.queries.some((entry) => entry.name === operation && entry.request.project_id === 2),
        `${operation} must be read for the Project the route names`,
      ).toBe(true)
    }
    expect(wrapper.get('[data-testid="lane-ticket-14"]').text()).toBe('TWO-T20')
    expect(state.commands).toEqual([])
  })

  it('drops a command answered after the Project route changed', async () => {
    const slow = deferred<unknown>()
    const state = harness({
      ...workspaceAnswers(),
      'lane.create': () => slow.promise,
    })
    const wrapper = await mountAt(WorkspacesView, '/projects/1/workspaces', state.transport)

    await wrapper.get('[data-testid="lane-create"]').trigger('submit')
    await router.push('/projects/2/workspaces')
    await flushPromises()
    slow.settle({ id: 99, project_id: 1, workspace_id: null, ticket_id: null, version: 1 })
    await flushPromises()

    expect(state.commands.map((entry) => entry.name)).toEqual(['lane.create'])
    expect(wrapper.get('[data-testid="lane-ticket-14"]').text()).toBe('TWO-T20')
    expect(wrapper.find('[data-testid="lane-row-7"]').exists()).toBe(false)
    expect(
      state.queries.filter(
        (entry) => entry.name === 'lane.list' && entry.request.project_id === 1,
      ).length,
      'the Project left behind must not be read again for a command it no longer owns',
    ).toBe(1)
  })

  it('never labels the Runs of the Project left behind as the Project arrived at', async () => {
    const slow = deferred<unknown>()
    const state = harness(
      workspaceAnswers({
        'run.list': (request: Record<string, unknown>) =>
          request.project_id === 1
            ? slow.promise
            : { project_id: 2, runs: [run(6, 2, 20)] },
      }),
    )
    const wrapper = await mountAt(WorkspacesView, '/projects/1/workspaces', state.transport)

    await router.push('/projects/2/workspaces')
    await flushPromises()
    slow.settle({ project_id: 1, runs: [run(3, 1, 10)] })
    await flushPromises()

    const lane = wrapper.get('[data-testid="lane-attempts-14"]')
    expect(lane.text()).toContain('Run 6')
    expect(lane.text()).not.toContain('Run 3')
    expect(state.commands).toEqual([])
  })
})

function profileAnswers(overrides: Record<string, (request: Record<string, unknown>) => unknown> = {}) {
  return {
    'project.list': () => ({ projects }),
    'profile.list': () => ({
      profiles: [
        {
          name: 'deep',
          harness: 'claude-code',
          model: 'opus',
          effort: 'high',
          usage_pool: 'operator',
          fallback: null,
          retired: false,
          version: 1,
        },
      ],
    }),
    'ticket.list': (request: Record<string, unknown>) => ({
      tickets: [ticket((request.project_id as number) * 10, request.project_id as number)],
    }),
    'run.list': (request: Record<string, unknown>) => ({ project_id: request.project_id, runs: [] }),
    'ticket.assign': (request: Record<string, unknown>) => ({
      ...ticket(request.ticket_id as number, 1),
      profile: request.profile,
      version: 2,
    }),
    ...overrides,
  }
}

describe('Profiles keeps assignment inside the selected Project', () => {
  it('never offers the Tickets of the Project left behind', async () => {
    const slow = deferred<unknown>()
    const state = harness(
      profileAnswers({
        'ticket.list': (request: Record<string, unknown>) =>
          request.project_id === 1 ? slow.promise : { tickets: [ticket(20, 2)] },
      }),
    )
    const wrapper = await mountAt(ProfilesView, '/settings/profiles', state.transport)

    await wrapper.get('[data-testid="assign-project"]').setValue('2')
    await flushPromises()
    slow.settle({ tickets: [ticket(10, 1)] })
    await flushPromises()

    const offered = wrapper
      .get('[data-testid="assign-ticket"]')
      .findAll('option')
      .map((option) => (option.element as HTMLOptionElement).value)
      .filter((value) => value.length > 0)
    expect(offered).toEqual(['20'])
    expect(state.commands).toEqual([])
  })

  it('clears the Ticket picked in the Project left behind', async () => {
    const state = harness(profileAnswers())
    const wrapper = await mountAt(ProfilesView, '/settings/profiles', state.transport)

    await wrapper.get('[data-testid="assign-ticket"]').setValue('10')
    await wrapper.get('[data-testid="assign-project"]').setValue('2')
    await flushPromises()

    expect((wrapper.get('[data-testid="assign-ticket"]').element as HTMLSelectElement).value).toBe('')
    expect(state.commands).toEqual([])
  })

  it('refuses to assign a Ticket the selected Project does not hold', async () => {
    const state = harness(profileAnswers())
    const wrapper = await mountAt(ProfilesView, '/settings/profiles', state.transport)

    await wrapper.get('[data-testid="assign-ticket"]').setValue('10')
    await wrapper.get('[data-testid="assign-profile"]').setValue('deep')
    await wrapper.get('[data-testid="assign-project"]').setValue('2')
    await wrapper.get('[data-testid="assign-submit"]').trigger('submit')
    await flushPromises()

    expect(state.commands).toEqual([])
  })

  it('never applies an assignment answered after the Project changed', async () => {
    const slow = deferred<unknown>()
    const state = harness(
      profileAnswers({
        'ticket.assign': () => slow.promise,
      }),
    )
    const wrapper = await mountAt(ProfilesView, '/settings/profiles', state.transport)

    await wrapper.get('[data-testid="assign-ticket"]').setValue('10')
    await wrapper.get('[data-testid="assign-profile"]').setValue('deep')
    await wrapper.get('[data-testid="assign-submit"]').trigger('submit')
    await wrapper.get('[data-testid="assign-project"]').setValue('2')
    await flushPromises()
    slow.settle({ ...ticket(10, 1), profile: 'deep', version: 2 })
    await flushPromises()

    expect(state.commands.map((entry) => entry.name)).toEqual(['ticket.assign'])
    const rows = wrapper.findAll('[data-testid="assign-tickets"] li').map((row) => row.text())
    expect(rows.join(' ')).toContain('TWO-T20')
    expect(rows.join(' ')).not.toContain('deep')
  })

  it('never labels the Runs of the Project left behind as the Project arrived at', async () => {
    const slow = deferred<unknown>()
    const state = harness(
      profileAnswers({
        'run.list': (request: Record<string, unknown>) =>
          request.project_id === 1 ? slow.promise : { project_id: 2, runs: [] },
      }),
    )
    const wrapper = await mountAt(ProfilesView, '/settings/profiles', state.transport)

    await wrapper.get('[data-testid="assign-project"]').setValue('2')
    await flushPromises()
    slow.settle({ project_id: 1, runs: [run(3, 1, 10)] })
    await flushPromises()

    expect(wrapper.text()).toContain('Effective fallback in TWO')
    expect(wrapper.get('[data-testid="profile-effective-deep"]').text()).toContain('No Run in TWO')
    expect(state.commands).toEqual([])
  })
})

describe('Activity follows the Project a link names', () => {
  it('adopts a Project named after the surface is already mounted', async () => {
    const state = harness({
      'project.list': () => ({ projects }),
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
      'timeline.query': () => ({ events: [] }),
      'ruling.list': () => ({ rulings: [] }),
      'deferral.list': () => ({ deferrals: [] }),
    })
    const wrapper = await mountAt(HomeView, '/activity?project=1', state.transport)

    expect((wrapper.get('[data-testid="home-project-select"]').element as HTMLSelectElement).value).toBe('1')

    await router.push('/activity?project=2')
    await flushPromises()

    expect((wrapper.get('[data-testid="home-project-select"]').element as HTMLSelectElement).value).toBe('2')
    expect(state.commands).toEqual([])
  })
})
