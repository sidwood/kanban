// KAN-T30-AC2: the detail drawer shows Linear issue detail (U2).
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type {
  ProfileSnapshotRecord,
  RunRecord,
  RunRecoveryListResponse,
  TicketRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import { harness, ticket } from '../test/shell-harness'
import type { HarnessOptions } from '../test/shell-harness'
import BoardView from './BoardView.vue'

// The Implementation every drawer opens on, unless a test names
// another.
const implementation = (overrides: Partial<TicketRecord> = {}): TicketRecord =>
  ticket({
    id: 8,
    number: 13,
    kind: 'implementation',
    state: 'active',
    title: 'Serve the lifecycle command surface',
    slice: 'Serve the lifecycle command surface',
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

const mountedBoards: VueWrapper[] = []

// The board over one shell, with the drawer open on one Ticket.
async function openDrawer(shell: ReturnType<typeof harness>, ticketId: number) {
  await router.push('/projects/1/board')
  await router.isReady()
  const wrapper = mount(BoardView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: shell.transport },
    },
    attachTo: document.body,
  })
  mountedBoards.push(wrapper)
  await flushPromises()
  await wrapper.find(`[data-testid="open-ticket-${ticketId}"]`).trigger('click')
  await flushPromises()
  return wrapper
}

async function type(testid: string, value: string): Promise<void> {
  const element = document.querySelector(`[data-testid="${testid}"]`)
  if (!(element instanceof HTMLTextAreaElement)) throw new Error(`no field ${testid}`)
  element.value = value
  element.dispatchEvent(new Event('input'))
  await flushPromises()
}

async function click(testid: string): Promise<void> {
  const element = document.querySelector(`[data-testid="${testid}"]`)
  if (!(element instanceof HTMLButtonElement)) throw new Error(`no ${testid}`)
  expect(element.disabled, `${testid} is offered`).toBe(false)
  element.click()
  await flushPromises()
}

// A run the core has judged stopped: it may be resumed in place or
// retried in a new attempt.
const stopped: RunRecoveryListResponse = {
  run_id: 3,
  version: 2,
  can_resume: true,
  can_retry: true,
  pending_resume: false,
  records: [],
}

const recoverable: HarnessOptions['override'] = (name) =>
  name === 'run.recovery.list' ? Promise.resolve(stopped) : undefined

const sentAs = (shell: ReturnType<typeof harness>, name: string): unknown[] =>
  shell.command.mock.calls.filter(([sent]) => sent === name).map(([, request]) => request)

const runReads = (shell: ReturnType<typeof harness>): number =>
  shell.query.mock.calls.filter(([name]) => name === 'run.list').length

beforeEach(() => {
  localStorage.clear()
})

afterEach(() => {
  for (const wrapper of mountedBoards.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

describe('issue drawer', () => {
  it('loads issue detail through ticket.get and issue.get when it opens', async () => {
    const shell = harness({ tickets: [implementation()] })
    await openDrawer(shell, 8)

    expect(shell.query).toHaveBeenCalledWith('ticket.get', { ticket_id: 8 })
    expect(shell.query).toHaveBeenCalledWith('issue.get', { identifier: 'CORE-13' })
    expect(document.querySelector('[data-testid="issue-detail-panel"]')).toBeTruthy()
    expect(document.querySelector('[data-testid="drawer-criteria"]')).toBeFalsy()
  })

  it('lists every historical attempt for the open ticket', async () => {
    const attempts = [
      run({ id: 1, created_at: 10, effective: snapshot('first-run') }),
      run({ id: 2, created_at: 30, effective: snapshot('second-run'), fallback: false }),
      run({ id: 6, ticket_id: 9, created_at: 40, effective: snapshot('other-ticket-run') }),
    ]
    const shell = harness({
      tickets: [implementation(), implementation({ id: 9, number: 14 })],
      runs: attempts,
    })
    await openDrawer(shell, 8)

    const history = document.querySelector('[data-testid="drawer-attempts"]')
    expect(history?.textContent).toContain('first-run')
    expect(history?.textContent).toContain('second-run')
    expect(history?.textContent).not.toContain('other-ticket-run')
    expect(history?.querySelectorAll('[data-testid^="drawer-attempt-"]')).toHaveLength(2)
  })

  it('asks the core to resume a stopped run from the drawer and reads the attempts again', async () => {
    const shell = harness({ tickets: [implementation()], runs: [run()], override: recoverable })
    await openDrawer(shell, 8)
    const before = runReads(shell)

    await type('run-recovery-summary-3', 'The harness exited mid-run; resume where it stopped.')
    await click('run-recovery-resume-3')

    expect(sentAs(shell, 'run.recovery.resume')).toEqual([
      {
        mutation: { optimistic_version: 2, idempotency_key: expect.any(String) },
        run_id: 3,
        summary: 'The harness exited mid-run; resume where it stopped.',
      },
    ])
    expect(runReads(shell)).toBeGreaterThan(before)
  })

  it('asks the core to retry a stopped run from the drawer and shows the new attempt', async () => {
    const runs = [run()]
    const shell = harness({ tickets: [implementation()], runs, override: recoverable })
    const core = shell.command.getMockImplementation() as (
      name: string,
      request: unknown,
    ) => Promise<unknown>
    shell.command.mockImplementation((name: string, request: unknown) => {
      // The core queues a new attempt for the Ticket when it retries.
      if (name === 'run.recovery.retry') {
        runs.push(run({ id: 7, created_at: 50, effective: snapshot('retried-run') }))
      }
      return core(name, request)
    })
    await openDrawer(shell, 8)
    expect(document.querySelector('[data-testid="drawer-attempt-7"]')).toBeNull()

    await type('run-recovery-summary-3', 'The run died holding the lane; start it again.')
    await click('run-recovery-retry-3')

    expect(sentAs(shell, 'run.recovery.retry')).toEqual([
      {
        mutation: { optimistic_version: 2, idempotency_key: expect.any(String) },
        run_id: 3,
        summary: 'The run died holding the lane; start it again.',
      },
    ])
    expect(document.querySelector('[data-testid="drawer-attempt-7"]')?.textContent).toContain(
      'retried-run',
    )
  })
})
