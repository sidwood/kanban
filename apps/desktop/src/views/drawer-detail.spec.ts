// KAN-T139-AC4 (KAN-S10-US5): the drawer derives criteria revisions,
// dependencies, execution, attempts, reviews, findings, evidence, and
// the timeline from authoritative records. Nothing is invented: a
// record the core does not hold is stated as absent, never filled in.
import { flushPromises, mount } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type {
  CriterionBindingRecord,
  EvidenceRecord,
  FindingRecord,
  LaneRecord,
  ProfileSnapshotRecord,
  ReviewExecutionRecord,
  RunRecord,
  TicketRecord,
  WorkspaceRecord,
} from '@kanban/contracts'
import router from '../router'
import { kanbanTransportKey } from '../core/transport'
import { harness, ticket } from '../test/shell-harness'
import type { HarnessOptions } from '../test/shell-harness'
import BoardView from './BoardView.vue'

const implementation = (overrides: Partial<TicketRecord> = {}): TicketRecord =>
  ticket({
    id: 8,
    number: 13,
    kind: 'implementation',
    state: 'in_review',
    spec_id: 4,
    title: null,
    slice: 'Serve the lifecycle command surface',
    criteria: [
      { outcome: 'The commands are served.', stories: ['CORE-S4-US2'] },
      { outcome: 'The core refuses misuse.', stories: ['CORE-S4-US3'] },
    ],
    subtype: null,
    mode: null,
    completion: [],
    profile: 'glm-implementer',
    version: 5,
    ...overrides,
  })

const snapshot = (name: string): ProfileSnapshotRecord => ({
  name,
  harness: 'claude-code',
  model: 'opus',
  effort: 'high',
  usage_pool: 'operator',
})

const run = (overrides: Partial<RunRecord> = {}): RunRecord => ({
  id: 3,
  project_id: 1,
  ticket_id: 8,
  dispatch_request_id: 4,
  status: 'executing',
  requested: snapshot('glm-implementer'),
  effective: snapshot('glm-fallback'),
  fallback: true,
  fallback_path: ['glm-implementer', 'glm-fallback'],
  created_at: 20,
  version: 1,
  ...overrides,
})

const lane: LaneRecord = { id: 11, project_id: 1, ticket_id: 8, workspace_id: 21, version: 2 }

const workspace: WorkspaceRecord = {
  id: 21,
  project_id: 1,
  path: '/workspaces/kanban.lane-11',
  is_seed: false,
  health: 'assigned',
  observation: {
    branch: 'kan-recovery-t139',
    checkout: 'branch',
    head: 'f00dcafe',
    lane_assignment: 11,
    repository_identity: 'kanban',
    unique_unlanded_commits: false,
    working_tree_clean: true,
  },
  reuse: { clean: true, free_of_unlanded_commits: true, reusable: false, unassigned: false },
  version: 4,
}

const bindings: CriterionBindingRecord[] = [
  {
    ticket_id: 8,
    criterion_index: 0,
    kind: 'acceptance',
    evidence_id: 31,
    review: 'validated',
    satisfied: true,
    void: false,
    tip: 'f00dcafe',
  },
  {
    ticket_id: 8,
    criterion_index: 1,
    kind: 'acceptance',
    evidence_id: 32,
    review: 'pending',
    satisfied: false,
    void: true,
    tip: 'deadbeef',
  },
]

const review: ReviewExecutionRecord = {
  id: 55,
  project_id: 1,
  ticket_id: 8,
  submission_id: 9,
  configuration_version: 2,
  tip: 'f00dcafe',
  status: 'in_progress',
  version: 6,
  bounce: null,
  stages: [
    {
      index: 0,
      status: 'waiting',
      slots: [
        {
          id: 71,
          occupant: { kind: 'profile', name: 'sol-reviewer' },
          requirement: 'required',
          requested: snapshot('sol-reviewer'),
          effective: snapshot('sol-reviewer'),
          fallback_path: [],
          dispatch_request_id: 12,
          verdict: {
            approve: false,
            counts_for_resolution: true,
            summary: 'The landing path is unguarded.',
            tip: 'f00dcafe',
            submission_id: 9,
            findings: [],
          },
        },
        {
          id: 72,
          occupant: { kind: 'human' },
          requirement: 'required',
          requested: null,
          effective: null,
          fallback_path: [],
          dispatch_request_id: null,
          verdict: null,
        },
      ],
    },
  ],
}

// The prior gate the core expired: the only attempt and revalidation
// the drawer can be shown are the ones a terminal execution wrote.
const expired: ReviewExecutionRecord = {
  ...review,
  id: 54,
  status: 'expired',
  version: 3,
  stages: [{ index: 0, status: 'waiting', slots: [] }],
}

const finding: FindingRecord = {
  id: 'f-1',
  project_id: 1,
  ticket_id: 8,
  review_id: 55,
  slot_id: 71,
  submission_id: 9,
  tip: 'f00dcafe',
  blocking: true,
  counts_for_resolution: true,
  promotion: null,
  finding: {
    severity: 'p1',
    in_scope: true,
    summary: 'Landing drops the integration branch.',
    evidence: 'The landing log names the drop.',
    location: 'crates/kanban-app/src/landing.rs:88',
    proposed_resolution: 'Guard the branch before the merge.',
  },
}

const evidence: EvidenceRecord = {
  id: 31,
  project_id: 1,
  entity_kind: 'ticket',
  entity_id: '8',
  evidence_kind: 'repository',
  commit_identity: 'f00dcafe',
  content_hash: 'sha256:abc',
  relative_path: 'temp/review-ready.md',
}

function shell(options: Partial<HarnessOptions> = {}) {
  return harness({
    tickets: [implementation()],
    runs: [run()],
    ...options,
  })
}

const mountedBoards: VueWrapper[] = []

async function openDrawer(transport: ReturnType<typeof harness>['transport'], ticketId = 8) {
  await router.push('/projects/1/board')
  await router.isReady()
  const wrapper = mount(BoardView, {
    global: {
      plugins: [createPinia(), router],
      provide: { [kanbanTransportKey as symbol]: transport },
    },
    attachTo: document.body,
  })
  mountedBoards.push(wrapper)
  await flushPromises()
  await wrapper.find(`[data-testid="open-ticket-${ticketId}"]`).trigger('click')
  await flushPromises()
  return wrapper
}

const text = (testid: string): string =>
  document.querySelector(`[data-testid="${testid}"]`)?.textContent?.replace(/\s+/g, ' ').trim() ?? ''

beforeEach(() => {
  localStorage.clear()
})

afterEach(() => {
  for (const wrapper of mountedBoards.splice(0)) wrapper.unmount()
  document.body.innerHTML = ''
})

describe('the drawer detail, derived from authoritative records', () => {
  it('shows each criterion revision the core bound: tip, evidence review, and void', async () => {
    const { transport, query } = shell({ bindings: { 8: bindings } })
    await openDrawer(transport)

    expect(query).toHaveBeenCalledWith('criterion.bindings', { ticket_id: 8 })
    expect(text('drawer-criterion-revision-0')).toContain('f00dcafe')
    expect(text('drawer-criterion-revision-0')).toContain('validated')
    expect(text('drawer-criterion-revision-1')).toContain('deadbeef')
    expect(text('drawer-criterion-revision-1')).toContain('pending')
    expect(text('drawer-criterion-revision-1')).toContain('void')
  })

  it('states a criterion no revision is bound to rather than inventing one', async () => {
    const { transport } = shell({ bindings: { 8: [bindings[0]!] } })
    await openDrawer(transport)

    expect(text('drawer-criterion-revision-0')).toContain('f00dcafe')
    expect(text('drawer-criterion-revision-1')).toBe('No evidence bound yet')
  })

  it('reads dependencies and external blockers from ticket.dependencies', async () => {
    const { transport, query } = shell({
      dependencies: {
        8: {
          dependencies: [
            { from_ticket_id: 4, from_project_id: 1, from_number: 9, from_state: 'active' },
          ],
          blockers: [
            { id: 51, ticket_id: 8, description: 'The vendor has not shipped the fix.' },
          ],
        },
      },
    })
    await openDrawer(transport)

    expect(query).toHaveBeenCalledWith('ticket.dependencies', { ticket_id: 8 })
    expect(text('drawer-dependency-4')).toContain('CORE-T9')
    expect(text('drawer-dependency-4')).toContain('active')
    expect(text('drawer-blocker-51')).toContain('The vendor has not shipped the fix.')
  })

  // SOL-T139-A-06: a Project-scoped number is unique only inside its
  // Project, so a blocking Ticket in another Project is named in full.
  it('names a cross-Project dependency by the Project that owns it', async () => {
    const { transport } = shell({
      dependencies: {
        8: {
          dependencies: [
            { from_ticket_id: 4, from_project_id: 2, from_number: 9, from_state: 'active' },
            { from_ticket_id: 5, from_project_id: 1, from_number: 9, from_state: 'ready' },
          ],
          blockers: [],
        },
      },
    })
    await openDrawer(transport)

    expect(text('drawer-dependency-4')).toContain('EDGE-T9')
    expect(text('drawer-dependency-4')).not.toContain('CORE-T9')
    expect(text('drawer-dependency-5')).toContain('CORE-T9')
  })

  it('shows the planned and effective profile, the fallback, and the lane and workspace', async () => {
    const { transport, query } = shell({ lanes: [lane], workspaces: [workspace] })
    await openDrawer(transport)

    expect(query).toHaveBeenCalledWith('lane.list', { project_id: 1 })
    expect(query).toHaveBeenCalledWith('workspace.list', { project_id: 1 })
    const execution = text('drawer-execution')
    expect(execution).toContain('glm-implementer')
    expect(execution).toContain('glm-fallback')
    expect(execution).toContain('fallback')
    expect(execution).toContain('Lane 11')
    expect(execution).toContain('/workspaces/kanban.lane-11')
    expect(execution).toContain('Run 3')
  })

  it('says a Ticket with no run, lane, or workspace has none', async () => {
    const { transport } = shell({ runs: [] })
    await openDrawer(transport)

    const execution = text('drawer-execution')
    expect(execution).toContain('No run has executed this Ticket')
    expect(execution).not.toContain('Lane')
    expect(execution).not.toContain('fallback')
  })

  it('reads the review stages, their verdicts, and the revalidation the core flags', async () => {
    const { transport, query } = shell({ reviews: [expired, review] })
    await openDrawer(transport)

    expect(query).toHaveBeenCalledWith('review.history', { ticket_id: 8 })
    expect(query).toHaveBeenCalledWith('review.latest', { ticket_id: 8 })
    expect(text('drawer-review-slot-71')).toContain('sol-reviewer')
    expect(text('drawer-review-slot-71')).toContain('rejected')
    expect(text('drawer-review-slot-72')).toContain('Human')
    expect(text('drawer-review-slot-72')).toContain('no verdict')
    expect(text('drawer-review-attempt-1')).toContain('expired')
    expect(text('drawer-review-attempt-1')).toContain('gate expired')
    expect(text('drawer-review-revalidation')).toContain('revalidation')
  })

  it('lists the findings the core holds for the Ticket with their full structure', async () => {
    const { transport, query } = shell({ findings: [finding] })
    await openDrawer(transport)

    expect(query).toHaveBeenCalledWith('finding.list', { project_id: 1 })
    const shown = text('drawer-finding-f-1')
    expect(shown).toContain('p1')
    expect(shown).toContain('Landing drops the integration branch.')
    expect(shown).toContain('crates/kanban-app/src/landing.rs:88')
    expect(shown).toContain('Guard the branch before the merge.')
    expect(shown).toContain('blocking')
  })

  it('leaves out findings recorded against another Ticket', async () => {
    const { transport } = shell({
      findings: [{ ...finding, id: 'f-2', ticket_id: 99 }],
    })
    await openDrawer(transport)

    expect(document.querySelector('[data-testid="drawer-finding-f-2"]')).toBeNull()
    expect(text('drawer-findings')).toContain('No findings recorded')
  })

  it('reads the evidence attached to this Ticket alone', async () => {
    const { transport, query } = shell({
      evidence: [evidence, { ...evidence, id: 32, entity_id: '99' }],
    })
    await openDrawer(transport)

    expect(query).toHaveBeenCalledWith('evidence.list', {
      project_id: 1,
      entity_kind: 'ticket',
      entity_id: '8',
    })
    expect(text('drawer-evidence-31')).toContain('repository')
    expect(text('drawer-evidence-31')).toContain('f00dcafe')
    expect(document.querySelector('[data-testid="drawer-evidence-32"]')).toBeNull()
  })

  it('keeps the attempts and the timeline the drawer already derived', async () => {
    const { transport } = shell({
      runs: [run({ id: 1, created_at: 10, effective: snapshot('first-run') }), run()],
    })
    await openDrawer(transport)

    expect(document.querySelectorAll('[data-testid^="drawer-attempt-"]')).toHaveLength(2)
    expect(document.querySelector('[data-testid="drawer-timeline"]')).not.toBeNull()
  })

  // SOL-T139-A-05: an unanswered source knows nothing about its own
  // records and nothing at all about anyone else's.
  it('one unanswered source never turns the other sections into false absence', async () => {
    const { transport } = shell({
      findings: [finding],
      evidence: [evidence],
      reviews: [review],
      dependencies: {
        8: {
          dependencies: [
            { from_ticket_id: 4, from_project_id: 1, from_number: 9, from_state: 'active' },
          ],
          blockers: [],
        },
      },
      override: (name) =>
        name === 'finding.list'
          ? Promise.reject({ code: 'unavailable', message: 'the core is offline' })
          : undefined,
    })
    await openDrawer(transport)

    expect(text('drawer-findings')).not.toContain('No findings recorded')
    expect(text('drawer-findings-unreadable')).toContain('could not be read')
    expect(text('drawer-detail-error')).toContain('the core is offline')
    // Every other section still stands on its own authority.
    expect(text('drawer-dependency-4')).toContain('CORE-T9')
    expect(text('drawer-evidence-31')).toContain('repository')
    expect(text('drawer-review-slot-71')).toContain('sol-reviewer')
    expect(text('drawer-execution')).toContain('Run 3')
    expect(text('drawer-criterion-revision-0')).toBe('No evidence bound yet')
  })

  it('keeps the run it did read when the Lane and Workspace read fails', async () => {
    const { transport } = shell({
      lanes: [lane],
      workspaces: [workspace],
      override: (name) =>
        name === 'lane.list'
          ? Promise.reject({ code: 'unavailable', message: 'the core is offline' })
          : undefined,
    })
    await openDrawer(transport)

    const shown = text('drawer-execution')
    expect(shown).toContain('Run 3')
    expect(shown).toContain('glm-fallback')
    expect(shown).toContain('The Lane and Workspace could not be read.')
    expect(shown).not.toContain('Lane 11')
  })

  it('states no records of a section whose own authority did not answer', async () => {
    const { transport } = shell({
      reviews: [review],
      override: (name) =>
        name === 'review.latest'
          ? Promise.reject({ code: 'unavailable', message: 'the core is offline' })
          : undefined,
    })
    await openDrawer(transport)

    expect(text('drawer-reviews')).not.toContain('No review has run')
    expect(text('drawer-reviews-unreadable')).toContain('could not be read')
    expect(document.querySelector('[data-testid="drawer-review-decision"]')).toBeNull()
    expect(text('drawer-evidence')).toContain('No evidence attached')
    expect(text('drawer-findings')).toContain('No findings recorded')
    expect(text('drawer-dependencies')).toContain('Nothing holds this Ticket back')
  })

  it('never writes one Ticket detail over another: a superseded read renders nowhere', async () => {
    const second = implementation({ id: 9, number: 14, state: 'active', criteria: [] })
    const { transport } = shell({
      tickets: [implementation(), second],
      bindings: { 8: bindings, 9: [] },
    })
    const wrapper = await openDrawer(transport)

    await wrapper.find('[data-testid="open-ticket-9"]').trigger('click')
    await flushPromises()

    expect(document.querySelector('[data-testid="drawer-criterion-revision-0"]')).toBeNull()
  })
})
