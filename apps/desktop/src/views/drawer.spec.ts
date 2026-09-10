// KAN-T30-AC2: the detail drawer shows full ticket detail, historical
// attempts, and the embedded timeline from KAN-T9.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type {
  ProfileSnapshotRecord,
  RunRecord,
  TicketRecord,
  TimelineQueryResponse,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import { harness, ticket } from '../test/shell-harness'
import type { HarnessOptions } from '../test/shell-harness'
import BoardView from './BoardView.vue'

// The Implementation every drawer opens on, unless a test names a
// Task instead.
const implementation = (overrides: Partial<TicketRecord> = {}): TicketRecord =>
  ticket({
    id: 8,
    number: 13,
    kind: 'implementation',
    state: 'active',
    spec_id: 4,
    title: null,
    slice: 'Serve the lifecycle command surface',
    criteria: [{ outcome: 'The commands are served.', stories: ['CORE-S4-US2'] }],
    subtype: null,
    mode: null,
    completion: [],
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

// The board over the shared harness, with the detail and timeline
// answers a test steers by hand: the drawer reads its record through
// ticket.get, never from the projection's summary.
function shell(options: {
  tickets?: TicketRecord[]
  runs?: RunRecord[]
  ticketGet?: TicketRecord
  timeline?: TimelineQueryResponse
} = {}) {
  const override: HarnessOptions['override'] = (name) => {
    if (name === 'ticket.get' && options.ticketGet) return Promise.resolve(options.ticketGet)
    if (name === 'timeline.query' && options.timeline) return Promise.resolve(options.timeline)
    return undefined
  }
  return harness({
    tickets: options.tickets ?? [implementation()],
    runs: options.runs ?? [],
    override,
  })
}

const mountedBoards: VueWrapper[] = []

async function openDrawer(transport: ReturnType<typeof harness>['transport'], ticketId = 8) {
  await router.push('/projects/1/board')
  await router.isReady()
  const wrapper = mount(BoardView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
    attachTo: document.body,
  })
  mountedBoards.push(wrapper)
  await flushPromises()
  await wrapper.find(`[data-testid="open-ticket-${ticketId}"]`).trigger('click')
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  localStorage.clear()
  document.documentElement.classList.remove('dark')
})

afterEach(() => {
  for (const wrapper of mountedBoards.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

describe('ticket drawer', () => {
  it('loads full ticket detail through ticket.get when it opens', async () => {
    const detail = implementation({
      criteria: [
        { outcome: 'The commands are served.', stories: ['CORE-S4-US2'] },
        { outcome: 'The core refuses misuse.', stories: ['CORE-S4-US3'] },
      ],
    })
    const { transport, query } = shell({ ticketGet: detail })
    await openDrawer(transport)

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
      completion: ['The old exports are archived.', 'The audit trail remains.'],
    })
    const { transport, query } = shell({ tickets: [task], ticketGet: task })
    await openDrawer(transport, 7)

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
    const { transport } = shell({ runs: attempts })
    await openDrawer(transport)

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
    const { transport, query } = shell({ timeline })
    await openDrawer(transport)

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
