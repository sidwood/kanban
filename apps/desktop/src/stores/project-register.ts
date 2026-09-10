// The Project register. No delete exists: a Project is archived, never
// removed (KAN-S1-US4, KAN-S1-US6).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { MutationContext, ProjectRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import { adoptScope, emptyScope, scopeHolds } from '../core/scope-authority'

// A fresh idempotency key per logical request, so a retried transport
// cannot apply one intent twice.
function mutationFor(optimisticVersion: number): MutationContext {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

// A blank Herdr session means absence, which selects Herdr's default
// session, so it reaches the core as null.
export interface RegistrationDraft {
  code: string
  name: string
  repository: string
  seed_workspace: string
  default_branch: string
  herdr_workspace: string
  herdr_session: string
  initiative_id?: number | null
}

export interface SettingsOutcome {
  landed: boolean
  refusal: string | null
}

// The settings an operator owns after registration. The code, the
// target repository, and the Seed Workspace are absent because the
// core holds them immutable: identity is minted once, and the
// repository and the Seed anchor the work a Project already holds.
export interface ProjectSettingsDraft {
  name: string
  default_branch: string
  herdr_workspace: string
  herdr_session: string
  initiative_id?: number | null
}

export const useProjectRegisterStore = defineStore('project-register', {
  state: () => ({
    ...emptyScope(),
    projects: [] as ProjectRecord[],
    loaded: false,
    loading: false,
    error: null as string | null,
  }),
  actions: {
    async refresh(transport: ShellTransport): Promise<void> {
      const claim = adoptScope(this, 'project-register')
      this.loading = true
      this.error = null
      try {
        const response = await new KanbanClient(transport).queryProjectList()
        if (!scopeHolds(this, claim)) return
        this.projects = response.projects
        this.loaded = true
        this.error = null
      } catch (failure) {
        if (!scopeHolds(this, claim)) return
        this.error = asApiError(failure).message
      } finally {
        if (scopeHolds(this, claim)) this.loading = false
      }
    },
    async register(transport: ShellTransport, draft: RegistrationDraft): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandProjectRegister({
          mutation: mutationFor(0),
          code: draft.code,
          name: draft.name,
          repository: draft.repository,
          seed_workspace: draft.seed_workspace,
          default_branch: draft.default_branch,
          herdr_workspace: draft.herdr_workspace,
          herdr_session: draft.herdr_session.trim() ? draft.herdr_session : null,
          initiative_id: draft.initiative_id ?? null,
        }),
      )
    },
    async update(
      transport: ShellTransport,
      id: number,
      optimisticVersion: number,
      draft: ProjectSettingsDraft,
    ): Promise<SettingsOutcome> {
      let landed: ProjectRecord
      try {
        landed = await new KanbanClient(transport).commandProjectUpdate({
          mutation: mutationFor(optimisticVersion),
          project_id: id,
          name: draft.name,
          default_branch: draft.default_branch,
          herdr_workspace: draft.herdr_workspace,
          herdr_session: draft.herdr_session.trim() ? draft.herdr_session : null,
          initiative_id: draft.initiative_id ?? null,
        })
      } catch (failure) {
        return { landed: false, refusal: asApiError(failure).message }
      }
      this.projects = this.projects.some((project) => project.id === landed.id)
        ? this.projects.map((project) => project.id === landed.id ? landed : project)
        : [...this.projects, landed]
      this.loaded = true
      await this.refresh(transport)
      return { landed: true, refusal: null }
    },
    async archive(transport: ShellTransport, id: number): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandProjectArchive({
          mutation: mutationFor(this.versionOf(id)),
          project_id: id,
        }),
      )
    },
    async mutate(
      transport: ShellTransport,
      command: (client: KanbanClient) => Promise<ProjectRecord>,
    ): Promise<boolean> {
      try {
        await command(new KanbanClient(transport))
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return false
      }
      await this.refresh(transport)
      return true
    },
    versionOf(id: number): number {
      const record = this.projects.find((project) => project.id === id)
      if (!record) {
        throw new Error(`project ${id} is not loaded`)
      }
      return record.version
    },
  },
})
