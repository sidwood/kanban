// Workspace state per Project: list, register, and observe through
// the generated client (KAN-S6-US1).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { MutationContext, WorkspaceRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

function mutationFor(optimisticVersion: number): MutationContext {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

export interface WorkspaceRegistrationDraft {
  path: string
}

export const useWorkspacesStore = defineStore('workspaces', {
  state: () => ({
    projectId: null as number | null,
    workspaces: [] as WorkspaceRecord[],
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    async load(transport: ShellTransport, projectId: number): Promise<void> {
      this.projectId = projectId
      try {
        const response = await new KanbanClient(transport).queryWorkspaceList({ project_id: projectId })
        this.workspaces = response.workspaces
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    async register(
      transport: ShellTransport,
      projectId: number,
      draft: WorkspaceRegistrationDraft,
    ): Promise<void> {
      await this.mutate(transport, projectId, (client) =>
        client.commandWorkspaceRegister({
          mutation: mutationFor(0),
          project_id: projectId,
          path: draft.path,
        }),
      )
    },
    async observe(transport: ShellTransport, projectId: number, workspaceId: number): Promise<void> {
      const record = this.workspaces.find((workspace) => workspace.id === workspaceId)
      if (!record) {
        throw new Error(`workspace ${workspaceId} is not loaded`)
      }
      await this.mutate(transport, projectId, (client) =>
        client.commandWorkspaceObserve({
          mutation: mutationFor(record.version),
          workspace_id: workspaceId,
        }),
      )
    },
    async mutate(
      transport: ShellTransport,
      projectId: number,
      command: (client: KanbanClient) => Promise<WorkspaceRecord>,
    ): Promise<void> {
      try {
        await command(new KanbanClient(transport))
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return
      }
      await this.load(transport, projectId)
    },
  },
})
