import { describe, expect, it } from 'vitest'
import { harness, ticket } from './shell-harness'

const save = (optimisticVersion: number, extra: Record<string, unknown> = {}) => ({
  mutation: { optimistic_version: optimisticVersion, idempotency_key: 'save-1' },
  identifier: 'CORE-12',
  ...extra,
})

describe('the shell double issue save', () => {
  it('refuses a save at a version the issue has moved past', async () => {
    const shell = harness({ tickets: [ticket({ id: 7, number: 12, version: 5 })] })

    await expect(
      shell.transport.command('issue.save', save(4, { description: 'Stale words' })),
    ).rejects.toEqual({
      code: 'stale_version',
      message: 'optimistic version 4 is stale; the aggregate is at version 5',
      current_version: 5,
    })
    expect(shell.tickets[0]?.version).toBe(5)
  })

  it('admits a save at the version the issue stands at', async () => {
    const shell = harness({ tickets: [ticket({ id: 7, number: 12, version: 5 })] })

    await expect(shell.transport.command('issue.save', save(5, { state: 'Active' }))).resolves.toEqual({
      identifier: 'CORE-12',
      ticket_id: 7,
      url: 'kanban://issue/CORE-12',
    })
    expect(shell.tickets[0]).toMatchObject({ state: 'active', version: 6 })
  })

  it('refuses a creation that names a version other than zero', async () => {
    const shell = harness({ tickets: [] })

    await expect(
      shell.transport.command('issue.save', {
        mutation: { optimistic_version: 2, idempotency_key: 'create-1' },
        project_id: 1,
        title: 'Archive the newer exports',
      }),
    ).rejects.toEqual({
      code: 'stale_version',
      message: 'optimistic version 2 is stale; the aggregate is at version 0',
      current_version: 0,
    })
    expect(shell.tickets).toEqual([])
  })
})
