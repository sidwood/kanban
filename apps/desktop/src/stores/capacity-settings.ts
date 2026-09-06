// Capacity settings state, driven entirely through the generated
// client: the global defaults that constrain active runs by harness,
// model family, and usage pool, and the stricter caps plus maximum
// active Lane count one Project may impose (KAN-S7-US3, DR-EP-06,
// DR-EP-07). An empty draft field means no cap on that dimension,
// every other value is sent for the Core to judge, and refusals are
// reported, never swallowed.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  CapacityGlobalDefaults,
  CapacityProjectCaps,
  ProjectRecord,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

function mutationFor(optimisticVersion: number) {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

// What one draft field asks of the Core. An empty field clears the
// cap; any parseable number — zero, negative, or fractional included
// — is the Operator's typed input, sent for the Core's typed
// validation to refuse; the UI invents no clamp of its own. Only
// text no JSON payload can carry is refused here, because JSON would
// encode it as null and silently clear the cap instead. A number
// input hands Vue's number cast a number, so both shapes arrive
// here.
type CapDraft =
  | { kind: 'omit' }
  | { kind: 'send'; value: number }
  | { kind: 'refuse'; reason: string }

function capOf(draft: string | number, field: string): CapDraft {
  const text = `${draft}`.trim()
  if (text.length === 0) {
    return { kind: 'omit' }
  }
  const parsed = Number(text)
  if (Number.isFinite(parsed)) {
    return { kind: 'send', value: parsed }
  }
  return {
    kind: 'refuse',
    reason: `the ${field} cap must be a number; leave the field empty to clear it`,
  }
}

// The draft text for one stored cap: absence is an empty field.
function draftOf(cap: number | null | undefined): string {
  return cap === null || cap === undefined ? '' : `${cap}`
}

export const useCapacityStore = defineStore('capacity-settings', {
  state: () => ({
    projects: [] as ProjectRecord[],
    selectedProjectId: null as number | null,
    defaults: null as CapacityGlobalDefaults | null,
    caps: null as CapacityProjectCaps | null,
    harness: '' as string | number,
    model: '' as string | number,
    usagePool: '' as string | number,
    lanes: '' as string | number,
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    async refresh(transport: ShellTransport): Promise<void> {
      const client = new KanbanClient(transport)
      try {
        const [projects, defaults] = await Promise.all([
          client.queryProjectList({}),
          client.queryCapacityDefaultsGet({}),
        ])
        this.projects = projects.projects.filter((project) => !project.archived)
        this.defaults = defaults.defaults
        this.loaded = true
        this.error = null
        if (this.selectedProjectId === null && this.projects.length > 0) {
          await this.selectProject(transport, this.projects[0].id)
        } else if (this.selectedProjectId !== null) {
          await this.loadProject(transport, this.selectedProjectId)
        }
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    async selectProject(transport: ShellTransport, projectId: number): Promise<void> {
      this.selectedProjectId = projectId
      await this.loadProject(transport, projectId)
    },
    async loadProject(transport: ShellTransport, projectId: number): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryCapacitySettingsGet({
          project_id: projectId,
        })
        this.adoptCaps(response.caps)
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Hold the loaded record and mirror its caps into the draft
    // fields the inputs bind to.
    adoptCaps(caps: CapacityProjectCaps) {
      this.caps = caps
      this.harness = draftOf(caps.max_active_per_harness)
      this.model = draftOf(caps.max_active_per_model)
      this.usagePool = draftOf(caps.max_active_per_usage_pool)
      this.lanes = draftOf(caps.max_active_lanes)
    },
    async saveDefaults(transport: ShellTransport): Promise<void> {
      if (this.defaults === null) {
        return
      }
      const defaults = this.defaults
      try {
        const updated = await new KanbanClient(transport).commandCapacityDefaultsUpdate({
          mutation: mutationFor(defaults.version),
          max_active_per_harness: defaults.max_active_per_harness,
          max_active_per_model: defaults.max_active_per_model,
          max_active_per_usage_pool: defaults.max_active_per_usage_pool,
        })
        this.defaults = updated
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Replace the selected Project's caps wholesale: a field left
    // empty clears its cap, so the global default stands on that
    // dimension again (DR-EP-07), and every other value reaches the
    // Core for judgement.
    async saveProjectCaps(transport: ShellTransport): Promise<void> {
      if (this.selectedProjectId === null || this.caps === null) {
        return
      }
      const caps = this.caps
      const harness = capOf(this.harness, 'harness family')
      const model = capOf(this.model, 'model family')
      const pool = capOf(this.usagePool, 'usage pool')
      const lanes = capOf(this.lanes, 'active Lane')
      const refusal = [harness, model, pool, lanes].find(
        (draft): draft is { kind: 'refuse'; reason: string } => draft.kind === 'refuse',
      )
      if (refusal) {
        this.error = refusal.reason
        return
      }
      try {
        const updated = await new KanbanClient(transport).commandCapacitySettingsUpdate({
          mutation: mutationFor(caps.version),
          project_id: this.selectedProjectId,
          ...(harness.kind === 'send' ? { max_active_per_harness: harness.value } : {}),
          ...(model.kind === 'send' ? { max_active_per_model: model.value } : {}),
          ...(pool.kind === 'send' ? { max_active_per_usage_pool: pool.value } : {}),
          ...(lanes.kind === 'send' ? { max_active_lanes: lanes.value } : {}),
        })
        this.adoptCaps(updated)
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
  },
})
