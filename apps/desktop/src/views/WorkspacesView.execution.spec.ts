// What a Lane is actually running (KAN-T140-AC3, KAN-S6-US2): the
// Ticket it holds, the Run occupying it now, every attempt behind that
// Run, and — the only capacity surface this view carries — the summary
// of the caps that bound the Project.
import { mount, flushPromises } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type {
  CapacityGlobalDefaults,
  CapacityProjectCaps,
  LaneRecord,
  ProjectRecord,
  RunRecord,
  TicketRecord,
  WorkspaceRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import WorkspacesView from './WorkspacesView.vue'

const project: ProjectRecord = {
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
  counters: { plan: 0, spec: 0, ticket: 9 },
  version: 1,
}

const workspace: WorkspaceRecord = {
  id: 1,
  project_id: 1,
  path: '/workspaces/kanban.feature',
  is_seed: false,
  health: 'assigned',
  observation: {
    repository_identity: 'identity',
    checkout: 'branch',
    branch: 'feature',
    head: 'abc123',
    working_tree_clean: true,
    unique_unlanded_commits: false,
    lane_assignment: 1,
  },
  reuse: { reusable: false, clean: true, unassigned: false, free_of_unlanded_commits: true },
  version: 3,
}

const lane: LaneRecord = { id: 1, project_id: 1, workspace_id: 1, ticket_id: 5, version: 3 }

const ticket = {
  id: 5,
  project_id: 1,
  number: 12,
  kind: 'implementation',
  priority: 'high',
  state: 'active',
  spec_id: 2,
  slice: 'Recover the support surfaces.',
  criteria: [],
  bug: null,
  completion: [],
  pinned_spec_version: 1,
  version: 4,
} as unknown as TicketRecord

function snapshot(name: string) {
  return { name, harness: 'claude-code', model: 'opus', effort: 'high', usage_pool: 'operator' }
}

function run(overrides: Partial<RunRecord> = {}): RunRecord {
  return {
    id: 8,
    project_id: 1,
    ticket_id: 5,
    dispatch_request_id: 3,
    requested: snapshot('deep'),
    effective: snapshot('deep'),
    fallback: false,
    fallback_path: [],
    status: 'executing',
    created_at: 1789000000,
    version: 1,
    ...overrides,
  }
}

const defaults: CapacityGlobalDefaults = {
  max_active_per_harness: 4,
  max_active_per_model: 3,
  max_active_per_usage_pool: 2,
  version: 1,
}

const caps: CapacityProjectCaps = {
  max_active_lanes: 2,
  max_active_per_harness: 1,
  max_active_per_model: null,
  max_active_per_usage_pool: null,
  version: 1,
}

interface Options {
  runs?: RunRecord[]
  lanes?: LaneRecord[]
  tickets?: TicketRecord[]
}

function harness(options: Options = {}) {
  const query = vi.fn((name: string) => {
    switch (name) {
      case 'project.list':
        return Promise.resolve({ projects: [project] })
      case 'workspace.list':
        return Promise.resolve({ workspaces: [workspace] })
      case 'lane.list':
        return Promise.resolve({ lanes: options.lanes ?? [lane] })
      case 'ticket.list':
        return Promise.resolve({ tickets: options.tickets ?? [ticket] })
      case 'run.list':
        return Promise.resolve({ project_id: 1, runs: options.runs ?? [run()] })
      case 'capacity.defaults.get':
        return Promise.resolve({ defaults })
      case 'capacity.settings.get':
        return Promise.resolve({ project_id: 1, caps })
      default:
        return Promise.reject(new Error(`unexpected query ${name}`))
    }
  })
  const command = vi.fn(() => Promise.resolve({}))
  const transport = {
    query,
    command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, query, command }
}

const mounted: VueWrapper[] = []
afterEach(() => {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
})

async function mountView(options: Options = {}, path = '/projects/1/workspaces') {
  const state = harness(options)
  await router.push(path)
  await router.isReady()
  const wrapper = mount(WorkspacesView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: state.transport },
    },
  })
  mounted.push(wrapper)
  await flushPromises()
  return { wrapper, ...state }
}

describe('WorkspacesView execution', () => {
  it('names the Ticket a Lane holds and the Run occupying it now', async () => {
    const { wrapper } = await mountView()

    const row = wrapper.get('[data-testid="lane-row-1"]')
    expect(row.get('[data-testid="lane-ticket-1"]').text()).toContain('CORE-T12')
    const current = row.get('[data-testid="lane-run-1"]')
    expect(current.text()).toContain('Run 8')
    expect(current.text()).toContain('deep')
  })

  it('tells the truth about a Run whose effective profile is not the one requested', async () => {
    const { wrapper } = await mountView({
      runs: [
        run({
          requested: snapshot('deep'),
          effective: snapshot('standard'),
          fallback: true,
          fallback_path: ['deep', 'standard'],
        }),
      ],
    })

    const current = wrapper.get('[data-testid="lane-run-1"]')
    expect(current.text()).toContain('requested deep')
    expect(current.text()).toContain('effective standard')
    expect(current.get('[data-testid="lane-run-fallback-1"]').text()).toContain('deep → standard')
  })

  it('keeps settled Runs as attempts behind the current one', async () => {
    const { wrapper } = await mountView({
      runs: [
        run({ id: 6, status: 'superseded' }),
        run({ id: 7, status: 'submitted' }),
        run({ id: 8, status: 'executing' }),
      ],
    })

    expect(wrapper.get('[data-testid="lane-run-1"]').text()).toContain('Run 8')
    const attempts = wrapper.get('[data-testid="lane-attempts-1"]')
    expect(attempts.findAll('[data-testid^="lane-attempt-"]').map((entry) => entry.text())).toEqual([
      expect.stringContaining('Run 8'),
      expect.stringContaining('Run 7'),
      expect.stringContaining('Run 6'),
    ])
  })

  it('says so when a Lane holds a Ticket with no Run yet', async () => {
    const { wrapper } = await mountView({ runs: [] })

    expect(wrapper.find('[data-testid="lane-run-1"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="lane-no-run-1"]').text()).toContain('No Run')
  })

  it('summarises the caps that bound this Project and offers no capacity editing', async () => {
    const { wrapper } = await mountView()

    const summary = wrapper.get('[data-testid="capacity-summary"]')
    expect(summary.get('[data-testid="capacity-lanes"]').text()).toContain('1 of 2')
    expect(summary.get('[data-testid="capacity-harness"]').text()).toContain('1')
    expect(summary.get('[data-testid="capacity-harness"]').text()).toContain('Project')
    expect(summary.get('[data-testid="capacity-model"]').text()).toContain('3')
    expect(summary.get('[data-testid="capacity-model"]').text()).toContain('global')
    expect(summary.findAll('input')).toHaveLength(0)
    expect(summary.findAll('button')).toHaveLength(0)
  })

  it('marks the exact Run a link names', async () => {
    const { wrapper } = await mountView(
      { runs: [run({ id: 6, status: 'superseded' }), run({ id: 8 })] },
      '/projects/1/workspaces?run=6',
    )

    expect(wrapper.get('[data-testid="lane-attempt-6"]').attributes('data-linked')).toBe('true')
    expect(wrapper.get('[data-testid="lane-attempt-8"]').attributes('data-linked')).toBeUndefined()
  })
})
