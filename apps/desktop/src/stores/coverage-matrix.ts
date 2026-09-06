// The coverage matrix of the Spec on display: the story-to-criterion-
// to-Ticket rows the core renders for one Spec version (DR-PS-18),
// completing the KAN-T16 planning diagnostics. The store keeps the
// picked Project's Spec list so the view can switch Specs, and reads
// every matrix through the generated client; only the latest read
// ever writes state, and a refused one leaves no stale matrix behind.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { SpecCoverageMatrixResponse, SpecListResponse, SpecRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

export const useCoverageMatrixStore = defineStore('coverage-matrix', {
  state: () => ({
    specs: [] as SpecRecord[],
    pickedSpecId: null as number | null,
    report: null as SpecCoverageMatrixResponse | null,
    loaded: false,
    error: null as string | null,
    // The reads issued so far, so only the latest one — the Spec
    // actually on display — ever writes state.
    issued: 0,
  }),
  actions: {
    // Load one Project's Specs and read the matrix of the Spec the
    // picker lands on: the kept pick when it still belongs to the
    // Project, else the first Spec. A load another load or clear()
    // has superseded writes nothing.
    async loadSpecs(transport: ShellTransport, projectId: number): Promise<void> {
      const attempt = ++this.issued
      try {
        const response: SpecListResponse =
          await new KanbanClient(transport).querySpecList({ project_id: projectId })
        if (attempt !== this.issued) {
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
        if (attempt !== this.issued) {
          return
        }
        this.specs = []
        this.pickedSpecId = null
        this.report = null
        this.loaded = false
        this.error = asApiError(failure).message
      }
    },
    // Switch the picker to one of the loaded Specs and read its
    // matrix; a null version reads the version the Spec's Tickets
    // answer to.
    async pick(transport: ShellTransport, specId: number): Promise<void> {
      this.pickedSpecId = specId
      await this.read(transport, specId)
    },
    // Read one Spec's coverage matrix. A read another read, load, or
    // clear() has superseded writes nothing; a refused one leaves no
    // stale matrix on display.
    async read(
      transport: ShellTransport,
      specId: number,
      version: number | null = null,
    ): Promise<void> {
      const attempt = ++this.issued
      try {
        const report = await new KanbanClient(transport).querySpecCoverageMatrix({
          spec_id: specId,
          version,
        })
        if (attempt !== this.issued) {
          return
        }
        this.report = report
        this.loaded = true
        this.error = null
      } catch (failure) {
        if (attempt !== this.issued) {
          return
        }
        this.report = null
        this.loaded = false
        this.error = asApiError(failure).message
      }
    },
    // Forget the matrix when no Project is on display; anything still
    // in flight is superseded.
    clear(): void {
      this.issued += 1
      this.specs = []
      this.pickedSpecId = null
      this.report = null
      this.loaded = false
      this.error = null
    },
  },
})
