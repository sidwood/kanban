// Lane state per Project: list, create, and Workspace claim
// assignment through the generated client (KAN-S6-US2).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { LaneRecord, MutationContext } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

function mutationFor(optimisticVersion: number): MutationContext {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

export const useLanesStore = defineStore('lanes', {
  state: () => ({
    projectId: null as number | null,
    lanes: [] as LaneRecord[],
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    async load(transport: ShellTransport, projectId: number): Promise<void> {
      this.projectId = projectId
      try {
        const response = await new KanbanClient(transport).queryLaneList({ project_id: projectId })
        this.lanes = response.lanes
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
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
    async mutate(
      transport: ShellTransport,
      projectId: number,
      command: (client: KanbanClient) => Promise<LaneRecord>,
    ): Promise<void> {
      let failure: unknown = null
      try {
        await command(new KanbanClient(transport))
        this.error = null
      } catch (error) {
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
