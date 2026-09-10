// Workspace state per Project: list, register, and observe through
// the generated client (KAN-S6-US1). The store holds one Project at a
// time: only the latest load writes state, a command for a Project it
// no longer holds is refused, and an answer arriving after the Project
// changed writes nothing (KAN-T145).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { MutationContext, WorkspaceRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import {
  adoptScope,
  emptyScope,
  issueCommand,
  projectScopeKey,
  releaseScope,
  scopeHolds,
} from '../core/scope-authority'

function mutationFor(optimisticVersion: number): MutationContext {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

export interface WorkspaceRegistrationDraft {
  path: string
}

export const useWorkspacesStore = defineStore('workspaces', {
  state: () => ({
    ...emptyScope(),
    projectId: null as number | null,
    workspaces: [] as WorkspaceRecord[],
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    async load(transport: ShellTransport, projectId: number): Promise<void> {
      const claim = adoptScope(this, projectScopeKey(projectId))
      this.projectId = projectId
      try {
        const response = await new KanbanClient(transport).queryWorkspaceList({ project_id: projectId })
        if (!scopeHolds(this, claim)) return
        this.workspaces = response.workspaces
        this.loaded = true
        this.error = null
      } catch (failure) {
        if (!scopeHolds(this, claim)) return
        this.error = asApiError(failure).message
      }
    },
    // Forget the listing when the surface leaves this Project;
    // anything still in flight is superseded.
    clear(): void {
      releaseScope(this)
      this.projectId = null
      this.workspaces = []
      this.loaded = false
      this.error = null
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
    // Run one command against `projectId` and read that Project back.
    // A Project this store does not hold is never commanded, and a
    // surface that left the Project while the command was in flight
    // gets neither the refusal nor the reload: the answer belongs to
    // the scope that asked for it.
    async mutate(
      transport: ShellTransport,
      projectId: number,
      command: (client: KanbanClient) => Promise<WorkspaceRecord>,
    ): Promise<void> {
      if (this.projectId !== projectId) {
        this.error = `Project ${projectId} is not the Project on display.`
        return
      }
      const claim = issueCommand(this)
      try {
        await command(new KanbanClient(transport))
        if (!scopeHolds(this, claim)) return
        this.error = null
      } catch (failure) {
        if (!scopeHolds(this, claim)) return
        this.error = asApiError(failure).message
        return
      }
      await this.load(transport, projectId)
    },
  },
})
