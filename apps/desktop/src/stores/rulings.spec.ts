import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ApiError } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useRulingsStore } from './rulings'

function transportRejectingWith(failure: unknown): ShellTransport {
  return {
    query: vi.fn().mockRejectedValue(failure),
    command: vi.fn(),
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport
}

describe('rulings store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('renders a structured API refusal through the error helper', async () => {
    const refusal: ApiError = {
      code: 'not_found',
      message: 'ruling 9',
    }
    const store = useRulingsStore()

    await store.load(transportRejectingWith(refusal), 1)

    expect(store.error).toBe('ruling 9')
    expect(store.rulings).toEqual([])
    expect(store.deferrals).toEqual([])
  })

  it('does not render structured core refusals as [object Object]', async () => {
    const refusal: ApiError = {
      code: 'invalid_request',
      message: 'a ruling summary cannot be blank',
    }
    const store = useRulingsStore()

    await store.load(transportRejectingWith(refusal), 1)

    expect(store.error).not.toBe('[object Object]')
    expect(store.error).toBe('a ruling summary cannot be blank')
  })
})
