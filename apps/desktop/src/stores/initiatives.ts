// The Initiative management state, driven entirely through the
// generated client: list, create, rename, and archive, with each
// mutation carrying the record's optimistic version and a fresh
// idempotency key. No delete exists (KAN-S1-US6).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { InitiativeRecord, MutationContext } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

// One mutation's context: a fresh idempotency key per logical
// request, and the optimistic version the caller believes the
// aggregate is at.
function mutationFor(optimisticVersion: number): MutationContext {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

export const useInitiativesStore = defineStore('initiatives', {
  state: () => ({
    initiatives: [] as InitiativeRecord[],
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    // Load every Initiative, archived included.
    async refresh(transport: ShellTransport): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryInitiativeList()
        this.initiatives = response.initiatives
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Create an Initiative; a fresh aggregate is expected at
    // version 0.
    async create(transport: ShellTransport, name: string): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandInitiativeCreate({ mutation: mutationFor(0), name }),
      )
    },
    // Rename an active Initiative, carrying its current version.
    async rename(transport: ShellTransport, id: number, name: string): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandInitiativeRename({
          mutation: mutationFor(this.versionOf(id)),
          initiative_id: id,
          name,
        }),
      )
    },
    // Archive an Initiative. Archiving is terminal and preserves
    // every recorded fact.
    async archive(transport: ShellTransport, id: number): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandInitiativeArchive({
          mutation: mutationFor(this.versionOf(id)),
          initiative_id: id,
        }),
      )
    },
    // Run one command; a refusal is reported, a success refreshes
    // the records.
    async mutate(
      transport: ShellTransport,
      command: (client: KanbanClient) => Promise<InitiativeRecord>,
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
    // The stored version of one Initiative, or a reported error when
    // the record is not loaded.
    versionOf(id: number): number {
      const record = this.initiatives.find((initiative) => initiative.id === id)
      if (!record) {
        throw new Error(`initiative ${id} is not loaded`)
      }
      return record.version
    },
  },
})
