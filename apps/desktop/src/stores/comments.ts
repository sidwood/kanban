// Comment state with revision history, driven through the generated
// client: create, edit, and query revisions (KAN-S2-US2).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type {
  CommentRecord,
  CommentRevisionRecord,
  MutationContext,
  TimelineEntityRef,
} from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

function mutationFor(optimisticVersion: number): MutationContext {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

export const useCommentsStore = defineStore('comments', {
  state: () => ({
    current: null as CommentRecord | null,
    revisions: [] as CommentRevisionRecord[],
    loaded: false,
    error: null as string | null,
  }),
  actions: {
    async loadRevisions(transport: ShellTransport, commentId: number): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryCommentRevisions({
          comment_id: commentId,
        })
        this.current = response.comment
        this.revisions = response.revisions
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    async create(
      transport: ShellTransport,
      projectId: string,
      target: TimelineEntityRef,
      text: string,
    ): Promise<CommentRecord | null> {
      return this.mutate(transport, (client) =>
        client.commandCommentCreate({
          mutation: mutationFor(0),
          project_id: projectId,
          target,
          text,
        }),
      )
    },
    async edit(transport: ShellTransport, commentId: number, text: string): Promise<void> {
      if (!this.current || this.current.id !== commentId) {
        throw new Error(`comment ${commentId} is not loaded`)
      }
      await this.mutate(transport, (client) =>
        client.commandCommentEdit({
          mutation: mutationFor(this.current!.version),
          comment_id: commentId,
          text,
        }),
      )
    },
    async mutate(
      transport: ShellTransport,
      command: (client: KanbanClient) => Promise<CommentRecord>,
    ): Promise<CommentRecord | null> {
      try {
        const record = await command(new KanbanClient(transport))
        this.current = record
        this.error = null
        await this.loadRevisions(transport, record.id)
        return record
      } catch (failure) {
        this.error = asApiError(failure).message
        return null
      }
    },
  },
})
