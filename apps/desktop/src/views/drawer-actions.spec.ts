// Drawer footer: edit, park, and workflow moves (Linear issue model).
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type {
  IssueRecord,
  ProfileSnapshotRecord,
  ReviewExecutionRecord,
  ReviewSlotRecord,
  TicketRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import { useTicketDetailStore } from '../stores/ticket-detail'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import { harness, ticket } from '../test/shell-harness'
import BoardView from './BoardView.vue'

const implementation = (overrides: Partial<TicketRecord> = {}): TicketRecord =>
  ticket({
    id: 8,
    number: 13,
    kind: 'implementation',
    state: 'in_review',
    spec_id: 4,
    title: 'Serve the lifecycle command surface',
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

const mountedBoards: VueWrapper[] = []

async function openDrawer(state: ReturnType<typeof harness>, ticketId = 8, pinia = createPinia()) {
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
  await wrapper.find(`[data-testid="open-ticket-${ticketId}"]`).trigger('click')
  await flushPromises()
  return wrapper
}

const submitButton = (testid: string): HTMLButtonElement => {
  const element = document.querySelector(`[data-testid="${testid}"]`)
  if (!(element instanceof HTMLButtonElement)) throw new Error(`no ${testid}`)
  return element
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

interface HeldRead {
  answer: () => void
  fail: () => void
}

function holdReads(state: ReturnType<typeof harness>, name: string, count = 1): HeldRead[] {
  const core = state.query.getMockImplementation() as (
    query: string,
    request: unknown,
  ) => Promise<unknown>
  const held: HeldRead[] = []
  state.query.mockImplementation((query: string, request: unknown) => {
    if (query !== name || held.length >= count) return core(query, request)
    const answer = core(query, request)
    return new Promise((resolve, reject) => {
      held.push({
        answer: () => void answer.then(resolve, reject),
        fail: () => reject({ code: 'unavailable', message: `the ${name} read failed` }),
      })
    })
  })
  return held
}

function holdCommands(
  state: ReturnType<typeof harness>,
  name: string,
  count = 1,
): HeldRead[] {
  const core = state.command.getMockImplementation() as (
    command: string,
    request: unknown,
  ) => Promise<unknown>
  const held: HeldRead[] = []
  state.command.mockImplementation((command: string, request: unknown) => {
    if (command !== name || held.length >= count) return core(command, request)
    return new Promise((resolve, reject) => {
      held.push({
        answer: () => void core(command, request).then(resolve, reject),
        fail: () => reject({ code: 'unavailable', message: `the ${name} command failed` }),
      })
    })
  })
  return held
}

async function saveDescription(text: string): Promise<void> {
  await click('issue-description-edit')
  await type('issue-description-input', text)
  await click('issue-description-save')
}

async function forceClick(testid: string): Promise<void> {
  const element = submitButton(testid)
  const disabled = element.disabled
  element.disabled = false
  element.click()
  element.disabled = disabled
  await flushPromises()
}

async function submitForm(testid: string): Promise<void> {
  const element = document.querySelector(`[data-testid="${testid}"]`)
  if (!(element instanceof HTMLFormElement)) throw new Error(`no form ${testid}`)
  element.dispatchEvent(new Event('submit', { cancelable: true }))
  await flushPromises()
}

const descriptionDraft = (): string | null => {
  const field = document.querySelector('[data-testid="issue-description-input"]')
  return field instanceof HTMLTextAreaElement ? field.value : null
}

const versionsSent = (state: ReturnType<typeof harness>, name: string): number[] =>
  issued(state, name).map(
    (request) => (request as { mutation: { optimistic_version: number } }).mutation.optimistic_version,
  )

beforeEach(() => {
  localStorage.clear()
})

afterEach(() => {
  for (const wrapper of mountedBoards.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

describe('the drawer footer actions', () => {
  it('Edit opens the issue editor on the open Ticket', async () => {
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

  it.each([
    {
      command: 'Park',
      from: 'ready' as const,
      to: 'Parked',
      act: () => click('drawer-park'),
    },
    {
      command: 'Unpark',
      from: 'parked' as const,
      to: 'Ready',
      act: () => click('drawer-unpark'),
    },
    {
      command: 'a workflow move',
      from: 'ready' as const,
      to: 'Active',
      act: () => type('drawer-move', 'Active'),
    },
    {
      command: 'emergency recovery',
      from: 'active' as const,
      to: 'Ready',
      act: async () => {
        await click('drawer-recover')
        await type('recovery-to', 'ready')
        await type('recovery-who', 'Sid')
        await type('recovery-why', 'The run died holding the lane.')
        await click('recovery-submit')
      },
    },
  ])('after $command the issue panel shows the new state and saves at the new version', async ({ from, to, act }) => {
    const state = harness({ tickets: [implementation({ state: from })] })
    await openDrawer(state)

    await act()

    expect(
      document.querySelector('[data-testid="issue-history-row-0"] td')?.textContent?.trim(),
    ).toBe(to)
    await click('issue-description-edit')
    await type('issue-description-input', 'The commands are served over the socket.')
    await click('issue-description-save')

    expect(issued(state, 'issue.save').at(-1)).toEqual({
      mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
      identifier: 'CORE-13',
      description: 'The commands are served over the socket.',
    })
    expect(document.querySelector('[data-testid="issue-description-input"]')).toBeNull()
  })

  it('sends the footer command after a description save at the version that save left', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)

    await click('issue-description-edit')
    await type('issue-description-input', 'The commands are served over the socket.')
    await click('issue-description-save')
    await click('drawer-park')

    expect(issued(state, 'issue.save')[0]).toEqual({
      mutation: { optimistic_version: 5, idempotency_key: expect.any(String) },
      identifier: 'CORE-13',
      description: 'The commands are served over the socket.',
    })
    expect(issued(state, 'ticket.park')[0]).toEqual({
      mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
      ticket_id: 8,
    })
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Parked')
  })

  it('offers every workflow state except the current one in the move select', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)

    const move = document.querySelector('[data-testid="drawer-move"]') as HTMLSelectElement
    expect(move).not.toBeNull()
    expect([...move.options].map((option) => option.value)).toContain('Active')
    expect([...move.options].map((option) => option.value)).not.toContain('Ready')
  })

  it("offers exactly the Project's own workflow states, read from the core", async () => {
    // A workflow unlike the defaults, so a fallback cannot pass.
    const states = [
      { name: 'Draft', state_type: 'backlog' },
      { name: 'Ready', state_type: 'unstarted' },
      { name: 'In Progress', state_type: 'started' },
      { name: 'Shipped', state_type: 'completed' },
    ]
    const state = harness({
      tickets: [
        ticket({ id: 7, number: 12, state: 'ready' }),
        ticket({ id: 9, number: 14, state: 'draft' }),
      ],
      override: (name) =>
        name === 'issue.list_workflow_states' ? Promise.resolve({ states }) : undefined,
    })
    const moves = (): string[] =>
      [...(document.querySelector('[data-testid="drawer-move"]') as HTMLSelectElement).options]
        .map((option) => option.value)
        .filter((value) => value !== '')

    const wrapper = await openDrawer(state, 7)
    expect(moves()).toEqual(['Draft', 'In Progress', 'Shipped'])

    await wrapper.find('[data-testid="open-ticket-9"]').trigger('click')
    await flushPromises()
    expect(moves()).toEqual(['Ready', 'In Progress', 'Shipped'])

    const reads = state.query.mock.calls.filter(([name]) => name === 'issue.list_workflow_states')
    expect(reads.length).toBeGreaterThan(0)
    expect(reads.map(([, request]) => request)).toEqual(reads.map(() => ({ project_id: 1 })))
  })

  it('moves through issue.save with a workflow state name', async () => {
    const state = harness({ tickets: [ticket({ id: 7, state: 'ready' })] })
    await openDrawer(state, 7)

    await type('drawer-move', 'Active')

    expect(issued(state, 'issue.save')).toEqual([
      {
        mutation: { optimistic_version: 3, idempotency_key: expect.any(String) },
        identifier: 'CORE-12',
        state: 'Active',
      },
    ])
  })
})

describe('the drawer Ticket a description save reads again', () => {
  it('keeps the newer Ticket when an earlier save reads its Ticket last', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)
    const reads = holdReads(state, 'ticket.get')

    await saveDescription('The commands are served over the socket.')
    await saveDescription('The commands are served over the socket and the pipe.')
    expect(reads).toHaveLength(1)
    reads[0]!.answer()
    await flushPromises()
    await click('drawer-park')

    expect(versionsSent(state, 'issue.save')).toEqual([5, 6])
    expect(issued(state, 'ticket.park')).toEqual([
      { mutation: { optimistic_version: 7, idempotency_key: expect.any(String) }, ticket_id: 8 },
    ])
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Parked')
  })

  it.each([
    ['is still out', false],
    ['fails', true],
  ])('sends the footer command at the version the save left while its Ticket read %s', async (_outcome, fails) => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)
    const reads = holdReads(state, 'ticket.get')

    await saveDescription('The commands are served over the socket.')
    expect(reads).toHaveLength(1)
    if (fails) reads[0]!.fail()
    await flushPromises()
    await click('drawer-park')
    reads[0]!.answer()
    await flushPromises()

    expect(issued(state, 'ticket.park')).toEqual([
      { mutation: { optimistic_version: 6, idempotency_key: expect.any(String) }, ticket_id: 8 },
    ])
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Parked')
    expect(document.querySelector('[data-testid="drawer-unpark"]')).not.toBeNull()
  })

  it('writes nothing from a late Ticket read over the Ticket the drawer moved to', async () => {
    const state = harness({
      tickets: [
        implementation({ state: 'ready' }),
        implementation({
          id: 9,
          number: 14,
          state: 'ready',
          title: 'Edge review',
          slice: 'Guard the edge review',
        }),
      ],
    })
    await openDrawer(state)
    const reads = holdReads(state, 'ticket.get')

    await saveDescription('The commands are served over the socket.')
    expect(reads).toHaveLength(1)
    await click('open-ticket-9')
    reads[0]!.answer()
    await flushPromises()
    await click('drawer-park')

    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-14')
    expect(issued(state, 'ticket.park')).toEqual([
      { mutation: { optimistic_version: 5, idempotency_key: expect.any(String) }, ticket_id: 9 },
    ])
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
  })

  it('keeps the recovery confirmation being filled in through a save', async () => {
    const state = harness({ tickets: [implementation({ state: 'active' })] })
    await openDrawer(state)

    await click('drawer-recover')
    await type('recovery-to', 'ready')
    await type('recovery-who', 'Sid')
    await type('recovery-why', 'The run died holding the lane.')
    await saveDescription('The commands are served over the socket.')
    await click('recovery-submit')

    expect(issued(state, 'ticket.emergency.override')).toEqual([
      {
        mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
        ticket_id: 8,
        to: 'ready',
        who: 'Sid',
        why: 'The run died holding the lane.',
      },
    ])
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Ready')
  })

  it('keeps the review decision being written through a save', async () => {
    const state = harness({ tickets: [implementation()], reviews: [review] })
    await openDrawer(state)

    await click('drawer-review-decision')
    await type('review-decision-summary', 'The slice holds; the gates are green.')
    await saveDescription('The commands are served over the socket.')
    await click('review-decision-submit')

    expect(issued(state, 'review.human.submit')).toEqual([
      {
        mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
        review_id: 55,
        slot_id: 72,
        tip: 'f00dcafe',
        approve: true,
        summary: 'The slice holds; the gates are green.',
        findings: [],
      },
    ])
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
  })
})

describe('the drawer issue a footer command reads again', () => {
  it.each([
    ['is still out', false],
    ['fails', true],
  ])('saves the description at the version the command left while its issue read %s', async (_outcome, fails) => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)
    const reads = holdReads(state, 'issue.get')

    await click('drawer-park')
    expect(reads).toHaveLength(1)
    if (fails) reads[0]!.fail()
    await flushPromises()
    await saveDescription('The commands are served over the socket.')
    reads[0]!.answer()
    await flushPromises()

    expect(issued(state, 'issue.save')).toEqual([
      {
        mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
        identifier: 'CORE-13',
        description: 'The commands are served over the socket.',
      },
    ])
    expect(document.querySelector('[data-testid="issue-description-input"]')).toBeNull()
    expect(
      document.querySelector('[data-testid="issue-history-row-0"] td')?.textContent?.trim(),
    ).toBe('Parked')
    await saveDescription('The commands are served over the socket and the pipe.')
    expect(versionsSent(state, 'issue.save')).toEqual([6, 7])
  })

  it('keeps the newer issue when an earlier command reads its issue last', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)
    holdReads(state, 'issue.list', Infinity)
    const reads = holdReads(state, 'issue.get', 2)

    await click('drawer-park')
    await click('drawer-unpark')
    expect(reads).toHaveLength(2)
    reads[0]!.answer()
    await flushPromises()
    await saveDescription('The commands are served over the socket.')

    expect(versionsSent(state, 'ticket.park')).toEqual([5])
    expect(versionsSent(state, 'ticket.unpark')).toEqual([6])
    expect(issued(state, 'issue.save')).toEqual([
      {
        mutation: { optimistic_version: 7, idempotency_key: expect.any(String) },
        identifier: 'CORE-13',
        description: 'The commands are served over the socket.',
      },
    ])
    expect(document.querySelector('[data-testid="issue-description-input"]')).toBeNull()
  })

  it('keeps the description being written while the issue is read again', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)
    const reads = holdReads(state, 'issue.get')
    const draft = (): string | null => {
      const field = document.querySelector('[data-testid="issue-description-input"]')
      return field instanceof HTMLTextAreaElement ? field.value : null
    }

    await click('issue-description-edit')
    await type('issue-description-input', 'The commands are served over the socket.')
    await click('drawer-park')
    expect(reads).toHaveLength(1)
    expect(draft()).toBe('The commands are served over the socket.')
    reads[0]!.answer()
    await flushPromises()
    expect(draft()).toBe('The commands are served over the socket.')
    await click('issue-description-save')

    expect(issued(state, 'issue.save')).toEqual([
      {
        mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
        identifier: 'CORE-13',
        description: 'The commands are served over the socket.',
      },
    ])
    expect(draft()).toBeNull()
  })
})

describe('the drawer footer while a saved description is read again', () => {
  it.each([
    {
      command: 'Park',
      from: 'ready' as const,
      to: 'Parked',
      control: 'drawer-park',
      prepare: async () => {},
      press: () => click('drawer-park'),
      force: () => forceClick('drawer-park'),
      sent: (state: ReturnType<typeof harness>) => issued(state, 'ticket.park'),
      request: { mutation: { optimistic_version: 6, idempotency_key: expect.any(String) }, ticket_id: 8 },
    },
    {
      command: 'Unpark',
      from: 'parked' as const,
      to: 'Ready',
      control: 'drawer-unpark',
      prepare: async () => {},
      press: () => click('drawer-unpark'),
      force: () => forceClick('drawer-unpark'),
      sent: (state: ReturnType<typeof harness>) => issued(state, 'ticket.unpark'),
      request: { mutation: { optimistic_version: 6, idempotency_key: expect.any(String) }, ticket_id: 8 },
    },
    {
      command: 'a workflow move',
      from: 'ready' as const,
      to: 'Active',
      control: 'drawer-move',
      prepare: async () => {},
      press: () => type('drawer-move', 'Active'),
      force: () => type('drawer-move', 'Active'),
      sent: (state: ReturnType<typeof harness>) =>
        issued(state, 'issue.save').filter((request) => 'state' in (request as object)),
      request: {
        mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
        identifier: 'CORE-13',
        state: 'Active',
      },
    },
    {
      command: 'emergency recovery',
      from: 'active' as const,
      to: 'Ready',
      control: 'recovery-submit',
      prepare: async () => {
        await click('drawer-recover')
        await type('recovery-to', 'ready')
        await type('recovery-who', 'Sid')
        await type('recovery-why', 'The run died holding the lane.')
      },
      press: () => click('recovery-submit'),
      force: () => submitForm('recovery-confirm'),
      sent: (state: ReturnType<typeof harness>) => issued(state, 'ticket.emergency.override'),
      request: {
        mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
        ticket_id: 8,
        to: 'ready',
        who: 'Sid',
        why: 'The run died holding the lane.',
      },
    },
  ])(
    'sends no $command while the saved description is read again, then sends it at the version that read returns',
    async ({ from, to, control, prepare, press, force, sent, request }) => {
      const state = harness({ tickets: [implementation({ state: from })] })
      await openDrawer(state)
      await prepare()
      const reads = holdReads(state, 'issue.get')

      await saveDescription('The commands are served over the socket.')
      expect(reads).toHaveLength(1)
      expect(issued(state, 'issue.save')).toEqual([
        {
          mutation: { optimistic_version: 5, idempotency_key: expect.any(String) },
          identifier: 'CORE-13',
          description: 'The commands are served over the socket.',
        },
      ])
      expect(
        (document.querySelector(`[data-testid="${control}"]`) as HTMLButtonElement | HTMLSelectElement)
          .disabled,
      ).toBe(true)
      await press()
      await force()
      expect(sent(state)).toEqual([])

      reads[0]!.answer()
      await flushPromises()
      expect(
        (document.querySelector(`[data-testid="${control}"]`) as HTMLButtonElement | HTMLSelectElement)
          .disabled,
      ).toBe(false)
      await press()

      expect(sent(state)).toEqual([request])
      expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
      expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain(to)
    },
  )

  it('keeps the footer and the description save shut when the saved description cannot be read, until it is read again', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)
    const reads = holdReads(state, 'issue.get', 2)
    const shut = async (): Promise<void> => {
      expect(
        document.querySelector('[data-testid="issue-detail-description"] [role="alert"]')?.textContent,
      ).toContain('could not be read again')
      expect(descriptionDraft()).toBe('The commands are served over the socket.')
      expect(submitButton('drawer-park').disabled).toBe(true)
      expect(submitButton('issue-description-save').disabled).toBe(true)
      await click('drawer-park')
      await forceClick('drawer-park')
      await forceClick('issue-description-save')
      expect(issued(state, 'ticket.park')).toEqual([])
      expect(versionsSent(state, 'issue.save')).toEqual([5])
    }

    await saveDescription('The commands are served over the socket.')
    expect(reads).toHaveLength(1)
    reads[0]!.fail()
    await flushPromises()
    await shut()

    await click('issue-description-reread')
    expect(reads).toHaveLength(2)
    expect(document.querySelector('[data-testid="issue-description-reread"]')).toBeNull()
    expect(submitButton('drawer-park').disabled).toBe(true)
    reads[1]!.fail()
    await flushPromises()
    await shut()

    await click('issue-description-reread')
    expect(document.querySelector('[data-testid="issue-description-reread"]')).toBeNull()
    expect(document.querySelector('[data-testid="issue-detail-description"] [role="alert"]')).toBeNull()
    expect(descriptionDraft()).toBeNull()
    await click('drawer-park')

    expect(issued(state, 'ticket.park')).toEqual([
      { mutation: { optimistic_version: 6, idempotency_key: expect.any(String) }, ticket_id: 8 },
    ])
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Parked')
    await saveDescription('The commands are served over the socket and the pipe.')
    expect(versionsSent(state, 'issue.save')).toEqual([5, 7])
  })

  it('keeps the footer shut when an issue read from before the save answers while the save is read again', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)
    const reads = holdReads(state, 'issue.get', 2)

    await click('drawer-park')
    await saveDescription('The commands are served over the socket.')
    expect(reads).toHaveLength(2)
    const ticketReads = holdReads(state, 'ticket.get')
    reads[0]!.answer()
    await flushPromises()
    expect(submitButton('drawer-unpark').disabled).toBe(true)
    await click('drawer-unpark')
    await forceClick('drawer-unpark')
    expect(issued(state, 'ticket.unpark')).toEqual([])

    reads[1]!.answer()
    await flushPromises()
    expect(ticketReads).toHaveLength(1)
    await click('drawer-unpark')
    ticketReads[0]!.answer()
    await flushPromises()

    expect(versionsSent(state, 'ticket.park')).toEqual([5])
    expect(versionsSent(state, 'issue.save')).toEqual([6])
    expect(issued(state, 'ticket.unpark')).toEqual([
      { mutation: { optimistic_version: 7, idempotency_key: expect.any(String) }, ticket_id: 8 },
    ])
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Ready')
  })

  it('opens the footer at the version the read returns when the save leaves the version where it was', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    const core = state.command.getMockImplementation() as (
      name: string,
      request: unknown,
    ) => Promise<unknown>
    state.command.mockImplementation((name: string, request: unknown) =>
      name === 'issue.save'
        ? Promise.resolve({ identifier: 'CORE-13', ticket_id: 8, url: 'kanban://issue/CORE-13' })
        : core(name, request),
    )
    await openDrawer(state)
    const reads = holdReads(state, 'issue.get')

    await saveDescription('Serve the lifecycle command surface')
    expect(reads).toHaveLength(1)
    expect(submitButton('drawer-park').disabled).toBe(true)
    await forceClick('drawer-park')
    expect(issued(state, 'ticket.park')).toEqual([])

    reads[0]!.answer()
    await flushPromises()
    await click('drawer-park')

    expect(versionsSent(state, 'issue.save')).toEqual([5])
    expect(issued(state, 'ticket.park')).toEqual([
      { mutation: { optimistic_version: 5, idempotency_key: expect.any(String) }, ticket_id: 8 },
    ])
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Parked')
  })

  it('leaves the Ticket the drawer moves to open to commands and writes nothing over it when the read lands', async () => {
    const state = harness({
      tickets: [
        implementation({ state: 'ready' }),
        implementation({
          id: 9,
          number: 14,
          state: 'ready',
          title: 'Edge review',
          slice: 'Guard the edge review',
        }),
      ],
    })
    await openDrawer(state)
    const reads = holdReads(state, 'issue.get')

    await saveDescription('The commands are served over the socket.')
    expect(reads).toHaveLength(1)
    await click('open-ticket-9')
    expect(submitButton('drawer-park').disabled).toBe(false)
    await click('drawer-park')
    await saveDescription('The edge review is guarded.')
    reads[0]!.answer()
    await flushPromises()
    await click('drawer-unpark')

    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-14')
    expect(issued(state, 'ticket.park')).toEqual([
      { mutation: { optimistic_version: 5, idempotency_key: expect.any(String) }, ticket_id: 9 },
    ])
    expect(issued(state, 'issue.save')).toEqual([
      {
        mutation: { optimistic_version: 5, idempotency_key: expect.any(String) },
        identifier: 'CORE-13',
        description: 'The commands are served over the socket.',
      },
      {
        mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
        identifier: 'CORE-14',
        description: 'The edge review is guarded.',
      },
    ])
    expect(issued(state, 'ticket.unpark')).toEqual([
      { mutation: { optimistic_version: 7, idempotency_key: expect.any(String) }, ticket_id: 9 },
    ])
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
  })

  it('opens the Ticket again at its current version after the drawer closed with the save still being read', async () => {
    const state = harness({ tickets: [implementation({ state: 'ready' })] })
    await openDrawer(state)
    const reads = holdReads(state, 'issue.get')

    await saveDescription('The commands are served over the socket.')
    expect(reads).toHaveLength(1)
    ;(document.querySelector('[aria-label="Close panel"]') as HTMLElement).click()
    await flushPromises()
    await click('open-ticket-8')
    await click('drawer-park')
    reads[0]!.answer()
    await flushPromises()
    await saveDescription('The commands are served over the socket and the pipe.')

    expect(issued(state, 'ticket.park')).toEqual([
      { mutation: { optimistic_version: 6, idempotency_key: expect.any(String) }, ticket_id: 8 },
    ])
    expect(versionsSent(state, 'issue.save')).toEqual([5, 7])
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Parked')
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
  })

  it.each([
    {
      route: 'closed and opened again',
      close: true,
      outcome: 'moves the version on',
      unchanged: false,
      settle: 'answer' as const,
      park: 6,
      next: 7,
    },
    {
      route: 'closed and opened again',
      close: true,
      outcome: 'leaves the version where it was',
      unchanged: true,
      settle: 'answer' as const,
      park: 5,
      next: 6,
    },
    {
      route: 'closed and opened again',
      close: true,
      outcome: 'fails',
      unchanged: false,
      settle: 'fail' as const,
      park: 5,
      next: 6,
    },
    {
      route: 'opened again from its card',
      close: false,
      outcome: 'moves the version on',
      unchanged: false,
      settle: 'answer' as const,
      park: 6,
      next: 7,
    },
  ])(
    'acts at the current version on the Ticket $route before its save was answered, when the save $outcome',
    async ({ close, unchanged, settle, park, next }) => {
      const state = harness({ tickets: [implementation({ state: 'ready' })] })
      if (unchanged) {
        const core = state.command.getMockImplementation() as (
          name: string,
          request: unknown,
        ) => Promise<unknown>
        state.command.mockImplementation((name: string, request: unknown) =>
          name === 'issue.save'
            ? Promise.resolve({ identifier: 'CORE-13', ticket_id: 8, url: 'kanban://issue/CORE-13' })
            : core(name, request),
        )
      }
      await openDrawer(state)
      const saves = holdCommands(state, 'issue.save')

      await saveDescription('The commands are served over the socket.')
      expect(saves).toHaveLength(1)
      if (close) {
        ;(document.querySelector('[aria-label="Close panel"]') as HTMLElement).click()
        await flushPromises()
      }
      await click('open-ticket-8')
      saves[0]![settle]()
      await flushPromises()

      expect(submitButton('drawer-park').disabled).toBe(false)
      await click('drawer-park')
      expect(issued(state, 'ticket.park')).toEqual([
        { mutation: { optimistic_version: park, idempotency_key: expect.any(String) }, ticket_id: 8 },
      ])
      expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
      expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Parked')
      await saveDescription('The commands are served over the socket and the pipe.')
      expect(versionsSent(state, 'issue.save')).toEqual([5, next])
      expect(document.querySelector('[data-testid="issue-detail-description"] [role="alert"]')).toBeNull()
      expect(descriptionDraft()).toBeNull()
    },
  )

  it('leaves the Ticket the drawer moved to before a save was answered open to commands and unwritten', async () => {
    const state = harness({
      tickets: [
        implementation({ state: 'ready' }),
        implementation({
          id: 9,
          number: 14,
          state: 'ready',
          title: 'Edge review',
          slice: 'Guard the edge review',
          version: 2,
        }),
      ],
    })
    await openDrawer(state)
    const saves = holdCommands(state, 'issue.save')

    await saveDescription('The commands are served over the socket.')
    expect(saves).toHaveLength(1)
    await click('open-ticket-9')
    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-14')
    expect(submitButton('drawer-park').disabled).toBe(false)
    await click('drawer-park')
    saves[0]!.answer()
    await flushPromises()

    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-14')
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Parked')
    expect(submitButton('drawer-unpark').disabled).toBe(false)
    await click('drawer-unpark')
    await saveDescription('The edge review is guarded.')

    expect(issued(state, 'ticket.park')).toEqual([
      { mutation: { optimistic_version: 2, idempotency_key: expect.any(String) }, ticket_id: 9 },
    ])
    expect(issued(state, 'ticket.unpark')).toEqual([
      { mutation: { optimistic_version: 3, idempotency_key: expect.any(String) }, ticket_id: 9 },
    ])
    expect(issued(state, 'issue.save')).toEqual([
      {
        mutation: { optimistic_version: 5, idempotency_key: expect.any(String) },
        identifier: 'CORE-13',
        description: 'The commands are served over the socket.',
      },
      {
        mutation: { optimistic_version: 4, idempotency_key: expect.any(String) },
        identifier: 'CORE-14',
        description: 'The edge review is guarded.',
      },
    ])
    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-14')
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
    expect(document.querySelector('[data-testid="issue-detail-description"] [role="alert"]')).toBeNull()
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Ready')
  })

  it.each([
    { save: 'is answered after the card is clicked', hold: 'issue.save' as const },
    { save: 'was answered and is still read when the card is clicked', hold: 'issue.get' as const },
  ])(
    'sends nothing from the footer while the open Ticket clicked again is read after its save $save, then acts at the version read',
    async ({ hold }) => {
      const state = harness({
        tickets: [
          implementation({ state: 'ready' }),
          implementation({
            id: 9,
            number: 14,
            state: 'ready',
            title: 'Edge review',
            slice: 'Guard the edge review',
            version: 2,
          }),
        ],
      })
      await openDrawer(state)
      const held = hold === 'issue.save' ? holdCommands(state, hold) : holdReads(state, hold)

      await saveDescription('The commands are served over the socket.')
      expect(held).toHaveLength(1)
      const ticketReads = holdReads(state, 'ticket.get')
      await click('open-ticket-8')
      if (hold === 'issue.save') held[0]!.answer()
      await flushPromises()
      expect(ticketReads).toHaveLength(1)

      await click('drawer-park')
      expect(issued(state, 'ticket.park')).toEqual([])
      await forceClick('drawer-park')
      expect(issued(state, 'ticket.park')).toEqual([])

      ticketReads[0]!.answer()
      await flushPromises()
      expect(submitButton('drawer-park').disabled).toBe(false)
      await click('drawer-park')
      expect(issued(state, 'ticket.park')).toEqual([
        { mutation: { optimistic_version: 6, idempotency_key: expect.any(String) }, ticket_id: 8 },
      ])
      expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Parked')
      expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()

      await click('open-ticket-9')
      expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-14')
      expect(submitButton('drawer-park').disabled).toBe(false)
      await click('drawer-park')
      expect(issued(state, 'ticket.park')).toEqual([
        { mutation: { optimistic_version: 6, idempotency_key: expect.any(String) }, ticket_id: 8 },
        { mutation: { optimistic_version: 2, idempotency_key: expect.any(String) }, ticket_id: 9 },
      ])
      expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Parked')
      expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
    },
  )
})

describe('the human review decision in the drawer', () => {
  it('offers a decision for the human slot the core is waiting on', async () => {
    const state = harness({ tickets: [implementation()], reviews: [review] })
    await openDrawer(state)

    expect(state.query).toHaveBeenCalledWith('review.latest', { ticket_id: 8 })
    expect(document.querySelector('[data-testid="drawer-review-decision"]')).not.toBeNull()
    await click('drawer-review-decision')
    expect(document.querySelector('[data-testid="review-decision"]')?.textContent).toContain(
      'Human slot 72',
    )
    expect(issued(state, 'review.human.submit')).toEqual([])
  })

  it('Approve sends the review version, the review, the slot, the tip, and the summary', async () => {
    const state = harness({ tickets: [implementation()], reviews: [review] })
    await openDrawer(state)

    await click('drawer-review-decision')
    await type('review-decision-summary', 'The slice holds; the gates are green.')
    await click('review-decision-approve')
    await click('review-decision-submit')

    expect(issued(state, 'review.human.submit')).toEqual([
      {
        mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
        review_id: 55,
        slot_id: 72,
        tip: 'f00dcafe',
        approve: true,
        summary: 'The slice holds; the gates are green.',
        findings: [],
      },
    ])
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
    expect(state.reviews.get(55).status).toBe('approved')
  })

  it('after a verdict lands the issue panel reads the issue again and saves at the version it reads', async () => {
    const state = harness({ tickets: [implementation()], reviews: [review] })
    const core = state.command.getMockImplementation() as (
      name: string,
      request: unknown,
    ) => Promise<unknown>
    state.command.mockImplementation(async (name: string, request: unknown) => {
      const answer = await core(name, request)
      if (name === 'review.human.submit') {
        const held = state.tickets.findIndex((entry) => entry.id === 8)
        state.tickets.splice(held, 1, { ...state.tickets[held]!, state: 'approved', version: 6 })
      }
      return answer
    })
    await openDrawer(state)

    await click('drawer-review-decision')
    await type('review-decision-summary', 'The slice holds; the gates are green.')
    await click('review-decision-submit')

    expect(
      document.querySelector('[data-testid="issue-history-row-0"] td')?.textContent?.trim(),
    ).toBe('Approved')
    await click('issue-description-edit')
    await type('issue-description-input', 'The commands are served over the socket.')
    await click('issue-description-save')

    expect(issued(state, 'issue.save').at(-1)).toEqual({
      mutation: { optimistic_version: 6, idempotency_key: expect.any(String) },
      identifier: 'CORE-13',
      description: 'The commands are served over the socket.',
    })
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
  })

  it('sends the footer command after a verdict at the version the issue read again', async () => {
    const state = harness({ tickets: [implementation()], reviews: [review] })
    const core = state.command.getMockImplementation() as (
      name: string,
      request: unknown,
    ) => Promise<unknown>
    state.command.mockImplementation(async (name: string, request: unknown) => {
      const answer = await core(name, request)
      if (name === 'review.human.submit') {
        const held = state.tickets.findIndex((entry) => entry.id === 8)
        state.tickets.splice(held, 1, { ...state.tickets[held]!, state: 'approved', version: 6 })
      }
      return answer
    })
    await openDrawer(state)

    await click('drawer-review-decision')
    await type('review-decision-summary', 'The slice holds; the gates are green.')
    await click('review-decision-submit')
    await click('drawer-park')

    expect(issued(state, 'ticket.park')).toEqual([
      { mutation: { optimistic_version: 6, idempotency_key: expect.any(String) }, ticket_id: 8 },
    ])
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
  })

  it('keeps the issue a verdict reads again off the Ticket the drawer moved to', async () => {
    const solo: ReviewExecutionRecord = {
      ...review,
      stages: [{ index: 0, status: 'waiting', slots: [humanSlot(72)] }],
    }
    let holding = false
    const held: (() => void)[] = []
    const state = harness({
      tickets: [
        implementation(),
        implementation({ id: 9, number: 14, title: 'Edge review', slice: 'Guard the edge review' }),
      ],
      reviews: [solo],
      override: (name, request) => {
        if (name !== 'issue.get' || !holding) return undefined
        if ((request as { identifier: string }).identifier !== 'CORE-13') return undefined
        return new Promise<IssueRecord>((resolve) => {
          held.push(() =>
            resolve({
              identifier: 'CORE-13',
              url: 'kanban://issue/CORE-13',
              ticket_id: 8,
              project_id: 1,
              title: 'Serve the lifecycle command surface',
              description: 'Serve the lifecycle command surface',
              state: 'Approved',
              state_type: 'started',
              priority: 3,
              labels: [],
              relations: [],
              state_history: [],
              version: 5,
              created_at: '2026-01-01T00:00:00Z',
              updated_at: '2026-01-01T00:00:00Z',
            }),
          )
        })
      },
    })
    const pinia = createPinia()
    const wrapper = await openDrawer(state, 8, pinia)
    await click('drawer-review-decision')
    await type('review-decision-summary', 'The slice holds; the gates are green.')
    holding = true
    await click('review-decision-submit')
    expect(issued(state, 'review.human.submit')).toHaveLength(1)
    expect(held.length).toBeGreaterThan(0)

    await wrapper.find('[data-testid="open-ticket-9"]').trigger('click')
    await flushPromises()
    for (const release of held.splice(0)) release()
    await flushPromises()

    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-14')
    expect(
      document.querySelector('[data-testid="issue-description-body"]')?.textContent?.trim(),
    ).toBe('Guard the edge review')
    expect(useTicketDetailStore(pinia).ticketId).toBe(9)
  })

  it('Reject sends every field of the finding the rejection resolves on', async () => {
    const state = harness({ tickets: [implementation()], reviews: [review] })
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

    expect(issued(state, 'review.human.submit')).toEqual([
      {
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
      },
    ])
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
    expect(state.reviews.get(55).status).toBe('rejected')
  })

  it('offers no decision for a human slot in a stage the core has not reached', async () => {
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
    expect(document.querySelector('[data-testid="review-decision"]')).toBeNull()
  })

  it('writes nothing over the Ticket the drawer moved to when a verdict lands late', async () => {
    const solo: ReviewExecutionRecord = {
      ...review,
      stages: [{ index: 0, status: 'waiting', slots: [humanSlot(72)] }],
    }
    const state = harness({
      tickets: [implementation(), implementation({ id: 9, number: 14, title: 'Edge review' })],
      reviews: [solo],
    })
    const core = state.command.getMockImplementation() as (
      name: string,
      request: unknown,
    ) => Promise<unknown>
    let release: (() => void) | null = null
    state.command.mockImplementation((name: string, request: unknown) =>
      name === 'review.human.submit'
        ? new Promise((resolve, reject) => {
            release = () => core(name, request).then(resolve, reject)
          })
        : core(name, request),
    )
    const pinia = createPinia()
    const wrapper = await openDrawer(state, 8, pinia)
    await click('drawer-review-decision')
    await type('review-decision-summary', 'The slice holds; the gates are green.')
    await click('review-decision-submit')
    expect(release).not.toBeNull()

    await wrapper.find('[data-testid="open-ticket-9"]').trigger('click')
    await flushPromises()
    release!()
    await flushPromises()

    expect(state.reviews.get(55).status).toBe('approved')
    expect(document.querySelector('[role="dialog"] h2')?.textContent).toContain('CORE-14')
    const detail = useTicketDetailStore(pinia)
    expect(detail.ticketId).toBe(9)
    expect(detail.review).toBeNull()
    expect(detail.error).toBeNull()
    expect(document.querySelector('[data-testid="drawer-review-slot-72"]')).toBeNull()
    expect(document.querySelector('[data-testid="drawer-review-decision"]')).toBeNull()
    expect(document.querySelector('[data-testid="drawer-action-error"]')).toBeNull()
  })
})

describe('the emergency recovery confirmation', () => {
  it('sends nothing when it opens', async () => {
    const state = harness({ tickets: [implementation({ state: 'active' })] })
    await openDrawer(state)

    await click('drawer-recover')

    expect(document.querySelector('[data-testid="recovery-confirm"]')).not.toBeNull()
    expect(state.command.mock.calls).toEqual([])
  })

  it('keeps Submit disabled, and sends nothing, until state, operator and reason are all set', async () => {
    const state = harness({ tickets: [implementation({ state: 'active' })] })
    await openDrawer(state)
    await click('drawer-recover')
    const form = () => document.querySelector('[data-testid="recovery-confirm"]') as HTMLFormElement
    const submitIncomplete = async () => {
      expect(submitButton('recovery-submit').disabled).toBe(true)
      form().dispatchEvent(new Event('submit', { cancelable: true }))
      await flushPromises()
      expect(issued(state, 'ticket.emergency.override')).toEqual([])
    }

    await submitIncomplete()
    await type('recovery-to', 'ready')
    await submitIncomplete()
    await type('recovery-who', 'Sid')
    await submitIncomplete()
    await type('recovery-to', '')
    await type('recovery-why', 'The run died holding the lane.')
    await submitIncomplete()
    await type('recovery-to', 'ready')
    await type('recovery-who', '')
    await submitIncomplete()

    await type('recovery-who', 'Sid')
    expect(submitButton('recovery-submit').disabled).toBe(false)
  })

  it('Submit sends the override with the state, the operator and the reason', async () => {
    const state = harness({ tickets: [implementation({ state: 'active' })] })
    await openDrawer(state)

    await click('drawer-recover')
    await type('recovery-to', 'ready')
    await type('recovery-who', 'Sid')
    await type('recovery-why', 'The run died holding the lane.')
    await click('recovery-submit')

    expect(issued(state, 'ticket.emergency.override')).toEqual([
      {
        mutation: { optimistic_version: 5, idempotency_key: expect.any(String) },
        ticket_id: 8,
        to: 'ready',
        who: 'Sid',
        why: 'The run died holding the lane.',
      },
    ])
    expect(document.querySelector('[data-testid="recovery-confirm"]')).toBeNull()
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Ready')
  })

  it('Cancel sends nothing and leaves the Ticket as it stood', async () => {
    const state = harness({ tickets: [implementation({ state: 'active' })] })
    await openDrawer(state)

    await click('drawer-recover')
    await type('recovery-to', 'ready')
    await type('recovery-who', 'Sid')
    await type('recovery-why', 'The run died holding the lane.')
    await click('recovery-cancel')

    expect(document.querySelector('[data-testid="recovery-confirm"]')).toBeNull()
    expect(state.command.mock.calls).toEqual([])
    expect(state.tickets).toEqual([implementation({ state: 'active' })])
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Active')
  })

  it('a refusal shows the core message and keeps the confirmation open', async () => {
    const state = harness({ tickets: [implementation({ state: 'active' })] })
    const core = state.command.getMockImplementation() as (
      name: string,
      request: unknown,
    ) => Promise<unknown>
    state.command.mockImplementation((name: string, request: unknown) =>
      name === 'ticket.emergency.override'
        ? Promise.reject({ code: 'invalid_request', message: 'a Ticket reason cannot be blank' })
        : core(name, request),
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
    expect(document.querySelector('[data-testid="drawer-state"]')?.textContent).toContain('Active')
  })
})
