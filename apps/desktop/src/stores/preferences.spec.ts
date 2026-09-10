// How the operator keeps the shell arranged is the core's record,
// not the browser's: the rail's collapse and each scope's collapsed
// columns are read and written through the production shell
// preference operations, so the arrangement survives a reload, a new
// window, and a cleared browser origin (KAN-T137-AC1, KAN-T137-AC3).
//
// The schedules below separate the two moments a client cannot tell
// apart on its own: the moment the core commits a request, and the
// moment its answer reaches this window (KAN-T144-AC2). A window can
// only place its own answers against another window's by never
// having two of them out at once, so every overlapping schedule here
// asserts both what the store shows and what the core actually
// holds.
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it } from 'vitest'
import type { ShellPreferencesRecord, ShellPreferencesUpdateRequest } from '@kanban/contracts'
import { harness } from '../test/shell-harness'
import type { Harness } from '../test/shell-harness'
import { usePreferencesStore } from './preferences'

/** Every arrangement write the core was asked for, in order. */
function writes(shell: Harness): ShellPreferencesUpdateRequest[] {
  return shell.command.mock.calls
    .filter(([name]) => name === 'shell.preferences.update')
    .map(([, request]) => request as ShellPreferencesUpdateRequest)
}

/** Every arrangement read the core was asked for, in order. */
function reads(shell: Harness): unknown[] {
  return shell.query.mock.calls.filter(([name]) => name === 'shell.preferences')
}

/** Another window's arrangement write, made straight through the
 * transport at the version the core holds. */
function otherWindow(
  shell: Harness,
  version: number,
  record: Omit<ShellPreferencesUpdateRequest, 'mutation'>,
): Promise<unknown> {
  return shell.transport.command('shell.preferences.update', {
    ...record,
    mutation: { optimistic_version: version, idempotency_key: 'other-window' },
  } as ShellPreferencesUpdateRequest)
}

/** Hold the core's answer to the first arrangement write, so a test
 * can act again while that write is still on the wire. The answer is
 * worked out when it is released, which is when the core would have
 * judged it. */
function holdFirstWrite(shell: Harness) {
  const answer = shell.command.getMockImplementation() as (
    name: string,
    request: unknown,
  ) => Promise<unknown>
  const waiting: Array<() => void> = []
  let held = false
  shell.command.mockImplementation((name: string, request: unknown) => {
    if (held || name !== 'shell.preferences.update') return answer(name, request)
    held = true
    return new Promise((resolve) => {
      waiting.push(() => resolve(answer(name, request)))
    })
  })
  return { open: () => waiting.splice(0).forEach((release) => release()) }
}

/** Let the core take the first arrangement write now and hold its
 * answer on the wire. This is the gap no client can see — the core
 * has committed, the window has not been told, and anything that
 * happens meanwhile happens after the commit whatever this window
 * believes (KAN-T144-AC2). */
function commitFirstWrite(shell: Harness) {
  const answer = shell.command.getMockImplementation() as (
    name: string,
    request: unknown,
  ) => Promise<unknown>
  const waiting: Array<() => void> = []
  let held = false
  shell.command.mockImplementation((name: string, request: unknown) => {
    if (held || name !== 'shell.preferences.update') return answer(name, request)
    held = true
    const judged = answer(name, request)
    // A refusal still travelling is nobody's rejection yet.
    void judged.catch(() => undefined)
    return new Promise((resolve) => {
      waiting.push(() => resolve(judged))
    })
  })
  return { open: () => waiting.splice(0).forEach((release) => release()) }
}

/** Hold the core's answer to the first arrangement read, so a test
 * can write while that read is still on the wire. The record is read
 * when the query is made, which is what makes the answer older than
 * anything written after it. `issued` settles when that read reaches
 * the core, so a test can wait for it without polling. */
function holdFirstRead(shell: Harness) {
  const answer = shell.query.getMockImplementation() as (
    name: string,
    request: unknown,
  ) => Promise<unknown>
  const waiting: Array<() => void> = []
  let held = false
  let reached = (): void => undefined
  const issued = new Promise<void>((resolve) => {
    reached = resolve
  })
  shell.query.mockImplementation((name: string, request: unknown) => {
    if (held || name !== 'shell.preferences') return answer(name, request)
    held = true
    const read = answer(name, request)
    reached()
    return new Promise((resolve) => {
      waiting.push(() => resolve(read))
    })
  })
  return { issued, open: () => waiting.splice(0).forEach((release) => release()) }
}

describe('the shell preferences store', () => {
  it('reads the arrangement the core holds', async () => {
    setActivePinia(createPinia())
    const shell = harness({
      preferences: {
        rail_open: false,
        collapsed_columns: [{ scope: { project: 1 }, columns: ['backlog', 'ready'] }],
        version: 4,
      },
    })
    const preferences = usePreferencesStore()

    await preferences.refresh(shell.transport)

    expect(preferences.railOpen).toBe(false)
    expect(preferences.collapsedFor('project:1')).toEqual(['backlog', 'ready'])
    expect(preferences.isCollapsed('project:1', 'ready')).toBe(true)
    expect(preferences.isCollapsed('global', 'ready')).toBe(false)
    expect(preferences.loaded).toBe(true)
  })

  it('writes a collapse through the production command, whole and at the held version', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)

    await preferences.setRailOpen(shell.transport, false)
    await preferences.toggle(shell.transport, 'project:1', 'review')

    expect(shell.command).toHaveBeenCalledWith(
      'shell.preferences.update',
      expect.objectContaining({
        rail_open: false,
        collapsed_columns: [{ scope: { project: 1 }, columns: ['review'] }],
        mutation: expect.objectContaining({ optimistic_version: 1 }),
      }),
    )
    expect(shell.preferences()).toEqual({
      rail_open: false,
      collapsed_columns: [{ scope: { project: 1 }, columns: ['review'] }],
      version: 2,
    })
    expect(preferences.version).toBe(2)
  })

  it('is read back by a fresh store over the same core, which is what a reload is', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const before = usePreferencesStore()
    await before.refresh(shell.transport)
    await before.toggle(shell.transport, 'global', 'done')

    setActivePinia(createPinia())
    const after = usePreferencesStore()
    await after.refresh(shell.transport)

    expect(after.collapsedFor('global')).toEqual(['done'])
    // Nothing of the arrangement was kept in browser storage.
    expect(window.localStorage.length).toBe(0)
  })

  it('collapses and expands every column a control names at once', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)

    await preferences.setCollapsed(shell.transport, 'project:1', ['parked', 'ready'], true)
    expect(preferences.collapsedFor('project:1')).toEqual(['parked', 'ready'])

    await preferences.setCollapsed(shell.transport, 'project:1', ['ready'], false)
    expect(preferences.collapsedFor('project:1')).toEqual(['parked'])

    await preferences.expandAll(shell.transport, 'project:1')
    expect(preferences.collapsedFor('project:1')).toEqual([])
  })

  it('keeps each scope apart', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)

    await preferences.toggle(shell.transport, 'project:1', 'review')
    await preferences.toggle(shell.transport, 'project:2', 'done')

    expect(preferences.collapsedFor('project:1')).toEqual(['review'])
    expect(preferences.collapsedFor('project:2')).toEqual(['done'])
    expect(preferences.collapsedFor('global')).toEqual([])
  })

  it('reports a refused write and restores the arrangement the core holds', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)
    // Another window arranged the shell first, so this write is stale.
    await otherWindow(shell, 0, {
      rail_open: true,
      collapsed_columns: [{ scope: 'global', columns: ['draft'] }],
    })

    await preferences.toggle(shell.transport, 'global', 'review')

    expect(preferences.error).toContain('moved on')
    expect(preferences.collapsedFor('global')).toEqual(['draft'])
    expect(preferences.version).toBe(1)
  })

  it('keeps both choices when two controls are used before either is answered', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const gate = holdFirstWrite(shell)
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)

    const railed = preferences.setRailOpen(shell.transport, false)
    const columned = preferences.toggle(shell.transport, 'global', 'review')
    gate.open()
    await Promise.all([railed, columned])

    expect(preferences.error).toBeNull()
    expect(preferences.railOpen).toBe(false)
    expect(preferences.collapsedFor('global')).toEqual(['review'])
    expect(shell.preferences()).toEqual({
      rail_open: false,
      collapsed_columns: [{ scope: 'global', columns: ['review'] }],
      version: 2,
    })
    // The second write was serialised behind the first, against the
    // version the first one earned.
    expect(writes(shell).map((write) => write.mutation.optimistic_version)).toEqual([0, 1])
  })

  it('asks the core for nothing new while a write of its own is still travelling', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)
    // The operator collapses the rail; the core takes it at once and
    // the answer is still on the wire when a reconnect re-reads the
    // arrangement.
    const write = commitFirstWrite(shell)
    const railed = preferences.setRailOpen(shell.transport, false)
    const reconnect = preferences.refresh(shell.transport)

    // The read waits for the answer this window is owed: only then is
    // the snapshot it gets one the core took after the commit.
    expect(reads(shell)).toHaveLength(1)
    write.open()
    await Promise.all([railed, reconnect])

    expect(reads(shell)).toHaveLength(2)
    expect(preferences.error).toBeNull()
    expect(preferences.railOpen).toBe(false)
    expect(preferences.version).toBe(1)
    expect(preferences.pending).toEqual([])
    expect(shell.preferences()).toEqual({
      rail_open: false,
      collapsed_columns: [],
      version: 1,
    })
  })

  it("follows another window's arrangement committed after this window's write", async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)

    // The core commits this window's rail collapse as version 1...
    const write = commitFirstWrite(shell)
    const railed = preferences.setRailOpen(shell.transport, false)
    // ...another window collapses a column of another scope as
    // version 2 before this window has been told...
    await otherWindow(shell, 1, {
      rail_open: false,
      collapsed_columns: [{ scope: { project: 2 }, columns: ['draft'] }],
    })
    // ...and a reconnect re-reads the arrangement.
    const reconnect = preferences.refresh(shell.transport)
    write.open()
    await Promise.all([railed, reconnect])

    // The answer this window was owed is older than what the core
    // holds, and the read that followed it says so.
    expect(preferences.error).toBeNull()
    expect(preferences.railOpen).toBe(false)
    expect(preferences.collapsedFor('project:2')).toEqual(['draft'])
    expect(preferences.version).toBe(2)
    expect(preferences.pending).toEqual([])
    expect(shell.preferences()).toEqual({
      rail_open: false,
      collapsed_columns: [{ scope: { project: 2 }, columns: ['draft'] }],
      version: 2,
    })
  })

  it('writes over what a read still on the wire turns out to have found', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)
    // Another window collapses a column of another scope...
    await otherWindow(shell, 0, {
      rail_open: true,
      collapsed_columns: [{ scope: { project: 2 }, columns: ['draft'] }],
    })
    // ...a reconnect re-reads the arrangement and that read is still
    // on the wire...
    const read = holdFirstRead(shell)
    const reconnect = preferences.refresh(shell.transport)
    await read.issued
    // ...when the operator collapses the rail.
    const railed = preferences.setRailOpen(shell.transport, false)
    read.open()
    await Promise.all([reconnect, railed])

    // The write was composed from what that read found, so it was
    // taken rather than needlessly refused, and the other window's
    // column stands.
    expect(preferences.error).toBeNull()
    expect(preferences.railOpen).toBe(false)
    expect(preferences.collapsedFor('project:2')).toEqual(['draft'])
    expect(preferences.version).toBe(2)
    expect(preferences.pending).toEqual([])
    expect(shell.preferences()).toEqual({
      rail_open: false,
      collapsed_columns: [{ scope: { project: 2 }, columns: ['draft'] }],
      version: 2,
    })
  })

  it('keeps a write made while a reconnect read was still on the wire', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)
    // A reconnect re-reads the arrangement; the answer is still on
    // the wire when the operator collapses the rail.
    const read = holdFirstRead(shell)
    const reconnect = preferences.refresh(shell.transport)
    await read.issued
    const railed = preferences.setRailOpen(shell.transport, false)
    read.open()
    await Promise.all([reconnect, railed])

    expect(preferences.railOpen).toBe(false)
    expect(preferences.version).toBe(1)
    expect(preferences.collapsedFor('global')).toEqual([])
    expect(preferences.pending).toEqual([])
  })

  it('keeps a landed write when the read issued behind it is answered late', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)
    // The core takes the rail collapse, and the reconnect read that
    // waited for that answer is itself slow to come back.
    const write = commitFirstWrite(shell)
    const railed = preferences.setRailOpen(shell.transport, false)
    const read = holdFirstRead(shell)
    const reconnect = preferences.refresh(shell.transport)

    write.open()
    await railed
    await read.issued
    read.open()
    await reconnect

    expect(preferences.error).toBeNull()
    expect(preferences.railOpen).toBe(false)
    expect(preferences.version).toBe(1)
    expect(preferences.pending).toEqual([])
  })

  it('follows a core restored behind this window while its answer was travelling', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)

    // The core commits the rail collapse as version 1...
    const write = commitFirstWrite(shell)
    const railed = preferences.setRailOpen(shell.transport, false)
    // ...and the database is restored from a backup before this
    // window is told, so the record the answer carries is one the
    // core no longer holds.
    const restored = harness({
      preferences: {
        rail_open: true,
        collapsed_columns: [{ scope: 'global', columns: ['done'] }],
        version: 0,
      },
    })
    const reconnect = preferences.refresh(restored.transport)
    write.open()
    await Promise.all([railed, reconnect])

    expect(preferences.railOpen).toBe(true)
    expect(preferences.collapsedFor('global')).toEqual(['done'])
    expect(preferences.version).toBe(0)
    expect(preferences.pending).toEqual([])
    expect(restored.preferences()).toMatchObject({ rail_open: true, version: 0 })

    // The restored core can still be written to: nothing pins the
    // store to a version it once saw.
    await preferences.setRailOpen(restored.transport, false)

    expect(preferences.error).toBeNull()
    expect(preferences.railOpen).toBe(false)
    expect(restored.preferences()).toEqual({
      rail_open: false,
      collapsed_columns: [{ scope: 'global', columns: ['done'] }],
      version: 1,
    })
  })

  it('reads only once a whole queue of writes has been answered', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)

    const write = commitFirstWrite(shell)
    const railed = preferences.setRailOpen(shell.transport, false)
    const columned = preferences.toggle(shell.transport, 'global', 'review')
    const reconnect = preferences.refresh(shell.transport)

    expect(reads(shell)).toHaveLength(1)
    write.open()
    await Promise.all([railed, columned, reconnect])

    expect(reads(shell)).toHaveLength(2)
    expect(preferences.error).toBeNull()
    expect(preferences.railOpen).toBe(false)
    expect(preferences.collapsedFor('global')).toEqual(['review'])
    expect(preferences.version).toBe(2)
    expect(preferences.pending).toEqual([])
    expect(writes(shell).map((write) => write.mutation.optimistic_version)).toEqual([0, 1])
  })

  it('keeps both choices when the read behind two queued writes is answered last', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)

    const write = commitFirstWrite(shell)
    const railed = preferences.setRailOpen(shell.transport, false)
    const columned = preferences.toggle(shell.transport, 'global', 'review')
    const read = holdFirstRead(shell)
    const reconnect = preferences.refresh(shell.transport)
    write.open()
    await Promise.all([railed, columned])
    await read.issued
    read.open()
    await reconnect

    expect(preferences.error).toBeNull()
    expect(preferences.railOpen).toBe(false)
    expect(preferences.collapsedFor('global')).toEqual(['review'])
    expect(preferences.version).toBe(2)
    expect(preferences.pending).toEqual([])
  })

  it('runs every read that was waiting behind another', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    const read = holdFirstRead(shell)
    const first = preferences.refresh(shell.transport)
    await read.issued
    // Another window arranges the shell while that read is held, so
    // the two reads have different things to say.
    await otherWindow(shell, 0, {
      rail_open: false,
      collapsed_columns: [{ scope: 'global', columns: ['draft'] }],
    })
    const second = preferences.refresh(shell.transport)
    read.open()
    await Promise.all([first, second])

    // The second read was made, not lost behind the first, and it is
    // the one the store rests on.
    expect(reads(shell)).toHaveLength(2)
    expect(preferences.railOpen).toBe(false)
    expect(preferences.collapsedFor('global')).toEqual(['draft'])
    expect(preferences.version).toBe(1)
    expect(preferences.error).toBeNull()
  })

  it('carries a choice made behind a refused write onto what the core holds', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const gate = holdFirstWrite(shell)
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)

    // The operator collapses the rail...
    const railed = preferences.setRailOpen(shell.transport, false)
    // ...another window arranges a column of another scope, so the
    // rail write is about to be refused as stale...
    await otherWindow(shell, 0, {
      rail_open: true,
      collapsed_columns: [{ scope: { project: 2 }, columns: ['draft'] }],
    })
    // ...and the operator collapses a board column before the
    // refusal arrives.
    const columned = preferences.toggle(shell.transport, 'project:1', 'review')
    gate.open()
    await Promise.all([railed, columned])

    // The refusal is reported rather than swallowed, and the refused
    // choice is not applied behind the operator's back.
    expect(preferences.error).toContain('moved on')
    expect(preferences.railOpen).toBe(true)
    // The other window's arrangement stands: this window wrote the
    // whole arrangement, but only the part it was asked about.
    expect(preferences.collapsedFor('project:2')).toEqual(['draft'])
    // The choice made behind the refusal reached the core.
    expect(preferences.collapsedFor('project:1')).toEqual(['review'])
    expect(shell.preferences().version).toBe(2)
    expect(shell.preferences().rail_open).toBe(true)
    expect(shell.preferences().collapsed_columns).toEqual(
      expect.arrayContaining([
        { scope: { project: 2 }, columns: ['draft'] },
        { scope: { project: 1 }, columns: ['review'] },
      ]),
    )
    expect(shell.preferences().collapsed_columns).toHaveLength(2)
  })

  it('keeps a refused write visibly refused while a read waits behind it', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)
    // Another window arranges a column of another scope first, so
    // the rail write is refused the moment the core sees it...
    await otherWindow(shell, 0, {
      rail_open: true,
      collapsed_columns: [{ scope: { project: 2 }, columns: ['draft'] }],
    })
    const write = commitFirstWrite(shell)
    const railed = preferences.setRailOpen(shell.transport, false)
    // ...and a reconnect re-reads the arrangement while the refusal
    // is still travelling. The reconciling read the refusal itself
    // makes is the one inside that write's turn, so it cannot be
    // waiting for the turn it is already in.
    const reconnect = preferences.refresh(shell.transport)
    write.open()
    await Promise.all([railed, reconnect])

    expect(preferences.error).toContain('moved on')
    expect(preferences.railOpen).toBe(true)
    expect(preferences.collapsedFor('project:2')).toEqual(['draft'])
    expect(preferences.version).toBe(1)
    expect(preferences.pending).toEqual([])
    // One refused attempt, and the read behind it did not quietly
    // take the refusal off the screen.
    expect(writes(shell).filter((write) => write.mutation.idempotency_key !== 'other-window'))
      .toHaveLength(1)
  })

  it('reports a write the core could not take at all and rests on what it holds', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)
    const answer = shell.command.getMockImplementation() as (
      name: string,
      request: unknown,
    ) => Promise<unknown>
    shell.command.mockImplementation((name: string, request: unknown) =>
      name === 'shell.preferences.update'
        ? Promise.reject({ code: 'unavailable', message: 'the core is offline' })
        : answer(name, request),
    )

    await preferences.toggle(shell.transport, 'global', 'review')

    expect(preferences.error).toContain('offline')
    expect(preferences.collapsedFor('global')).toEqual([])
    expect(preferences.pending).toEqual([])
    expect(preferences.version).toBe(0)
    expect(shell.preferences().version).toBe(0)
  })

  it('runs a read that was waiting behind a write the core could not take at all', async () => {
    setActivePinia(createPinia())
    let served = 0
    const moved: ShellPreferencesRecord = {
      rail_open: false,
      collapsed_columns: [{ scope: { project: 2 }, columns: ['draft'] }],
      version: 5,
    }
    // Every read after the first finds the arrangement another window
    // has moved on to.
    const shell = harness({
      override: (name: string) =>
        name === 'shell.preferences' && served++ > 0 ? Promise.resolve({ ...moved }) : undefined,
    })
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)
    const answer = shell.command.getMockImplementation() as (
      name: string,
      request: unknown,
    ) => Promise<unknown>
    shell.command.mockImplementation((name: string, request: unknown) =>
      name === 'shell.preferences.update'
        ? Promise.reject({ code: 'unavailable', message: 'the core is offline' })
        : answer(name, request),
    )

    const columned = preferences.toggle(shell.transport, 'global', 'review')
    const reconnect = preferences.refresh(shell.transport)
    await Promise.all([columned, reconnect])

    // A write the core refused ends its turn like any other, so the
    // read behind it was made rather than stranded.
    expect(reads(shell)).toHaveLength(3)
    expect(preferences.version).toBe(5)
    expect(preferences.collapsedFor('project:2')).toEqual(['draft'])
    expect(preferences.collapsedFor('global')).toEqual([])
    expect(preferences.pending).toEqual([])
    // The operator's own message stands over a read's silence.
    expect(preferences.error).toContain('offline')
  })

  it('follows the core to a restored arrangement at a version it has passed', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)
    await preferences.toggle(shell.transport, 'global', 'review')
    expect(preferences.version).toBe(1)

    // The database is restored from a backup, so the core now holds
    // an older record at a version this store has already passed.
    const restored = harness({
      preferences: {
        rail_open: false,
        collapsed_columns: [{ scope: { project: 1 }, columns: ['draft'] }],
        version: 0,
      },
    })
    await preferences.refresh(restored.transport)

    expect(preferences.version).toBe(0)
    expect(preferences.railOpen).toBe(false)
    expect(preferences.collapsedFor('project:1')).toEqual(['draft'])
    expect(preferences.collapsedFor('global')).toEqual([])

    // The restored core can still be written to: nothing pins the
    // store to a version it once saw.
    await preferences.toggle(restored.transport, 'global', 'review')

    expect(preferences.error).toBeNull()
    expect(preferences.collapsedFor('global')).toEqual(['review'])
    expect(restored.preferences().version).toBe(1)
  })

  it('stops writing once every choice has been answered', async () => {
    setActivePinia(createPinia())
    const shell = harness()
    const preferences = usePreferencesStore()
    await preferences.refresh(shell.transport)
    // Another window moved on, so the first write is refused.
    await otherWindow(shell, 0, {
      rail_open: true,
      collapsed_columns: [{ scope: 'global', columns: ['draft'] }],
    })

    await preferences.toggle(shell.transport, 'global', 'review')

    // One refused attempt, and no retry loop behind it.
    expect(writes(shell).filter((write) => write.mutation.idempotency_key !== 'other-window'))
      .toHaveLength(1)
    expect(preferences.error).toContain('moved on')
    expect(preferences.collapsedFor('global')).toEqual(['draft'])
    expect(preferences.version).toBe(1)
  })
})
