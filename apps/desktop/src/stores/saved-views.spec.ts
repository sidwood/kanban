import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { SavedViewRecord, ViewListResponse, ViewScope } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useGlobalBoardStore } from './global-board'
import { ownedCopy, useSavedViewsStore } from './saved-views'

// The generated defaults: one global view and one per Project, each
// carrying the everyday perspective (DR-BP-06).
function defaults(): SavedViewRecord[] {
  const base = {
    filter: {},
    expanded_groups: [],
    hidden_columns: ['draft' as const],
    mode: 'board' as const,
    done_placement: 'column' as const,
    sorting: 'priority' as const,
    is_default: true,
    version: 1,
  }
  return [
    { ...base, id: 1, name: 'All work', scope: 'global' as const },
    { ...base, id: 2, name: 'All work', scope: { project: 1 } },
    { ...base, id: 3, name: 'All work', scope: { project: 2 } },
  ]
}

/** One named perspective away from every default, so a restoration
 * proves each owned property. */
function reviewQueue(id: number): SavedViewRecord {
  return {
    id,
    name: 'Review queue',
    scope: 'global',
    filter: {
      initiatives: [2],
      projects: [1],
      plans: [7],
      specs: [4],
      kinds: ['implementation', 'bug'],
      states: ['in_review'],
      priorities: ['urgent', 'high'],
      lanes: [5],
      profiles: ['standard'],
      attention: ['review_request', 'stale_run'],
    },
    expanded_groups: ['backlog', 'staged'],
    hidden_columns: ['draft', 'done'],
    mode: 'register',
    done_placement: 'table',
    sorting: 'readiness',
    is_default: false,
    version: 4,
  }
}

// A recording transport whose view answers the test steers.
function harness(views: SavedViewRecord[]) {
  const queries: Array<{ name: string; request: unknown }> = []
  const commands: Array<{ name: string; request: unknown }> = []
  const transport = {
    query: (name: string, request: unknown) => {
      queries.push({ name, request })
      if (name === 'view.list') {
        return Promise.resolve({ views } satisfies ViewListResponse)
      }
      return Promise.resolve({ views: [] })
    },
    command: vi.fn((name: string, request: unknown) => {
      commands.push({ name, request })
      if (name === 'view.create') {
        const body = request as { scope: ViewScope; name: string } & Record<string, unknown>
        return Promise.resolve({
          id: views.length + 10,
          name: body.name,
          scope: body.scope,
          filter: {},
          expanded_groups: [],
          hidden_columns: [],
          mode: 'board',
          done_placement: 'column',
          sorting: 'priority',
          is_default: false,
          version: 1,
        } satisfies SavedViewRecord)
      }
      const body = request as { view_id: number } & Record<string, unknown>
      const standing = views.find((view) => view.id === body.view_id) ?? views[0]
      const updated: SavedViewRecord = {
        ...standing,
        expanded_groups: (body.expanded_groups as SavedViewRecord['expanded_groups']) ?? standing.expanded_groups,
        hidden_columns: (body.hidden_columns as SavedViewRecord['hidden_columns']) ?? standing.hidden_columns,
        mode: (body.mode as SavedViewRecord['mode']) ?? standing.mode,
        done_placement: (body.done_placement as SavedViewRecord['done_placement']) ?? standing.done_placement,
        sorting: (body.sorting as SavedViewRecord['sorting']) ?? standing.sorting,
        version: standing.version + 1,
      }
      return Promise.resolve(updated)
    }),
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, queries, commands }
}

describe('saved views store', () => {
  it('loads every scope with its generated default active', async () => {
    setActivePinia(createPinia())
    const { transport, queries } = harness(defaults())
    const views = useSavedViewsStore()

    await views.refresh(transport)

    expect(queries).toEqual([{ name: 'view.list', request: {} }])
    expect(views.views).toHaveLength(3)
    expect(views.loaded).toBe(true)
    expect(views.activeGlobalView?.id).toBe(1)
    expect(views.activeProjectView(1)?.id).toBe(2)
    expect(views.activeProjectView(2)?.id).toBe(3)
  })

  it('reports a failed load without pretending to be loaded', async () => {
    setActivePinia(createPinia())
    const transport = {
      query: () => Promise.reject({ code: 'unavailable', message: 'the core is offline' }),
      command: vi.fn(),
      subscribe: () => () => undefined,
    } as unknown as ShellTransport
    const views = useSavedViewsStore()

    await views.refresh(transport)

    expect(views.error).toBe('the core is offline')
    expect(views.loaded).toBe(false)
    expect(views.views).toEqual([])
  })

  it('switching views restores every owned property exactly', async () => {
    setActivePinia(createPinia())
    const queue = reviewQueue(8)
    const { transport } = harness([...defaults(), queue])
    const views = useSavedViewsStore()
    const board = useGlobalBoardStore()
    await views.refresh(transport)

    // The default opens on the whole board.
    expect(board.filter).toEqual({
      initiatives: [],
      projects: [],
      plans: [],
      specs: [],
      kinds: [],
      states: [],
      priorities: [],
      lanes: [],
      profiles: [],
      attention: [],
    })

    expect(views.switchGlobalView(queue.id)).toBe(true)

    // Every axis of the owned filter lands exactly, none merged away.
    expect(board.filter).toEqual(queue.filter)
    // The presentation properties ride the active view, whole.
    expect(ownedCopy(views.activeGlobalView as SavedViewRecord)).toEqual({
      filter: queue.filter,
      expanded_groups: queue.expanded_groups,
      hidden_columns: queue.hidden_columns,
      mode: queue.mode,
      done_placement: queue.done_placement,
      sorting: queue.sorting,
    })

    // Switching back restores the default's properties exactly — an
    // empty filter included, so no axis of the queue survives.
    expect(views.switchGlobalView(1)).toBe(true)
    expect(board.filter).toEqual({
      initiatives: [],
      projects: [],
      plans: [],
      specs: [],
      kinds: [],
      states: [],
      priorities: [],
      lanes: [],
      profiles: [],
      attention: [],
    })
    expect(views.activeGlobalView?.expanded_groups).toEqual([])
    expect(views.activeGlobalView?.hidden_columns).toEqual(['draft'])
    expect(views.activeGlobalView?.mode).toBe('board')
    expect(views.activeGlobalView?.done_placement).toBe('column')
    expect(views.activeGlobalView?.sorting).toBe('priority')
  })

  it('refuses a switch to a view that does not stand', async () => {
    setActivePinia(createPinia())
    const { transport } = harness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)

    expect(views.switchGlobalView(99)).toBe(false)
    expect(views.activeGlobalView?.id).toBe(1)

    // A Project's scope holds only its own views.
    expect(views.switchProjectView(1, 1)).toBe(false)
    expect(views.activeProjectView(1)?.id).toBe(2)
    expect(views.switchProjectView(1, 2)).toBe(true)
  })

  it('writing one property through keeps the others it owns', async () => {
    setActivePinia(createPinia())
    const queue = reviewQueue(8)
    const { transport, commands } = harness([...defaults(), queue])
    const views = useSavedViewsStore()
    await views.refresh(transport)
    views.switchProjectView(1, 8 - 8) // project 1 keeps its default
    expect(views.activeProjectView(1)?.id).toBe(2)

    // Expand Backlog on the global queue: the whole owned set
    // travels, the untouched properties keeping their values.
    await views.reviseOwnedSet(transport, queue.id, { expanded_groups: ['backlog'] })

    const update = commands.find((entry) => entry.name === 'view.update')
    expect(update?.request).toMatchObject({
      view_id: queue.id,
      expanded_groups: ['backlog'],
      hidden_columns: queue.hidden_columns,
      mode: queue.mode,
      done_placement: queue.done_placement,
      sorting: queue.sorting,
    })
    expect(
      (update?.request as { mutation: { optimistic_version: number } }).mutation
        .optimistic_version,
    ).toBe(queue.version)
  })

  it('saving the current perspective names it in its scope', async () => {
    setActivePinia(createPinia())
    const { transport, commands } = harness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)

    const created = await views.createView(transport, 'Deep work', 'global', {
      filter: { states: ['active'] },
      expanded_groups: [],
      hidden_columns: ['draft'],
      mode: 'board',
      done_placement: 'column',
      sorting: 'priority',
    })

    expect(created?.name).toBe('Deep work')
    const request = commands.find((entry) => entry.name === 'view.create')?.request
    expect(request).toMatchObject({
      scope: 'global',
      name: 'Deep work',
      filter: { states: ['active'] },
    })
    expect(views.views.map((view) => view.name)).toContain('Deep work')
  })
})
