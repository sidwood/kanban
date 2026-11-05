import { describe, expect, it } from 'vitest'
import { editorHarness, implementationTicket } from './editor-harness'

describe('the editor double issue save', () => {
  it('keeps the saved priority on the issue and the legacy Ticket priority as it stood', async () => {
    const editor = editorHarness({ tickets: [implementationTicket({ priority: 'high' })] })

    await editor.transport.command('issue.save', {
      mutation: { optimistic_version: 1, idempotency_key: 'save-1' },
      identifier: 'CORE-17',
      priority: 4,
    })

    expect(editor.tickets[0]).toMatchObject({ priority: 'high', version: 2 })
    await expect(
      editor.transport.query('issue.get', { identifier: 'CORE-17' }),
    ).resolves.toMatchObject({ priority: 4, version: 2 })
  })

  it('reads a never-saved issue priority from the record the core migrated', async () => {
    const editor = editorHarness({ tickets: [implementationTicket({ priority: 'low' })] })

    await expect(
      editor.transport.query('issue.get', { identifier: 'CORE-17' }),
    ).resolves.toMatchObject({ priority: 4 })
  })
})
