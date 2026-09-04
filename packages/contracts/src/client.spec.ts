import { describe, expect, it } from 'vitest'

import {
  KANBAN_CLIENT_OPERATIONS,
  KanbanClient,
  type KanbanOperationName,
  type KanbanTransport,
} from './client.js'

describe('generated client', () => {
  it('lists only application-layer operations', () => {
    expect(KANBAN_CLIENT_OPERATIONS).toStrictEqual([
      'health.get',
      'initiative.create',
      'initiative.rename',
      'initiative.archive',
      'initiative.list',
      'timeline.query',
    ])
  })

  it('routes queries through the transport boundary', async () => {
    const calls: Array<{ name: string; request: unknown }> = []
    const transport: KanbanTransport = {
      query: async <Request, Response>(
        name: KanbanOperationName,
        request: Request,
      ): Promise<Response> => {
        calls.push({ name, request })
        return { connected: true, service_version: '0.1.0' } as Response
      },
      command: async () => {
        throw new Error('unexpected command')
      },
      subscribe: () => () => undefined,
    }
    const client = new KanbanClient(transport)

    await client.queryHealthGet()

    expect(calls).toStrictEqual([{ name: 'health.get', request: {} }])
  })
})
