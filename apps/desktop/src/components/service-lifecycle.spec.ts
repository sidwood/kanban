import { createPinia } from 'pinia'
import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import type { LoginLaunchState, ServiceStopWarning } from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import type { ShellConnectionState } from '../core/transport'
import { useServiceLifecycleStore } from '../stores/service-lifecycle'
import ServiceLifecycle from './ServiceLifecycle.vue'

const registration: LoginLaunchState = { instance_id: 'instance', version: 0, enabled: false, error: null }
const warning: ServiceStopWarning = { instance_id: 'instance', version: 0, warning_id: 'current', capabilities: ['MCP'] }

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((accept, fail) => { resolve = accept; reject = fail })
  return { promise, resolve, reject }
}

function harness(pinia = createPinia(), initial: Promise<LoginLaunchState> = Promise.resolve(registration)) {
  const query = vi.fn().mockImplementation(async (name: string): Promise<unknown> => name === 'service.stop_warning' ? warning : registration)
  query.mockReturnValueOnce(initial)
  const command = vi.fn().mockResolvedValue({ status: 'change_requested' })
  let connectionHandler: ((state: ShellConnectionState) => void) | undefined
  const unsubscribe = vi.fn()
  const transport = {
    query, command,
    subscribe: () => () => undefined,
    onConnectionChange(handler: (state: ShellConnectionState) => void) {
      connectionHandler = handler
      return unsubscribe
    },
  }
  const wrapper = mount(ServiceLifecycle, { global: { plugins: [pinia], provide: { [kanbanTransportKey as symbol]: transport } } })
  return { wrapper, store: useServiceLifecycleStore(pinia), transport, query, command, unsubscribe, announce: (state: ShellConnectionState) => connectionHandler?.(state) }
}

describe('service lifecycle binding', () => {
  it('keeps disconnected controls unavailable until a connected readback', async () => {
    const { wrapper, store, transport, query, announce } = harness()
    await flushPromises()
    announce('disconnected')
    await flushPromises()
    const queriesBefore = query.mock.calls.length
    await store.refreshLogin(transport)
    await store.reviewStop(transport)
    expect(query).toHaveBeenCalledTimes(queriesBefore)
    expect(store.login).toBeNull()
    expect(wrapper.get('[data-testid="review-stop"]').attributes('disabled')).toBeDefined()
    const refreshButton = wrapper.findAll('button').find(button => button.text() === 'Refresh registration')!
    expect(refreshButton.attributes('disabled')).toBeDefined()

    announce('connected')
    await flushPromises()
    expect(store.login).toEqual(registration)
    expect(refreshButton.attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })

  it.each([
    ['disconnect', 'command', 'response'], ['disconnect', 'command', 'error'],
    ['disconnect', 'readback', 'response'], ['disconnect', 'readback', 'error'],
    ['binding', 'command', 'response'], ['binding', 'command', 'error'],
    ['binding', 'readback', 'response'], ['binding', 'readback', 'error'],
    ['instance', 'command', 'response'], ['instance', 'command', 'error'],
    ['instance', 'readback', 'response'], ['instance', 'readback', 'error'],
    ['disconnect', 'stop', 'response'], ['disconnect', 'stop', 'error'],
    ['binding', 'stop', 'response'], ['binding', 'stop', 'error'],
    ['instance', 'stop', 'response'], ['instance', 'stop', 'error'],
  ] as const)('keeps newer mutation state after %s invalidates an old %s %s', async (change, phase, outcome) => {
    const pinia = createPinia()
    const first = harness(pinia)
    await flushPromises()
    const older = deferred<unknown>()
    if (phase === 'stop') await first.store.reviewStop(first.transport)
    if (phase !== 'readback') first.command.mockReturnValueOnce(older.promise)
    else first.query.mockReturnValueOnce(older.promise)
    const pending = phase === 'stop' ? first.store.confirmStop(first.transport) : first.store.setLogin(first.transport, true)
    await flushPromises()

    const confirmed = { ...registration, instance_id: 'replacement', version: 1, enabled: true }
    let current = first
    if (change === 'binding') current = harness(pinia, Promise.resolve(confirmed))
    else if (change === 'instance') {
      first.query.mockResolvedValueOnce(confirmed)
      await first.store.refreshLogin(first.transport)
    }
    else {
      first.announce('disconnected')
      first.query.mockResolvedValueOnce(confirmed)
      first.announce('connected')
    }
    await flushPromises()
    const mutation = deferred<unknown>()
    current.command.mockReturnValueOnce(mutation.promise)
    const newer = current.store.setLogin(current.transport, false)
    const queriesBefore = current.query.mock.calls.length

    if (outcome === 'response') older.resolve(phase === 'readback' ? registration : { status: phase === 'stop' ? 'stop_requested' : 'change_requested' })
    else older.reject(new Error('Old operation failed'))
    await pending
    await flushPromises()

    expect(current.store.login).toEqual(confirmed)
    expect(current.store.error).toBeNull()
    expect(current.store.message).toBeNull()
    expect(current.store.busy).toBe(true)
    expect(current.query).toHaveBeenCalledTimes(queriesBefore)
    expect(current.wrapper.get('[data-testid="login-disable"]').attributes('disabled')).toBeDefined()
    const readback = { ...confirmed, version: 2, enabled: false }
    current.query.mockResolvedValueOnce(readback)
    mutation.resolve({ status: 'change_requested' })
    await newer
    expect(current.store.login).toEqual(readback)
    expect(current.store.busy).toBe(false)
    first.wrapper.unmount()
    if (current !== first) current.wrapper.unmount()
  })

  it.each(['response', 'error'] as const)('ignores the previous binding\'s login %s and cleanup', async (outcome) => {
    const pinia = createPinia()
    const first = harness(pinia)
    await flushPromises()
    const older = deferred<LoginLaunchState>()
    first.query.mockReturnValueOnce(older.promise)
    const pending = first.store.refreshLogin(first.transport)
    const current = deferred<LoginLaunchState>()
    const second = harness(pinia, current.promise)
    await flushPromises()
    expect(second.wrapper.get('[data-testid="login-state"]').text()).toContain('Unknown')
    expect(second.wrapper.get('[data-testid="login-enable"]').attributes('disabled')).toBeDefined()

    const confirmed = { ...registration, instance_id: 'replacement', enabled: true }
    current.resolve(confirmed)
    await flushPromises()
    first.announce('disconnected')
    first.wrapper.unmount()
    if (outcome === 'response') older.resolve(registration)
    else older.reject(new Error('Old binding failed'))
    await pending
    await flushPromises()

    expect(second.store.login).toEqual(confirmed)
    expect(second.store.error).toBeNull()
    expect(second.wrapper.get('[data-testid="login-enable"]').attributes('disabled')).toBeUndefined()
    expect(first.unsubscribe).toHaveBeenCalledOnce()
    second.wrapper.unmount()
    expect(second.store.login).toBeNull()
    expect(second.unsubscribe).toHaveBeenCalledOnce()
  })

  it.each(['response', 'error'] as const)('does not restore disconnected controls from a pending login %s', async (outcome) => {
    const { wrapper, store, transport, query, announce, unsubscribe } = harness()
    await flushPromises()
    await wrapper.get('[data-testid="review-stop"]').trigger('click')
    await flushPromises()
    const older = deferred<LoginLaunchState>()
    query.mockImplementationOnce(() => older.promise)
    const refresh = store.refreshLogin(transport)

    announce('disconnected')
    await flushPromises()
    expect(wrapper.get('[data-testid="login-state"]').text()).toContain('Unknown')
    expect(wrapper.get('[data-testid="login-enable"]').attributes('disabled')).toBeDefined()
    expect(wrapper.find('[data-testid="confirm-stop"]').exists()).toBe(false)

    if (outcome === 'response') older.resolve({ ...registration, enabled: true })
    else older.reject(new Error('Old connection failed'))
    await refresh
    await flushPromises()
    expect(store.login).toBeNull()
    expect(store.error).toBeNull()
    expect(wrapper.get('[data-testid="login-enable"]').attributes('disabled')).toBeDefined()

    const confirmed = { ...registration, instance_id: 'replacement', version: 1, enabled: true }
    query.mockResolvedValueOnce(confirmed)
    announce('connected')
    await flushPromises()
    expect(store.login).toEqual(confirmed)
    expect(wrapper.get('[data-testid="login-state"]').text()).toContain('Enabled')
    wrapper.unmount()
    expect(unsubscribe).toHaveBeenCalledOnce()
  })
})
