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
import type { TicketReadinessResponse, TicketRecord } from '@kanban/contracts'
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

const waiting = (from_number: number): TicketReadinessResponse['blocked_by'][number] => ({
  Ticket: {
    from_number,
    from_project_id: 1,
    from_state: 'active',
    from_ticket_id: from_number,
  },
})

const chip = (wrapper: VueWrapper, kind: string, ticketId: number) =>
  wrapper.find(`[data-testid="card-chip-${kind}-${ticketId}"]`)

// A chip renders its label and its value in spans of their own; this
// reads the value alone.
const chipValue = (wrapper: VueWrapper, kind: string, ticketId: number) =>
  chip(wrapper, kind, ticketId).findAll('span')[1]?.text()

describe('board cards', () => {
  it('gives every card the fixed regions: number with Project code, kind, and title', async () => {
    const { wrapper } = await mounted()

    for (const [id, number, kindLabel, title] of [
      [7, 'CORE-T12', 'Task Ticket', 'Archive the old exports'],
      [8, 'CORE-T13', 'Implementation Ticket', 'Serve the lifecycle command surface'],
      [9, 'CORE-T14', 'Bug Ticket', 'Clone guard misses a dirty tree'],
    ] as const) {
      expect(wrapper.find(`[data-testid="card-number-${id}"]`).text()).toBe(number)
      expect(wrapper.find(`[data-testid="card-kind-${id}"]`).text()).toBe(kindLabel)
      expect(wrapper.find(`[data-testid="open-ticket-${id}"]`).text()).toBe(title)
    }
  })

  it('shows the priority and progress chips on every card', async () => {
    const { wrapper } = await mounted()

    expect(chip(wrapper, 'priority', 7).text()).toBe('PriorityHigh')
    expect(chip(wrapper, 'progress', 7).text()).toBe('Progress0/1 outcomes')
    expect(chip(wrapper, 'priority', 8).text()).toBe('PriorityUrgent')
    expect(chip(wrapper, 'progress', 8).text()).toBe('Progress0/3 criteria')
    expect(chip(wrapper, 'progress', 9).text()).toBe('Progress0/2 criteria')
  })

  it('reads the criterion bindings the core holds, so progress is completion and not a total', async () => {
    const { wrapper, query } = await mounted({
      override: (name, request) => {
        if (name !== 'criterion.bindings') return undefined
        const { ticket_id } = request as { ticket_id: number }
        if (ticket_id !== 8) return Promise.resolve({ bindings: [] })
        return Promise.resolve({
          bindings: [
            {
              ticket_id,
              criterion_index: 0,
              kind: 'acceptance',
              evidence_id: 1,
              tip: 'a1b2c3',
              review: 'validated',
              satisfied: true,
              void: false,
            },
            {
              ticket_id,
              criterion_index: 1,
              kind: 'acceptance',
              evidence_id: 2,
              tip: 'a1b2c3',
              review: 'pending',
              satisfied: false,
              void: false,
            },
          ],
        })
      },
    })

    expect(
      query.mock.calls.filter(([name]) => name === 'criterion.bindings').map(([, request]) => request),
    ).toContainEqual({ ticket_id: 8 })
    expect(chip(wrapper, 'progress', 8).text()).toBe('Progress1/3 criteria')
    expect(chip(wrapper, 'progress', 8).attributes('title')).toContain('1 awaiting approval')
    expect(chip(wrapper, 'progress', 9).text()).toBe('Progress0/2 criteria')
  })

  it('reads the progress again when the core announces a criterion change', async () => {
    // What the bindings say changes under a mounted board, the way it
    // does when a reviewer validates or a run completes a criterion
    // from somewhere else (KAN-T137-AC7).
    let satisfied = false
    const { wrapper, emit } = await mounted({
      override: (name, request) => {
        if (name !== 'criterion.bindings') return undefined
        const { ticket_id } = request as { ticket_id: number }
        if (ticket_id !== 8 || !satisfied) return Promise.resolve({ bindings: [] })
        return Promise.resolve({
          bindings: [
            {
              ticket_id,
              criterion_index: 0,
              kind: 'acceptance',
              evidence_id: 1,
              tip: 'a1b2c3',
              review: 'validated',
              satisfied: true,
              void: false,
            },
          ],
        })
      },
    })
    expect(chip(wrapper, 'progress', 8).text()).toBe('Progress0/3 criteria')

    satisfied = true
    emit({
      sequence: 7,
      event_type: 'criterion.binding.changed',
      payload: {
        ticket_id: 8,
        criterion_index: 0,
        kind: 'acceptance',
        evidence_id: 1,
        tip: 'a1b2c3',
        review: 'validated',
        satisfied: true,
        void: false,
      },
    })
    await flushPromises()

    expect(chip(wrapper, 'progress', 8).text()).toBe('Progress1/3 criteria')
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
    const { wrapper } = await mounted({ tickets: [unqualified], extras: {} })

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
    const { wrapper } = await mounted({ blockers: { 8: [waiting(3), waiting(5)] } })

    expect(chip(wrapper, 'spec', 8).text()).toBe('SpecCORE-S9')
    expect(chip(wrapper, 'implementer', 8).text()).toContain('glm-implementer')
    expect(chip(wrapper, 'lane', 8).text()).toBe('LaneLane 3')
    expect(chip(wrapper, 'blockers', 8).text()).toBe('Blockers2 blockers')
  })

  it('wears the reviewers configured on an Implementation, and none when none are', async () => {
    const withoutReviewers = ticket({
      id: 20,
      number: 21,
      kind: 'implementation',
      state: 'ready',
      title: null,
      slice: 'Carry the work through review',
      spec_id: 6,
      criteria: [],
      subtype: null,
      mode: null,
      completion: [],
      profile: 'glm-implementer',
    })
    const { wrapper } = await mounted({
      tickets: [...boardTickets(), withoutReviewers],
      override: (name, request) => {
        if (name !== 'ticket.review.config') return undefined
        const { ticket_id } = request as { ticket_id: number }
        return Promise.resolve({
          config:
            ticket_id === 8
              ? {
                  ticket_id,
                  version: 1,
                  stages: [
                    {
                      slots: [
                        { occupant: { kind: 'profile', name: 'review-strict' }, requirement: 'required' },
                        { occupant: { kind: 'profile', name: 'sonnet-stage' }, requirement: 'required' },
                        { occupant: { kind: 'human' }, requirement: 'optional' },
                      ],
                    },
                  ],
                }
              : null,
        })
      },
    })

    // More than two reviewers collapse to +N (DR-BP-14).
    expect(chipValue(wrapper, 'reviewers', 8)).toBe('review-strict, sonnet-stage +1')
    expect(chip(wrapper, 'reviewers', 8).attributes('title')).toBe('review-strict, sonnet-stage, Human')
    expect(chip(wrapper, 'reviewers', 20).exists()).toBe(false)
  })

  it('renders the Spec\'s minted number, never its row id', async () => {
    // Spec 6 is this Project's second: the ids below it belong to
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
    const { wrapper } = await mounted({ tickets: [gapped], extras: { 11: { spec_number: 2 } } })

    expect(chip(wrapper, 'spec', 11).text()).toBe('SpecCORE-S2')
    expect(wrapper.text()).not.toContain('CORE-S6')
  })

  it('omits the Spec chip when the projection resolved no number', async () => {
    // The Ticket names a Spec the projection could not resolve; the
    // card invents no identity from the id (KAN-T126-AC2).
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
    const { wrapper } = await mounted({ tickets: [orphan], extras: { 12: { spec_number: null } } })

    expect(chip(wrapper, 'spec', 12).exists()).toBe(false)
    expect(wrapper.text()).not.toContain('CORE-S99')
  })

  it('populates the Lane chip from the Lane the projection resolved', async () => {
    const { wrapper } = await mounted()

    // The chip comes from the projection's `lane_id` — not from any
    // local board state.
    expect(chip(wrapper, 'lane', 8).text()).toBe('LaneLane 3')
    // A Ticket no Lane holds carries no Lane chip.
    expect(chip(wrapper, 'lane', 7).exists()).toBe(false)
    expect(chip(wrapper, 'lane', 9).exists()).toBe(false)
  })

  it('adds the bug chips: severity, frequency, origin, and profiles', async () => {
    const { wrapper } = await mounted()

    expect(chip(wrapper, 'severity', 9).text()).toBe('SeverityHigh')
    expect(chip(wrapper, 'severity', 9).attributes('data-tone')).toBe('caution')
    expect(chip(wrapper, 'frequency', 9).text()).toBe('FrequencyIntermittent')
    expect(chip(wrapper, 'origin', 9).text()).toBe('OriginA landing run failed')
    expect(chip(wrapper, 'profiles', 9).text()).toContain('glm-triage')
    // A standalone Bug carries no Spec.
    expect(chip(wrapper, 'spec', 9).exists()).toBe(false)
  })

  it('adds the task chips: subtype, mode, schedule, and executor', async () => {
    const { wrapper } = await mounted()

    expect(chip(wrapper, 'subtype', 7).text()).toBe('SubtypeOperational')
    expect(chip(wrapper, 'mode', 7).text()).toBe('ModeHuman')
    expect(chip(wrapper, 'schedule', 7).text()).toBe('Scheduled2026-09-12')
    expect(chip(wrapper, 'executor', 7).text()).toBe('ExecutorOperator')
    // A Task attaches to no Spec and holds no blockers here.
    expect(chip(wrapper, 'spec', 7).exists()).toBe(false)
    expect(chip(wrapper, 'blockers', 7).exists()).toBe(false)
  })

  it('keeps one kind of chip off another kind of card', async () => {
    const { wrapper } = await mounted()

    // The Task carries no severity; the Bug carries no Lane; the
    // Implementation carries no subtype — the vocabulary decides.
    expect(chip(wrapper, 'severity', 7).exists()).toBe(false)
    expect(chip(wrapper, 'lane', 9).exists()).toBe(false)
    expect(chip(wrapper, 'subtype', 8).exists()).toBe(false)
  })
})
