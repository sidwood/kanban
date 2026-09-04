import { describe, expect, it } from 'vitest'

import { KANBAN_CLIENT_OPERATIONS, KanbanClient } from './client.js'

describe('generated client', () => {
  it('lists only application-layer operations', () => {
    expect(KANBAN_CLIENT_OPERATIONS).toStrictEqual(['health.get'])
  })

  it('routes queries through the transport boundary', async () => {
    const calls: Array<{ name: string; request: unknown }> = []
    const client = new KanbanClient({
      query: async (name, request) => {
        calls.push({ name, request })
        return { connected: true, service_version: '0.1.0' }
      },
      command: async () => {
        throw new Error('unexpected command')
      },
      subscribe: () => () => undefined,
    })

    await client.queryHealthGet()

    expect(calls).toStrictEqual([{ name: 'health.get', request: {} }])
  })
})
