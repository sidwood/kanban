import { describe, expect, it } from 'vitest'
import { legacyStateToWorkflowName, workflowMoveTargets } from './issue-workflow'

describe('issue workflow helpers', () => {
  it('offers every default state except the current one from Draft', () => {
    const targets = workflowMoveTargets('draft', [])
    expect(targets).not.toContain('Draft')
    expect(targets).toContain('Done')
    expect(targets).toContain('Active')
    expect(targets.length).toBe(12)
  })

  it('maps legacy states to workflow names for moves', () => {
    expect(legacyStateToWorkflowName('in_review')).toBe('In Review')
  })
})
