import { describe, expect, it } from 'vitest'
import { harness, ticket } from './shell-harness'

const save = (optimisticVersion: number, extra: Record<string, unknown> = {}) => ({
  mutation: { optimistic_version: optimisticVersion, idempotency_key: 'save-1' },
  identifier: 'CORE-12',
  ...extra,
})

const sharingNumber = () => [
  ticket({ id: 7, project_id: 1, number: 12, version: 3 }),
  ticket({ id: 8, project_id: 2, number: 12, version: 7 }),
]

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

  it('refuses a stale save to an issue whose number another project shares', async () => {
    const shell = harness({ tickets: sharingNumber() })

    await expect(
      shell.transport.command('issue.save', save(3, { identifier: 'EDGE-12', description: 'Stale words' })),
    ).rejects.toEqual({
      code: 'stale_version',
      message: 'optimistic version 3 is stale; the aggregate is at version 7',
      current_version: 7,
    })
    expect(shell.tickets).toEqual(sharingNumber())
  })

  it('admits a save to an issue whose number another project shares', async () => {
    const shell = harness({ tickets: sharingNumber() })

    await expect(
      shell.transport.command('issue.save', save(7, { identifier: 'EDGE-12', state: 'Active' })),
    ).resolves.toEqual({
      identifier: 'EDGE-12',
      ticket_id: 8,
      url: 'kanban://issue/EDGE-12',
    })
    expect(shell.tickets).toEqual([
      ticket({ id: 7, project_id: 1, number: 12, version: 3 }),
      ticket({ id: 8, project_id: 2, number: 12, state: 'active', version: 8 }),
    ])
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
