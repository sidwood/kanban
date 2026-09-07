// KAN-T30-AC2: the detail drawer shows full ticket detail, historical
// attempts, and the embedded timeline from KAN-T9.
import { flushPromises, mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type {
  ProfileSnapshotRecord,
  ProjectListResponse,
  RunListResponse,
  RunRecord,
  SpecListResponse,
  TicketListResponse,
  TicketRecord,
  TimelineQueryResponse,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import BoardView from './BoardView.vue'

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
  id: 8,
  project_id: 1,
  number: 13,
  kind: 'implementation',
  priority: 'normal',
  state: 'active',
  spec_id: 4,
  title: null,
  slice: 'Serve the lifecycle command surface',
  criteria: [{ outcome: 'The commands are served.', stories: ['KAN-S4-US2'] }],
  bug: null,
  subtype: null,
  mode: null,
  completion: [],
  scheduled_for: null,
  due: null,
  profile: 'glm-implementer',
  version: 5,
  ...overrides,
})

const snapshot = (name: string): ProfileSnapshotRecord => ({
  name,
  harness: 'claude-code',
  model: 'opus',
  effort: 'high',
  usage_pool: 'operator',
})

const run = (overrides: Partial<RunRecord> = {}): RunRecord => ({
  id: 3,
  project_id: 1,
  ticket_id: 8,
  dispatch_request_id: 4,
  status: 'executing',
  requested: snapshot('glm-implementer'),
  effective: snapshot('glm-fallback'),
  fallback: true,
  fallback_path: ['glm-implementer', 'glm-fallback'],
  created_at: 20,
  version: 1,
  ...overrides,
})

function harness(options: {
  tickets?: TicketRecord[]
  runs?: RunRecord[]
  ticketGet?: TicketRecord
  timeline?: TimelineQueryResponse
}) {
  const tickets = options.tickets ?? [ticket()]
  const query = vi.fn((name: string, request: unknown) => {
    if (name === 'project.list') {
      return Promise.resolve({ projects: [project] } satisfies ProjectListResponse)
    }
    if (name === 'lane.list') {
      return Promise.resolve({ lanes: [] })
    }
    if (name === 'run.list') {
      const { project_id } = request as { project_id: number }
      return Promise.resolve({
        project_id,
        runs: options.runs ?? [],
      } satisfies RunListResponse)
    }
    if (name === 'spec.list') {
      return Promise.resolve({
        specs: [
          {
            execution: 'planned',
            id: 4,
            name: 'Serve the lifecycle command surface',
            number: 9,
            plan_id: null,
            project_id: 1,
            version: 2,
          },
        ],
      } satisfies SpecListResponse)
    }
    if (name === 'ticket.readiness') {
      const { ticket_id } = request as { ticket_id: number }
      return Promise.resolve({
        blocked_by: [],
        ready: true,
        state: 'active',
        ticket_id,
      })
    }
    if (name === 'ticket.get') {
      const { ticket_id } = request as { ticket_id: number }
      const answer = options.ticketGet ?? tickets.find((entry) => entry.id === ticket_id) ?? ticket()
      return Promise.resolve(answer)
    }
    if (name === 'timeline.query') {
      return Promise.resolve(options.timeline ?? { events: [] } satisfies TimelineQueryResponse)
    }
    return Promise.resolve({ tickets } satisfies TicketListResponse)
  })
  return {
    query,
    transport: {
      query,
      command: vi.fn(),
      subscribe: () => () => undefined,
      onConnectionChange: () => () => undefined,
    } as unknown as ShellTransport,
  }
}

async function openDrawer(
  transport: ShellTransport,
  ticketId = 8,
  testId = `open-ticket-${ticketId}`,
) {
  document.body.innerHTML = ''
  document.documentElement.classList.remove('dark')
  localStorage.clear()
  await router.push('/projects/1/board')
  const wrapper = mount(BoardView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
    attachTo: document.body,
  })
  await flushPromises()
  await wrapper.find(`[data-testid="${testId}"]`).trigger('click')
  await flushPromises()
  return {
    wrapper,
    query: transport.query as ReturnType<typeof vi.fn>,
    unmount: () => wrapper.unmount(),
  }
}

afterEach(() => {
  document.body.innerHTML = ''
  document.documentElement.classList.remove('dark')
  localStorage.clear()
})

describe('ticket drawer', () => {
  it('loads full ticket detail through ticket.get when it opens', async () => {
    const detail = ticket({
      criteria: [
        { outcome: 'The commands are served.', stories: ['KAN-S4-US2'] },
        { outcome: 'The core refuses misuse.', stories: ['KAN-S4-US3'] },
      ],
    })
    const { query } = await openDrawer(harness({ ticketGet: detail }).transport)

    expect(query).toHaveBeenCalledWith('ticket.get', { ticket_id: 8 })
    expect(document.querySelector('[data-testid="drawer-criteria"]')?.textContent).toContain(
      'The commands are served.',
    )
    expect(document.querySelector('[data-testid="drawer-criteria"]')?.textContent).toContain(
      'The core refuses misuse.',
    )
  })

  it('shows task completion criteria the list summary does not carry', async () => {
    const task = ticket({
      id: 7,
      number: 12,
      kind: 'task',
      state: 'ready',
      title: 'Archive the old exports',
      slice: null,
      criteria: [],
      completion: ['The old exports are archived.', 'The audit trail remains.'],
      subtype: 'operational',
      mode: 'human',
      profile: null,
      version: 3,
    })
    const { query } = await openDrawer(
      harness({ tickets: [task], ticketGet: task }).transport,
      7,
    )

    expect(query).toHaveBeenCalledWith('ticket.get', { ticket_id: 7 })
    const completion = document.querySelector('[data-testid="drawer-completion"]')
    expect(completion?.textContent).toContain('The old exports are archived.')
    expect(completion?.textContent).toContain('The audit trail remains.')
  })

  it('lists every historical attempt for the open ticket', async () => {
    const attempts = [
      run({ id: 1, created_at: 10, effective: snapshot('first-run') }),
      run({ id: 2, created_at: 30, effective: snapshot('second-run'), fallback: false }),
    ]
    await openDrawer(harness({ runs: attempts }).transport)

    const history = document.querySelector('[data-testid="drawer-attempts"]')
    expect(history?.textContent).toContain('first-run')
    expect(history?.textContent).toContain('second-run')
    expect(history?.querySelectorAll('[data-testid^="drawer-attempt-"]')).toHaveLength(2)
  })

  it('embeds the timeline scoped to the open ticket', async () => {
    const timeline: TimelineQueryResponse = {
      events: [
        {
          id: 1,
          scope: { project: 1 },
          kind: 'transition',
          entity: { kind: 'ticket', id: '8' },
          recorded_at: '2026-09-04T12:00:01Z',
          detail: { to: 'active' },
        },
      ],
    }
    const { query } = await openDrawer(harness({ timeline }).transport)

    expect(query).toHaveBeenCalledWith(
      'timeline.query',
      expect.objectContaining({
        scope: { project: 1 },
        entity: { kind: 'ticket', id: '8' },
      }),
    )
    expect(document.querySelector('[data-testid="drawer-timeline"]')).not.toBeNull()
    expect(document.querySelector('[data-testid="timeline-event-1"]')?.textContent).toContain(
      'transition',
    )
  })
})
