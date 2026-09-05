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
    expect(KANBAN_OPERATION_COMMANDS).toStrictEqual({
      'health.get': 'health_get',
      'initiative.create': 'initiative_create',
      'initiative.rename': 'initiative_rename',
      'initiative.archive': 'initiative_archive',
      'initiative.list': 'initiative_list',
      'project.register': 'project_register',
      'project.archive': 'project_archive',
      'project.list': 'project_list',
      'plan.create': 'plan_create',
      'plan.spec.add': 'plan_spec_add',
      'plan.spec.remove': 'plan_spec_remove',
      'plan.spec.move': 'plan_spec_move',
      'plan.edge.add': 'plan_edge_add',
      'plan.edge.remove': 'plan_edge_remove',
      'plan.activate': 'plan_activate',
      'plan.replan': 'plan_replan',
      'plan.complete': 'plan_complete',
      'plan.cancel': 'plan_cancel',
      'plan.archive': 'plan_archive',
      'plan.list': 'plan_list',
      'plan.get': 'plan_get',
      'spec.create': 'spec_create',
      'spec.content.update': 'spec_content_update',
      'spec.version.approve': 'spec_version_approve',
      'spec.version.supersede': 'spec_version_supersede',
      'spec.plan.join': 'spec_plan_join',
      'spec.execution.move': 'spec_execution_move',
      'spec.list': 'spec_list',
      'spec.get': 'spec_get',
      'spec.version.get': 'spec_version_get',
      'spec.coverage.check': 'spec_coverage_check',
      'timeline.query': 'timeline_query',
      'comment.create': 'comment_create',
      'comment.edit': 'comment_edit',
      'comment.revisions': 'comment_revisions',
      'ruling.record': 'ruling_record',
      'ruling.supersede': 'ruling_supersede',
      'ruling.list': 'ruling_list',
      'deferral.record': 'deferral_record',
      'deferral.supersede': 'deferral_supersede',
      'deferral.list': 'deferral_list',
      'evidence.attach': 'evidence_attach',
      'evidence.list': 'evidence_list',
      'herdr.settings.get': 'herdr_settings_get',
      'herdr.settings.update': 'herdr_settings_update',
      'herdr.defaults.get': 'herdr_defaults_get',
      'herdr.defaults.update': 'herdr_defaults_update',
      'workspace.register': 'workspace_register',
      'workspace.observe': 'workspace_observe',
      'workspace.retire': 'workspace_retire',
      'workspace.list': 'workspace_list',
    })
  })

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
      'spec.coverage.check',
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
      'workspace.retire',
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
      'workspace.retired',
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
