// How the operator keeps the shell arranged: whether the navigation
// rail stands open, and which board columns are collapsed to their
// rail in each scope. This is per-operator data in the authoritative
// store, driven entirely through the generated client — never browser
// state — so the arrangement survives a reload, a new window, and a
// cleared browser origin (KAN-S5-US1, KAN-S5-US3). A Saved View owns
// which columns are hidden and which groups are open (DR-BP-05);
// these are the decisions no view owns.
//
// One update carries the whole arrangement, so the store keeps two
// things apart: the record the core last answered, and what the
// operator has asked for since. The screen shows the second over the
// first — a control that waits for a round trip is a control that
// feels broken — and each request is composed from the core's own
// latest record, so a change another window made to something nobody
// here asked about is written back exactly as the core holds it.
//
// Only the core knows the order things happened in. A window sees two
// moments of its own, when it asked and when it was answered, and
// neither of them places its request against another window's: the
// core commits somewhere inside that gap, and the gap is invisible
// from here. A window that has two requests out at once therefore
// cannot say which of the two answers speaks for the newer record,
// and no counter of its own asking can tell it — that is the whole
// defect this store is built around (KAN-T144-AC1).
//
// So this store never has two requests out at once. Each public read
// and each write drain takes a turn on the wire and asks for nothing
// until the turn before it has been answered. Every answer is then
// the newest record this window could hold: a read taken after a
// write was answered is a snapshot of a core that had already
// committed that write, and a write composed after a read was
// answered carries what that read found. Nothing is weighed,
// dropped, or compared by version, so a core restored from a backup
// to an older record is followed rather than refused for ever
// (KAN-T144-AC3).
//
// The reconciling read a refusal makes is the one read that takes no
// turn: it runs inside the write drain that already holds one, to see
// what the core kept instead of the request it would not take. Making
// it wait for a turn would be making it wait for itself.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { BoardColumn, ScopedCollapsedColumns, ViewScope } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import type { ScopeKey } from './scope'

/** The wire scope one scope key names. */
function wireScopeOf(key: ScopeKey): ViewScope {
  return key === 'global' ? 'global' : { project: Number(key.slice('project:'.length)) }
}

/** The scope key one wire scope names. */
function keyOfWireScope(scope: ViewScope): ScopeKey {
  return scope === 'global' ? 'global' : `project:${scope.project}`
}

type CollapsedByScope = Partial<Record<ScopeKey, BoardColumn[]>>

/** The whole arrangement one record carries. */
interface Arrangement {
  railOpen: boolean
  collapsed: CollapsedByScope
}

/** One thing the operator has asked of the arrangement that the core
 * has not answered yet. Each names only what it was asked about, so
 * asking it again of a record the core answered since leaves
 * everything that record holds besides alone. */
type ArrangementIntent =
  | { kind: 'rail'; open: boolean }
  | { kind: 'columns'; key: ScopeKey; columns: readonly BoardColumn[]; collapsed: boolean }
  | { kind: 'expanded'; key: ScopeKey }

/** What one store instance has on the wire: the turn being taken,
 * ended whichever way it ends. This is deliberately not store state:
 * a promise is nothing a surface renders, and nothing should watch
 * it. */
const wire = new WeakMap<object, Promise<void>>()

const over = (): undefined => undefined

/** Take this store's turn on the wire: run `work` once every turn
 * taken before it has ended, and end this one when it does — a
 * refused or unreachable request ends its turn like any other, so
 * nothing waiting behind it is stranded. The turn is forgotten once
 * the wire falls quiet, so the next thing the operator asks for is
 * put to the core straight away rather than a tick later. */
function inTurn<T>(store: object, work: () => Promise<T>): Promise<T> {
  const waiting = wire.get(store)
  const taken = waiting === undefined ? work() : waiting.then(work)
  const ended = taken.then(over, over)
  wire.set(store, ended)
  void ended.then(() => {
    if (wire.get(store) === ended) wire.delete(store)
  })
  return taken
}

function byScope(collapsed: readonly ScopedCollapsedColumns[]): CollapsedByScope {
  const held: CollapsedByScope = {}
  for (const scoped of collapsed) held[keyOfWireScope(scoped.scope)] = [...scoped.columns]
  return held
}

function scopedColumns(collapsed: CollapsedByScope): ScopedCollapsedColumns[] {
  return (Object.entries(collapsed) as [ScopeKey, BoardColumn[]][])
    .filter(([, columns]) => columns.length > 0)
    .map(([key, columns]) => ({ scope: wireScopeOf(key), columns }))
}

/** The arrangement `base` becomes once every intent has been asked of
 * it, in the order the operator asked. */
function asked(base: Arrangement, intents: readonly ArrangementIntent[]): Arrangement {
  let railOpen = base.railOpen
  const collapsed: CollapsedByScope = { ...base.collapsed }
  for (const intent of intents) {
    if (intent.kind === 'rail') {
      railOpen = intent.open
      continue
    }
    if (intent.kind === 'expanded') {
      collapsed[intent.key] = []
      continue
    }
    const held = collapsed[intent.key] ?? []
    collapsed[intent.key] = intent.collapsed
      ? [...held, ...intent.columns.filter((column) => !held.includes(column))]
      : held.filter((column) => !intent.columns.includes(column))
  }
  return { railOpen, collapsed }
}

export const usePreferencesStore = defineStore('preferences', {
  state: () => ({
    /** The arrangement the core last answered. */
    record: { railOpen: true, collapsed: {} } as Arrangement,
    /** The optimistic version that record carried. */
    version: 0,
    /** What the operator has asked for since, oldest first. */
    pending: [] as ArrangementIntent[],
    /** What last went wrong, and whether it was the fate of something
     * the operator asked for. A read speaks only into silence: a
     * refusal stays on screen until the operator's next choice
     * settles it, rather than being quietly cleared by a reconnect
     * (KAN-T144-AC3). */
    trouble: null as { message: string; asked: boolean } | null,
    loaded: false,
    /** The rail is a shell control. A click must collapse or expand
     * it even when the core is down; the core may remember the
     * choice later, but it does not own the click. */
    sessionRailOpen: null as boolean | null,
  }),
  getters: {
    /** The arrangement on screen: the record the core holds, with
     * everything the operator has asked for since over it. */
    arrangement: (state): Arrangement => asked(state.record, state.pending),
    /** What the operator was last told went wrong. */
    error: (state): string | null => state.trouble?.message ?? null,
    /** Whether the rail stands open with its labels. */
    railOpen(): boolean {
      return this.sessionRailOpen ?? this.arrangement.railOpen
    },
    /** The columns one scope keeps collapsed. */
    collapsedFor(): (key: ScopeKey) => readonly BoardColumn[] {
      return (key: ScopeKey) => this.arrangement.collapsed[key] ?? []
    },
    /** Whether one scope keeps one column collapsed. */
    isCollapsed(): (key: ScopeKey, column: BoardColumn) => boolean {
      return (key: ScopeKey, column: BoardColumn) => this.collapsedFor(key).includes(column)
    },
  },
  actions: {
    // Read the arrangement the core holds, unless it already has
    // been: a surface that needs it on mount asks for this, and the
    // shell re-reads on every connection.
    async ensureLoaded(transport: ShellTransport): Promise<void> {
      if (this.loaded) return
      await this.refresh(transport)
    },
    // Read the arrangement the core holds, in this store's own turn
    // on the wire — behind every request of its own still travelling,
    // so the snapshot the core takes is one it took after committing
    // them. That is what makes the answer placeable at all: the core
    // decided the order, and this window waited to be told.
    async refresh(transport: ShellTransport): Promise<void> {
      await inTurn(this, () => this.reconcile(transport))
    },
    // Open or collapse the rail. The screen changes first; the core
    // is told when a transport exists, and a dead core must not put
    // the rail back.
    async setRailOpen(transport: ShellTransport | undefined, open: boolean): Promise<void> {
      this.sessionRailOpen = open
      if (!transport) return
      await this.ask(transport, { kind: 'rail', open })
    },
    // Collapse one column of one scope, or expand it again.
    async toggle(transport: ShellTransport, key: ScopeKey, column: BoardColumn): Promise<void> {
      await this.setCollapsed(transport, key, [column], !this.isCollapsed(key, column))
    },
    // Put several columns of one scope into the same state at once.
    async setCollapsed(
      transport: ShellTransport,
      key: ScopeKey,
      columns: readonly BoardColumn[],
      collapsed: boolean,
    ): Promise<void> {
      await this.ask(transport, { kind: 'columns', key, columns: [...columns], collapsed })
    },
    // Every column of one scope open again.
    async expandAll(transport: ShellTransport, key: ScopeKey): Promise<void> {
      await this.ask(transport, { kind: 'expanded', key })
    },
    // Ask the core for one change to the arrangement. The surface
    // rearranges at once, and the request takes its turn on the wire,
    // so two controls used together are written one after the other
    // rather than racing each other to a version only one of them can
    // hold (KAN-T137-AC1, KAN-T137-AC3). A control pressed while a
    // drain is running joins that drain's queue and its own turn then
    // finds nothing left to write.
    async ask(transport: ShellTransport, intent: ArrangementIntent): Promise<void> {
      this.pending = [...this.pending, intent]
      await inTurn(this, () => this.flush(transport))
    },
    // Write everything the operator has asked for, one request at a
    // time, until nothing is left unanswered. Each request carries the
    // whole arrangement — the record the core answered with every
    // pending request over it — against the version that record
    // carried. The core's answer to a request it took is this store's
    // newest record, so the store rests on it before the requests it
    // answered leave the queue: no choice is ever let go without the
    // record that carries it. A refusal is reported and the requests
    // it answered are let go, leaving the operator looking at what the
    // core holds; anything asked for behind it is then written over
    // that record, so the newest choice survives a refusal without
    // either window's arrangement being guessed at. The loop only ever
    // shortens the queue, so a refusal can never set it spinning.
    async flush(transport: ShellTransport): Promise<void> {
      const client = new KanbanClient(transport)
      let refusal: string | null = null
      while (this.pending.length > 0) {
        const answering = this.pending.length
        const wanted = asked(this.record, this.pending)
        try {
          const updated = await client.commandShellPreferencesUpdate({
            mutation: {
              optimistic_version: this.version,
              idempotency_key: crypto.randomUUID(),
            },
            rail_open: wanted.railOpen,
            collapsed_columns: scopedColumns(wanted.collapsed),
          })
          this.adopt(updated)
          this.pending = this.pending.slice(answering)
          this.trouble = refusal === null ? null : { message: refusal, asked: true }
        } catch (failure) {
          refusal = asApiError(failure).message
          this.pending = this.pending.slice(answering)
          await this.reconcile(transport)
          this.trouble = { message: refusal, asked: true }
        }
      }
    },
    // Read what the core holds and rest on it, taking no turn of its
    // own. Only the write drain calls this directly, from inside the
    // turn it already holds, to see what the core kept instead of a
    // request it would not take; everything else reaches it through
    // `refresh`.
    async reconcile(transport: ShellTransport): Promise<void> {
      try {
        const record = await new KanbanClient(transport).queryShellPreferences({})
        this.adopt(record)
        if (this.trouble?.asked !== true) this.trouble = null
      } catch (failure) {
        if (this.trouble?.asked !== true) {
          this.trouble = { message: asApiError(failure).message, asked: false }
        }
      }
    },
    /** Rest on the record the core answered. */
    adopt(record: {
      rail_open: boolean
      collapsed_columns: ScopedCollapsedColumns[]
      version: number
    }): void {
      this.record = { railOpen: record.rail_open, collapsed: byScope(record.collapsed_columns) }
      this.version = record.version
      this.loaded = true
    },
  },
})
