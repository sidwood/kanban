// The card regions and their chips, rendered from real Tickets: every
// card shows number, kind, title, Project code, priority, and
// progress, and each kind adds its own chips from the closed
// vocabulary (KAN-T26-AC1, KAN-T26-AC2, KAN-T26-AC3). The Spec and
// Lane chips wear what the board projection resolved beside the
// Ticket; the reviewers chip wears the slots configured on an
// Implementation.
import { mount, flushPromises } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { TicketRecord } from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import { harness, ticket } from '../test/shell-harness'
import type { HarnessOptions } from '../test/shell-harness'
import BoardView from './BoardView.vue'

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

// What the projection resolved beside the Implementation: Spec 4 is
// this Project's ninth, and Lane 3 holds the Ticket.
const boardExtras: HarnessOptions['extras'] = { 8: { spec_number: 9, lane_id: 3 } }

const mountedBoards: VueWrapper[] = []

async function mounted(options: HarnessOptions = {}) {
  const shell = harness({ tickets: boardTickets(), extras: boardExtras, ...options })
  await router.push('/projects/1/board')
  await router.isReady()
  const wrapper = mount(BoardView, {
    attachTo: document.body,
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: shell.transport },
    },
  })
  mountedBoards.push(wrapper)
  await flushPromises()
  return { wrapper, ...shell }
}

beforeEach(() => {
  localStorage.clear()
  document.documentElement.classList.remove('dark')
})

afterEach(() => {
  for (const wrapper of mountedBoards.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

const chip = (wrapper: VueWrapper, kind: string, ticketId: number) =>
  wrapper.find(`[data-testid="card-chip-${kind}-${ticketId}"]`)

describe('board cards', () => {
  it('gives every card the fixed regions: number and title without kind badges', async () => {
    const { wrapper } = await mounted()

    for (const [id, number, title] of [
      [7, 'CORE-12', 'Archive the old exports'],
      [8, 'CORE-13', 'Serve the lifecycle command surface'],
      [9, 'CORE-14', 'Clone guard misses a dirty tree'],
    ] as const) {
      expect(wrapper.find(`[data-testid="card-number-${id}"]`).text()).toBe(number)
      expect(wrapper.find(`[data-testid="card-kind-${id}"]`).exists()).toBe(false)
      expect(wrapper.find(`[data-testid="open-ticket-${id}"]`).text()).toBe(title)
    }
  })

  it('shows Linear priority chips on every card', async () => {
    const { wrapper } = await mounted()

    expect(chip(wrapper, 'priority', 7).text()).toBe('PriorityHigh')
    expect(chip(wrapper, 'priority', 8).text()).toBe('PriorityUrgent')
    expect(chip(wrapper, 'severity', 9).exists()).toBe(false)
  })
})
