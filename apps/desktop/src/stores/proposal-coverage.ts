// The coverage basis of each Ticket graph proposal (KAN-T140-AC1,
// DR-PS-14, DR-PS-17): a proposal is recorded against one Spec content
// version, and the approval gate refuses it on that version's story
// coverage — never on whatever version the Spec is operating at now.
// Only the latest scope's answers are kept, so a Spec the operator has
// left cannot lend its basis to the one they are in (KAN-T145).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { SpecCoverageMatrixResponse } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { adoptScope, emptyScope, releaseScope, scopeHolds } from '../core/scope-authority'

/** One Spec content version's coverage, as the gate reads it. */
export interface VersionCoverage {
  /** The stories of that version no attached Ticket's criterion claims. */
  uncovered: string[]
  /** Why the version's coverage could not be read, when it could not. */
  error: string | null
}

export const useProposalCoverageStore = defineStore('proposal-coverage', {
  state: () => ({
    ...emptyScope(),
    specId: null as number | null,
    byVersion: {} as Record<number, VersionCoverage>,
  }),
  getters: {
    // One version's coverage, once it has been read.
    coverageOf: (state) => {
      return (version: number): VersionCoverage | null => state.byVersion[version] ?? null
    },
  },
  actions: {
    // Each read is pinned to the version the proposal names, never to
    // the version the Spec is operating at now.
    async load(
      transport: ShellTransport,
      specId: number,
      versions: readonly number[],
    ): Promise<void> {
      const claim = adoptScope(this, `spec:${specId}`)
      this.specId = specId
      this.byVersion = {}
      const client = new KanbanClient(transport)
      const answers = await Promise.all(
        [...new Set(versions)].map(async (version): Promise<[number, VersionCoverage]> => {
          try {
            const report: SpecCoverageMatrixResponse = await client.querySpecCoverageMatrix({
              spec_id: specId,
              version,
            })
            return [
              version,
              {
                uncovered: report.stories
                  .filter((row) => row.claims.length === 0)
                  .map((row) => row.story),
                error: null,
              },
            ]
          } catch (failure) {
            return [version, { uncovered: [], error: asApiError(failure).message }]
          }
        }),
      )
      if (!scopeHolds(this, claim)) return
      this.byVersion = Object.fromEntries(answers)
    },
    // Forget every basis when no Spec is on display; anything still in
    // flight is superseded.
    clear(): void {
      releaseScope(this)
      this.specId = null
      this.byVersion = {}
    },
  },
})
