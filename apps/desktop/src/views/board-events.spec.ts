import { describe, expect, it } from 'vitest'
import type { KanbanLiveEvent } from '@kanban/contracts'
import { issuesNamedBy } from './board-events'

function event(event_type: string, payload: Record<string, unknown>): KanbanLiveEvent {
  return { sequence: 1, event_type, payload } as unknown as KanbanLiveEvent
}

describe('the issues a live event names', () => {
  it.each([
    ['ticket.created', { id: 7 }, [7]],
    ['ticket.assigned', { id: 7 }, [7]],
    ['ticket.state.changed', { id: 7 }, [7]],
    ['ticket.edited', { id: 7 }, [7]],
    ['ticket.spec.moved', { id: 7 }, [7]],
    ['ticket.pinned', { id: 7 }, [7]],
    ['ticket.review.configured', { ticket_id: 7, stages: [], version: 2 }, [7]],
    ['criterion.binding.changed', { ticket_id: 7 }, [7]],
    ['dispatch.requested', { ticket_id: 7 }, [7]],
    ['dispatch.claimed', { ticket_id: 7 }, [7]],
    ['run.acknowledged', { ticket_id: 7 }, [7]],
    ['ticket.graph.approved', { tickets: [7, 8] }, [7, 8]],
    ['lane.created', { id: 3, project_id: 1, ticket_id: 7, version: 1 }, [7]],
    ['lane.workspace.assigned', { id: 3, project_id: 1, ticket_id: 7, workspace_id: 2, version: 2 }, [7]],
    ['lane.workspace.released', { id: 3, project_id: 1, ticket_id: 7, workspace_id: null, version: 3 }, [7]],
    ['lane.ticket.assigned', { id: 3, project_id: 1, ticket_id: 7, version: 2 }, [7]],
    ['lane.ticket.released', { id: 3, project_id: 1, ticket_id: 7, version: 3 }, [7]],
  ])('%s names the issue its payload carries', (type, payload, named) => {
    expect(issuesNamedBy(event(type, payload))).toEqual(named)
  })

  it.each([
    ['lane.created', { id: 3, project_id: 1, version: 1 }],
    ['lane.ticket.released', { id: 3, project_id: 1, ticket_id: null, version: 3 }],
    ['project.updated', { id: 1, code: 'CORE' }],
    ['spec.version.approved', { id: 4, project_id: 1 }],
  ])('%s without a ticket names no issue, so the board reads its whole list', (type, payload) => {
    expect(issuesNamedBy(event(type, payload))).toBeNull()
  })
})
