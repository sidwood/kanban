// The Ticket editor state, driven entirely through the generated
// client: the Tickets of one Project and the per-kind creation the
// editor drives — an Implementation attached to exactly one Spec,
// carrying its slice description and story-linked criteria; a Bug
// quick-captured with title, actual behaviour, and reporter evidence
// and nothing else required; a Task bounded by a subtype, a
// human-or-agent mode, completion criteria, and optional schedule or
// due-date timing stored for KAN-S11. Every kind takes the closed
// priority vocabulary, creation lands in draft, and a refusal is
// reported, never swallowed (KAN-S4-US1 through KAN-S4-US4).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  TaskMode,
  TaskSubtype,
  TicketCreateRequest,
  TicketKind,
  TicketPriority,
  TicketRecord,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

// The closed Ticket kind, priority, and Task subtype and mode
// vocabularies the editor offers.
export const TICKET_KINDS: TicketKind[] = ['implementation', 'bug', 'task']
export const TICKET_PRIORITIES: TicketPriority[] = ['urgent', 'high', 'normal', 'low']
export const TASK_SUBTYPES: TaskSubtype[] = [
  'operational',
  'investigative',
  'administrative',
  'research',
  'prototype',
  'migration',
  'manual',
]
export const TASK_MODES: TaskMode[] = ['human', 'agent']

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
  actualBehaviour: string
  reporterEvidence: string
  specId: number | null
  slice: string
  criteria: TicketCriterionDraft[]
  subtype: TaskSubtype
  mode: TaskMode
  completion: string[]
  scheduledFor: string
  due: string
}

// A fresh draft: a normal-priority Bug, the lightest capture. The
// Task fields default the first subtype and the human mode, ready
// for the switch to a bounded kind.
export function blankTicketDraft(): TicketDraft {
  return {
    kind: 'bug',
    priority: 'normal',
    title: '',
    actualBehaviour: '',
    reporterEvidence: '',
    specId: null,
    slice: '',
    criteria: [{ outcome: '', stories: '' }],
    subtype: 'operational',
    mode: 'human',
    completion: [''],
    scheduledFor: '',
    due: '',
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
    if (draft.kind === 'bug') {
      // Quick capture needs the two capture facts beside the title
      // and nothing else (DR-TK-08).
      request.actual_behaviour = draft.actualBehaviour
      request.reporter_evidence = draft.reporterEvidence
    }
    if (draft.kind === 'task') {
      // A Task carries its bounded fields, never story-linked
      // criteria: completion states outcomes alone.
      request.subtype = draft.subtype
      request.mode = draft.mode
      request.completion = [...draft.completion]
      const scheduledFor = draft.scheduledFor.trim()
      if (scheduledFor !== '') {
        request.scheduled_for = scheduledFor
      }
      const due = draft.due.trim()
      if (due !== '') {
        request.due = due
      }
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
