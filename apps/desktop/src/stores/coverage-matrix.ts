// The Project's Spec list and picked Spec matrix share Project authority, while
// every matrix response must also match the current pick.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { SpecCoverageMatrixResponse, SpecListResponse, SpecRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import {
  adoptScope,
  emptyScope,
  issueRead,
  projectScopeKey,
  releaseScope,
  scopeHolds,
} from '../core/scope-authority'

function leftBehind(picked: number | null, specId: number): boolean {
  return picked !== null && picked !== specId
}

export const useCoverageMatrixStore = defineStore('coverage-matrix', {
  state: () => ({
    ...emptyScope(),
    projectId: null as number | null,
    specs: [] as SpecRecord[],
    pickedSpecId: null as number | null,
    report: null as SpecCoverageMatrixResponse | null,
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    async loadSpecs(transport: ShellTransport, projectId: number): Promise<void> {
      const claim = adoptScope(this, projectScopeKey(projectId))
      this.projectId = projectId
      try {
        const response: SpecListResponse =
          await new KanbanClient(transport).querySpecList({ project_id: projectId })
        if (!scopeHolds(this, claim)) {
          return
        }
        this.specs = response.specs
        const kept = this.pickedSpecId
        this.pickedSpecId = response.specs.some((spec) => spec.id === kept)
          ? kept
          : (response.specs[0]?.id ?? null)
        if (this.pickedSpecId === null) {
          this.report = null
          this.loaded = false
          return
        }
        await this.read(transport, this.pickedSpecId)
      } catch (failure) {
        if (!scopeHolds(this, claim)) {
          return
        }
        this.specs = []
        this.pickedSpecId = null
        this.report = null
        this.loaded = false
        this.error = asApiError(failure).message
      }
    },
    async pick(transport: ShellTransport, specId: number): Promise<void> {
      this.pickedSpecId = specId
      this.report = null
      this.loaded = false
      this.error = null
      await this.read(transport, specId)
    },
    async read(
      transport: ShellTransport,
      specId: number,
      version: number | null = null,
    ): Promise<void> {
      const claim = issueRead(this)
      try {
        const report = await new KanbanClient(transport).querySpecCoverageMatrix({
          spec_id: specId,
          version,
        })
        if (!scopeHolds(this, claim) || leftBehind(this.pickedSpecId, specId)) {
          return
        }
        this.report = report
        this.loaded = true
        this.error = null
      } catch (failure) {
        if (!scopeHolds(this, claim) || leftBehind(this.pickedSpecId, specId)) {
          return
        }
        this.report = null
        this.loaded = false
        this.error = asApiError(failure).message
      }
    },
    forgetPick(): void {
      issueRead(this)
      this.pickedSpecId = null
      this.report = null
      this.loaded = false
    },
    clear(): void {
      releaseScope(this)
      this.projectId = null
      this.specs = []
      this.pickedSpecId = null
      this.report = null
      this.loaded = false
      this.error = null
    },
  },
})
