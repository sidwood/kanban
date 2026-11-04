import { describe, expect, it } from 'vitest'

import {
  KANBAN_CLIENT_EVENTS,
  KANBAN_CLIENT_OPERATIONS,
  KANBAN_OPERATION_COMMANDS,
  KanbanClient,
  parseKanbanLiveEvent,
  type KanbanOperationName,
  type KanbanTransport,
} from './client.js'
import type { EventEnvelope } from './types.js'

describe('generated client', () => {
  it('maps every operation to its typed Tauri command', () => {
    const operations = [...KANBAN_CLIENT_OPERATIONS]
    expect(Object.keys(KANBAN_OPERATION_COMMANDS).sort()).toStrictEqual(operations.sort())
    for (const operation of operations) {
      const command = KANBAN_OPERATION_COMMANDS[operation as KanbanOperationName]
      expect(command.length).toBeGreaterThan(0)
    }
  })

  it('lists only application-layer operations', () => {
    for (const operation of KANBAN_CLIENT_OPERATIONS) {
      expect(KANBAN_OPERATION_COMMANDS).toHaveProperty(operation)
    }
    expect(KANBAN_CLIENT_OPERATIONS).toContain('comment.resolve')
    expect(KANBAN_CLIENT_OPERATIONS).toContain('issue.list_projects')
    expect(KANBAN_CLIENT_OPERATIONS).toContain('issue.retire_label')
    expect(KANBAN_CLIENT_OPERATIONS).toContain('issue.create_label')
    expect(KANBAN_CLIENT_OPERATIONS).toContain('issue.create_milestone')
    expect(KANBAN_CLIENT_OPERATIONS).toContain('issue.update_project_settings')
  })

  it('lists only application-layer live events', () => {
    expect(KANBAN_CLIENT_EVENTS).toStrictEqual([
      'initiative.created',
      'initiative.renamed',
      'initiative.archived',
      'project.registered',
      'project.updated',
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
      'ticket.created',
      'ticket.assigned',
      'ticket.state.changed',
      'ticket.edited',
      'ticket.review.configured',
      'ticket.spec.moved',
      'ticket.pinned',
      'ticket.graph.approved',
      'profile.defined',
      'profile.updated',
      'profile.retired',
      'comment.created',
      'comment.edited',
      'ruling.recorded',
      'ruling.superseded',
      'deferral.recorded',
      'deferral.superseded',
      'evidence.attached',
      'evidence.listed',
      'criterion.binding.changed',
      'workspace.registered',
      'workspace.observed',
      'workspace.retired',
      'lane.created',
      'lane.workspace.assigned',
      'lane.workspace.released',
      'lane.ticket.assigned',
      'lane.ticket.released',
      'clone.created',
      'clone.removed',
      'dispatch.requested',
      'dispatch.claimed',
      'run.acknowledged',
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
