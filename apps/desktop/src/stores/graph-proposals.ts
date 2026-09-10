// The Ticket graph proposals of one Spec, driven entirely through the
// generated client (KAN-S4-US8, DR-PS-16, DR-PS-17): the agent-proposed
// graphs recorded against a Spec content version, and the human gate
// that approves one. Approval is the operator's own act: the store
// sends `ticket.graph.approve` against the proposal's own aggregate
// version and reports the gate's refusal rather than swallowing it,
// so a graph that is incomplete, not granular, not verifiable, not
// story-covered, or carrying an unassignable profile stays proposed
// and says why.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { TicketGraphRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import {
  adoptScope,
  emptyScope,
  issueCommand,
  releaseScope,
  scopeHolds,
} from '../core/scope-authority'

/** The key one Spec's proposals are held under. */
function keyOf(specId: number): string {
  return `spec:${specId}`
}

export const useGraphProposalsStore = defineStore('graph-proposals', {
  state: () => ({
    ...emptyScope(),
    specId: null as number | null,
    proposals: [] as TicketGraphRecord[],
    loaded: false,
    error: null as string | null,
    // The gate's own refusal, against the proposal it refused, so the
    // operator reads the core's words beside the graph they name.
    refusal: null as { proposalId: number; message: string } | null,
  }),
  getters: {
    // The graph already approved for a Spec version, when one is.
    approved(state): TicketGraphRecord | null {
      return state.proposals.find((entry) => entry.state === 'approved') ?? null
    },
  },
  actions: {
    // Read one Spec's proposals, oldest first. A read another read or
    // clear() has superseded writes nothing.
    async load(transport: ShellTransport, specId: number): Promise<void> {
      const claim = adoptScope(this, keyOf(specId))
      this.specId = specId
      this.refusal = null
      try {
        const response = await new KanbanClient(transport).queryTicketGraphList({
          spec_id: specId,
        })
        if (!scopeHolds(this, claim)) return
        this.proposals = response.proposals
        this.loaded = true
        this.error = null
      } catch (failure) {
        if (!scopeHolds(this, claim)) return
        this.proposals = []
        this.loaded = false
        this.error = asApiError(failure).message
      }
    },
    // Put one proposal through the human gate. The gate is the
    // operator's own act on the Spec on display, so a proposal that
    // Spec does not own is never sent, and an approval answered after
    // the Spec changed reloads nothing. Reports whether it landed; a
    // refusal is reported and the proposal stands.
    async approve(transport: ShellTransport, proposal: TicketGraphRecord): Promise<boolean> {
      if (this.specId === null || this.specId !== proposal.spec_id) {
        this.refusal = {
          proposalId: proposal.id,
          message: 'This proposal belongs to a Spec the surface has left.',
        }
        return false
      }
      const claim = issueCommand(this)
      try {
        await new KanbanClient(transport).commandTicketGraphApprove({
          mutation: {
            optimistic_version: proposal.version,
            idempotency_key: crypto.randomUUID(),
          },
          proposal_id: proposal.id,
        })
      } catch (failure) {
        if (!scopeHolds(this, claim)) return false
        this.refusal = { proposalId: proposal.id, message: asApiError(failure).message }
        return false
      }
      if (!scopeHolds(this, claim)) return false
      this.refusal = null
      await this.load(transport, proposal.spec_id)
      return true
    },
    // Forget the proposals when no Spec is on display; anything still
    // in flight is superseded.
    clear(): void {
      releaseScope(this)
      this.specId = null
      this.proposals = []
      this.loaded = false
      this.error = null
      this.refusal = null
    },
  },
})
