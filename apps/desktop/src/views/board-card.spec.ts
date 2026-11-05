import { describe, expect, it } from 'vitest'
import { ticket } from '../test/shell-harness'
import { boardCardNumber, issueDisplayKey } from './board-card'

describe('issue display keys', () => {
  it('uses Linear-style TEAM-N keys without the legacy T prefix', () => {
    expect(issueDisplayKey('VIS', 1)).toBe('VIS-1')
    expect(boardCardNumber(ticket({ number: 12 }), 'CORE')).toBe('CORE-12')
  })
})
