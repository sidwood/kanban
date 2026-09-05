// Herdr observation settings and connection diagnostics, driven
// entirely through the generated client (KAN-S8-US1, DR-HB-11).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  HerdrConnectionDiagnostics,
  HerdrGlobalDefaults,
  HerdrProjectSettings,
  MutationContext,
  ProjectRecord,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

function mutationFor(optimisticVersion: number): MutationContext {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

export const useHerdrSettingsStore = defineStore('herdr-settings', {
  state: () => ({
    projects: [] as ProjectRecord[],
    selectedProjectId: null as number | null,
    settings: null as HerdrProjectSettings | null,
    diagnostics: null as HerdrConnectionDiagnostics | null,
    defaults: null as HerdrGlobalDefaults | null,
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    async refresh(transport: ShellTransport): Promise<void> {
      const client = new KanbanClient(transport)
      try {
        const [projects, defaults] = await Promise.all([
          client.queryProjectList({}),
          client.queryHerdrDefaultsGet({}),
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
        const response = await new KanbanClient(transport).queryHerdrSettingsGet({ project_id: projectId })
        this.settings = response.settings
        this.diagnostics = response.diagnostics
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    async saveProjectSettings(transport: ShellTransport): Promise<void> {
      if (this.selectedProjectId === null || this.settings === null) {
        return
      }
      const settings = this.settings
      try {
        const updated = await new KanbanClient(transport).commandHerdrSettingsUpdate({
          mutation: mutationFor(settings.version),
          project_id: this.selectedProjectId,
          reconciliation_interval_secs: settings.reconciliation_interval_secs,
          polling_fallback_enabled: settings.polling_fallback_enabled,
          polling_fallback_interval_secs: settings.polling_fallback_interval_secs,
          stall_deadline_secs: settings.stall_deadline_secs,
          missing_result_deadline_secs: settings.missing_result_deadline_secs,
        })
        this.settings = updated
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
      await this.loadProject(transport, this.selectedProjectId)
    },
    async saveDefaults(transport: ShellTransport): Promise<void> {
      if (this.defaults === null) {
        return
      }
      const defaults = this.defaults
      try {
        const updated = await new KanbanClient(transport).commandHerdrDefaultsUpdate({
          mutation: mutationFor(defaults.version),
          reconciliation_interval_secs: defaults.reconciliation_interval_secs,
          stall_deadline_secs: defaults.stall_deadline_secs,
          missing_result_deadline_secs: defaults.missing_result_deadline_secs,
        })
        this.defaults = updated
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
  },
})
