// The Project registration state, driven entirely through the
// generated client: list, register, and archive, with each mutation
// carrying the record's optimistic version and a fresh idempotency
// key. No delete exists (KAN-S1-US4, KAN-S1-US6).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { MutationContext, ProjectRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

// One mutation's context: a fresh idempotency key per logical
// request, and the optimistic version the caller believes the
// aggregate is at.
function mutationFor(optimisticVersion: number): MutationContext {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

// The anchors one registration collects in the form; the generated
// request is built from them at the client call, never sent as-is.
export interface RegistrationDraft {
  code: string
  name: string
  repository: string
  seed_workspace: string
  default_branch: string
  herdr_session: string
  initiative_id?: number | null
}

export const useProjectRegisterStore = defineStore('project-register', {
  state: () => ({
    projects: [] as ProjectRecord[],
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    // Load every Project, archived included.
    async refresh(transport: ShellTransport): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryProjectList()
        this.projects = response.projects
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Register a Project with its anchors; a fresh aggregate is
    // expected at version 0.
    async register(transport: ShellTransport, draft: RegistrationDraft): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandProjectRegister({
          mutation: mutationFor(0),
          code: draft.code,
          name: draft.name,
          repository: draft.repository,
          seed_workspace: draft.seed_workspace,
          default_branch: draft.default_branch,
          herdr_session: draft.herdr_session,
          initiative_id: draft.initiative_id ?? null,
        }),
      )
    },
    // Archive a Project. Archiving is terminal and preserves every
    // recorded fact.
    async archive(transport: ShellTransport, id: number): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandProjectArchive({
          mutation: mutationFor(this.versionOf(id)),
          project_id: id,
        }),
      )
    },
    // Run one command; a refusal is reported, a success refreshes
    // the records.
    async mutate(
      transport: ShellTransport,
      command: (client: KanbanClient) => Promise<ProjectRecord>,
    ): Promise<void> {
      try {
        await command(new KanbanClient(transport))
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return
      }
      await this.refresh(transport)
    },
    // The stored version of one Project, or a reported error when the
    // record is not loaded.
    versionOf(id: number): number {
      const record = this.projects.find((project) => project.id === id)
      if (!record) {
        throw new Error(`project ${id} is not loaded`)
      }
      return record.version
    },
  },
})
