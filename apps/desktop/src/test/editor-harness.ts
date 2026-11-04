// The transport the Ticket dialog specs mount over: a steerable
// answer per operation name, every query and command recorded in
// issue order. Nothing here is production code.
import type {
  IssueRecord,
  ProjectRecord,
  SpecRecord,
  TicketPriority,
  TicketRecord,
} from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'

export const editorProject: ProjectRecord = {
  id: 4,
  code: 'CORE',
  name: 'Control plane',
  repository: '/repositories/kanban',
  seed_workspace: '/workspaces/kanban.seed',
  default_branch: 'main',
  herdr_session: 'kanban-main',
  herdr_workspace: 'kanban.seed',
  initiative_id: null,
  archived: false,
  counters: { plan: 2, spec: 3, ticket: 3 },
  version: 1,
}

export const otherProject: ProjectRecord = {
  ...editorProject,
  id: 5,
  code: 'EDGE',
  name: 'Edge tooling',
}

export const editorSpec: SpecRecord = {
  id: 7,
  project_id: 4,
  number: 1,
  name: 'Registration',
  execution: 'planned',
  plan_id: null,
  version: 3,
}

/** One quick-captured Bug, no qualification yet. */
export function capturedBug(overrides: Partial<TicketRecord> = {}): TicketRecord {
  return {
    id: 2,
    project_id: 4,
    number: 18,
    kind: 'bug',
    priority: 'urgent',
    state: 'draft',
    spec_id: null,
    title: 'Landing drops the integration branch',
    slice: null,
    criteria: [],
    bug: {
      actual_behaviour: 'The integration branch is dropped after a review lands.',
      reporter_evidence: 'The landing log names the drop immediately after the merge.',
      external_references: [],
      occurrence_snapshots: [],
      evidence_ids: [],
    },
    subtype: null,
    mode: null,
    completion: [],
    scheduled_for: null,
    due: null,
    profile: null,
    version: 1,
    ...overrides,
  }
}

/** The same Bug once a whole qualification stands on it. */
export function qualifiedBug(overrides: Partial<TicketRecord> = {}): TicketRecord {
  const captured = capturedBug()
  return {
    ...captured,
    id: 19,
    number: 19,
    bug: {
      ...captured.bug!,
      qualification: {
        expected_behaviour: 'The integration branch survives every landing.',
        reproduction: 'Re land a reviewed change; the branch list still names it.',
        environment: 'macOS 26, Kanban 0.1.0.',
        severity: 'high',
        frequency: 'Every landing so far.',
        affected_scope: 'All landing reviews.',
        risk: 'Duplicate landings and lost review state.',
        criteria: [
          { outcome: 'The integration branch survives a landing.', stories: ['CORE-S1-US1'] },
        ],
        verification_steps: [{ command: 'cargo test -p kanban-storage tickets' }],
      },
      external_references: [{ uri: 'https://example.invalid/issues/12', label: 'The report' }],
      occurrence_snapshots: [
        { observed_at: '2026-09-05T07:41:00Z', observation: 'The log shows the drop.' },
      ],
      evidence_ids: [3],
    },
    version: 3,
    ...overrides,
  }
}

const LINEAR_PRIORITY: Record<TicketPriority, number> = {
  urgent: 1,
  high: 2,
  normal: 3,
  low: 4,
}

function issueIdentifier(project: ProjectRecord, ticket: TicketRecord): string {
  return `${project.code}-${ticket.number}`
}

function ticketByIdentifier(
  identifier: string,
  projects: readonly ProjectRecord[],
  tickets: readonly TicketRecord[],
): { project: ProjectRecord; ticket: TicketRecord } | undefined {
  for (const ticket of tickets) {
    const project = projects.find((entry) => entry.id === ticket.project_id)
    if (project && issueIdentifier(project, ticket) === identifier) {
      return { project, ticket }
    }
  }
  return undefined
}

function ticketToIssue(project: ProjectRecord, ticket: TicketRecord): IssueRecord {
  const identifier = issueIdentifier(project, ticket)
  return {
    identifier,
    ticket_id: ticket.id,
    project_id: ticket.project_id,
    title: ticket.title ?? '',
    description: ticket.slice ?? '',
    priority: LINEAR_PRIORITY[ticket.priority],
    state: 'Draft',
    state_type: 'unstarted',
    labels: [],
    relations: [],
    state_history: [],
    due_date: ticket.due,
    milestone_id: null,
    parent: null,
    children: [],
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    url: `https://kanban.invalid/${identifier}`,
    version: ticket.version,
  }
}

export function implementationTicket(overrides: Partial<TicketRecord> = {}): TicketRecord {
  return {
    id: 1,
    project_id: 4,
    number: 17,
    kind: 'implementation',
    priority: 'high',
    state: 'draft',
    spec_id: 7,
    title: 'Spec authoring creates content versions end to end',
    slice: 'Acceptance criteria and story links live in the description.',
    criteria: [{ outcome: 'Specs mint unique numbers.', stories: ['CORE-S1-US1'] }],
    bug: null,
    subtype: null,
    mode: null,
    completion: [],
    scheduled_for: null,
    due: null,
    profile: null,
    version: 1,
    ...overrides,
  }
}

export interface EditorHarnessOptions {
  projects?: readonly ProjectRecord[]
  specs?: readonly SpecRecord[]
  tickets?: readonly TicketRecord[]
  /** Answer one operation by hand; `undefined` falls through. */
  answer?: (name: string, request: unknown) => Promise<unknown> | undefined
}

export interface EditorHarness {
  transport: ShellTransport
  operations: Array<{ kind: 'query' | 'command'; name: string; request: unknown }>
  /** The Tickets the core holds; a command replaces the record it
   * lands, so a fresh read sees what was saved. */
  tickets: TicketRecord[]
  requests: (name: string) => unknown[]
}

export function editorHarness(options: EditorHarnessOptions = {}): EditorHarness {
  const operations: EditorHarness['operations'] = []
  const projects = options.projects ?? [editorProject]
  const specs = options.specs ?? [editorSpec]
  const tickets: TicketRecord[] = [...(options.tickets ?? [])]

  function held(ticketId: number): TicketRecord | undefined {
    return tickets.find((entry) => entry.id === ticketId)
  }

  function replace(landed: TicketRecord): TicketRecord {
    const position = tickets.findIndex((entry) => entry.id === landed.id)
    if (position >= 0) tickets.splice(position, 1, landed)
    else tickets.push(landed)
    return landed
  }

  const query = (name: string, request: unknown): Promise<unknown> => {
    operations.push({ kind: 'query', name, request })
    const answered = options.answer?.(name, request)
    if (answered !== undefined) return answered
    switch (name) {
      case 'project.list':
        return Promise.resolve({ projects: [...projects] })
      case 'spec.list': {
        const { project_id } = request as { project_id: number }
        return Promise.resolve({ specs: specs.filter((spec) => spec.project_id === project_id) })
      }
      case 'ticket.list': {
        const { project_id } = request as { project_id: number }
        return Promise.resolve({
          tickets: tickets.filter((entry) => entry.project_id === project_id),
        })
      }
      case 'ticket.get': {
        const { ticket_id } = request as { ticket_id: number }
        const found = held(ticket_id)
        return found
          ? Promise.resolve({ ...found })
          : Promise.reject({ code: 'not_found', message: `ticket ${ticket_id}` })
      }
      case 'issue.get': {
        const { identifier } = request as { identifier: string }
        const match = ticketByIdentifier(identifier, projects, tickets)
        return match
          ? Promise.resolve(ticketToIssue(match.project, match.ticket))
          : Promise.reject({ code: 'not_found', message: `issue ${identifier}` })
      }
      default:
        return Promise.resolve({})
    }
  }

  const command = (name: string, request: unknown): Promise<unknown> => {
    operations.push({ kind: 'command', name, request })
    const answered = options.answer?.(name, request)
    if (answered !== undefined) return answered
    const body = request as {
      ticket_id?: number
      project_id?: number
      kind?: TicketRecord['kind']
      priority?: TicketRecord['priority']
      title?: string
      slice?: string
      spec_id?: number
      actual_behaviour?: string
      reporter_evidence?: string
      subtype?: TicketRecord['subtype']
      mode?: TicketRecord['mode']
      completion?: string[]
      criteria?: TicketRecord['criteria']
      qualification?: NonNullable<TicketRecord['bug']>['qualification']
      external_references?: NonNullable<TicketRecord['bug']>['external_references']
      occurrence_snapshots?: NonNullable<TicketRecord['bug']>['occurrence_snapshots']
      evidence_ids?: number[]
    }
    switch (name) {
      case 'issue.save': {
        const save = body as {
          identifier?: string | null
          mutation?: { optimistic_version?: number }
          project_id?: number
          title?: string
          description?: string
          priority?: number
          state?: string
          due_date?: string | null
        }
        if (save.identifier) {
          const match = ticketByIdentifier(save.identifier, projects, tickets)
          if (!match) {
            return Promise.reject({ code: 'not_found', message: 'issue' })
          }
          const { ticket: standing } = match
          const expected = save.mutation?.optimistic_version
          if (expected !== undefined && expected !== standing.version) {
            return Promise.reject({ code: 'stale_version', message: 'the Ticket moved on' })
          }
          const priority =
            save.priority === undefined
              ? standing.priority
              : (Object.entries(LINEAR_PRIORITY).find(([, value]) => value === save.priority)?.[0] ??
                  standing.priority) as TicketPriority
          const updated = replace({
            ...standing,
            title: save.title ?? standing.title,
            slice: save.description ?? standing.slice,
            priority,
            due: save.due_date === null ? null : (save.due_date ?? standing.due),
            version: standing.version + 1,
          })
          return Promise.resolve({
            identifier: save.identifier,
            ticket_id: updated.id,
            url: `https://kanban.invalid/${save.identifier}`,
          })
        }
        const project_id = save.project_id ?? 4
        const project = projects.find((entry) => entry.id === project_id)
        void project
        const created: TicketRecord = {
          id: 100 + tickets.length,
          project_id,
          number: 30 + tickets.length,
          kind: 'task',
          priority: 'normal',
          state: 'draft',
          spec_id: null,
          title: save.title ?? null,
          slice: save.description ?? null,
          criteria: [],
          bug: null,
          subtype: null,
          mode: null,
          completion: [],
          scheduled_for: null,
          due: save.due_date ?? null,
          profile: null,
          version: 1,
        }
        return Promise.resolve(replace(created))
      }
      case 'ticket.create': {
        const created: TicketRecord = {
          id: 100 + tickets.length,
          project_id: body.project_id ?? 4,
          number: 30 + tickets.length,
          kind: body.kind ?? 'bug',
          priority: body.priority ?? 'normal',
          state: 'draft',
          spec_id: body.spec_id ?? null,
          title: body.title ?? null,
          slice: body.slice ?? null,
          criteria: body.criteria ?? [],
          bug:
            body.kind === 'bug'
              ? {
                  actual_behaviour: body.actual_behaviour ?? '',
                  reporter_evidence: body.reporter_evidence ?? '',
                  external_references: [],
                  occurrence_snapshots: [],
                  evidence_ids: [],
                }
              : null,
          subtype: body.subtype ?? null,
          mode: body.mode ?? null,
          completion: body.completion ?? [],
          scheduled_for: null,
          due: null,
          profile: null,
          version: 1,
        }
        return Promise.resolve(replace(created))
      }
      case 'ticket.edit': {
        const standing = held(body.ticket_id ?? -1)
        if (!standing) return Promise.reject({ code: 'not_found', message: 'ticket' })
        return Promise.resolve(
          replace({
            ...standing,
            ...(body.title !== undefined ? { title: body.title } : {}),
            ...(body.slice !== undefined ? { slice: body.slice } : {}),
            version: standing.version + 1,
          }),
        )
      }
      case 'ticket.bug.qualify': {
        const standing = held(body.ticket_id ?? -1)
        if (!standing?.bug) return Promise.reject({ code: 'not_found', message: 'bug' })
        return Promise.resolve(
          replace({
            ...standing,
            bug: { ...standing.bug, qualification: body.qualification },
            version: standing.version + 1,
          }),
        )
      }
      case 'ticket.bug.facts': {
        const standing = held(body.ticket_id ?? -1)
        if (!standing?.bug) return Promise.reject({ code: 'not_found', message: 'bug' })
        return Promise.resolve(
          replace({
            ...standing,
            bug: {
              ...standing.bug,
              external_references: body.external_references ?? [],
              occurrence_snapshots: body.occurrence_snapshots ?? [],
              evidence_ids: body.evidence_ids ?? [],
            },
            version: standing.version + 1,
          }),
        )
      }
      default:
        return Promise.resolve({})
    }
  }

  const transport = {
    query,
    command,
    subscribe: () => () => undefined,
    onConnectionChange: () => () => undefined,
  } as unknown as ShellTransport

  return {
    transport,
    operations,
    tickets,
    requests: (name: string) =>
      operations.filter((entry) => entry.name === name).map((entry) => entry.request),
  }
}
