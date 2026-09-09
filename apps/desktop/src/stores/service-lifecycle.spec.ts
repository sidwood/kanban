import { createPinia, setActivePinia } from 'pinia'
import { flushPromises } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import type { LoginLaunchState, ServiceStopWarning } from '@kanban/contracts'
import { useServiceLifecycleStore } from './service-lifecycle'

const registration: LoginLaunchState = { instance_id: 'instance', version: 0, enabled: false, error: null }
const warning: ServiceStopWarning = { instance_id: 'instance', version: 0, warning_id: 'current', capabilities: ['MCP'] }

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((accept, fail) => { resolve = accept; reject = fail })
  return { promise, resolve, reject }
}

function harness() {
  setActivePinia(createPinia())
  const query = vi.fn().mockResolvedValue(registration)
  const command = vi.fn().mockResolvedValue({ status: 'change_requested' })
  const transport = { query, command, subscribe: () => () => undefined, onConnectionChange: () => () => undefined }
  return { store: useServiceLifecycleStore(), transport, query, command }
}

describe('service lifecycle login readback', () => {
  it.each(['response', 'error'] as const)('keeps an ambiguous stop error instead of an older login %s', async (outcome) => {
    const { store, transport, query, command } = harness()
    await store.refreshLogin(transport)
    query.mockResolvedValueOnce(warning)
    await store.reviewStop(transport)
    const older = deferred<LoginLaunchState>()
    query.mockReturnValueOnce(older.promise)
    const pending = store.refreshLogin(transport)
    command.mockRejectedValueOnce(new Error('Connection ended'))
    await store.confirmStop(transport)
    expect(store.error).toContain('Service state is unknown')

    if (outcome === 'response') older.resolve(registration)
    else older.reject(new Error('Old read failed'))
    await pending
    expect(store.error).toContain('Service state is unknown')
    expect(store.warning).toBeNull()
  })

  it.each(['response', 'error'] as const)('invalidates a pending login %s when a warning identifies a replacement service', async (outcome) => {
    const { store, transport, query } = harness()
    await store.refreshLogin(transport)
    const older = deferred<LoginLaunchState>()
    const replacement = { ...warning, instance_id: 'replacement' }
    query.mockReturnValueOnce(older.promise).mockResolvedValueOnce(replacement)
    const pending = store.refreshLogin(transport)
    await store.reviewStop(transport)
    expect(store.login).toBeNull()
    expect(store.warning).toEqual(replacement)
    expect(store.reviewing).toBe(true)

    if (outcome === 'response') older.resolve(registration)
    else older.reject(new Error('Old service failed'))
    await pending
    expect(store.login).toBeNull()
    expect(store.error).toBeNull()
    expect(store.warning).toEqual(replacement)
  })

  it.each(['response', 'error'] as const)('invalidates an old warning %s when login readback identifies a replacement service', async (outcome) => {
    const { store, transport, query } = harness()
    await store.refreshLogin(transport)
    const older = deferred<ServiceStopWarning>()
    const confirmed = { ...registration, instance_id: 'replacement', enabled: true, error: 'Current OS detail' }
    query.mockReturnValueOnce(older.promise).mockResolvedValueOnce(confirmed)
    const pending = store.reviewStop(transport)
    await store.refreshLogin(transport)

    if (outcome === 'response') older.resolve(warning)
    else older.reject(new Error('Old service failed'))
    await pending
    expect(store.login).toEqual(confirmed)
    expect(store.error).toBe('Current OS detail')
    expect(store.warning).toBeNull()
    expect(store.reviewing).toBe(false)
  })

  it.each(['response', 'error'] as const)('invalidates an older refresh %s when a mutation starts', async (outcome) => {
    const { store, transport, query, command } = harness()
    await store.refreshLogin(transport)
    const older = deferred<LoginLaunchState>()
    const mutation = deferred<unknown>()
    const readback = deferred<LoginLaunchState>()
    query.mockReturnValueOnce(older.promise).mockReturnValueOnce(readback.promise)
    command.mockReturnValueOnce(mutation.promise)
    const refresh = store.refreshLogin(transport)
    const change = store.setLogin(transport, true)

    if (outcome === 'response') older.resolve({ ...registration, error: 'Outdated OS error' })
    else older.reject(new Error('Old connection failed'))
    await refresh

    expect(store.login).toEqual(registration)
    expect(store.error).toBeNull()
    expect(store.busy).toBe(true)
    expect(command).toHaveBeenCalledWith('service.login_launch.set', {
      mutation: { optimistic_version: 0, idempotency_key: expect.any(String) },
      instance_id: 'instance',
      enabled: true,
    })
    mutation.resolve({ status: 'change_requested' })
    await flushPromises()
    expect(store.login?.enabled).toBe(false)
    expect(store.busy).toBe(true)
    const confirmed = { ...registration, version: 1, enabled: true }
    readback.resolve(confirmed)
    await change
    expect(store.login).toEqual(confirmed)
    expect(store.busy).toBe(false)
  })

  it.each(['response', 'error'] as const)('ignores an older refresh %s after newer OS readback', async (outcome) => {
    const { store, transport, query } = harness()
    const older = deferred<LoginLaunchState>()
    const confirmed: LoginLaunchState = { instance_id: 'replacement', version: 1, enabled: true, error: 'Current OS detail' }
    query.mockReturnValueOnce(older.promise).mockResolvedValueOnce(confirmed)
    const first = store.refreshLogin(transport)
    await store.refreshLogin(transport)
    expect(store.login).toEqual(confirmed)

    if (outcome === 'response') older.resolve(registration)
    else older.reject(new Error('Old connection failed'))
    await first

    expect(store.login).toEqual(confirmed)
    expect(store.error).toBe('Current OS detail')
  })
})
