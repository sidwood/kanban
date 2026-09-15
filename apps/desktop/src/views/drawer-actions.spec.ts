// KAN-T139-AC5 (KAN-S10-US6): the drawer's footer actions run real
// commands. Park runs `ticket.park`, a human review decision runs
// `review.human.submit` against the reviewed tip, and emergency
// recovery runs `ticket.emergency.override` only behind a real
// confirmation carrying the operator and the reason the audit row
// records. Nothing here raises an explanatory toast instead of
// acting, and cancelling acts on nothing at all.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type {
  ProfileSnapshotRecord,
  ReviewExecutionRecord,
  ReviewSlotRecord,
  TicketRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import { harness, ticket } from '../test/shell-harness'
import type { HarnessOptions } from '../test/shell-harness'
import BoardView from './BoardView.vue'

const implementation = (overrides: Partial<TicketRecord> = {}): TicketRecord =>
  ticket({
    id: 8,
    number: 13,
    kind: 'implementation',
    state: 'in_review',
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

const humanSlot = (id: number): ReviewSlotRecord => ({
  id,
  occupant: { kind: 'human' },
  requirement: 'required',
  requested: null,
  effective: null,
  fallback_path: [],
  dispatch_request_id: null,
  verdict: null,
})

const profileSlot = (id: number, verdict: ReviewSlotRecord['verdict'] = null): ReviewSlotRecord => ({
  id,
  occupant: { kind: 'profile', name: 'sol-reviewer' },
  requirement: 'required',
  requested: snapshot('sol-reviewer'),
  effective: snapshot('sol-reviewer'),
  fallback_path: [],
  dispatch_request_id: 12,
  verdict,
})

const approval = (summary: string): NonNullable<ReviewSlotRecord['verdict']> => ({
  approve: true,
  counts_for_resolution: true,
  summary,
  tip: 'f00dcafe',
  submission_id: 9,
  findings: [],
})

// One stage the core is resolving now: the agent slot has reported and
// the human slot is the one it waits on.
const review: ReviewExecutionRecord = {
  id: 55,
  project_id: 1,
  ticket_id: 8,
  submission_id: 9,
  configuration_version: 2,
  tip: 'f00dcafe',
  status: 'in_progress',
  version: 6,
  bounce: null,
  stages: [
    {
      index: 0,
      status: 'waiting',
      slots: [humanSlot(72), profileSlot(71, approval('The slice holds.'))],
    },
  ],
}

// The review the core holds is discoverable from the Ticket alone;
// an in-progress execution records no history attempt at all
// (crates/kanban-storage/src/review_execution.rs `record_gate_outcome`).
const reviewed: Partial<HarnessOptions> = { reviews: [review] }

const mountedBoards: VueWrapper[] = []

async function openDrawer(state: ReturnType<typeof harness>, ticketId = 8) {
  await router.push('/projects/1/board')
  await router.isReady()
  const wrapper = mount(BoardView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: state.transport },
    },
    attachTo: document.body,
  })
  mountedBoards.push(wrapper)
  await flushPromises()
  await wrapper.find(`[data-testid="open-ticket-${ticketId}"]`).trigger('click')
  await flushPromises()
  return wrapper
}

async function click(testid: string): Promise<void> {
  const element = document.querySelector(`[data-testid="${testid}"]`)
  if (!(element instanceof HTMLElement)) throw new Error(`no ${testid}`)
  element.click()
  await flushPromises()
}

async function type(testid: string, value: string): Promise<void> {
  const element = document.querySelector(`[data-testid="${testid}"]`)
  if (
    !(element instanceof HTMLInputElement) &&
    !(element instanceof HTMLTextAreaElement) &&
    !(element instanceof HTMLSelectElement)
  ) {
    throw new Error(`no field ${testid}`)
  }
  element.value = value
  element.dispatchEvent(new Event('input'))
  element.dispatchEvent(new Event('change'))
  await flushPromises()
}

const issued = (state: ReturnType<typeof harness>, name: string): unknown[] =>
  state.command.mock.calls.filter((call) => call[0] === name).map((call) => call[1])

beforeEach(() => {
  localStorage.clear()
})

afterEach(() => {
  for (const wrapper of mountedBoards.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

describe('the drawer footer actions', () => {
  it('Edit opens the kind-adaptive dialog on the open Ticket', async () => {
    const state = harness({ tickets: [implementation()] })
    const pinia = createPinia()
    await router.push('/projects/1/board')
    await router.isReady()
    const wrapper = mount(BoardView, {
      global: {
        plugins: [pinia, router],
        provide: { [kanbanTransportKey as symbol]: state.transport },
      },
      attachTo: document.body,
    })
    mountedBoards.push(wrapper)
    await flushPromises()
    await wrapper.find('[data-testid="open-ticket-8"]').trigger('click')
    await flushPromises()

    await click('drawer-edit')

    const dialog = useTicketDialogStore(pinia)
    expect(dialog.editorOpen).toBe(true)
    expect(dialog.editor).toMatchObject({ mode: 'edit', ticketId: 8, projectId: 1 })
  })

  it('Park runs ticket.park at the open version and follows the record back', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)

    await click('drawer-park')

    expect(issued(state, 'ticket.park')[0]).toEqual({
      mutation: { optimistic_version: 5, idempotency_key: expect.any(String) },
      ticket_id: 8,
    })
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Parked')
    expect(document.querySelector('[data-testid="drawer-unpark"]')).not.toBeNull()
  })

  it('re-reads the timeline after a command, so the audit it appended is on show', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)
    const before = state.query.mock.calls.filter((call) => call[0] === 'timeline.query').length

    await click('drawer-park')

    expect(
      state.query.mock.calls.filter((call) => call[0] === 'timeline.query').length,
    ).toBeGreaterThan(before)
  })

  it('a refused Park reports the core message and leaves the state alone', async () => {
    const state = harness({
      tickets: [implementation({ state: 'ready' })],
    })
    state.command.mockImplementation((name: string) =>
      name === 'ticket.park'
        ? Promise.reject({ code: 'invalid_request', message: 'executing work cannot be parked' })
        : Promise.resolve({}),
    )
    await openDrawer(state)

    await click('drawer-park')

    expect(document.querySelector('[data-testid="drawer-action-error"]')?.textContent).toContain(
      'executing work cannot be parked',
    )
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Ready')
  })

  // SOL-T139-A-01: the open review is reached from the Ticket, not from
  // an attempt history that an in-progress execution never writes.
  it('finds the open review through the Ticket, with no history attempt to name it', async () => {
    const state = harness({ tickets: [implementation()], ...reviewed })
    await openDrawer(state)

    expect(state.query).toHaveBeenCalledWith('review.latest', { ticket_id: 8 })
    expect(state.reviews.history(8).attempts).toEqual([])
    expect(document.querySelector('[data-testid="drawer-review-decision"]')).not.toBeNull()
    expect(document.querySelector('[data-testid="drawer-review-slot-72"]')).not.toBeNull()
  })

  it('a human review decision submits the slot, the reviewed tip, and the summary', async () => {
    const state = harness({ tickets: [implementation()], ...reviewed })
    await openDrawer(state)

    await click('drawer-review-decision')
    await type('review-decision-summary', 'The slice holds; the gates are green.')
    await click('review-decision-approve')
    await click('review-decision-submit')

    expect(issued(state, 'review.human.submit')[0]).toEqual({
      mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
      review_id: 55,
      slot_id: 72,
      tip: 'f00dcafe',
      approve: true,
      summary: 'The slice holds; the gates are green.',
      findings: [],
    })
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
  })

  it('a review decision without a summary is never sent', async () => {
    const state = harness({ tickets: [implementation()], ...reviewed })
    await openDrawer(state)

    await click('drawer-review-decision')
    const submit = document.querySelector('[data-testid="review-decision-submit"]')
    expect((submit as HTMLButtonElement).disabled).toBe(true)
    await click('review-decision-submit')

    expect(issued(state, 'review.human.submit')).toHaveLength(0)
  })

  // SOL-T139-A-03: a rejection resolves on a blocking in-scope finding;
  // the core refuses one carrying anything less.
  it('a Reject carries the canonical finding fields the rejection resolves on', async () => {
    const state = harness({ tickets: [implementation()], ...reviewed })
    await openDrawer(state)

    await click('drawer-review-decision')
    await type('review-decision-summary', 'The landing path is unguarded.')
    await click('review-decision-reject')
    await type('review-finding-severity', 'p1')
    await type('review-finding-summary', 'Landing drops the integration branch.')
    await type('review-finding-evidence', 'The landing log names the drop.')
    await type('review-finding-location', 'crates/kanban-app/src/landing.rs:88')
    await type('review-finding-resolution', 'Guard the branch before the merge.')
    await click('review-decision-submit')

    expect(issued(state, 'review.human.submit')[0]).toEqual({
      mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
      review_id: 55,
      slot_id: 72,
      tip: 'f00dcafe',
      approve: false,
      summary: 'The landing path is unguarded.',
      findings: [
        {
          severity: 'p1',
          in_scope: true,
          summary: 'Landing drops the integration branch.',
          evidence: 'The landing log names the drop.',
          location: 'crates/kanban-app/src/landing.rs:88',
          proposed_resolution: 'Guard the branch before the merge.',
        },
      ],
    })
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
    expect(state.reviews.get(55).status).toBe('rejected')
  })

  it('never sends a rejection the core would refuse for want of a blocking finding', async () => {
    const state = harness({ tickets: [implementation()], ...reviewed })
    await openDrawer(state)

    await click('drawer-review-decision')
    await type('review-decision-summary', 'This needs work.')
    await click('review-decision-reject')

    expect(
      (document.querySelector('[data-testid="review-decision-submit"]') as HTMLButtonElement)
        .disabled,
    ).toBe(true)
    await type('review-finding-severity', 'p3')
    await type('review-finding-summary', 'A label reads oddly.')
    await type('review-finding-evidence', 'The panel shows it.')
    await type('review-finding-location', 'review panel')
    await type('review-finding-resolution', 'Reword the label.')

    expect(document.querySelector('[data-testid="review-decision-incomplete"]')?.textContent)
      .toContain('in-scope P0 to P2 finding')
    expect(
      (document.querySelector('[data-testid="review-decision-submit"]') as HTMLButtonElement)
        .disabled,
    ).toBe(true)
    await click('review-decision-submit')
    expect(issued(state, 'review.human.submit')).toHaveLength(0)
  })

  // SOL-T139-A-04: the verdict writes an attempt, a revalidation and an
  // audit row that the returned record does not carry.
  it('a landed approval refreshes the review history and the Ticket audit', async () => {
    const solo: ReviewExecutionRecord = {
      ...review,
      stages: [{ index: 0, status: 'waiting', slots: [humanSlot(72)] }],
    }
    const state = harness({ tickets: [implementation()], reviews: [solo] })
    await openDrawer(state)
    const auditBefore = document.querySelectorAll('[data-testid^="timeline-event-"]').length

    await click('drawer-review-decision')
    await type('review-decision-summary', 'The slice holds; the gates are green.')
    await click('review-decision-submit')

    expect(state.reviews.get(55).status).toBe('approved')
    expect(document.querySelector('[data-testid="drawer-review-attempt-1"]')?.textContent)
      .toContain('approved')
    expect(document.querySelector('[data-testid="drawer-review-revalidation"]')).toBeNull()
    expect(
      document.querySelectorAll('[data-testid^="timeline-event-"]').length,
    ).toBeGreaterThan(auditBefore)
  })

  it('a landed rejection shows the new attempt and the revalidation the gate now needs', async () => {
    const solo: ReviewExecutionRecord = {
      ...review,
      stages: [{ index: 0, status: 'waiting', slots: [humanSlot(72)] }],
    }
    const state = harness({ tickets: [implementation()], reviews: [solo] })
    await openDrawer(state)
    expect(document.querySelector('[data-testid="drawer-review-revalidation"]')).toBeNull()

    await click('drawer-review-decision')
    await type('review-decision-summary', 'The landing path is unguarded.')
    await click('review-decision-reject')
    await type('review-finding-severity', 'p1')
    await type('review-finding-summary', 'Landing drops the integration branch.')
    await type('review-finding-evidence', 'The landing log names the drop.')
    await type('review-finding-location', 'crates/kanban-app/src/landing.rs:88')
    await type('review-finding-resolution', 'Guard the branch before the merge.')
    await click('review-decision-submit')

    expect(document.querySelector('[data-testid="drawer-review-attempt-1"]')?.textContent)
      .toContain('failed')
    expect(document.querySelector('[data-testid="drawer-review-revalidation"]')?.textContent)
      .toContain('revalidation')
  })

  it('a verdict landing after the drawer moved on writes nothing over the new Ticket', async () => {
    const solo: ReviewExecutionRecord = {
      ...review,
      stages: [{ index: 0, status: 'waiting', slots: [humanSlot(72)] }],
    }
    const state = harness({
      tickets: [implementation(), implementation({ id: 9, number: 14, criteria: [] })],
      reviews: [solo],
    })
    const core = state.command.getMockImplementation() as (
      name: string,
      request: unknown,
    ) => Promise<unknown>
    let release: (() => void) | null = null
    state.command.mockImplementation((name: string, request: unknown) =>
      name === 'review.human.submit'
        ? new Promise((resolve) => {
            release = () => resolve(core(name, request))
          })
        : core(name, request),
    )
    const wrapper = await openDrawer(state)
    await click('drawer-review-decision')
    await type('review-decision-summary', 'The slice holds; the gates are green.')
    await click('review-decision-submit')

    await wrapper.find('[data-testid="open-ticket-9"]').trigger('click')
    await flushPromises()
    release!()
    await flushPromises()

    expect(document.querySelector('[data-testid="drawer-review-slot-72"]')).toBeNull()
    expect(document.querySelector('[data-testid="drawer-review-attempt-1"]')).toBeNull()
  })

  it('offers no human review decision when the core holds no human slot waiting', async () => {
    const state = harness({ tickets: [implementation()] })
    await openDrawer(state)

    expect(document.querySelector('[data-testid="drawer-review-decision"]')).toBeNull()
  })

  // SOL-T139-A-02: the core admits a verdict only in the stage it is
  // resolving now, so a later human stage is offered nothing.
  it('offers no decision for a human stage the core has not reached yet', async () => {
    const ordered: ReviewExecutionRecord = {
      ...review,
      stages: [
        { index: 0, status: 'waiting', slots: [profileSlot(71)] },
        { index: 1, status: 'waiting', slots: [humanSlot(72)] },
      ],
    }
    const state = harness({ tickets: [implementation()], reviews: [ordered] })
    await openDrawer(state)

    expect(document.querySelector('[data-testid="drawer-review-slot-72"]')).not.toBeNull()
    expect(document.querySelector('[data-testid="drawer-review-decision"]')).toBeNull()
  })

  it('offers the human stage its decision once the core has reached it', async () => {
    const ordered: ReviewExecutionRecord = {
      ...review,
      stages: [
        { index: 0, status: 'approved', slots: [profileSlot(71, approval('Reviewed.'))] },
        { index: 1, status: 'waiting', slots: [humanSlot(72)] },
      ],
    }
    const state = harness({ tickets: [implementation()], reviews: [ordered] })
    await openDrawer(state)

    await click('drawer-review-decision')
    await type('review-decision-summary', 'The slice holds; the gates are green.')
    await click('review-decision-submit')

    expect(issued(state, 'review.human.submit')[0]).toMatchObject({ slot_id: 72 })
    expect(state.reviews.get(55).status).toBe('approved')
  })

  it('emergency recovery acts only after a confirmation carrying operator and reason', async () => {
    const state = harness({ tickets: [implementation({ state: 'active' })] })
    await openDrawer(state)

    await click('drawer-recover')
    expect(document.querySelector('[data-testid="recovery-confirm"]')).not.toBeNull()
    // The confirmation is the act; opening it issues nothing.
    expect(issued(state, 'ticket.emergency.override')).toHaveLength(0)

    const submit = document.querySelector('[data-testid="recovery-submit"]') as HTMLButtonElement
    expect(submit.disabled).toBe(true)
    await type('recovery-to', 'ready')
    await type('recovery-who', 'Sid')
    await type('recovery-why', 'The run died holding the lane.')
    expect(
      (document.querySelector('[data-testid="recovery-submit"]') as HTMLButtonElement).disabled,
    ).toBe(false)
    await click('recovery-submit')

    expect(issued(state, 'ticket.emergency.override')[0]).toEqual({
      mutation: { optimistic_version: 5, idempotency_key: expect.any(String) },
      ticket_id: 8,
      to: 'ready',
      who: 'Sid',
      why: 'The run died holding the lane.',
    })
    expect(document.querySelector('[data-testid="recovery-confirm"]')).toBeNull()
  })

  it('cancelling the recovery confirmation leaves the Ticket exactly as it stood', async () => {
    const state = harness({ tickets: [implementation({ state: 'active' })] })
    await openDrawer(state)

    await click('drawer-recover')
    await type('recovery-to', 'ready')
    await type('recovery-who', 'Sid')
    await type('recovery-why', 'The run died holding the lane.')
    await click('recovery-cancel')

    expect(document.querySelector('[data-testid="recovery-confirm"]')).toBeNull()
    expect(issued(state, 'ticket.emergency.override')).toHaveLength(0)
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Active')
  })

  it('a refused recovery reports the core message and keeps the confirmation open', async () => {
    const state = harness({ tickets: [implementation({ state: 'active' })] })
    state.command.mockImplementation((name: string) =>
      name === 'ticket.emergency.override'
        ? Promise.reject({ code: 'invalid_request', message: 'a Ticket reason cannot be blank' })
        : Promise.resolve({}),
    )
    await openDrawer(state)

    await click('drawer-recover')
    await type('recovery-to', 'ready')
    await type('recovery-who', 'Sid')
    await type('recovery-why', 'The run died holding the lane.')
    await click('recovery-submit')

    expect(document.querySelector('[data-testid="drawer-action-error"]')?.textContent).toContain(
      'a Ticket reason cannot be blank',
    )
    expect(document.querySelector('[data-testid="recovery-confirm"]')).not.toBeNull()
  })

  it('keeps the drag Task-only: an agent-owned kind offers no drag and no move select', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    const wrapper = await openDrawer(state)

    const card = wrapper.find('[data-testid="kanban-card-8"]')
    expect(card.attributes('draggable')).toBe('false')
    expect(document.querySelector('[data-testid="drawer-move"]')).toBeNull()
    expect(document.querySelector('[data-testid="drawer-agent-owned"]')?.textContent).toContain(
      'agent-owned',
    )
  })

  it('offers a Task its legal moves as the core judges them', async () => {
    const state = harness({ tickets: [ticket({ id: 7, state: 'ready' })] })
    await openDrawer(state, 7)

    const move = document.querySelector('[data-testid="drawer-move"]') as HTMLSelectElement
    expect([...move.options].map((option) => option.value)).toEqual(['', 'parked', 'active'])
    await type('drawer-move', 'active')

    expect(issued(state, 'ticket.transition')[0]).toMatchObject({ ticket_id: 7, to: 'active' })
  })
})
