// The Bug editor state, driven entirely through the generated client:
// the qualification that completes a Bug before it may leave draft
// (DR-TK-09) and the vendor-neutral External References, Occurrence
// Snapshots, and Evidence Items it carries while it waits (DR-TK-10).
// Severity takes the closed critical, high, medium, low vocabulary
// and only qualification sets it (DR-LC-13); a refusal is reported,
// never swallowed (KAN-S4-US3).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  TicketBugFactsRequest,
  TicketBugQualifyRequest,
  TicketRecord,
  TicketSeverity,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { parseStoryLinks, type TicketCriterionDraft } from './ticket-editor'

// The closed Bug severity vocabulary the qualification form offers
// (DR-LC-13).
export const BUG_SEVERITIES: TicketSeverity[] = ['critical', 'high', 'medium', 'low']

// The qualification draft the form edits: the ten facts a Bug needs
// before it may leave draft, sent whole in one act (DR-TK-09).
export interface BugQualificationDraft {
  expectedBehaviour: string
  reproduction: string
  environment: string
  severity: TicketSeverity
  frequency: string
  affectedScope: string
  risk: string
  criteria: TicketCriterionDraft[]
  verificationSteps: string[]
}

// A fresh qualification draft: no severity chosen, one empty
// criterion row, one empty step.
export function blankBugQualificationDraft(): BugQualificationDraft {
  return {
    expectedBehaviour: '',
    reproduction: '',
    environment: '',
    severity: 'medium',
    frequency: '',
    affectedScope: '',
    risk: '',
    criteria: [{ outcome: '', stories: '' }],
    verificationSteps: [''],
  }
}

// One External Reference row before its URI is validated core-side.
export interface ExternalReferenceDraft {
  uri: string
  label: string
}

// One Occurrence Snapshot row before its moment is validated
// core-side.
export interface OccurrenceSnapshotDraft {
  observedAt: string
  observation: string
}

// The facts draft the form edits: the three vendor-neutral
// collections, replaced whole in one act (DR-TK-10).
export interface BugFactsDraft {
  externalReferences: ExternalReferenceDraft[]
  occurrenceSnapshots: OccurrenceSnapshotDraft[]
  evidenceIds: string
}

// A fresh facts draft: one empty row per collection, no evidence.
export function blankBugFactsDraft(): BugFactsDraft {
  return {
    externalReferences: [{ uri: '', label: '' }],
    occurrenceSnapshots: [{ observedAt: '', observation: '' }],
    evidenceIds: '',
  }
}

// Evidence identities arrive as one comma- or space-separated field;
// the request carries them as the numbers the core validates.
export function parseEvidenceIds(named: string): number[] {
  return named
    .split(/[\s,]+/)
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0)
    .map((entry) => Number(entry))
    .filter((entry) => Number.isInteger(entry) && entry > 0)
}

// Build the typed qualification request for one Bug: the whole
// qualification, story links parsed, guarded by the version the
// record was read at.
export function bugQualifyRequestOf(
  ticketId: number,
  draft: BugQualificationDraft,
  optimisticVersion: number,
  idempotencyKey: string,
): TicketBugQualifyRequest {
  return {
    mutation: { optimistic_version: optimisticVersion, idempotency_key: idempotencyKey },
    ticket_id: ticketId,
    qualification: {
      expected_behaviour: draft.expectedBehaviour,
      reproduction: draft.reproduction,
      environment: draft.environment,
      severity: draft.severity,
      frequency: draft.frequency,
      affected_scope: draft.affectedScope,
      risk: draft.risk,
      criteria: draft.criteria.map((criterion) => ({
        outcome: criterion.outcome,
        stories: parseStoryLinks(criterion.stories),
      })),
      verification_steps: draft.verificationSteps
        .map((step) => step.trim())
        .filter((step) => step.length > 0)
        .map((command) => ({ command })),
    },
  }
}

// Build the typed facts request for one Bug: the three collections
// replaced whole, empty rows dropped, evidence identities parsed.
export function bugFactsRequestOf(
  ticketId: number,
  draft: BugFactsDraft,
  optimisticVersion: number,
  idempotencyKey: string,
): TicketBugFactsRequest {
  return {
    mutation: { optimistic_version: optimisticVersion, idempotency_key: idempotencyKey },
    ticket_id: ticketId,
    external_references: draft.externalReferences
      .map((reference) => ({
        uri: reference.uri.trim(),
        label: reference.label.trim(),
      }))
      .filter((reference) => reference.uri.length > 0)
      .map((reference) => ({
        uri: reference.uri,
        ...(reference.label.length > 0 ? { label: reference.label } : {}),
      })),
    occurrence_snapshots: draft.occurrenceSnapshots
      .map((snapshot) => ({
        observed_at: snapshot.observedAt.trim(),
        observation: snapshot.observation.trim(),
      }))
      .filter((snapshot) => snapshot.observed_at.length > 0),
    evidence_ids: parseEvidenceIds(draft.evidenceIds),
  }
}

export const useBugEditorStore = defineStore('bug-editor', {
  state: () => ({
    error: null as string | null,
  }),
  actions: {
    // Qualify one Bug with its whole qualification. Returns the
    // landed record, or null with the refusal reported.
    async qualify(
      transport: ShellTransport,
      ticketId: number,
      optimisticVersion: number,
      draft: BugQualificationDraft,
    ): Promise<TicketRecord | null> {
      const request = bugQualifyRequestOf(ticketId, draft, optimisticVersion, crypto.randomUUID())
      try {
        const landed = await new KanbanClient(transport).commandTicketBugQualify(request)
        this.error = null
        return landed
      } catch (failure) {
        this.error = asApiError(failure).message
        return null
      }
    },
    // Record the vendor-neutral collections one Bug carries. Returns
    // the landed record, or null with the refusal reported.
    async recordFacts(
      transport: ShellTransport,
      ticketId: number,
      optimisticVersion: number,
      draft: BugFactsDraft,
    ): Promise<TicketRecord | null> {
      const request = bugFactsRequestOf(ticketId, draft, optimisticVersion, crypto.randomUUID())
      try {
        const landed = await new KanbanClient(transport).commandTicketBugFacts(request)
        this.error = null
        return landed
      } catch (failure) {
        this.error = asApiError(failure).message
        return null
      }
    },
  },
})
