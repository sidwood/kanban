// Lane state per Project: list, create, and Workspace claim
// assignment through the generated client (KAN-S6-US2). The store
// holds one Project at a time: only the latest load writes state, a
// command for a Project it no longer holds is refused, and an answer
// arriving after the Project changed writes nothing (KAN-T145).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { LaneRecord, MutationContext } from '@kanban/contracts'
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

export const useLanesStore = defineStore('lanes', {
  state: () => ({
    ...emptyScope(),
    projectId: null as number | null,
    lanes: [] as LaneRecord[],
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    async load(transport: ShellTransport, projectId: number): Promise<void> {
      const claim = adoptScope(this, projectScopeKey(projectId))
      this.projectId = projectId
      try {
        const response = await new KanbanClient(transport).queryLaneList({ project_id: projectId })
        if (!scopeHolds(this, claim)) return
        this.lanes = response.lanes
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
      this.lanes = []
      this.loaded = false
      this.error = null
    },
    async create(transport: ShellTransport, projectId: number): Promise<void> {
      await this.mutate(transport, projectId, (client) =>
        client.commandLaneCreate({
          mutation: mutationFor(0),
          project_id: projectId,
        }),
      )
    },
    async assignWorkspace(
      transport: ShellTransport,
      projectId: number,
      laneId: number,
      workspaceId: number,
    ): Promise<void> {
      const lane = this.lanes.find((entry) => entry.id === laneId)
      if (!lane) {
        throw new Error(`lane ${laneId} is not loaded`)
      }
      await this.mutate(transport, projectId, (client) =>
        client.commandLaneWorkspaceAssign({
          mutation: mutationFor(lane.version),
          lane_id: laneId,
          workspace_id: workspaceId,
        }),
      )
    },
    async releaseWorkspace(
      transport: ShellTransport,
      projectId: number,
      laneId: number,
    ): Promise<void> {
      const lane = this.lanes.find((entry) => entry.id === laneId)
      if (!lane) {
        throw new Error(`lane ${laneId} is not loaded`)
      }
      await this.mutate(transport, projectId, (client) =>
        client.commandLaneWorkspaceRelease({
          mutation: mutationFor(lane.version),
          lane_id: laneId,
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
      command: (client: KanbanClient) => Promise<LaneRecord>,
    ): Promise<void> {
      if (this.projectId !== projectId) {
        this.error = `Project ${projectId} is not the Project on display.`
        return
      }
      const claim = issueCommand(this)
      let failure: unknown = null
      try {
        await command(new KanbanClient(transport))
        if (!scopeHolds(this, claim)) return
        this.error = null
      } catch (error) {
        if (!scopeHolds(this, claim)) return
        failure = error
        this.error = asApiError(error).message
      }
      await this.load(transport, projectId)
      if (failure) {
        // The reload refreshed the listing; the refusal still stands.
        this.error = asApiError(failure).message
      }
    },
  },
})
