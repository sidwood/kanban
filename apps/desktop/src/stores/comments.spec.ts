import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type {
  CommentRecord,
  CommentRevisionsResponse,
  TimelineEntityRef,
} from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useCommentsStore } from './comments'

function record(overrides: Partial<CommentRecord> = {}): CommentRecord {
  return {
    id: 1,
    project_id: 'kan',
    target: { kind: 'ticket', id: 'kan-t11' },
    text: 'First thought',
    version: 1,
    ...overrides,
  }
}

function target(): TimelineEntityRef {
  return { kind: 'ticket', id: 'kan-t11' }
}

function harness() {
  const operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }> = []
  const query = vi.fn()
  const command = vi.fn()
  const transport = {
    query: (name: string, request: unknown) => {
      operations.push({ kind: 'query', name, request })
      return query(name, request)
    },
    command: (name: string, request: unknown) => {
      operations.push({ kind: 'command', name, request })
      return command(name, request)
    },
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, operations, query, command }
}

describe('comments store', () => {
  it('loadRevisions resolves current text from the latest revision', async () => {
    setActivePinia(createPinia())
    const { transport, query } = harness()
    query.mockResolvedValue({
      comment: record({ text: 'Latest thought', version: 3 }),
      revisions: [
        { revision: 1, text: 'First thought', recorded_at: '2026-09-04T12:00:01Z' },
        { revision: 2, text: 'Second thought', recorded_at: '2026-09-04T12:00:02Z' },
        { revision: 3, text: 'Latest thought', recorded_at: '2026-09-04T12:00:03Z' },
      ],
    } satisfies CommentRevisionsResponse)
    const comments = useCommentsStore()

    await comments.loadRevisions(transport, 1)

    expect(comments.current?.text).toBe('Latest thought')
    expect(comments.revisions).toHaveLength(3)
    expect(comments.revisions[0]?.text).toBe('First thought')
    expect(comments.error).toBeNull()
  })

  it('creating sends version zero and a fresh idempotency key', async () => {
    setActivePinia(createPinia())
    const { transport, operations, command, query } = harness()
    command.mockResolvedValue(record())
    query.mockResolvedValue({
      comment: record(),
      revisions: [{ revision: 1, text: 'First thought', recorded_at: '2026-09-04T12:00:01Z' }],
    } satisfies CommentRevisionsResponse)
    const comments = useCommentsStore()

    await comments.create(transport, 'kan', target(), 'First thought')

    const create = operations.find((entry) => entry.name === 'comment.create')
    expect(create?.kind).toBe('command')
    const request = create?.request as {
      mutation: { optimistic_version: number; idempotency_key: string }
      project_id: string
      text: string
    }
    expect(request.project_id).toBe('kan')
    expect(request.text).toBe('First thought')
    expect(request.mutation.optimistic_version).toBe(0)
    expect(request.mutation.idempotency_key).toMatch(/[\w-]{8,}/)
    expect(comments.error).toBeNull()
  })

  it('editing carries the stored version and refreshes revision history', async () => {
    setActivePinia(createPinia())
    const { transport, operations, command, query } = harness()
    const stored = record({ version: 2, text: 'Second thought' })
    query.mockResolvedValue({
      comment: stored,
      revisions: [
        { revision: 1, text: 'First thought', recorded_at: '2026-09-04T12:00:01Z' },
        { revision: 2, text: 'Second thought', recorded_at: '2026-09-04T12:00:02Z' },
      ],
    } satisfies CommentRevisionsResponse)
    command.mockResolvedValue(record({ version: 3, text: 'Latest thought' }))
    const comments = useCommentsStore()
    await comments.loadRevisions(transport, 1)
    query.mockResolvedValue({
      comment: record({ version: 3, text: 'Latest thought' }),
      revisions: [
        { revision: 1, text: 'First thought', recorded_at: '2026-09-04T12:00:01Z' },
        { revision: 2, text: 'Second thought', recorded_at: '2026-09-04T12:00:02Z' },
        { revision: 3, text: 'Latest thought', recorded_at: '2026-09-04T12:00:03Z' },
      ],
    } satisfies CommentRevisionsResponse)

    await comments.edit(transport, 1, 'Latest thought')

    const edit = operations.find((entry) => entry.name === 'comment.edit')
    const request = edit?.request as {
      mutation: { optimistic_version: number }
      comment_id: number
      text: string
    }
    expect(request.comment_id).toBe(1)
    expect(request.text).toBe('Latest thought')
    expect(request.mutation.optimistic_version).toBe(2)
    expect(comments.current?.text).toBe('Latest thought')
    expect(comments.revisions).toHaveLength(3)
  })

  it('a refused command reports the message and keeps the loaded history', async () => {
    setActivePinia(createPinia())
    const { transport, command, query } = harness()
    query.mockResolvedValue({
      comment: record(),
      revisions: [{ revision: 1, text: 'First thought', recorded_at: '2026-09-04T12:00:01Z' }],
    } satisfies CommentRevisionsResponse)
    command.mockRejectedValue({
      code: 'invalid_request',
      message: 'comment text cannot be blank',
    })
    const comments = useCommentsStore()
    await comments.loadRevisions(transport, 1)

    await comments.edit(transport, 1, '   ')

    expect(comments.error).toBe('comment text cannot be blank')
    expect(comments.revisions).toHaveLength(1)
    expect(comments.current?.text).toBe('First thought')
  })
})
