// The card regions and their chips, rendered from real Tickets: every
// card shows number, kind, title, Project code, priority, and
// progress, and each kind adds its own chips from the closed
// vocabulary (KAN-T26-AC1, KAN-T26-AC2, KAN-T26-AC3). The Lane chip
// comes from the KAN-T32 Lane contract the generated client serves.
import { mount, flushPromises } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type {
  LaneListResponse,
  ProjectListResponse,
  SpecListResponse,
  TicketListResponse,
  TicketReadinessResponse,
  TicketRecord,
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

// One card per kind, each carrying the facts its chips resolve.
function boardTickets(): TicketRecord[] {
  return [
    ticket({
      id: 7,
      number: 12,
      state: 'ready',
      priority: 'high',
      scheduled_for: '2026-09-12T09:00:00Z',
    }),
    ticket({
      id: 8,
      number: 13,
      kind: 'implementation',
      state: 'active',
      priority: 'urgent',
      title: null,
      slice: 'Serve the lifecycle command surface',
      spec_id: 4,
      criteria: [
        { outcome: 'The commands are served.', stories: ['CORE-S4-US2'] },
        { outcome: 'The client drives them.', stories: ['CORE-S4-US2'] },
        { outcome: 'The core refuses misuse.', stories: ['CORE-S4-US3'] },
      ],
      subtype: null,
      mode: null,
      completion: [],
      profile: 'glm-implementer',
      version: 5,
    }),
    ticket({
      id: 9,
      number: 14,
      kind: 'bug',
      state: 'approved',
      title: 'Clone guard misses a dirty tree',
      criteria: [],
      bug: {
        actual_behaviour: 'The guard lands a dirty tree.',
        evidence_ids: [],
        external_references: [],
        occurrence_snapshots: [],
        qualification: {
          affected_scope: 'The clone guard',
          criteria: [
            { outcome: 'A dirty tree is refused.', stories: ['CORE-S6-US2'] },
            { outcome: 'The refusal is recorded.', stories: ['CORE-S6-US2'] },
          ],
          environment: 'macOS 15',
          expected_behaviour: 'A dirty tree is refused.',
          frequency: 'Intermittent',
          reproduction: 'Claim a dirty clone.',
          risk: 'Landing over uncommitted work.',
          severity: 'high',
          verification_steps: [{ command: 'git status' }],
        },
        reporter_evidence: 'A landing run failed',
      },
      subtype: null,
      mode: null,
      completion: [],
      profile: 'glm-triage',
      version: 2,
    }),
  ]
}

// The KAN-T32 Lane contract: Lane 3 holds the Implementation.
function lanes(): LaneListResponse['lanes'] {
  return [
    { id: 3, project_id: 1, workspace_id: 11, ticket_id: 8, version: 2 },
    { id: 4, project_id: 1, workspace_id: null, ticket_id: null, version: 1 },
  ]
}

// The Project's Specs: row ids every Project shares, numbers each
// mints its own — the identity a card renders is the number.
function specs(): SpecListResponse['specs'] {
  return [
    {
      execution: 'planned',
      id: 4,
      name: 'Serve the lifecycle command surface',
      number: 9,
      plan_id: null,
      project_id: 1,
      version: 2,
    },
    {
      execution: 'ready',
      id: 6,
      name: 'Carry the work through review',
      number: 2,
      plan_id: null,
      project_id: 1,
      version: 3,
    },
  ]
}

async function mounted(
  tickets: TicketRecord[],
  laneList: LaneListResponse['lanes'] = lanes(),
  blockers: Record<number, TicketReadinessResponse['blocked_by']> = {},
  specList: SpecListResponse['specs'] = specs(),
): Promise<{ wrapper: ReturnType<typeof mount>; query: ReturnType<typeof vi.fn> }> {
  document.documentElement.classList.remove('dark')
  localStorage.clear()
  const query = vi.fn((name: string, request: unknown) => {
    if (name === 'project.list') {
      return Promise.resolve({ projects: [project] } satisfies ProjectListResponse)
    }
    if (name === 'lane.list') {
      return Promise.resolve({ lanes: laneList } satisfies LaneListResponse)
    }
    if (name === 'run.list') {
      return Promise.resolve({ project_id: 1, runs: [] })
    }
    if (name === 'spec.list') {
      return Promise.resolve({ specs: specList } satisfies SpecListResponse)
    }
    if (name === 'ticket.readiness') {
      const { ticket_id } = request as { ticket_id: number }
      const blocked_by = blockers[ticket_id] ?? []
      return Promise.resolve({
        blocked_by,
        ready: blocked_by.length === 0,
        state: 'ready',
        ticket_id,
      } satisfies TicketReadinessResponse)
    }
    return Promise.resolve({ tickets } satisfies TicketListResponse)
  })
  const transport = {
    query,
    command: vi.fn(),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  router.push('/projects/1/board')
  await router.isReady()
  const wrapper = mount(BoardView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
  })
  await flushPromises()
  return { wrapper, query }
}

afterEach(() => {
  document.documentElement.classList.remove('dark')
  localStorage.clear()
})

const waiting = (from_number: number): TicketReadinessResponse['blocked_by'][number] => ({
  Ticket: {
    from_number,
    from_project_id: 1,
    from_state: 'active',
    from_ticket_id: from_number,
  },
})

const chip = (wrapper: ReturnType<typeof mount>, kind: string, ticketId: number) =>
  wrapper.find(`[data-testid="card-chip-${kind}-${ticketId}"]`)

describe('board cards', () => {
  it('gives every card the fixed regions: number with Project code, kind, and title', async () => {
    const { wrapper } = await mounted(boardTickets())

    for (const [id, number, kindLabel, title] of [
      [7, 'KAN-T12', 'Task Ticket', 'Archive the old exports'],
      [8, 'KAN-T13', 'Implementation Ticket', 'Serve the lifecycle command surface'],
      [9, 'KAN-T14', 'Bug Ticket', 'Clone guard misses a dirty tree'],
    ] as const) {
      expect(wrapper.find(`[data-testid="card-number-${id}"]`).text()).toBe(number)
      expect(wrapper.find(`[data-testid="card-kind-${id}"]`).text()).toBe(kindLabel)
      expect(wrapper.find(`[data-testid="open-ticket-${id}"]`).text()).toBe(title)
    }
  })

  it('shows the priority and progress chips on every card', async () => {
    const { wrapper } = await mounted(boardTickets())

    expect(chip(wrapper, 'priority', 7).text()).toBe('PriorityHigh')
    expect(chip(wrapper, 'progress', 7).text()).toBe('Progress1 outcomes')
    expect(chip(wrapper, 'priority', 8).text()).toBe('PriorityUrgent')
    expect(chip(wrapper, 'progress', 8).text()).toBe('Progress3 criteria')
    expect(chip(wrapper, 'progress', 9).text()).toBe('Progress2 criteria')
  })

  it('shows an unqualified Bug a progress that invents nothing', async () => {
    const unqualified = ticket({
      id: 10,
      number: 15,
      kind: 'bug',
      state: 'draft',
      title: 'Clone guard misses a dirty tree',
      criteria: [],
      bug: {
        actual_behaviour: 'The guard lands a dirty tree.',
        evidence_ids: [],
        external_references: [],
        occurrence_snapshots: [],
        qualification: null,
        reporter_evidence: 'A landing run failed',
      },
      subtype: null,
      mode: null,
      completion: [],
      profile: null,
    })
    const { wrapper } = await mounted([unqualified])

    // Every card carries progress (DR-BP-08): the unqualified Bug's
    // names its state rather than a count it cannot honestly claim.
    expect(chip(wrapper, 'progress', 10).text()).toBe('ProgressNot yet qualified')
    expect(chip(wrapper, 'progress', 10).attributes('data-tone')).toBe('neutral')
    // Qualification still owns severity and frequency; until it
    // lands, those regions stay off the card.
    expect(chip(wrapper, 'severity', 10).exists()).toBe(false)
    expect(chip(wrapper, 'frequency', 10).exists()).toBe(false)
  })

  it('adds the implementation chips: spec, implementer, lane, and blockers', async () => {
    const { wrapper } = await mounted(boardTickets(), lanes(), { 8: [waiting(3), waiting(5)] })

    expect(chip(wrapper, 'spec', 8).text()).toBe('SpecKAN-S9')
    expect(chip(wrapper, 'implementer', 8).text()).toContain('glm-implementer')
    expect(chip(wrapper, 'blockers', 8).text()).toBe('Blockers2 blockers')
    // Reviewers populate as KAN-S9 lands; the region stays absent
    // until one is named.
    expect(chip(wrapper, 'reviewers', 8).exists()).toBe(false)
  })

  it('renders the Spec\'s minted number, never its row id', async () => {
    // Spec 4 is this Project's ninth: the ids below it belong to
    // other Projects, and a gap between numbers changes nothing.
    const gapped = ticket({
      id: 11,
      number: 15,
      kind: 'implementation',
      state: 'active',
      title: null,
      slice: 'Carry the work through review',
      spec_id: 6,
      criteria: [],
      subtype: null,
      mode: null,
      completion: [],
      profile: 'glm-implementer',
    })
    const { wrapper } = await mounted([gapped])

    expect(chip(wrapper, 'spec', 11).text()).toBe('SpecKAN-S2')
  })

  it('omits the Spec chip when the record does not resolve', async () => {
    // The Ticket names a Spec the board did not load; the card
    // invents no identity from the id (KAN-T126-AC2).
    const orphan = ticket({
      id: 12,
      number: 16,
      kind: 'implementation',
      state: 'active',
      title: null,
      slice: 'Serve a Spec gone missing',
      spec_id: 99,
      criteria: [],
      subtype: null,
      mode: null,
      completion: [],
      profile: 'glm-implementer',
    })
    const { wrapper } = await mounted([orphan])

    expect(chip(wrapper, 'spec', 12).exists()).toBe(false)
  })

  it('populates the Lane chip from the KAN-T32 Lane contract', async () => {
    const { wrapper } = await mounted(boardTickets())

    // The chip comes from `lane.list`, the landed Lane application
    // contract — not from any local board state.
    expect(chip(wrapper, 'lane', 8).text()).toBe('LaneLane 3')
    // A Ticket no Lane holds carries no Lane chip.
    expect(chip(wrapper, 'lane', 7).exists()).toBe(false)
    expect(chip(wrapper, 'lane', 9).exists()).toBe(false)
  })

  it('adds the bug chips: severity, frequency, origin, and profiles', async () => {
    const { wrapper } = await mounted(boardTickets())

    expect(chip(wrapper, 'severity', 9).text()).toBe('SeverityHigh')
    expect(chip(wrapper, 'severity', 9).attributes('data-tone')).toBe('caution')
    expect(chip(wrapper, 'frequency', 9).text()).toBe('FrequencyIntermittent')
    expect(chip(wrapper, 'origin', 9).text()).toBe('OriginA landing run failed')
    expect(chip(wrapper, 'profiles', 9).text()).toContain('glm-triage')
    // A standalone Bug carries no Spec.
    expect(chip(wrapper, 'spec', 9).exists()).toBe(false)
  })

  it('adds the task chips: subtype, mode, schedule, and executor', async () => {
    const { wrapper } = await mounted(boardTickets())

    expect(chip(wrapper, 'subtype', 7).text()).toBe('SubtypeOperational')
    expect(chip(wrapper, 'mode', 7).text()).toBe('ModeHuman')
    expect(chip(wrapper, 'schedule', 7).text()).toBe('Scheduled2026-09-12')
    expect(chip(wrapper, 'executor', 7).text()).toBe('ExecutorOperator')
    // A Task attaches to no Spec and holds no blockers here.
    expect(chip(wrapper, 'spec', 7).exists()).toBe(false)
    expect(chip(wrapper, 'blockers', 7).exists()).toBe(false)
  })

  it('keeps one kind of chip off another kind of card', async () => {
    const { wrapper } = await mounted(boardTickets())

    // The Task carries no severity; the Bug carries no Lane; the
    // Implementation carries no subtype — the vocabulary decides.
    expect(chip(wrapper, 'severity', 7).exists()).toBe(false)
    expect(chip(wrapper, 'lane', 9).exists()).toBe(false)
    expect(chip(wrapper, 'subtype', 8).exists()).toBe(false)
  })
})
