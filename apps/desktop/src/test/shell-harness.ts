// The transport the shell and board specs mount over: every query a
// route can spend is answered with a small, steerable fixture, and
// every command is recorded. Nothing here is production code.
import { vi } from 'vitest'
import type {
  AttentionListResponse,
  BoardFilter,
  BoardFilterOptions,
  BoardGlobalCard,
  BoardGlobalResponse,
  HealthResponse,
  KanbanLiveEvent,
  ShellPreferencesRecord,
  ShellPreferencesUpdateRequest,
  ProjectRecord,
  RunRecord,
  SavedViewRecord,
  TicketReadinessResponse,
  TicketRecord,
} from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'

export const coreProject: ProjectRecord = {
  id: 1,
  code: 'CORE',
  name: 'Control plane',
  repository: '/repositories/kanban',
  seed_workspace: '/workspaces/kanban.seed',
  default_branch: 'main',
  herdr_session: 'kanban-main',
  herdr_workspace: 'kanban.seed',
  initiative_id: null,
  archived: false,
  counters: { plan: 0, spec: 0, ticket: 0 },
  version: 1,
}

export const edgeProject: ProjectRecord = {
  ...coreProject,
  id: 2,
  code: 'EDGE',
  name: 'Edge tooling',
}

export const ticket = (overrides: Partial<TicketRecord> = {}): TicketRecord => ({
  id: 7,
  project_id: 1,
  number: 12,
  kind: 'task',
  priority: 'normal',
  state: 'ready',
  spec_id: null,
  title: 'Archive the old exports',
  slice: null,
  criteria: [],
  bug: null,
  subtype: 'operational',
  mode: 'human',
  completion: ['The old exports are archived.'],
  scheduled_for: null,
  due: null,
  profile: null,
  version: 3,
  ...overrides,
})

const GROUP_OF: Record<TicketRecord['state'], BoardGlobalCard['group'] | null> = {
  draft: 'draft',
  parked: 'backlog',
  blocked: 'backlog',
  scheduled: 'backlog',
  ready: 'backlog',
  active: 'current',
  in_review: 'review',
  approved: 'staged',
  landing: 'staged',
  done: 'done',
  cancelled: null,
  superseded: null,
}

// The canonical lifecycle, as the core's own table fixes it: what a
// human drag may reach from each state.
const LEGAL_TARGETS: Record<TicketRecord['state'], TicketRecord['state'][]> = {
  draft: ['parked', 'blocked', 'scheduled', 'ready'],
  parked: ['ready'],
  blocked: ['parked', 'ready'],
  scheduled: ['parked', 'ready'],
  ready: ['parked', 'active'],
  active: ['in_review'],
  in_review: ['active', 'approved'],
  approved: ['landing'],
  landing: ['done'],
  done: [],
  cancelled: [],
  superseded: [],
}

const PRIORITY_RANK = { urgent: 0, high: 1, normal: 2, low: 3 }
const READINESS_RANK: Record<TicketRecord['state'], number> = {
  draft: 0,
  parked: 1,
  blocked: 2,
  scheduled: 3,
  ready: 4,
  active: 5,
  in_review: 6,
  approved: 7,
  landing: 8,
  done: 9,
  cancelled: -1,
  superseded: -1,
}

/** The projection the core would return: grouped by the fixed
 * mapping, terminal states left out, in the canonical order —
 * priority, then readiness, then number. */
export function projection(
  tickets: readonly TicketRecord[],
  extras: Partial<Record<number, Partial<BoardGlobalCard>>> = {},
): BoardGlobalCard[] {
  return [...tickets]
    .sort(
      (a, b) =>
        PRIORITY_RANK[a.priority] - PRIORITY_RANK[b.priority] ||
        READINESS_RANK[b.state] - READINESS_RANK[a.state] ||
        a.number - b.number,
    )
    .flatMap((entry) => {
      const group = GROUP_OF[entry.state]
      if (group === null) return []
      return [
        {
          group,
          project_code: entry.project_id === 2 ? 'EDGE' : 'CORE',
          spec_number: null,
          lane_id: null,
          ticket: entry,
          ...extras[entry.id],
        },
      ]
    })
}

export const boardOptions: BoardFilterOptions = {
  initiatives: [{ id: 1, label: 'Personal tooling' }],
  projects: [
    { id: 1, label: 'CORE — Control plane' },
    { id: 2, label: 'EDGE — Edge tooling' },
  ],
  plans: [{ id: 3, label: 'CORE-P1' }],
  specs: [{ id: 4, label: 'CORE-S9 · Serve the lifecycle command surface' }],
  lanes: [{ id: 5, label: 'CORE lane 5' }],
  profiles: ['standard', 'deep'],
  attention: ['blocker', 'stale_run'],
}

function defaultView(id: number, scope: SavedViewRecord['scope']): SavedViewRecord {
  return {
    id,
    name: 'All work',
    scope,
    filter: scope === 'global' ? {} : { projects: [scope.project] },
    expanded_groups: [],
    hidden_columns: ['draft'],
    mode: 'board',
    done_placement: 'column',
    sorting: 'priority',
    is_default: true,
    version: 1,
  }
}

export function defaultViews(): SavedViewRecord[] {
  return [
    defaultView(1, 'global'),
    defaultView(2, { project: 1 }),
    defaultView(3, { project: 2 }),
  ]
}

/** A healthy core, every component reporting. */
export function healthy(): HealthResponse {
  return {
    connected: true,
    service_version: '0.1.0',
    service: { started_at: '2026-09-13T09:00:00Z' },
    database: { journal_mode: 'wal', last_change_at: null, schema_version: 1 },
    scheduler: { last_backup_success_at: null },
    mcp: { exposed_tools: 0 },
    herdr: { connection_diagnostic: null, sessions: [] },
    workspaces: {
      by_health: { assigned: 0, available: 0, dirty: 0, missing: 0, retired: 0, unobserved: 0 },
      last_change_at: null,
    },
  }
}

export interface HarnessOptions {
  tickets?: readonly TicketRecord[]
  extras?: Partial<Record<number, Partial<BoardGlobalCard>>>
  projects?: readonly ProjectRecord[]
  views?: readonly SavedViewRecord[]
  blockers?: Record<number, TicketReadinessResponse['blocked_by']>
  runs?: readonly RunRecord[]
  attention?: AttentionListResponse['items']
  health?: HealthResponse | null
  /** The shell arrangement the core already holds. */
  preferences?: ShellPreferencesRecord
  /** Queries answered by hand instead of the fixture. */
  override?: (name: string, request: unknown) => Promise<unknown> | undefined
}

export interface Harness {
  transport: ShellTransport
  query: ReturnType<typeof vi.fn>
  command: ReturnType<typeof vi.fn>
  /** The views the harness serves; a view.update or view.create
   * updates it, so a reload sees what was saved. */
  views: SavedViewRecord[]
  /** The Tickets the core holds; a test changes this to change what
   * the authoritative projection would answer next. */
  tickets: TicketRecord[]
  /** The shell arrangement the core holds; an update replaces it, so
   * a fresh mount reads back what was written. */
  preferences: () => ShellPreferencesRecord
  connection: (state: 'connected' | 'disconnected') => void
  /** Deliver one ordered live event to every subscriber, as the
   * shell's event stream would. */
  emit: (event: KanbanLiveEvent) => void
}

/** Whether one ticket passes one wire filter, the way the core
 * would judge the axes the specs exercise. */
function admits(filter: BoardFilter, entry: TicketRecord, extras: Partial<BoardGlobalCard>): boolean {
  if (filter.projects?.length && !filter.projects.includes(entry.project_id)) return false
  if (filter.kinds?.length && !filter.kinds.includes(entry.kind)) return false
  if (filter.states?.length && !filter.states.includes(entry.state)) return false
  if (filter.priorities?.length && !filter.priorities.includes(entry.priority)) return false
  if (filter.profiles?.length && !filter.profiles.includes(entry.profile ?? '')) return false
  if (filter.lanes?.length && !filter.lanes.includes(extras.lane_id ?? -1)) return false
  if (filter.initiatives?.length) return false
  if (filter.attention?.length) return false
  if (filter.plans?.length) return false
  if (filter.specs?.length) return false
  return true
}

export function harness(options: HarnessOptions = {}): Harness {
  const tickets: TicketRecord[] = [...(options.tickets ?? [])]
  const extras = options.extras ?? {}
  const projects = options.projects ?? [coreProject, edgeProject]
  const views: SavedViewRecord[] = [...(options.views ?? defaultViews())]
  const runs = options.runs ?? []
  let preferences: ShellPreferencesRecord = options.preferences ?? {
    rail_open: true,
    collapsed_columns: [],
    version: 0,
  }
  const connectionHandlers: Array<(state: 'connected' | 'disconnected') => void> = []
  const query = vi.fn((name: string, request: unknown): Promise<unknown> => {
    const overridden = options.override?.(name, request)
    if (overridden !== undefined) return overridden
    switch (name) {
      case 'health.get':
        return options.health === null
          ? Promise.reject({ code: 'unavailable', message: 'the core is offline' })
          : Promise.resolve(options.health ?? healthy())
      case 'project.list':
        return Promise.resolve({ projects })
      case 'view.list':
        return Promise.resolve({ views: [...views] })
      case 'attention.list':
        return Promise.resolve({ items: options.attention ?? [] })
      case 'board.global': {
        const { filter } = request as { filter: BoardFilter }
        const selected = tickets.filter((entry) => admits(filter, entry, extras[entry.id] ?? {}))
        return Promise.resolve({
          cards: projection(selected, extras),
          options: boardOptions,
        } satisfies BoardGlobalResponse)
      }
      case 'ticket.readiness': {
        const { ticket_id } = request as { ticket_id: number }
        const blocked_by = options.blockers?.[ticket_id] ?? []
        return Promise.resolve({
          blocked_by,
          ready: blocked_by.length === 0,
          state: 'ready',
          ticket_id,
        } satisfies TicketReadinessResponse)
      }
      case 'run.list': {
        const { project_id } = request as { project_id: number }
        return Promise.resolve({
          project_id,
          runs: runs.filter((run) => run.project_id === project_id),
        })
      }
      case 'ticket.get': {
        const { ticket_id } = request as { ticket_id: number }
        const found = tickets.find((entry) => entry.id === ticket_id)
        return found
          ? Promise.resolve(found)
          : Promise.reject({ code: 'not_found', message: `ticket ${ticket_id}` })
      }
      case 'timeline.query':
        return Promise.resolve({ events: [] })
      case 'search.global':
        return Promise.resolve({ hits: [] })
      case 'initiative.list':
        return Promise.resolve({ initiatives: [] })
      case 'profile.list':
        return Promise.resolve({ profiles: [] })
      case 'plan.list':
        return Promise.resolve({ plans: [] })
      case 'spec.list':
        return Promise.resolve({ specs: [] })
      case 'workspace.list':
        return Promise.resolve({ workspaces: [] })
      case 'lane.list':
        return Promise.resolve({ lanes: [] })
      case 'ticket.list':
        return Promise.resolve({ tickets: [] })
      case 'ticket.review.config':
        return Promise.resolve({ config: null })
      case 'ticket.transitions': {
        const { ticket_id } = request as { ticket_id: number }
        const found = tickets.find((entry) => entry.id === ticket_id)
        if (!found) return Promise.reject({ code: 'not_found', message: `ticket ${ticket_id}` })
        // Only a Task answers a human drag; the readiness gate the
        // core also applies is not exercised by these fixtures.
        return Promise.resolve({
          ticket_id,
          state: found.state,
          targets: found.kind === 'task' ? LEGAL_TARGETS[found.state] : [],
        })
      }
      case 'criterion.bindings':
        return Promise.resolve({ bindings: [] })
      case 'shell.preferences':
        return Promise.resolve({ ...preferences })
      default:
        return Promise.resolve({})
    }
  })
  const command = vi.fn((name: string, request: unknown): Promise<unknown> => {
    const body = request as Record<string, unknown>
    if (name === 'view.update') {
      const standing = views.find((view) => view.id === body.view_id)
      if (!standing) return Promise.reject({ code: 'not_found', message: 'view' })
      const updated = { ...standing, ...body, version: standing.version + 1 } as SavedViewRecord
      delete (updated as unknown as Record<string, unknown>).mutation
      delete (updated as unknown as Record<string, unknown>).view_id
      views.splice(views.indexOf(standing), 1, updated)
      return Promise.resolve(updated)
    }
    if (name === 'view.create') {
      const created = {
        id: 21,
        is_default: false,
        version: 1,
        ...body,
      } as unknown as SavedViewRecord
      delete (created as unknown as Record<string, unknown>).mutation
      views.push(created)
      return Promise.resolve(created)
    }
    if (name === 'shell.preferences.update') {
      const update = request as ShellPreferencesUpdateRequest
      if (update.mutation.optimistic_version !== preferences.version) {
        return Promise.reject({ code: 'stale_version', message: 'the arrangement moved on' })
      }
      preferences = {
        rail_open: update.rail_open,
        collapsed_columns: update.collapsed_columns ?? [],
        version: preferences.version + 1,
      }
      return Promise.resolve({ ...preferences })
    }
    if (name === 'ticket.transition') {
      const { ticket_id, to } = body as { ticket_id: number; to: TicketRecord['state'] }
      const found = tickets.find((entry) => entry.id === ticket_id) ?? ticket()
      const moved = { ...found, state: to, version: found.version + 1 }
      // The core keeps the move, so the next projection reads it.
      const position = tickets.indexOf(found)
      if (position >= 0) tickets.splice(position, 1, moved)
      return Promise.resolve(moved)
    }
    return Promise.resolve({})
  })
  const eventHandlers: Array<(event: KanbanLiveEvent) => void> = []
  const transport = {
    query,
    command,
    subscribe: (handler: (event: KanbanLiveEvent) => void) => {
      eventHandlers.push(handler)
      return () => {
        const index = eventHandlers.indexOf(handler)
        if (index >= 0) eventHandlers.splice(index, 1)
      }
    },
    onConnectionChange: (handler: (state: 'connected' | 'disconnected') => void) => {
      connectionHandlers.push(handler)
      return () => {
        const index = connectionHandlers.indexOf(handler)
        if (index >= 0) connectionHandlers.splice(index, 1)
      }
    },
  } as unknown as ShellTransport
  return {
    transport,
    query,
    command,
    views,
    tickets,
    preferences: () => ({ ...preferences }),
    connection: (state) => {
      for (const handler of [...connectionHandlers]) handler(state)
    },
    emit: (event) => {
      for (const handler of [...eventHandlers]) handler(event)
    },
  }
}
