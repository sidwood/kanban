// The review operations a surface spends, answered the way the core
// answers them. Nothing here is production code; it is the production
// rules restated, so a surface test that passes here could not pass
// against a backend the core would refuse.
//
// Rule for rule, this mirrors:
//   crates/kanban-domain/src/review_execution.rs — stage and sequence resolution
//   crates/kanban-domain/src/finding.rs          — finding detail and verdict admission
//   crates/kanban-app/src/review_execution.rs    — active slot, tip, counts_for_resolution
//   crates/kanban-storage/src/review_execution.rs — verdict commit, gate outcome, history
import type {
  ApiError,
  ReviewAttemptRecord,
  ReviewExecutionRecord,
  ReviewExecutionStatus,
  ReviewFindingRecord,
  ReviewHistoryResponse,
  ReviewHumanSubmitRequest,
  ReviewLatestResponse,
  ReviewSlotRecord,
  ReviewStageRecord,
  ReviewStageStatus,
  TimelineEventRecord,
} from '@kanban/contracts'

function refuse(code: ApiError['code'], message: string): never {
  throw { code, message } satisfies ApiError
}

/** `resolve_review_stage`: optional slots never hold a stage open, a
 * completed veto still counts, and every vote must bind the tip. */
function stageStatus(tip: string, slots: readonly ReviewSlotRecord[]): ReviewStageStatus {
  const votes = slots.map((slot) => ({
    required: slot.requirement === 'required',
    verdict: slot.verdict?.counts_for_resolution === true ? slot.verdict : null,
  }))
  if (votes.some((vote) => vote.required && vote.verdict === null)) return 'waiting'
  if (votes.some((vote) => vote.verdict && (!vote.verdict.approve || vote.verdict.tip !== tip))) {
    return 'rejected'
  }
  return 'approved'
}

/** `resolve_review_sequence`: the first unapproved stage decides. */
function executionStatus(stages: readonly ReviewStageRecord[]): ReviewExecutionStatus {
  for (const stage of stages) {
    if (stage.status === 'waiting') return 'in_progress'
    if (stage.status === 'rejected') return 'rejected'
  }
  return 'approved'
}

/** `advance`: every stage status is recomputed, then the execution's. */
function advance(review: ReviewExecutionRecord): ReviewExecutionRecord {
  const stages = review.stages.map((stage) => ({
    ...stage,
    status: stageStatus(review.tip, stage.slots),
  }))
  return { ...review, stages, status: executionStatus(stages) }
}

/** `counts_for_resolution`: only a slot of the first non-approved
 * stage of an in-progress review, and only while that stage waits. */
function countsForResolution(review: ReviewExecutionRecord, slotId: number): boolean {
  if (review.status !== 'in_progress') return false
  const active = review.stages.find((stage) => stage.status !== 'approved')
  if (!active || active.status !== 'waiting') return false
  return active.slots.some((slot) => slot.id === slotId)
}

/** `active_review_slot`: the slot must exist, carry no verdict yet, and
 * sit either in the currently active stage or in a completed optional
 * position that stays auditable. */
function activeSlot(review: ReviewExecutionRecord, slotId: number): ReviewSlotRecord {
  const stage = review.stages.find((entry) => entry.slots.some((slot) => slot.id === slotId))
  const slot = stage?.slots.find((entry) => entry.id === slotId)
  if (!stage || !slot) refuse('not_found', 'review slot')
  if (slot.verdict) refuse('invalid_request', 'review slot already submitted')
  const activeIndex = review.stages.find((entry) => entry.status !== 'approved')?.index ?? null
  const currentlyActive =
    review.status === 'in_progress' && stage.status === 'waiting' && activeIndex === stage.index
  const completedOptional =
    slot.requirement === 'optional' &&
    (activeIndex === null || stage.index <= activeIndex) &&
    (stage.status !== 'waiting' || review.status !== 'in_progress')
  if (!currentlyActive && !completedOptional) {
    refuse('invalid_request', 'review slot is not in an active or completed optional stage')
  }
  return slot
}

/** `validate_finding_details`: every narrative field must carry text. */
function validateFindings(findings: readonly ReviewFindingRecord[]): void {
  for (const finding of findings) {
    for (const field of ['summary', 'evidence', 'location', 'proposed_resolution'] as const) {
      if (finding[field].trim() === '') refuse('invalid_request', `a finding ${field} cannot be blank`)
    }
  }
}

/** `finding_blocks`: an in-scope P0 to P2 finding blocks approval. */
function blocks(finding: ReviewFindingRecord): boolean {
  return finding.in_scope && finding.severity !== 'p3'
}

/** `validate_finding_verdict`: a resolving verdict may neither approve
 * over a blocker nor reject without one. */
function validateVerdict(
  approve: boolean,
  findings: readonly ReviewFindingRecord[],
  counts: boolean,
): void {
  validateFindings(findings)
  if (!counts) return
  const blocking = findings.some(blocks)
  if (approve && blocking) {
    refuse('invalid_request', 'an approval cannot stand over a blocking in-scope P0 to P2 finding')
  }
  if (!approve && !blocking) {
    refuse('invalid_request', 'a blocking rejection requires an in-scope P0 to P2 finding')
  }
}

/** The core the review fixture serves from: the executions it holds,
 * the attempt history terminal executions record, and the audit rows
 * a verdict appends. */
export class ReviewCore {
  private readonly reviews: ReviewExecutionRecord[]
  private readonly attempts = new Map<number, ReviewAttemptRecord[]>()
  private readonly revalidation = new Set<number>()
  private readonly events: TimelineEventRecord[] = []
  private nextEventId = 9000

  constructor(reviews: readonly ReviewExecutionRecord[] = []) {
    this.reviews = reviews.map((review) => ({ ...review }))
    // A terminal execution has already written its attempt and the
    // revalidation its gate needs; history is never authored by hand.
    for (const review of this.reviews) this.recordGateOutcome(review)
  }

  /** `review.latest`: the Ticket's most recent execution, active or
   * terminal — the only Ticket-scoped way to reach an open review. */
  latest(ticketId: number): ReviewLatestResponse {
    const held = this.reviews.filter((review) => review.ticket_id === ticketId)
    return { ticket_id: ticketId, review: held[held.length - 1] ?? null }
  }

  /** `review.get`: by review id alone. */
  get(reviewId: number): ReviewExecutionRecord {
    const found = this.reviews.find((review) => review.id === reviewId)
    if (!found) refuse('not_found', `review ${reviewId}`)
    return found
  }

  /** `review.history`: prior attempts only. An in-progress execution
   * has recorded none, so it is unreachable from here. */
  history(ticketId: number): ReviewHistoryResponse {
    return {
      needs_revalidation: this.revalidation.has(ticketId),
      attempts: [...(this.attempts.get(ticketId) ?? [])],
    }
  }

  /** The Project-scoped audit the timeline reads. */
  timeline(projectId: number): TimelineEventRecord[] {
    return this.events.filter(
      (event) => event.scope !== 'global' && event.scope.project === projectId,
    )
  }

  /** `review.human.submit`: the whole admission path, in the core's
   * own order, followed by the projection it commits. */
  humanSubmit(request: ReviewHumanSubmitRequest): ReviewExecutionRecord {
    const findings = request.findings ?? []
    validateFindings(findings)
    if (request.summary.trim() === '') refuse('invalid_request', 'a review summary is required')
    const review = this.get(request.review_id)
    if (request.mutation.optimistic_version !== review.version) {
      refuse('stale_version', 'the review moved on')
    }
    const slot = activeSlot(review, request.slot_id)
    if (slot.occupant.kind !== 'human') {
      refuse('invalid_request', 'agent slots require their scoped run submission')
    }
    if (request.tip !== review.tip) {
      refuse('invalid_request', 'review verdict must bind the exact implementation tip')
    }
    const counts = countsForResolution(review, request.slot_id)
    validateVerdict(request.approve, findings, counts)
    const recorded: ReviewExecutionRecord = {
      ...review,
      version: review.version + 1,
      stages: review.stages.map((stage) => ({
        ...stage,
        slots: stage.slots.map((entry) =>
          entry.id === request.slot_id
            ? {
                ...entry,
                verdict: {
                  counts_for_resolution: counts,
                  submission_id: null,
                  tip: request.tip,
                  approve: request.approve,
                  summary: request.summary,
                  findings: [...findings],
                },
              }
            : entry,
        ),
      })),
    }
    const committed = advance(recorded)
    this.reviews.splice(this.reviews.indexOf(review), 1, committed)
    this.append(committed, 'review_slot_submitted')
    if (review.status !== 'rejected' && committed.status === 'rejected') {
      this.append(committed, 'review_stage_bounced')
    }
    this.recordGateOutcome(committed)
    return committed
  }

  /** `record_gate_outcome`: a terminal execution, and only a terminal
   * one, writes the attempt and the revalidation the gate now needs. */
  private recordGateOutcome(review: ReviewExecutionRecord): void {
    if (review.status === 'in_progress') return
    const held = this.attempts.get(review.ticket_id) ?? []
    const expired = review.status === 'expired'
    held.push({
      attempt: held.length + 1,
      review_id: review.id,
      outcome: review.status === 'approved' ? 'approved' : expired ? 'expired' : 'failed',
      // `record_expired_gate` records the expiry alone; only a
      // resolved gate carries the verdicts that resolved it.
      verdicts: expired
        ? []
        : review.stages
            .flatMap((stage) => stage.slots)
            .flatMap((slot) =>
              slot.verdict ? [slot.verdict.approve ? 'approved' : 'rejected'] : [],
            ),
      invalidations: expired ? ['gate expired'] : [],
    })
    this.attempts.set(review.ticket_id, held)
    if (review.status === 'approved') this.revalidation.delete(review.ticket_id)
    else this.revalidation.add(review.ticket_id)
  }

  /** `review_transition`: one audit row per accepted projection. */
  private append(review: ReviewExecutionRecord, action: string): void {
    this.nextEventId += 1
    this.events.push({
      id: this.nextEventId,
      kind: 'review',
      scope: { project: review.project_id },
      entity: { kind: 'ticket', id: String(review.ticket_id) },
      recorded_at: '2026-09-15T10:00:00Z',
      detail: {
        action,
        review_id: review.id,
        ticket_id: review.ticket_id,
        tip: review.tip,
        status: review.status,
        version: review.version,
      },
    })
  }
}
