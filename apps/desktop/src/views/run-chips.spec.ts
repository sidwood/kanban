// The run chips: the card's profile region speaking the run records
// the core owns (KAN-S9-US3, DR-EP-04). Before dispatch a card shows
// the planned profile; during execution it shows the effective
// profile the run froze, wearing the fallback indicator when the run
// fell back from what the assignment named (DR-BP-12, DR-BP-13).
import { mount, flushPromises } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type {
  ProfileSnapshotRecord,
  RunListResponse,
  RunRecord,
  TicketListResponse,
  TicketRecord,
} from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useRunsStore } from '../stores/runs'
import BoardView from './BoardView.vue'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'

const project = {
  id: 1,
  code: 'KAN',
  name: 'Control plane',
  repository: '/repositories/kanban',
  seed_workspace: '/workspaces/kanban.seed',
  default_branch: 'main',
  herdr_session: 'kanban-main',
  herdr_workspace: 'kanban.seed',
  initiative_id: null,
  archived: false,
  counters: { plan: 0, spec: 0, ticket: 0 },
  version: 1,
}

const ticket = (overrides: Partial<TicketRecord> = {}): TicketRecord => ({
  id: 7,
  project_id: 1,
  number: 12,
  kind: 'task',
  priority: 'normal',
  state: 'ready',
  spec_id: null,
  title: 'Archive the old exports',
  slice: null,
  criteria: [],
  bug: null,
  subtype: 'operational',
  mode: 'human',
  completion: ['The old exports are archived.'],
  scheduled_for: null,
  due: null,
  profile: null,
  version: 3,
  ...overrides,
})

const snapshot = (name: string, model: string): ProfileSnapshotRecord => ({
  name,
  harness: 'claude-code',
  model,
  effort: 'high',
  usage_pool: 'operator',
})

const run = (overrides: Partial<RunRecord> = {}): RunRecord => ({
  id: 3,
  project_id: 1,
  ticket_id: 8,
  dispatch_request_id: 4,
  status: 'executing',
  requested: snapshot('glm-implementer', 'opus'),
  effective: snapshot('glm-fallback', 'sonnet'),
  fallback: true,
  fallback_path: ['glm-implementer', 'glm-fallback'],
  created_at: 20,
  version: 1,
  ...overrides,
})

// The executing card every test mounts: an active Implementation with
// a planned profile the run may have fallen back from.
const executingTicket = ticket({
  id: 8,
  number: 13,
  kind: 'implementation',
  state: 'active',
  title: null,
  slice: 'Serve the lifecycle command surface',
  spec_id: null,
  subtype: null,
  mode: null,
  completion: [],
  profile: 'glm-implementer',
  version: 5,
})

// A recording transport whose query answers are steerable from the
// test.
function harness(runs: RunRecord[] = [], tickets: TicketRecord[] = [executingTicket]) {
  const query = vi.fn((name: string, request: unknown) => {
    if (name === 'view.list') {
      return Promise.resolve({
        views: [
          {
            id: 1,
            name: 'All work',
            scope: 'global',
            filter: {},
            expanded_groups: [],
            hidden_columns: ['draft'],
            mode: 'board',
            done_placement: 'column',
            sorting: 'priority',
            is_default: true,
            version: 1,
          },
          {
            id: 2,
            name: 'All work',
            scope: { project: 1 },
            filter: { projects: [1] },
            expanded_groups: [],
            hidden_columns: ['draft'],
            mode: 'board',
            done_placement: 'column',
            sorting: 'priority',
            is_default: true,
            version: 1,
          },
        ],
      })
    }
    if (name === 'project.list') {
      return Promise.resolve({ projects: [project] })
    }
    if (name === 'run.list') {
      const { project_id } = request as { project_id: number }
      return Promise.resolve({
        project_id,
        runs,
      } satisfies RunListResponse)
    }
    if (name === 'lane.list') {
      return Promise.resolve({ lanes: [] })
    }
    if (name === 'spec.list') {
      return Promise.resolve({ specs: [] })
    }
    if (name === 'ticket.readiness') {
      const { ticket_id } = request as { ticket_id: number }
      return Promise.resolve({
        blocked_by: [],
        ready: true,
        state: 'ready',
        ticket_id,
      })
    }
    return Promise.resolve({ tickets } satisfies TicketListResponse)
  })
  const transport = {
    query,
    command: vi.fn(),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, query }
}

describe('runs store', () => {
  it('loads the Project\'s runs through the generated client', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness([run()])
    const runs = useRunsStore()

    await runs.load(transport, 1)

    expect(query).toHaveBeenCalledWith('run.list', { project_id: 1 })
    expect(runs.runs).toHaveLength(1)
    expect(runs.loaded).toBe(true)
    expect(runs.error).toBeNull()
  })

  it('answers the execution facts of the Ticket executing now', async () => {
    setActivePinia(createPinia())
    const { transport } = harness([
      run({ ticket_id: 8, fallback: true }),
      run({ id: 4, ticket_id: 9, fallback: false, effective: snapshot('glm-implementer', 'opus') }),
    ])
    const runs = useRunsStore()
    await runs.load(transport, 1)

    expect(runs.executionFor(8)).toEqual({ effective: 'glm-fallback', fallback: true })
    expect(runs.executionFor(9)).toEqual({ effective: 'glm-implementer', fallback: false })
    // A Ticket with no run — before dispatch — has no execution facts.
    expect(runs.executionFor(7)).toBeNull()
  })
})

describe('BoardView run chips', () => {
  async function mountBoard(transport: ShellTransport) {
    document.documentElement.classList.remove('dark')
    localStorage.clear()
    await router.push('/projects/1/board')
    const wrapper = mount(BoardView, {
      global: {
        plugins: [createPinia(), router],
        provide: { [kanbanTransportKey as symbol]: transport },
      },
    })
    await flushPromises()
    return wrapper
  }

  it('shows the effective profile with the fallback indicator during execution', async () => {
    const { transport } = harness([run({ ticket_id: 8 })])
    const wrapper = await mountBoard(transport)

    const chip = wrapper.find('[data-testid="card-chip-implementer-8"]')
    expect(chip.text()).toContain('glm-fallback')
    expect(wrapper.find('[data-testid="card-fallback-8"]').exists()).toBe(true)
    expect(chip.attributes('title')).toContain('glm-implementer')
  })

  it('shows the planned profile before dispatch, with no fallback indicator', async () => {
    const { transport } = harness([])
    const wrapper = await mountBoard(transport)

    const chip = wrapper.find('[data-testid="card-chip-implementer-8"]')
    expect(chip.text()).toContain('glm-implementer')
    expect(wrapper.find('[data-testid="card-fallback-8"]').exists()).toBe(false)
  })
})
