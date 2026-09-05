// The Spec editor state, driven entirely through the generated
// client: the Specs of one Project, the nine PRD sections of the
// working content, the version switcher that keeps superseded
// versions visible beside the current one, the diff between two
// versions, and the approval, supersession, planning, and execution
// actions. Terminal execution states stay listed; approved and
// superseded versions never change (KAN-S3-US4, KAN-S3-US5).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  MutationContext,
  SpecContent,
  SpecExecutionState,
  SpecRecord,
  SpecVersionRecord,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

// One mutation's context: a fresh idempotency key per logical
// request, and the optimistic version the caller believes the
// aggregate is at.
function mutationFor(optimisticVersion: number): MutationContext {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

// The PRD sections in editorial order, the single list the editor,
// the diff, and the forms all iterate.
export const SPEC_SECTIONS = [
  'name',
  'short_description',
  'problem_statement',
  'solution',
  'user_stories',
  'implementation_decisions',
  'testing_decisions',
  'out_of_scope',
  'further_notes',
] as const

// One section's line diff between two versions.
export interface SectionDiff {
  section: (typeof SPEC_SECTIONS)[number]
  removed: string[]
  added: string[]
}

// Lines one text holds beyond the other, as a bag difference so
// repeated lines count once each time they appear.
function linesBeyond(held: string, other: string): string[] {
  const remaining = other.split('\n')
  const beyond: string[] = []
  for (const line of held.split('\n')) {
    const at = remaining.indexOf(line)
    if (at === -1) {
      beyond.push(line)
    } else {
      remaining.splice(at, 1)
    }
  }
  return beyond
}

// The section-by-section diff between two versions' content: what
// the later version removed and added, section by section. Sections
// that did not change carry empty sides.
export function diffContent(before: SpecContent, after: SpecContent): SectionDiff[] {
  return SPEC_SECTIONS.map((section) => {
    const held = before[section]
    const now = after[section]
    return {
      section,
      removed: held === now ? [] : linesBeyond(held, now),
      added: held === now ? [] : linesBeyond(now, held),
    }
  })
}

// The execution states the editor may move a Spec to directly;
// `planned` is reached by joining a Plan, never by a move.
export const MOVABLE_EXECUTION_STATES: SpecExecutionState[] = [
  'blocked',
  'ready',
  'active',
  'integration_review',
  'complete',
  'cancelled',
]

export const useSpecEditorStore = defineStore('spec-editor', {
  state: () => ({
    specs: [] as SpecRecord[],
    versions: [] as SpecVersionRecord[],
    selectedSpecId: null as number | null,
    selectedVersion: null as number | null,
    comparedVersion: null as number | null,
    loaded: false,
    error: null as string | null,
  }),
  getters: {
    // The Spec the editor has open, if any.
    selected(state): SpecRecord | null {
      return state.specs.find((spec) => spec.id === state.selectedSpecId) ?? null
    },
    // The newest version record: the Spec's working content, draft or
    // frozen.
    currentVersion(state): SpecVersionRecord | null {
      return state.versions.length ? state.versions[state.versions.length - 1] : null
    },
    // The version on display: the selected one, or the working
    // content when no version is selected.
    displayed(state): SpecVersionRecord | null {
      if (state.selectedVersion !== null) {
        return state.versions.find((version) => version.number === state.selectedVersion) ?? null
      }
      return this.currentVersion
    },
    // The still-approved version, when one is operative.
    approvedVersion(): SpecVersionRecord | null {
      return this.versions.find((version) => version.state === 'approved') ?? null
    },
    // The working draft, when the newest version is still editable.
    draft(): SpecVersionRecord | null {
      const newest = this.currentVersion
      return newest?.state === 'draft' ? newest : null
    },
    // Whether the approve action is available: a draft exists and no
    // other version is still approved, because supersession is
    // explicit.
    canApprove(): boolean {
      return this.draft !== null && this.approvedVersion === null
    },
    // The version the diff compares against the displayed one.
    compared(state): SpecVersionRecord | null {
      if (state.comparedVersion === null) {
        return null
      }
      return state.versions.find((version) => version.number === state.comparedVersion) ?? null
    },
    // The section-by-section diff between the compared version and
    // the displayed one; null when no comparison is set.
    diff(): SectionDiff[] | null {
      const compared = this.compared
      const displayed = this.displayed
      if (!compared || !displayed) {
        return null
      }
      return diffContent(compared.content, displayed.content)
    },
  },
  actions: {
    // Load every Spec of one Project, terminal execution states
    // included.
    async refresh(transport: ShellTransport, projectId: number): Promise<void> {
      try {
        const response = await new KanbanClient(transport).querySpecList({ project_id: projectId })
        this.specs = response.specs
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Open one Spec: select it and load its record and every content
    // version, showing the working content first.
    async open(transport: ShellTransport, specId: number): Promise<void> {
      this.selectedSpecId = specId
      this.selectedVersion = null
      this.comparedVersion = null
      try {
        const response = await new KanbanClient(transport).querySpecGet({ spec_id: specId })
        this.versions = response.versions
        const known = this.specs.findIndex((spec) => spec.id === response.spec.id)
        if (known === -1) {
          this.specs = [...this.specs, response.spec]
        } else {
          this.specs = this.specs.map((spec) =>
            spec.id === response.spec.id ? response.spec : spec,
          )
        }
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Select a Spec from the list without loading its versions.
    select(specId: number): void {
      this.selectedSpecId = specId
      this.selectedVersion = null
      this.comparedVersion = null
    },
    // Show one version's content.
    showVersion(number: number): void {
      this.selectedVersion = number
      if (this.comparedVersion === number) {
        this.comparedVersion = null
      }
    },
    // Show the working content.
    showCurrent(): void {
      this.selectedVersion = null
    },
    // Diff the displayed version against `number`; the same number
    // clears the comparison.
    compareWith(number: number): void {
      this.comparedVersion = this.comparedVersion === number ? null : number
    },
    // Author a Spec under the Project with its opening PRD content;
    // a fresh aggregate is expected at version 0.
    async create(transport: ShellTransport, projectId: number, content: SpecContent): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandSpecCreate({
          mutation: mutationFor(0),
          project_id: projectId,
          content,
        }),
      )
    },
    // Replace the working content: a draft edits in place, content
    // that has moved on mints a new draft version.
    async updateContent(transport: ShellTransport, content: SpecContent): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandSpecContentUpdate({
          mutation: mutationFor(this.versionOfSelected()),
          spec_id: this.selectedSpecId ?? 0,
          content,
        }),
      )
    },
    // Approve the working draft into immutable operative content.
    async approve(transport: ShellTransport): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandSpecVersionApprove({
          mutation: mutationFor(this.versionOfSelected()),
          spec_id: this.selectedSpecId ?? 0,
        }),
      )
    },
    // Supersede one version explicitly; the superseded version stays
    // queryable for the Tickets pinned to it.
    async supersede(transport: ShellTransport, version: number): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandSpecVersionSupersede({
          mutation: mutationFor(this.versionOfSelected()),
          spec_id: this.selectedSpecId ?? 0,
          version,
        }),
      )
    },
    // Join the Plan holding the Spec's number, planning its
    // execution.
    async joinPlan(transport: ShellTransport, planId: number): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandSpecPlanJoin({
          mutation: mutationFor(this.versionOfSelected()),
          spec_id: this.selectedSpecId ?? 0,
          plan_id: planId,
        }),
      )
    },
    // Move the execution state along the closed set.
    async moveExecution(transport: ShellTransport, execution: SpecExecutionState): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandSpecExecutionMove({
          mutation: mutationFor(this.versionOfSelected()),
          spec_id: this.selectedSpecId ?? 0,
          execution,
        }),
      )
    },
    // Run one command; a refusal is reported, and a success refreshes
    // the collection and opens the returned Spec — creation included,
    // which lands with nothing yet selected, so no minted Spec can
    // stay invisible and new versions show at once.
    async mutate(
      transport: ShellTransport,
      command: (client: KanbanClient) => Promise<SpecRecord>,
    ): Promise<void> {
      let landed: SpecRecord
      try {
        landed = await command(new KanbanClient(transport))
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return
      }
      await this.refresh(transport, landed.project_id)
      await this.open(transport, landed.id)
    },
    // The stored version of the selected Spec, or a reported error
    // when no Spec is open.
    versionOfSelected(): number {
      const spec = this.selected
      if (!spec) {
        throw new Error('no spec is selected')
      }
      return spec.version
    },
  },
})
