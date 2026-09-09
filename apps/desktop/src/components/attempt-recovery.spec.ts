import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { RunRecord, RunRecoveryListResponse, RunRecoveryRecord, RunRecoveryRuleRequest } from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { useConnectionStore } from '../stores/connection'
import AttemptHistory from './AttemptHistory.vue'
import RunRecoveryPanel from './RunRecoveryPanel.vue'

enableAutoUnmount(afterEach)
afterEach(() => { vi.useRealTimers() })

const profile = { name: 'standard', harness: 'codex', model: 'model-a', effort: 'medium', usage_pool: 'subscription' }
const attempt = {
  id: 4, project_id: 1, ticket_id: 3, dispatch_request_id: 2,
  requested: profile, effective: profile, fallback: false, fallback_path: [],
  status: 'executing', created_at: 1, version: 1,
} satisfies RunRecord
const summary = 'Keep this attempt for audit; no completion verdict is available.'
const record = { id: 9, run_id: 4, project_id: 1, action: 'operator_ruling', ruling_id: 8, summary, created_at: 2, version: 1 } satisfies RunRecoveryRecord

const initialHistory = {
  run_id: 4, version: 0, can_resume: true, can_retry: true, pending_resume: false, records: [],
} satisfies RunRecoveryListResponse

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((accept, fail) => { resolve = accept; reject = fail })
  return { promise, resolve, reject }
}

function controlledHarness() {
  const pinia = createPinia()
  const connection = useConnectionStore(pinia)
  connection.phase = 'connected'
  const query = vi.fn<() => Promise<RunRecoveryListResponse>>().mockResolvedValue(initialHistory)
  const command = vi.fn<(name: string, request: RunRecoveryRuleRequest) => Promise<RunRecoveryRecord>>().mockResolvedValue(record)
  const transport = {
    query, command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  const wrapper = mount(AttemptHistory, {
    props: { attempts: [attempt] },
    global: { plugins: [pinia], provide: { [kanbanTransportKey as symbol]: transport } },
  })
  return { wrapper, query, command, connection }
}

function harness(canResume = true, canRetry = true, failFirst = false) {
  const commands: Array<{ name: string; request: unknown }> = []
  let recorded = false
  let saved: RunRecoveryRecord = record
  const transport = {
    query: (name: string) => {
      if (name !== 'run.recovery.list') return Promise.reject(new Error(`Unexpected query: ${name}`))
      const response = { run_id: 4, version: recorded ? 1 : 0, can_resume: canResume, can_retry: canRetry, pending_resume: false, records: recorded ? [saved] : [] } satisfies RunRecoveryListResponse
      return Promise.resolve(response)
    },
    command: (name: string, request: unknown) => {
      commands.push({ name, request })
      if (failFirst && commands.length === 1) return Promise.reject(new Error('Connection interrupted before acknowledgement'))
      recorded = true
      const action = name === 'run.recovery.resume' ? 'resume' : name === 'run.recovery.retry' ? 'retry' : 'operator_ruling'
      saved = { ...record, action, replacement_dispatch_request_id: action === 'retry' ? 10 : null }
      return Promise.resolve(saved)
    },
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
  return {
    commands,
    wrapper: mount(AttemptHistory, {
      props: { attempts: [attempt] },
      global: { plugins: [createPinia()], provide: { [kanbanTransportKey as symbol]: transport } },
    }),
  }
}

describe('attempt recovery', () => {
  it('clears an obsolete read failure after connection refresh succeeds', async () => {
    const { wrapper, query, connection } = controlledHarness()
    await flushPromises()
    await wrapper.get('[data-testid="run-recovery-summary-4"]').setValue(summary)
    query.mockRejectedValueOnce(new Error('Core unavailable'))
    await wrapper.setProps({ attempts: [{ ...attempt, version: 2 }] })
    await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toContain('Core unavailable')

    connection.phase = 'connecting'
    await flushPromises()

    expect(query).toHaveBeenCalledTimes(3)
    expect(wrapper.find('[role="alert"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="run-recovery-resume-4"]').attributes('disabled')).toBeUndefined()
  })

  it('cancels scheduled pending-delivery polling on unmount', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    const { wrapper, query } = controlledHarness()
    await flushPromises()
    query.mockResolvedValue({ ...initialHistory, pending_resume: true, can_resume: false })
    await wrapper.setProps({ attempts: [{ ...attempt, version: 2 }] })
    await flushPromises()
    expect(vi.getTimerCount()).toBe(1)

    wrapper.unmount()

    expect(vi.getTimerCount()).toBe(0)
    await vi.advanceTimersByTimeAsync(10000)
    expect(query).toHaveBeenCalledTimes(2)
  })

  it('polls only while Core reports pending delivery', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    const { wrapper, query, command } = controlledHarness()
    await flushPromises()
    await vi.advanceTimersByTimeAsync(5000)
    expect(query).toHaveBeenCalledTimes(1)
    query.mockResolvedValue({ ...initialHistory, pending_resume: true, can_resume: false })
    await wrapper.setProps({ attempts: [{ ...attempt, version: 2 }] })
    await flushPromises()
    expect(wrapper.find('[role="status"]').exists()).toBe(true)

    query.mockResolvedValue(initialHistory)
    await vi.advanceTimersByTimeAsync(5000)
    await flushPromises()
    expect(query).toHaveBeenCalledTimes(3)
    expect(wrapper.find('[role="status"]').exists()).toBe(false)
    await vi.advanceTimersByTimeAsync(10000)
    expect(query).toHaveBeenCalledTimes(3)
    expect(command).not.toHaveBeenCalled()
  })

  it('does not restart pending-delivery polling when a read completes after unmount', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    const { wrapper, query } = controlledHarness()
    await flushPromises()
    const pendingHistory = { ...initialHistory, pending_resume: true, can_resume: false }
    query.mockResolvedValue(pendingHistory)
    await wrapper.setProps({ attempts: [{ ...attempt, version: 2 }] })
    await flushPromises()
    const inFlight = deferred<RunRecoveryListResponse>()
    query.mockReturnValueOnce(inFlight.promise)
    await vi.advanceTimersByTimeAsync(5000)
    expect(query).toHaveBeenCalledTimes(3)

    wrapper.unmount()
    inFlight.resolve(pendingHistory)
    await flushPromises()
    await vi.advanceTimersByTimeAsync(10000)
    expect(query).toHaveBeenCalledTimes(3)
  })

  it('renders Core pending delivery without claiming execution resumed', async () => {
    const { wrapper, query, command } = controlledHarness()
    await flushPromises()
    await wrapper.get('[data-testid="run-recovery-summary-4"]').setValue(summary)
    query.mockResolvedValue({ ...initialHistory, pending_resume: true, can_resume: false })
    await wrapper.setProps({ attempts: [{ ...attempt, version: 2 }] })
    await flushPromises()

    expect(wrapper.get('[role="status"]').text()).toBe('Resume requested; waiting for delivery. This is not an execution acknowledgement.')
    expect(wrapper.get('[data-testid="run-recovery-resume-4"]').attributes('disabled')).toBeDefined()
    expect(wrapper.get('[data-testid="run-recovery-retry-4"]').attributes('disabled')).toBeUndefined()
    expect(command).not.toHaveBeenCalled()

    query.mockResolvedValue(initialHistory)
    await wrapper.setProps({ attempts: [{ ...attempt, version: 3 }] })
    await flushPromises()
    expect(wrapper.find('[role="status"]').exists()).toBe(false)
    expect(wrapper.get('[data-testid="run-recovery-resume-4"]').attributes('disabled')).toBeUndefined()
    expect(wrapper.text()).not.toContain('Execution resumed')
  })

  it('keeps the in-flight request locked across a same-run refresh', async () => {
    const { wrapper, query, command } = controlledHarness()
    await flushPromises()
    await wrapper.get('[data-testid="run-recovery-summary-4"]').setValue(summary)
    const inFlight = deferred<RunRecoveryRecord>()
    command.mockReturnValueOnce(inFlight.promise)
    await wrapper.get('[data-testid="run-recovery-form-4"]').trigger('submit')
    const original = command.mock.calls[0]![1]
    query.mockResolvedValue({ ...initialHistory, version: 7 })
    await wrapper.setProps({ attempts: [{ ...attempt, version: 2 }] })
    await flushPromises()
    await wrapper.get('[data-testid="run-recovery-form-4"]').trigger('submit')
    expect(command).toHaveBeenCalledTimes(1)
    expect(wrapper.get('button[type="submit"]').attributes('disabled')).toBeDefined()

    inFlight.reject(new Error('Acknowledgement lost'))
    await flushPromises()
    await wrapper.get('[data-testid="run-recovery-form-4"]').trigger('submit')
    await flushPromises()
    expect(command).toHaveBeenCalledTimes(2)
    expect(command.mock.calls[1]![1]).toEqual(original)
    expect(wrapper.emitted('recovered')).toEqual([[4]])
  })

  it.each(['success', 'stale_version', 'transport'])('ignores a command %s after its attempt unmounts', async (outcome) => {
    const { wrapper, query, command, connection } = controlledHarness()
    await flushPromises()
    const panel = wrapper.getComponent(RunRecoveryPanel)
    await wrapper.get('[data-testid="run-recovery-summary-4"]').setValue(summary)
    const inFlight = deferred<RunRecoveryRecord>()
    command.mockReturnValueOnce(inFlight.promise)
    await wrapper.get('[data-testid="run-recovery-form-4"]').trigger('submit')
    await wrapper.setProps({ attempts: [] })

    if (outcome === 'success') inFlight.resolve(record)
    else if (outcome === 'stale_version') inFlight.reject({ code: 'stale_version', message: 'Recovery version changed' })
    else inFlight.reject(new Error('Acknowledgement lost'))
    connection.phase = 'disconnected'
    await flushPromises()

    expect(query).toHaveBeenCalledTimes(1)
    expect(panel.emitted('recovered')).toBeUndefined()
    expect(wrapper.emitted('recovered')).toBeUndefined()
    expect(wrapper.text()).toContain('No attempts yet.')
  })

  it.each(['success', 'failure'])('ignores a refresh %s after switching attempts', async (outcome) => {
    const { wrapper, query } = controlledHarness()
    await flushPromises()
    const obsolete = deferred<RunRecoveryListResponse>()
    query.mockReturnValueOnce(obsolete.promise)
    await wrapper.setProps({ attempts: [{ ...attempt, version: 2 }] })
    query.mockResolvedValue({ ...initialHistory, run_id: 5, can_resume: false, can_retry: false })
    await wrapper.setProps({ attempts: [{ ...attempt, id: 5 }] })
    await flushPromises()

    if (outcome === 'success') obsolete.resolve({ ...initialHistory, records: [record] })
    else obsolete.reject(new Error('Obsolete attempt failed'))
    await flushPromises()

    expect(query).toHaveBeenCalledTimes(3)
    expect(wrapper.get('[data-testid="run-recovery-history-5"]').text()).toBe('')
    expect(wrapper.find('[data-testid="run-recovery-form-4"]').exists()).toBe(false)
    expect(wrapper.find('[role="alert"]').exists()).toBe(false)
  })

  it.each(['stale_version', 'attempt', 'connection'])('blocks stale actions while a %s refresh is pending or failed', async (source) => {
    const { wrapper, query, command, connection } = controlledHarness()
    await flushPromises()
    const reason = wrapper.get('[data-testid="run-recovery-summary-4"]')
    await reason.setValue(summary)
    const fresh = deferred<RunRecoveryListResponse>()
    query.mockReturnValueOnce(fresh.promise)
    if (source === 'stale_version') {
      command.mockRejectedValueOnce({ code: 'stale_version', message: 'Recovery version changed' })
      await wrapper.get('[data-testid="run-recovery-form-4"]').trigger('submit')
    } else if (source === 'attempt') {
      await wrapper.setProps({ attempts: [{ ...attempt, version: 2 }] })
    } else {
      connection.phase = 'disconnected'
    }
    await flushPromises()

    expect(wrapper.get('[data-testid="run-recovery-resume-4"]').attributes('disabled')).toBeDefined()
    expect(wrapper.get('[data-testid="run-recovery-retry-4"]').attributes('disabled')).toBeDefined()
    expect(wrapper.get('button[type="submit"]').attributes('disabled')).toBeDefined()
    fresh.reject(new Error('Recovery refresh unavailable'))
    await flushPromises()

    expect(wrapper.get('button[type="submit"]').attributes('disabled')).toBeDefined()
    expect(wrapper.get('[data-testid="run-recovery-resume-4"]').attributes('disabled')).toBeDefined()
    expect(wrapper.get('[data-testid="run-recovery-retry-4"]').attributes('disabled')).toBeDefined()
    expect(wrapper.get('[role="alert"]').text()).toContain('Recovery refresh unavailable')
    expect((reason.element as HTMLTextAreaElement).value).toBe(summary)
    await wrapper.get('[data-testid="run-recovery-form-4"]').trigger('submit')
    await flushPromises()
    expect(command).toHaveBeenCalledTimes(source === 'stale_version' ? 1 : 0)
  })

  it.each(['success', 'failure'])('ignores an obsolete same-run refresh %s', async (outcome) => {
    const { wrapper, query } = controlledHarness()
    await flushPromises()
    await wrapper.get('[data-testid="run-recovery-summary-4"]').setValue(summary)
    const obsolete = deferred<RunRecoveryListResponse>()
    query.mockReturnValueOnce(obsolete.promise)
    await wrapper.setProps({ attempts: [{ ...attempt, version: 2 }] })
    query.mockResolvedValue({ ...initialHistory, version: 2, can_resume: false, can_retry: false, records: [record] })
    await wrapper.setProps({ attempts: [{ ...attempt, version: 3 }] })
    await flushPromises()

    if (outcome === 'success') obsolete.resolve(initialHistory)
    else obsolete.reject(new Error('Obsolete refresh failed'))
    await flushPromises()

    expect(query).toHaveBeenCalledTimes(3)
    expect(wrapper.get('[data-testid="run-recovery-history-4"]').text()).toContain(summary)
    expect(wrapper.get('[data-testid="run-recovery-resume-4"]').attributes('disabled')).toBeDefined()
    expect(wrapper.get('[data-testid="run-recovery-retry-4"]').attributes('disabled')).toBeDefined()
    expect(wrapper.find('[role="alert"]').exists()).toBe(false)
  })

  it('refreshes through connection verification phases without discarding an ambiguous request', async () => {
    const { wrapper, query, command, connection } = controlledHarness()
    await flushPromises()
    await wrapper.get('[data-testid="run-recovery-summary-4"]').setValue(summary)
    command.mockRejectedValueOnce(new Error('Connection interrupted before acknowledgement'))
    await wrapper.get('[data-testid="run-recovery-resume-4"]').trigger('click')
    await flushPromises()
    const originalRequest = command.mock.calls[0]![1]

    query.mockRejectedValueOnce(new Error('Core disconnected'))
    connection.phase = 'disconnected'
    await flushPromises()
    expect(query).toHaveBeenCalledTimes(2)

    query.mockResolvedValue({ ...initialHistory, version: 3 })
    connection.phase = 'connecting'
    await flushPromises()
    expect(query).toHaveBeenCalledTimes(3)
    connection.phase = 'connected'
    await flushPromises()
    expect(query).toHaveBeenCalledTimes(4)
    expect(command).toHaveBeenCalledTimes(1)

    await wrapper.get('[data-testid="run-recovery-resume-4"]').trigger('click')
    await flushPromises()
    expect(command).toHaveBeenCalledTimes(2)
    expect(command.mock.calls[1]![1]).toEqual(originalRequest)
    expect(wrapper.emitted('recovered')).toEqual([[4]])
  })

  it.each([
    { status: 'submitted' as const },
    { status: 'superseded' as const },
    { version: 2 },
  ])('refreshes Core eligibility when the same attempt changes: %j', async (change) => {
    const { wrapper, query, command } = controlledHarness()
    await flushPromises()
    await wrapper.get('[data-testid="run-recovery-summary-4"]').setValue(summary)
    query.mockResolvedValue({ ...initialHistory, can_resume: false, can_retry: false })

    await wrapper.setProps({ attempts: [{ ...attempt, ...change }] })
    await flushPromises()

    expect(query).toHaveBeenCalledTimes(2)
    expect(wrapper.get('[data-testid="run-recovery-resume-4"]').attributes('disabled')).toBeDefined()
    expect(wrapper.get('[data-testid="run-recovery-retry-4"]').attributes('disabled')).toBeDefined()
    expect(command).not.toHaveBeenCalled()
  })

  it.each(['resume', 'retry', 'rule'])('refreshes a definitively stale %s before accepting a new explicit request', async (action) => {
    const { wrapper, query, command } = controlledHarness()
    await flushPromises()
    const reason = wrapper.get('[data-testid="run-recovery-summary-4"]')
    await reason.setValue(summary)
    command.mockRejectedValueOnce({ code: 'stale_version', message: 'Recovery version changed' })
    query.mockResolvedValue({ ...initialHistory, version: 1, records: [record] })
    const submit = () => action === 'rule'
      ? wrapper.get('[data-testid="run-recovery-form-4"]').trigger('submit')
      : wrapper.get(`[data-testid="run-recovery-${action}-4"]`).trigger('click')

    await submit()
    await flushPromises()

    expect(query).toHaveBeenCalledTimes(2)
    expect(command).toHaveBeenCalledTimes(1)
    expect(wrapper.get('[data-testid="run-recovery-history-4"]').text()).toContain(summary)
    expect((reason.element as HTMLTextAreaElement).value).toBe(summary)
    expect(wrapper.emitted('recovered')).toBeUndefined()

    await submit()
    await flushPromises()

    const first = command.mock.calls[0]![1]
    const second = command.mock.calls[1]![1]
    expect(first.mutation.optimistic_version).toBe(0)
    expect(second).toEqual({ ...first, mutation: {
      optimistic_version: 1, idempotency_key: expect.any(String),
    } })
    expect(second.mutation.idempotency_key).not.toBe(first.mutation.idempotency_key)
    expect(wrapper.emitted('recovered')).toEqual([[4]])
  })

  it.each(['resume', 'retry'])('retries an interrupted %s with the original idempotency key', async (action) => {
    const { wrapper, commands } = harness(true, true, true)
    await flushPromises()
    await wrapper.get('[data-testid="run-recovery-summary-4"]').setValue(summary)
    await wrapper.get(`[data-testid="run-recovery-${action}-4"]`).trigger('click')
    await flushPromises()
    expect(wrapper.find('[role="alert"]').exists()).toBe(true)
    await wrapper.get(`[data-testid="run-recovery-${action}-4"]`).trigger('click')
    await flushPromises()
    expect(commands).toHaveLength(2)
    expect(commands[1]).toEqual(commands[0])
    expect(wrapper.emitted('recovered')).toEqual([[4]])
  })

  it.each([
    ['resume', 'run.recovery.resume'],
    ['retry', 'run.recovery.retry'],
  ])('requests %s only after an explicit operator click', async (action, operation) => {
    const { wrapper, commands } = harness()
    await flushPromises()
    expect(commands).toEqual([])
    const button = wrapper.get(`[data-testid="run-recovery-${action}-4"]`)
    expect(button.attributes('disabled')).toBeDefined()
    await wrapper.get('[data-testid="run-recovery-summary-4"]').setValue(summary)
    await button.trigger('click')
    await flushPromises()
    expect(commands).toEqual([{ name: operation, request: {
      run_id: 4, summary, mutation: { optimistic_version: 0, idempotency_key: expect.any(String) },
    } }])
    expect(wrapper.props('attempts')).toEqual([attempt])
  })

  it('uses the Core recovery choices instead of interpreting execution signals', async () => {
    const { wrapper, commands } = harness(false, false)
    await flushPromises()
    await wrapper.get('[data-testid="run-recovery-summary-4"]').setValue(summary)
    expect(wrapper.get('[data-testid="run-recovery-resume-4"]').attributes('disabled')).toBeDefined()
    expect(wrapper.get('[data-testid="run-recovery-retry-4"]').attributes('disabled')).toBeDefined()
    expect(commands).toEqual([])
  })

  it('records an explicit ruling without replacing the original attempt', async () => {
    const { wrapper, commands } = harness()
    await flushPromises()
    expect(wrapper.find('[data-testid="run-recovery-form-4"]').exists()).toBe(true)
    expect(commands).toEqual([])
    await wrapper.get('[data-testid="run-recovery-summary-4"]').setValue(summary)
    await wrapper.get('[data-testid="run-recovery-form-4"]').trigger('submit')
    await flushPromises()
    expect(commands).toEqual([{
      name: 'run.recovery.rule', request: {
        run_id: 4, summary,
        mutation: { optimistic_version: 0, idempotency_key: expect.any(String) },
      },
    }])
    expect(wrapper.get('[data-testid="run-recovery-history-4"]').text()).toContain(summary)
    expect(wrapper.get('[data-testid="drawer-attempt-4"]').text()).toContain('standard')
    expect(wrapper.props('attempts')).toEqual([attempt])
    expect(wrapper.emitted('recovered')).toEqual([[4]])
  })
})
