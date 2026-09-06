// The chip presentation model: the closed, versioned vocabulary the
// generated contracts carry, resolved against a real Ticket and the
// facts the board holds beside it (KAN-T26-AC2, KAN-T26-AC3).
import { describe, expect, it } from 'vitest'
import type {
  LaneRecord,
  TicketBugQualification,
  TicketRecord,
  TicketReadinessBlocker,
} from '@kanban/contracts'
import { CHIP_VOCABULARY } from '@kanban/contracts'
import {
  chipsFor,
  laneFor,
  type CardChip,
  type ChipSources,
} from './board-chips'

const qualification = (overrides: Partial<TicketBugQualification> = {}): TicketBugQualification => ({
  affected_scope: 'The clone guard',
  criteria: [
    { outcome: 'A dirty tree is refused.', stories: ['CORE-S6-US2'] },
    { outcome: 'The refusal is recorded.', stories: ['CORE-S6-US2'] },
  ],
  environment: 'macOS 15',
  expected_behaviour: 'A dirty tree is refused.',
  frequency: 'Always',
  reproduction: 'Claim a dirty clone.',
  risk: 'Landing over uncommitted work.',
  severity: 'high',
  verification_steps: [{ command: 'git status' }],
  ...overrides,
})

const ticket = (overrides: Partial<TicketRecord> = {}): TicketRecord => ({
  id: 7,
  project_id: 1,
  number: 12,
  kind: 'implementation',
  priority: 'normal',
  state: 'ready',
  spec_id: 4,
  title: null,
  slice: 'Serve the lifecycle command surface',
  criteria: [
    { outcome: 'The commands are served.', stories: ['CORE-S4-US2'] },
    { outcome: 'The client drives them.', stories: ['CORE-S4-US2'] },
    { outcome: 'The core refuses misuse.', stories: ['CORE-S4-US3'] },
  ],
  bug: null,
  subtype: null,
  mode: null,
  completion: [],
  scheduled_for: null,
  due: null,
  profile: 'glm-implementer',
  version: 3,
  ...overrides,
})

const sources = (overrides: Partial<ChipSources> = {}): ChipSources => ({
  projectCode: 'KAN',
  lane: null,
  blockers: [],
  reviewers: [],
  execution: null,
  ...overrides,
})

const lane = (overrides: Partial<LaneRecord> = {}): LaneRecord => ({
  id: 3,
  project_id: 1,
  workspace_id: 11,
  ticket_id: 7,
  version: 2,
  ...overrides,
})

const waiting = (from_number: number): TicketReadinessBlocker => ({
  Ticket: {
    from_number,
    from_project_id: 1,
    from_state: 'active',
    from_ticket_id: from_number,
  },
})

const kindsOf = (chips: readonly CardChip[]): readonly string[] =>
  chips.map((chip) => chip.kind)

const chipByKind = (chips: readonly CardChip[], kind: string): CardChip | undefined =>
  chips.find((chip) => chip.kind === kind)

describe('board chips', () => {
  it('carries the vocabulary the generated contracts pin', () => {
    // The board never keeps a list of its own: the chip set comes
    // from the schema's closed, versioned vocabulary (KAN-T26-AC4).
    expect(CHIP_VOCABULARY.version).toBeGreaterThan(0)
    expect(CHIP_VOCABULARY.sets.map((set) => set.kind)).toEqual([
      'implementation',
      'bug',
      'task',
    ])
  })

  it('gives every card the priority and progress chips', () => {
    for (const kind of ['implementation', 'bug', 'task'] as const) {
      const chips = chipsFor(
        ticket({
          kind,
          ...(kind === 'bug'
            ? {
                bug: {
                  actual_behaviour: 'The guard lands a dirty tree.',
                  evidence_ids: [],
                  external_references: [],
                  occurrence_snapshots: [],
                  qualification: qualification(),
                  reporter_evidence: 'A landing run failed',
                },
              }
            : {}),
          ...(kind === 'task'
            ? {
                title: 'Archive the old exports',
                spec_id: null,
                subtype: 'operational',
                mode: 'human',
                completion: ['The old exports are archived.'],
                criteria: [],
                profile: null,
              }
            : {}),
        }),
        sources(),
      )

      const kinds = kindsOf(chips)
      expect(kinds[0]).toBe('priority')
      expect(kinds).toContain('progress')
      expect(chipByKind(chips, 'priority')).toMatchObject({
        label: 'Priority',
        value: 'Normal',
      })
    }
  })

  it('resolves the progress chip per kind', () => {
    const implementation = chipsFor(ticket(), sources())
    expect(chipByKind(implementation, 'progress')).toMatchObject({
      label: 'Progress',
      value: '3 criteria',
    })

    const bug = chipsFor(
      ticket({
        kind: 'bug',
        title: 'Clone guard misses a dirty tree',
        criteria: [],
        bug: {
          actual_behaviour: 'The guard lands a dirty tree.',
          evidence_ids: [],
          external_references: [],
          occurrence_snapshots: [],
          qualification: qualification({
            criteria: [{ outcome: 'A dirty tree is refused.', stories: ['CORE-S6-US2'] }],
          }),
          reporter_evidence: 'A landing run failed',
        },
      }),
      sources(),
    )
    expect(chipByKind(bug, 'progress')).toMatchObject({ value: '1 criteria' })

    // An unqualified Bug carries no progress yet.
    const unqualified = chipsFor(
      ticket({
        kind: 'bug',
        title: 'Clone guard misses a dirty tree',
        criteria: [],
        bug: {
          actual_behaviour: 'The guard lands a dirty tree.',
          evidence_ids: [],
          external_references: [],
          occurrence_snapshots: [],
          qualification: null,
          reporter_evidence: 'A landing run failed',
        },
      }),
      sources(),
    )
    expect(chipByKind(unqualified, 'progress')).toBeUndefined()

    const task = chipsFor(
      ticket({
        kind: 'task',
        title: 'Archive the old exports',
        spec_id: null,
        subtype: 'operational',
        mode: 'human',
        completion: ['The old exports are archived.', 'The archive is readable.'],
        criteria: [],
        profile: null,
      }),
      sources(),
    )
    expect(chipByKind(task, 'progress')).toMatchObject({ value: '2 outcomes' })
  })

  it('adds the implementation chips: spec, implementer, reviewers, lane, blockers', () => {
    const chips = chipsFor(
      ticket(),
      sources({
        lane: lane(),
        blockers: [waiting(3), waiting(5)],
        reviewers: ['opus-max', 'sonnet-stage'],
      }),
    )

    expect(kindsOf(chips)).toEqual(
      CHIP_VOCABULARY.sets[0].chips,
    )
    expect(chipByKind(chips, 'spec')).toMatchObject({ label: 'Spec', value: 'KAN-S4' })
    expect(chipByKind(chips, 'implementer')).toMatchObject({
      label: 'Implementer',
      value: 'glm-implementer',
    })
    expect(chipByKind(chips, 'reviewers')).toMatchObject({
      value: 'opus-max, sonnet-stage',
    })
    expect(chipByKind(chips, 'lane')).toMatchObject({ label: 'Lane', value: 'Lane 3' })
    expect(chipByKind(chips, 'blockers')).toMatchObject({
      label: 'Blockers',
      value: '2 blockers',
    })
  })

  it('omits the absent optionals: spec, lane, blockers, reviewers, profile', () => {
    const chips = chipsFor(ticket({ spec_id: null, profile: null }), sources())

    expect(kindsOf(chips)).toEqual(['priority', 'progress'])
  })

  it('adds the bug chips: severity, frequency, origin, and optional spec', () => {
    const bug = {
      actual_behaviour: 'The guard lands a dirty tree.',
      evidence_ids: [],
      external_references: [],
      occurrence_snapshots: [],
      qualification: qualification({ severity: 'critical', frequency: 'Intermittent' }),
      reporter_evidence: 'A landing run failed',
    }

    const chips = chipsFor(
      ticket({ kind: 'bug', title: 'Clone guard misses a dirty tree', criteria: [], bug }),
      sources({ projectCode: 'KAN', blockers: [waiting(3)] }),
    )

    expect(kindsOf(chips)).toEqual(CHIP_VOCABULARY.sets[1].chips)
    expect(chipByKind(chips, 'spec')).toMatchObject({ value: 'KAN-S4' })
    expect(chipByKind(chips, 'severity')).toMatchObject({
      label: 'Severity',
      value: 'Critical',
      tone: 'critical',
    })
    expect(chipByKind(chips, 'frequency')).toMatchObject({
      label: 'Frequency',
      value: 'Intermittent',
    })
    expect(chipByKind(chips, 'origin')).toMatchObject({
      label: 'Origin',
      value: 'A landing run failed',
    })

    // The Bug's Spec is optional.
    const standalone = chipsFor(
      ticket({ kind: 'bug', title: 'Clone guard misses a dirty tree', criteria: [], spec_id: null, bug }),
      sources(),
    )
    expect(chipByKind(standalone, 'spec')).toBeUndefined()
  })

  it('adds the task chips: subtype, mode, schedule, and executor', () => {
    const chips = chipsFor(
      ticket({
        kind: 'task',
        title: 'Archive the old exports',
        spec_id: null,
        subtype: 'operational',
        mode: 'human',
        completion: ['The old exports are archived.'],
        criteria: [],
        scheduled_for: '2026-09-12T09:00:00Z',
        profile: null,
      }),
      sources({ blockers: [waiting(3)] }),
    )

    expect(kindsOf(chips)).toEqual(CHIP_VOCABULARY.sets[2].chips)
    expect(chipByKind(chips, 'subtype')).toMatchObject({
      label: 'Subtype',
      value: 'Operational',
    })
    expect(chipByKind(chips, 'mode')).toMatchObject({ label: 'Mode', value: 'Human' })
    expect(chipByKind(chips, 'schedule')).toMatchObject({
      label: 'Scheduled',
      value: '2026-09-12',
    })
    expect(chipByKind(chips, 'executor')).toMatchObject({
      label: 'Executor',
      value: 'Operator',
    })

    // A due date names itself; an agent Task executes its profile.
    const agent = chipsFor(
      ticket({
        kind: 'task',
        title: 'Archive the old exports',
        spec_id: null,
        subtype: 'operational',
        mode: 'agent',
        completion: ['The old exports are archived.'],
        criteria: [],
        due: '2026-10-01T00:00:00Z',
        profile: 'glm-implementer',
      }),
      sources(),
    )
    expect(chipByKind(agent, 'schedule')).toMatchObject({
      label: 'Due',
      value: '2026-10-01',
    })
    expect(chipByKind(agent, 'executor')).toMatchObject({
      value: 'glm-implementer',
    })

    // With neither timing, no schedule chip.
    const undated = chipsFor(
      ticket({
        kind: 'task',
        title: 'Archive the old exports',
        spec_id: null,
        subtype: 'operational',
        mode: 'human',
        completion: [],
        criteria: [],
        profile: null,
      }),
      sources(),
    )
    expect(chipByKind(undated, 'schedule')).toBeUndefined()
  })

  it('keeps one kind of chip off another kind of card', () => {
    // A Bug body on a Task card renders nothing: the vocabulary, not
    // the fields present, fixes the chip set.
    const chips = chipsFor(
      ticket({
        kind: 'task',
        title: 'Archive the old exports',
        spec_id: null,
        subtype: 'operational',
        mode: 'human',
        completion: [],
        criteria: [],
        profile: null,
        bug: {
          actual_behaviour: 'The guard lands a dirty tree.',
          evidence_ids: [],
          external_references: [],
          occurrence_snapshots: [],
          qualification: qualification(),
          reporter_evidence: 'A landing run failed',
        },
      }),
      sources(),
    )

    expect(kindsOf(chips)).not.toContain('severity')
    expect(kindsOf(chips)).not.toContain('origin')
  })

  it('collapses more than two reviewers to +N', () => {
    const four = chipsFor(
      ticket(),
      sources({ reviewers: ['opus-max', 'sonnet-stage', 'glm-reviewer', 'haiku-check'] }),
    )
    expect(chipByKind(four, 'reviewers')).toMatchObject({
      value: 'opus-max, sonnet-stage +2',
      detail: 'opus-max, sonnet-stage, glm-reviewer, haiku-check',
    })

    const two = chipsFor(ticket(), sources({ reviewers: ['opus-max', 'sonnet-stage'] }))
    expect(chipByKind(two, 'reviewers')).toMatchObject({
      value: 'opus-max, sonnet-stage',
    })
    expect(chipByKind(two, 'reviewers')?.detail).toBeUndefined()

    const none = chipsFor(ticket(), sources())
    expect(chipByKind(none, 'reviewers')).toBeUndefined()
  })

  it('shows the planned profile before dispatch', () => {
    const planned = chipsFor(ticket({ state: 'ready', profile: 'glm-implementer' }), sources())
    const implementer = chipByKind(planned, 'implementer')

    expect(implementer).toMatchObject({ value: 'glm-implementer' })
    expect(implementer?.fallback).toBeUndefined()

    // With no assignment, there is no profile to show.
    const unassigned = chipsFor(ticket({ profile: null }), sources())
    expect(chipByKind(unassigned, 'implementer')).toBeUndefined()
  })

  it('shows the effective profile with a fallback indicator during execution', () => {
    const fellBack = chipsFor(
      ticket({ state: 'active', profile: 'glm-implementer' }),
      sources({ execution: { effective: 'glm-fallback', fallback: true } }),
    )
    expect(chipByKind(fellBack, 'implementer')).toMatchObject({
      value: 'glm-fallback',
      fallback: true,
    })
    expect(chipByKind(fellBack, 'implementer')?.detail).toContain('glm-implementer')

    const held = chipsFor(
      ticket({ state: 'active', profile: 'glm-implementer' }),
      sources({ execution: { effective: 'glm-implementer', fallback: false } }),
    )
    expect(chipByKind(held, 'implementer')).toMatchObject({ value: 'glm-implementer' })
    expect(chipByKind(held, 'implementer')?.fallback).toBeUndefined()

    // Until KAN-S9 lands the run snapshot, an executing Ticket still
    // names only its planned profile.
    const withoutRun = chipsFor(ticket({ state: 'active' }), sources())
    expect(chipByKind(withoutRun, 'implementer')).toMatchObject({
      value: 'glm-implementer',
    })
  })

  it('finds the Lane holding a Ticket', () => {
    const lanes = [lane({ id: 2, ticket_id: null }), lane()]

    expect(laneFor(lanes, 7)?.id).toBe(3)
    expect(laneFor(lanes, 8)).toBeUndefined()
  })
})
