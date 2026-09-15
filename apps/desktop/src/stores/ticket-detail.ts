// Everything the drawer says about one Ticket beyond its own record,
// read from the core and from nowhere else: the criterion evidence
// bindings its revisions come from, the registered dependencies and
// external blockers holding it back, the runs that executed it with
// the Lane and Workspace they held, the review execution its stages
// and verdicts come from, the findings recorded against it, and the
// evidence attached to it (KAN-S10-US5, KAN-S10-US6). A record the
// core does not hold is absent here; nothing is filled in. A source
// the core could not answer for is unreadable, which is not the same
// claim as absent, and is never reported as one.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  CriterionBindingRecord,
  EvidenceRecord,
  FindingRecord,
  LaneRecord,
  ProjectRecord,
  ReviewAttemptRecord,
  ReviewExecutionRecord,
  ReviewFindingRecord,
  ReviewSlotRecord,
  ReviewStageRecord,
  RunRecord,
  TicketBlockerRecord,
  TicketDependencyRecord,
  TicketRecord,
  WorkspaceRecord,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

/** The sections of the drawer, each standing on its own authoritative
 * reads: one section's failure says nothing about another's records. */
export type DetailSection =
  | 'criteria'
  | 'dependencies'
  | 'execution'
  | 'placement'
  | 'review'
  | 'findings'
  | 'evidence'

/** What one source refused with, when it refused. */
function refusal(outcome: PromiseSettledResult<unknown>): unknown {
  return outcome.status === 'rejected' ? outcome.reason : null
}

/** The run that is executing the Ticket now, when one is. */
function executing(runs: readonly RunRecord[]): RunRecord | null {
  const found = runs.filter((run) => run.status === 'executing')
  return found[found.length - 1] ?? null
}

export const useTicketDetailStore = defineStore('ticket-detail', {
  state: () => ({
    /** The Ticket this detail speaks for; null when it speaks for
     * none. */
    ticketId: null as number | null,
    bindings: [] as CriterionBindingRecord[],
    dependencies: [] as TicketDependencyRecord[],
    blockers: [] as TicketBlockerRecord[],
    projects: [] as ProjectRecord[],
    runs: [] as RunRecord[],
    lanes: [] as LaneRecord[],
    workspaces: [] as WorkspaceRecord[],
    review: null as ReviewExecutionRecord | null,
    reviewAttempts: [] as ReviewAttemptRecord[],
    needsRevalidation: false,
    findings: [] as FindingRecord[],
    evidence: [] as EvidenceRecord[],
    loading: false,
    error: null as string | null,
    /** The sections whose authoritative reads did not answer, so the
     * drawer states no records rather than the absence of records. */
    unreadable: [] as DetailSection[],
    // The reads issued so far, so only the Ticket on display writes
    // state: an answer for a Ticket the drawer has left renders
    // nowhere.
    issued: 0,
  }),
  getters: {
    /** Whether one section's authoritative reads answered at all. */
    readable: (state) => (section: DetailSection): boolean =>
      !state.unreadable.includes(section),
    /** The revision bound to one criterion, when the core bound one. */
    bindingFor: (state) => (index: number): CriterionBindingRecord | null =>
      state.bindings.find((binding) => binding.criterion_index === index) ?? null,
    /** The code the core registered one Project under, so a Ticket in
     * another Project is named in full rather than by its
     * Project-scoped number alone. */
    projectCode: (state) => (projectId: number): string | null =>
      state.projects.find((entry) => entry.id === projectId)?.code ?? null,
    /** The run executing the Ticket now. */
    currentRun: (state): RunRecord | null => executing(state.runs),
    /** Every run the Ticket has minted, oldest first. */
    attempts: (state): readonly RunRecord[] =>
      [...state.runs].sort((left, right) => left.created_at - right.created_at),
    /** The Lane holding the Ticket, as the core assigned it. */
    lane: (state): LaneRecord | null =>
      state.lanes.find((entry) => entry.ticket_id === state.ticketId) ?? null,
    /** The Workspace that Lane holds. */
    workspace(state): WorkspaceRecord | null {
      const held = this.lane
      if (held?.workspace_id == null) return null
      return state.workspaces.find((entry) => entry.id === held.workspace_id) ?? null
    },
    /** The stage the core is resolving now: the first stage it has not
     * approved, and only while an in-progress review still waits on
     * it. A later stage is not open to a verdict and the core refuses
     * one (crates/kanban-app/src/review_execution.rs
     * `active_review_slot`). */
    activeStage: (state): ReviewStageRecord | null => {
      const review = state.review
      if (review === null || review.status !== 'in_progress') return null
      const stage = review.stages.find((entry) => entry.status !== 'approved')
      return stage?.status === 'waiting' ? stage : null
    },
    /** The human slots the core is waiting on now, in the order the
     * configuration put them. */
    waitingHumanSlots(): readonly ReviewSlotRecord[] {
      return (this.activeStage?.slots ?? []).filter(
        (slot: ReviewSlotRecord) => slot.occupant.kind === 'human' && slot.verdict == null,
      )
    },
  },
  actions: {
    /** Forget the Ticket the drawer has left, and supersede every
     * read still on the wire for it. */
    clear(): void {
      this.issued += 1
      this.ticketId = null
      this.bindings = []
      this.dependencies = []
      this.blockers = []
      this.projects = []
      this.runs = []
      this.lanes = []
      this.workspaces = []
      this.review = null
      this.reviewAttempts = []
      this.needsRevalidation = false
      this.findings = []
      this.evidence = []
      this.loading = false
      this.error = null
      this.unreadable = []
    },
    /** Read everything the drawer shows for one Ticket. Each source is
     * settled on its own: one refusal costs its own section and leaves
     * every other section's authoritative records standing. */
    async load(transport: ShellTransport, ticket: TicketRecord): Promise<void> {
      this.clear()
      this.ticketId = ticket.id
      const attempt = this.issued
      this.loading = true
      const client = new KanbanClient(transport)
      const project = { project_id: ticket.project_id }
      const [bindings, dependencies, projects, runs, lanes, workspaces, findings, evidence] =
        await Promise.allSettled([
          client.queryCriterionBindings({ ticket_id: ticket.id }),
          client.queryTicketDependencies({ ticket_id: ticket.id }),
          client.queryProjectList({}),
          client.queryRunList(project),
          client.queryLaneList(project),
          client.queryWorkspaceList(project),
          client.queryFindingList(project),
          client.queryEvidenceList({
            ...project,
            entity_kind: 'ticket',
            entity_id: String(ticket.id),
          }),
        ])
      if (attempt !== this.issued) return
      this.error = null
      if (bindings.status === 'fulfilled') {
        this.bindings = (bindings.value.bindings ?? []).filter(
          (binding) => binding.ticket_id === ticket.id,
        )
      } else this.unreadableSection('criteria', bindings.reason)
      // A dependency is named by its Project's code, so the registry
      // read is part of the dependency section's own authority.
      if (projects.status === 'fulfilled') this.projects = projects.value.projects ?? []
      if (dependencies.status === 'fulfilled' && projects.status === 'fulfilled') {
        this.dependencies = dependencies.value.dependencies ?? []
        this.blockers = dependencies.value.blockers ?? []
      } else {
        this.unreadableSection('dependencies', refusal(dependencies) ?? refusal(projects))
      }
      if (runs.status === 'fulfilled') {
        this.runs = (runs.value.runs ?? []).filter((run) => run.ticket_id === ticket.id)
      } else this.unreadableSection('execution', runs.reason)
      // The Lane and the Workspace it holds are records of their own,
      // so losing them costs the placement lines and nothing else.
      if (lanes.status === 'fulfilled' && workspaces.status === 'fulfilled') {
        this.lanes = lanes.value.lanes ?? []
        this.workspaces = workspaces.value.workspaces ?? []
      } else {
        this.unreadableSection('placement', refusal(lanes) ?? refusal(workspaces))
      }
      if (findings.status === 'fulfilled') {
        this.findings = (findings.value.findings ?? []).filter(
          (record) => record.ticket_id === ticket.id,
        )
      } else this.unreadableSection('findings', findings.reason)
      if (evidence.status === 'fulfilled') this.evidence = evidence.value.evidence ?? []
      else this.unreadableSection('evidence', evidence.reason)
      await this.loadReview(client, ticket.id, attempt)
      if (attempt === this.issued) this.loading = false
    },
    /** The review the core has open on this Ticket now, with the prior
     * attempts and the revalidation the gate holds. History names
     * terminal attempts alone, so the open review is reachable only
     * through the Ticket itself (`review.latest`). */
    async loadReview(client: KanbanClient, ticketId: number, attempt: number): Promise<void> {
      const [latest, history] = await Promise.allSettled([
        client.queryReviewLatest({ ticket_id: ticketId }),
        client.queryReviewHistory({ ticket_id: ticketId }),
      ])
      if (attempt !== this.issued) return
      if (latest.status === 'fulfilled' && history.status === 'fulfilled') {
        this.review = latest.value.review ?? null
        this.reviewAttempts = history.value.attempts ?? []
        this.needsRevalidation = history.value.needs_revalidation ?? false
        this.readableSection('review')
        return
      }
      this.unreadableSection('review', refusal(latest) ?? refusal(history))
    },
    /** Record that one section's authority did not answer, keeping the
     * first refusal as the drawer's reported failure. */
    unreadableSection(section: DetailSection, failure: unknown): void {
      if (!this.unreadable.includes(section)) this.unreadable.push(section)
      this.error ??= asApiError(failure).message
    },
    /** Record that one section's authority answered after all. */
    readableSection(section: DetailSection): void {
      this.unreadable = this.unreadable.filter((entry) => entry !== section)
    },
    /** Record one human reviewer's verdict on the slot the core is
     * still waiting on, against the exact tip the review is bound to,
     * carrying the findings a rejection resolves on (DR-EP-19).
     * Returns whether it landed; a refusal is reported. */
    async submitHumanVerdict(
      transport: ShellTransport,
      slotId: number,
      approve: boolean,
      summary: string,
      findings: readonly ReviewFindingRecord[],
    ): Promise<boolean> {
      const review = this.review
      if (review === null) {
        this.error = 'no review is open on this Ticket'
        return false
      }
      const attempt = this.issued
      const client = new KanbanClient(transport)
      try {
        const landed = await client.commandReviewHumanSubmit({
          mutation: {
            optimistic_version: review.version,
            idempotency_key: crypto.randomUUID(),
          },
          review_id: review.id,
          slot_id: slotId,
          tip: review.tip,
          approve,
          summary,
          findings: [...findings],
        })
        if (attempt !== this.issued) return false
        this.review = landed
        this.error = null
      } catch (failure) {
        if (attempt !== this.issued) return false
        this.error = asApiError(failure).message
        return false
      }
      // The verdict appends an attempt, may raise the revalidation the
      // gate now needs, and records the findings it resolved on; none
      // of that reaches the drawer through the returned record.
      await this.refreshReview(transport, review.ticket_id, review.project_id, attempt)
      return true
    },
    /** Read the review section again — after a landed verdict, say —
     * leaving every other section the drawer holds alone. */
    async refreshReview(
      transport: ShellTransport,
      ticketId: number,
      projectId: number,
      attempt: number,
    ): Promise<void> {
      const client = new KanbanClient(transport)
      await this.loadReview(client, ticketId, attempt)
      const [listed] = await Promise.allSettled([
        client.queryFindingList({ project_id: projectId }),
      ])
      if (attempt !== this.issued || listed === undefined) return
      if (listed.status === 'fulfilled') {
        this.findings = (listed.value.findings ?? []).filter(
          (record) => record.ticket_id === ticketId,
        )
        this.readableSection('findings')
      } else this.unreadableSection('findings', listed.reason)
    },
    /** Read the Ticket's runs again — after a recovery, say — leaving
     * everything else the drawer holds alone. */
    async refreshRuns(transport: ShellTransport, projectId: number): Promise<void> {
      const attempt = this.issued
      const ticketId = this.ticketId
      const response = await new KanbanClient(transport).queryRunList({ project_id: projectId })
      if (attempt !== this.issued) return
      this.runs = (response.runs ?? []).filter((run) => run.ticket_id === ticketId)
    },
  },
})
