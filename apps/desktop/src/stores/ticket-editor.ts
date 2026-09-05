// The Ticket editor state, driven entirely through the generated
// client: the Tickets of one Project and the per-kind creation the
// editor drives — an Implementation attached to exactly one Spec,
// carrying its slice description and story-linked criteria; a Bug or
// Task carrying a title and an optional attachment. Every kind takes
// the closed priority vocabulary, creation lands in draft, and a
// refusal is reported, never swallowed (KAN-S4-US1, KAN-S4-US2).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  TicketCreateRequest,
  TicketKind,
  TicketPriority,
  TicketRecord,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

// The closed Ticket kind and priority vocabularies the editor offers.
export const TICKET_KINDS: TicketKind[] = ['implementation', 'bug', 'task']
export const TICKET_PRIORITIES: TicketPriority[] = ['urgent', 'high', 'normal', 'low']

// One criterion row the editor holds before its story links parse.
export interface TicketCriterionDraft {
  outcome: string
  stories: string
}

// The per-kind creation draft the form edits. Fields a kind does not
// carry are ignored when its request is built.
export interface TicketDraft {
  kind: TicketKind
  priority: TicketPriority
  title: string
  specId: number | null
  slice: string
  criteria: TicketCriterionDraft[]
}

// A fresh draft: a normal-priority Bug, the lightest capture.
export function blankTicketDraft(): TicketDraft {
  return {
    kind: 'bug',
    priority: 'normal',
    title: '',
    specId: null,
    slice: '',
    criteria: [{ outcome: '', stories: '' }],
  }
}

// Story links arrive as one comma- or space-separated field; the
// request carries them as the identities the core parses.
export function parseStoryLinks(stories: string): string[] {
  return stories
    .split(/[\s,]+/)
    .map((named) => named.trim())
    .filter((named) => named.length > 0)
}

// Build the typed creation request for one draft: each kind sends
// exactly its own fields, and a fresh aggregate is expected at
// version 0.
export function ticketCreateRequestOf(
  projectId: number,
  draft: TicketDraft,
  idempotencyKey: string,
): TicketCreateRequest {
  const request: TicketCreateRequest = {
    mutation: { optimistic_version: 0, idempotency_key: idempotencyKey },
    project_id: projectId,
    kind: draft.kind,
    priority: draft.priority,
  }
  if (draft.kind === 'implementation') {
    request.spec_id = draft.specId ?? undefined
    request.slice = draft.slice
    request.criteria = draft.criteria.map((criterion) => ({
      outcome: criterion.outcome,
      stories: parseStoryLinks(criterion.stories),
    }))
  } else {
    request.title = draft.title
    if (draft.specId !== null) {
      request.spec_id = draft.specId
    }
  }
  return request
}

export const useTicketEditorStore = defineStore('ticket-editor', {
  state: () => ({
    tickets: [] as TicketRecord[],
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    // Load every Ticket of one Project, terminal lifecycle states
    // included.
    async refresh(transport: ShellTransport, projectId: number): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryTicketList({ project_id: projectId })
        this.tickets = response.tickets
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Create one Ticket under its kind's schema. Reports whether the
    // Ticket landed; a refusal is reported and the list stands.
    async create(
      transport: ShellTransport,
      projectId: number,
      draft: TicketDraft,
    ): Promise<boolean> {
      const request = ticketCreateRequestOf(projectId, draft, crypto.randomUUID())
      let landed: TicketRecord
      try {
        landed = await new KanbanClient(transport).commandTicketCreate(request)
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return false
      }
      await this.refresh(transport, landed.project_id)
      return true
    },
  },
})
