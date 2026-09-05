import { describe, expect, it } from 'vitest'

import {
  KANBAN_CLIENT_EVENTS,
  KANBAN_CLIENT_OPERATIONS,
  KanbanClient,
  parseKanbanLiveEvent,
  type KanbanOperationName,
  type KanbanTransport,
} from './client.js'
import type { EventEnvelope } from './types.js'

describe('generated client', () => {
  it('lists only application-layer operations', () => {
    expect(KANBAN_CLIENT_OPERATIONS).toStrictEqual([
      'health.get',
      'initiative.create',
      'initiative.rename',
      'initiative.archive',
      'initiative.list',
      'project.register',
      'project.archive',
      'project.list',
      'plan.create',
      'plan.spec.add',
      'plan.spec.remove',
      'plan.spec.move',
      'plan.edge.add',
      'plan.edge.remove',
      'plan.activate',
      'plan.replan',
      'plan.complete',
      'plan.cancel',
      'plan.archive',
      'plan.list',
      'plan.get',
      'spec.create',
      'spec.content.update',
      'spec.version.approve',
      'spec.version.supersede',
      'spec.plan.join',
      'spec.execution.move',
      'spec.list',
      'spec.get',
      'spec.version.get',
      'timeline.query',
      'comment.create',
      'comment.edit',
      'comment.revisions',
      'ruling.record',
      'ruling.supersede',
      'ruling.list',
      'deferral.record',
      'deferral.supersede',
      'deferral.list',
      'evidence.attach',
      'evidence.list',
      'herdr.settings.get',
      'herdr.settings.update',
      'herdr.defaults.get',
      'herdr.defaults.update',
      'workspace.register',
      'workspace.observe',
      'workspace.list',
    ])
  })

  it('lists only application-layer live events', () => {
    expect(KANBAN_CLIENT_EVENTS).toStrictEqual([
      'initiative.created',
      'initiative.renamed',
      'initiative.archived',
      'project.registered',
      'project.archived',
      'plan.created',
      'plan.activated',
      'plan.replanned',
      'plan.completed',
      'plan.cancelled',
      'plan.archived',
      'spec.created',
      'spec.planned',
      'spec.version.approved',
      'spec.version.superseded',
      'spec.execution.moved',
      'comment.created',
      'comment.edited',
      'ruling.recorded',
      'ruling.superseded',
      'deferral.recorded',
      'deferral.superseded',
      'evidence.attached',
      'evidence.listed',
      'workspace.registered',
      'workspace.observed',
    ])
  })

  it('refuses unknown live event envelopes', () => {
    const envelope: EventEnvelope = {
      sequence: 1,
      event_type: 'counter.bumped',
      payload: { to: 1 },
    }

    expect(() => parseKanbanLiveEvent(envelope)).toThrow(
      'unknown live event type `counter.bumped`',
    )
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
