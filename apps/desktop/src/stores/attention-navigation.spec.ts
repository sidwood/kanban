import { describe, expect, it } from 'vitest'
import type { AttentionItemRecord, AttentionSubjectKind } from '@kanban/contracts'
import { destinationForAttentionItem } from './attention-navigation'

function item(
  subject_kind: AttentionSubjectKind,
  subject_id: string,
  detail: AttentionItemRecord['detail'] = {},
): AttentionItemRecord {
  return {
    id: `${subject_kind}:${subject_id}`,
    project_id: 3,
    kind: 'blocker',
    subject_kind,
    subject_id,
    summary: 'Something needs the operator.',
    detail,
    active: true,
    acknowledged_at: null,
    acknowledged_by: null,
    first_seen_at: '2026-09-14T06:00:00Z',
    last_seen_at: '2026-09-14T06:00:00Z',
    version: 1,
  }
}

describe('attention destinations', () => {
  it('opens the exact object each typed source names', () => {
    expect(destinationForAttentionItem(item('ticket', '12')).route).toBe(
      '/projects/3/board?ticket=12',
    )
    expect(destinationForAttentionItem(item('project', '3')).route).toBe('/projects/3/board')
    expect(destinationForAttentionItem(item('run', '8')).route).toBe(
      '/projects/3/workspaces?run=8',
    )
    expect(destinationForAttentionItem(item('spec', '5')).route).toBe(
      '/planning?project=3&spec=5',
    )
    expect(destinationForAttentionItem(item('deferral', '2')).route).toBe(
      '/activity?project=3&deferral=2',
    )
  })

  it('opens the Ticket a Schedule source names, and nothing when it names none', () => {
    expect(
      destinationForAttentionItem(item('schedule', '4', { source: 'schedule_window', ticket_id: 9 }))
        .route,
    ).toBe('/projects/3/board?ticket=9')
    const bare = destinationForAttentionItem(item('schedule', '4', { source: 'schedule_window' }))
    expect(bare.route).toBeNull()
    expect(bare.label).toContain('no Ticket')
  })

  it('opens an observed role’s own activity in its own Project', () => {
    expect(destinationForAttentionItem(item('role', 'implementer')).route).toBe(
      '/activity?project=3&role=implementer',
    )
  })

  it('offers no destination for a source the application holds no surface for', () => {
    const destination = destinationForAttentionItem(item('graph', '17'))

    expect(destination.route, 'a graph must not be sent to a list').toBeNull()
    expect(destination.label).toContain('no surface of its own')
  })

  it('names a typed source for every subject kind in the closed vocabulary', () => {
    const kinds: AttentionSubjectKind[] = [
      'ticket',
      'run',
      'spec',
      'project',
      'deferral',
      'graph',
      'schedule',
      'role',
    ]
    for (const kind of kinds) {
      expect(destinationForAttentionItem(item(kind, '1')).label.length).toBeGreaterThan(0)
    }
  })
})
