import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import type { SavedViewRecord, ViewListResponse } from '@kanban/contracts'
import type { ShellTransport } from '../core/transport'
import { useSavedViewsStore } from './saved-views'

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
    {
      ...base,
      id: 5,
      name: 'Review queue',
      scope: 'global' as const,
      is_default: false,
      filter: { states: ['in_review'] },
      mode: 'register' as const,
      version: 3,
    },
  ]
}

function harness(views: SavedViewRecord[]) {
  const commands: Array<{ name: string; request: unknown }> = []
  const transport = {
    query: (name: string) => {
      if (name === 'view.list') return Promise.resolve({ views } satisfies ViewListResponse)
      return Promise.resolve({ views: [] })
    },
    command: vi.fn((name: string, request: unknown) => {
      commands.push({ name, request })
      const body = request as Record<string, unknown>
      if (name === 'view.create') {
        return Promise.resolve({
          id: 21,
          is_default: false,
          version: 1,
          ...body,
        })
      }
      const standing = views.find((view) => view.id === body.view_id) ?? views[0]
      return Promise.resolve({ ...standing, ...body, version: standing.version + 1 })
    }),
    subscribe: () => () => undefined,
  } as unknown as ShellTransport
  return { transport, commands }
}

/** The same harness with the command held open, so a test can edit
 * the working copy while a save is still on the wire. */
function pendingHarness(views: SavedViewRecord[]) {
  const base = harness(views)
  const waiting: Array<() => void> = []
  const command = base.transport.command as unknown as ReturnType<typeof vi.fn>
  const answer = command.getMockImplementation() as (name: string, request: unknown) => Promise<unknown>
  command.mockImplementation(
    (name: string, request: unknown) =>
      new Promise((resolve) => {
        waiting.push(() => resolve(answer(name, request)))
      }),
  )
  return { ...base, settle: () => waiting.splice(0).forEach((release) => release()) }
}

describe('saved view drift', () => {
  it('holds no drift while the board rests on the record', async () => {
    setActivePinia(createPinia())
    const { transport } = harness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)

    expect(views.isDrifted('global')).toBe(false)
    expect(views.isDrifted('project:1')).toBe(false)
    expect(views.workingFor('global')).toEqual({
      filter: {
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
      },
      expanded_groups: [],
      hidden_columns: ['draft'],
      mode: 'board',
      done_placement: 'column',
      sorting: 'priority',
    })
  })

  it('drifts on a revised property and returns on reset', async () => {
    setActivePinia(createPinia())
    const { transport, commands } = harness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)

    views.revise('global', { sorting: 'readiness' })
    expect(views.isDrifted('global')).toBe(true)
    expect(views.workingFor('global').sorting).toBe('readiness')
    // Nothing is written until the operator saves.
    expect(commands).toEqual([])

    views.revise('global', { sorting: 'priority' })
    expect(views.isDrifted('global')).toBe(false)

    views.revise('global', { hidden_columns: [] })
    expect(views.isDrifted('global')).toBe(true)
    views.resetWorking('global')
    expect(views.isDrifted('global')).toBe(false)
    expect(views.workingFor('global').hidden_columns).toEqual(['draft'])
  })

  it('keeps each scope\'s drift apart', async () => {
    setActivePinia(createPinia())
    const { transport } = harness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)

    views.revise('project:1', { mode: 'register' })
    expect(views.isDrifted('project:1')).toBe(true)
    expect(views.isDrifted('global')).toBe(false)
  })

  it('saves the whole working set to the active view and rests on the record it returns', async () => {
    setActivePinia(createPinia())
    const { transport, commands } = harness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)
    views.revise('global', { sorting: 'readiness', expanded_groups: ['backlog'] })
    views.revise('global', { filter: { ...views.workingFor('global').filter, kinds: ['task'] } })

    await views.saveWorking(transport, 'global')

    expect(commands).toHaveLength(1)
    expect(commands[0].name).toBe('view.update')
    expect(commands[0].request).toMatchObject({
      view_id: 1,
      sorting: 'readiness',
      expanded_groups: ['backlog'],
      hidden_columns: ['draft'],
      mode: 'board',
      done_placement: 'column',
      mutation: { optimistic_version: 1 },
    })
    expect(views.isDrifted('global')).toBe(false)
    expect(views.activeViewFor('global')?.version).toBe(2)
    expect(views.activeViewFor('global')?.sorting).toBe('readiness')
  })

  it('keeps the drift when the save is refused', async () => {
    setActivePinia(createPinia())
    const { transport } = harness(defaults())
    ;(transport.command as ReturnType<typeof vi.fn>).mockImplementation(() =>
      Promise.reject({ code: 'conflict', message: 'the view moved on' }),
    )
    const views = useSavedViewsStore()
    await views.refresh(transport)
    views.revise('global', { sorting: 'readiness' })

    await views.saveWorking(transport, 'global')

    expect(views.error).toBe('the view moved on')
    expect(views.isDrifted('global')).toBe(true)
    expect(views.workingFor('global').sorting).toBe('readiness')
  })

  it('switching views drops the drift and rests on the chosen record', async () => {
    setActivePinia(createPinia())
    const { transport } = harness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)
    views.revise('global', { sorting: 'readiness' })

    expect(views.switchView('global', 5)).toBe(true)

    expect(views.activeViewFor('global')?.id).toBe(5)
    expect(views.isDrifted('global')).toBe(false)
    expect(views.workingFor('global').mode).toBe('register')
    expect(views.workingFor('global').filter.states).toEqual(['in_review'])
    expect(views.workingFor('global').sorting).toBe('priority')
  })

  it('refuses a switch outside the scope', async () => {
    setActivePinia(createPinia())
    const { transport } = harness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)

    expect(views.switchView('project:1', 5)).toBe(false)
    expect(views.activeViewFor('project:1')?.id).toBe(2)
  })

  it('saves the working set as a new named view and switches to it', async () => {
    setActivePinia(createPinia())
    const { transport, commands } = harness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)
    views.revise('project:1', { done_placement: 'table' })

    const created = await views.saveWorkingAs(transport, 'project:1', 'Landing watch')

    expect(created?.id).toBe(21)
    expect(commands[0].name).toBe('view.create')
    expect(commands[0].request).toMatchObject({
      scope: { project: 1 },
      name: 'Landing watch',
      done_placement: 'table',
    })
    expect(views.activeViewFor('project:1')?.id).toBe(21)
    expect(views.isDrifted('project:1')).toBe(false)
  })

  it('holds the group sets in the fixed order, so toggling a group off and on is no drift', async () => {
    setActivePinia(createPinia())
    const { transport } = harness([
      ...defaults().slice(0, 2),
      {
        ...defaults()[0],
        id: 9,
        name: 'Wide open',
        is_default: false,
        expanded_groups: ['backlog', 'staged'],
        hidden_columns: ['draft', 'done'],
      },
    ])
    const views = useSavedViewsStore()
    await views.refresh(transport)
    views.switchView('global', 9)

    views.revise('global', { expanded_groups: ['staged'] })
    expect(views.isDrifted('global')).toBe(true)
    views.revise('global', { expanded_groups: ['staged', 'backlog'] })
    expect(views.workingFor('global').expanded_groups).toEqual(['backlog', 'staged'])
    expect(views.isDrifted('global')).toBe(false)

    views.revise('global', { hidden_columns: ['done'] })
    views.revise('global', { hidden_columns: ['done', 'draft'] })
    expect(views.workingFor('global').hidden_columns).toEqual(['draft', 'done'])
    expect(views.isDrifted('global')).toBe(false)
  })

  it('pins a Project scope\'s filter to its own Project, whatever the record says', async () => {
    setActivePinia(createPinia())
    const { transport, commands } = harness([
      ...defaults().slice(0, 1),
      { ...defaults()[1], filter: { projects: [2], kinds: ['bug'] } },
    ])
    const views = useSavedViewsStore()
    await views.refresh(transport)

    // A record another client saved naming another Project never
    // widens this Project's board, and never shows as drift.
    expect(views.workingFor('project:1').filter.projects).toEqual([1])
    expect(views.isDrifted('project:1')).toBe(false)

    views.revise('project:1', { filter: { ...views.workingFor('project:1').filter, projects: [2] } })
    expect(views.workingFor('project:1').filter.projects).toEqual([1])
    expect(views.isDrifted('project:1')).toBe(false)

    views.revise('project:1', { sorting: 'readiness' })
    await views.saveWorking(transport, 'project:1')
    expect(commands[0].request).toMatchObject({ view_id: 2, filter: expect.objectContaining({ projects: [1] }) })
  })

  it('offers the views of one scope', async () => {
    setActivePinia(createPinia())
    const { transport } = harness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)

    expect(views.viewsFor('global').map((view) => view.id)).toEqual([1, 5])
    expect(views.viewsFor('project:1').map((view) => view.id)).toEqual([2])
  })

  it('keeps an edit the operator made while the save was on the wire', async () => {
    setActivePinia(createPinia())
    const { transport, settle } = pendingHarness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)
    views.revise('global', { sorting: 'readiness' })

    const saving = views.saveWorking(transport, 'global')
    views.revise('global', { mode: 'register' })
    settle()
    await saving

    expect(views.workingFor('global').mode).toBe('register')
    expect(views.workingFor('global').sorting).toBe('readiness')
    expect(views.isDrifted('global')).toBe(true)
    expect(views.activeViewFor('global')?.sorting).toBe('readiness')
    expect(views.activeViewFor('global')?.mode).toBe('board')
  })

  it('lets go of the working copy only for the view the save was written to', async () => {
    setActivePinia(createPinia())
    const { transport, settle } = pendingHarness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)
    views.revise('global', { sorting: 'readiness' })

    const saving = views.saveWorking(transport, 'global')
    views.switchView('global', 5)
    views.revise('global', { done_placement: 'table' })
    settle()
    await saving

    expect(views.activeViewFor('global')?.id).toBe(5)
    expect(views.workingFor('global').done_placement).toBe('table')
    expect(views.workingFor('global').mode).toBe('register')
    expect(views.isDrifted('global')).toBe(true)
  })

  it('keeps an edit made while the new named view was being created', async () => {
    setActivePinia(createPinia())
    const { transport, settle } = pendingHarness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)
    views.revise('project:1', { done_placement: 'table' })

    const saving = views.saveWorkingAs(transport, 'project:1', 'Landing watch')
    views.revise('project:1', { sorting: 'readiness' })
    settle()
    await saving

    expect(views.activeViewFor('project:1')?.id).toBe(21)
    expect(views.workingFor('project:1').sorting).toBe('readiness')
    expect(views.workingFor('project:1').done_placement).toBe('table')
    expect(views.isDrifted('project:1')).toBe(true)
  })

  it('rests the board on a view named from the perspective it already holds', async () => {
    setActivePinia(createPinia())
    const { transport } = harness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)

    // Naming the perspective on show, with nothing drifted.
    const created = await views.saveWorkingAs(transport, 'project:1', 'Everything here')
    expect(created?.id).toBe(21)
    expect(views.activeViewFor('project:1')?.id).toBe(21)
    expect(views.isDrifted('project:1')).toBe(false)
  })

  it('leaves a view the operator chose while the new one was created standing', async () => {
    setActivePinia(createPinia())
    const { transport, settle } = pendingHarness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)
    views.revise('global', { sorting: 'readiness' })

    const saving = views.saveWorkingAs(transport, 'global', 'Landing watch')
    expect(views.switchView('global', 5)).toBe(true)
    settle()
    await saving

    // The named view is kept, so nothing the operator asked for is lost.
    expect(views.viewOf(21)?.name).toBe('Landing watch')
    expect(views.viewOf(21)?.sorting).toBe('readiness')
    // The board rests on the view chosen since, exactly as chosen.
    expect(views.activeViewFor('global')?.id).toBe(5)
    expect(views.workingFor('global').mode).toBe('register')
    expect(views.workingFor('global').sorting).toBe('priority')
    expect(views.isDrifted('global')).toBe(false)
  })

  it('leaves the board on its own record when the operator resets while the view is created', async () => {
    setActivePinia(createPinia())
    const { transport, settle } = pendingHarness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)
    views.revise('global', { sorting: 'readiness' })

    const saving = views.saveWorkingAs(transport, 'global', 'Landing watch')
    views.resetWorking('global')
    settle()
    await saving

    // The named view kept the perspective it was named for.
    expect(views.viewOf(21)?.sorting).toBe('readiness')
    // The board let that perspective go, and resting on the new view
    // would have handed it straight back.
    expect(views.activeViewFor('global')?.id).toBe(1)
    expect(views.workingFor('global').sorting).toBe('priority')
    expect(views.isDrifted('global')).toBe(false)
  })

  it('answers only the scope it was started for when the operator has moved to another', async () => {
    setActivePinia(createPinia())
    const { transport, settle } = pendingHarness(defaults())
    const views = useSavedViewsStore()
    await views.refresh(transport)
    views.revise('global', { sorting: 'readiness' })

    const saving = views.saveWorkingAs(transport, 'global', 'Landing watch')
    // The operator moves to a Project board and arranges that one
    // while the global view is still being created.
    views.revise('project:1', { done_placement: 'table' })
    settle()
    await saving

    expect(views.activeViewFor('global')?.id).toBe(21)
    expect(views.isDrifted('global')).toBe(false)
    expect(views.activeViewFor('project:1')?.id).toBe(2)
    expect(views.workingFor('project:1').done_placement).toBe('table')
    expect(views.isDrifted('project:1')).toBe(true)
  })
})
