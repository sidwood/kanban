// What one Execution Profile's fallback means (KAN-T140-AC4,
// KAN-S7-US1, DR-EP-02): the walk the catalogue plans, and the walk
// the core's Runs took. The two are separate truths — a catalogue
// change never rewrites a Run's snapshot (DR-EP-05) — so each is read
// from its own source and neither is inferred from the other.
import type { ProfileRecord, RunRecord } from '@kanban/contracts'

/** Why a planned fallback walk could not answer. `unknown` names a
 * successor the catalogue does not hold, `exhausted` a retired entry
 * whose policy names no successor, and `cycle` a successor already on
 * the walk. Retirement itself is not a break: the resolver crosses
 * retired hops. */
export type BrokenFallback = 'unknown' | 'exhausted' | 'cycle'

/** The fallback the catalogue plans for one entry. */
export interface PlannedFallback {
  /** The entries the walk touched, the requested one first, ending at
   * the entry that answered. */
  chain: string[]
  /** The entry a run requesting this one would use today, or null when
   * the walk could not answer at all. */
  effective: string | null
  /** The link the policy could not take, and why. */
  broken: { name: string; reason: BrokenFallback } | null
}

/** Walk one entry's fallback policy exactly as
 * `kanban_domain::resolve_effective` does: the walk crosses retired
 * hops, and stops at the first entry the catalogue still assigns. An
 * entry that answers ends the walk, so its own unused successor is
 * never read and never reported — the core would not read it either.
 * A retired entry is still assignable to nothing directly (DR-EP-03);
 * that restriction is the assignment picker's, not this walk's. */
export function plannedFallback(
  catalogue: readonly ProfileRecord[],
  name: string,
): PlannedFallback {
  const chain: string[] = []
  let cursor = name
  for (;;) {
    const held: ProfileRecord | undefined = catalogue.find((entry) => entry.name === cursor)
    if (!held) {
      return { chain, effective: null, broken: { name: cursor, reason: 'unknown' } }
    }
    chain.push(held.name)
    if (!held.retired) {
      return { chain, effective: held.name, broken: null }
    }
    const next: string | undefined = held.fallback?.trim()
    if (!next) {
      return { chain, effective: null, broken: { name: held.name, reason: 'exhausted' } }
    }
    if (chain.includes(next)) {
      return { chain, effective: null, broken: { name: next, reason: 'cycle' } }
    }
    cursor = next
  }
}

/** What the core's Runs record about one entry's fallback. */
export interface EffectiveFallback {
  /** Runs whose assignment requested this entry. */
  requested: number
  /** Of those, the ones whose effective profile was something else. */
  fellBack: number
  /** The distinct walks those Runs recorded, requested first. */
  paths: string[][]
}

/** Read one entry's effective fallback out of the Runs themselves: a
 * Run snapshots what was requested and what ran, and that snapshot is
 * the only truthful source for what fallback did (DR-EP-04). */
export function effectiveFallback(
  runs: readonly RunRecord[],
  name: string,
): EffectiveFallback {
  const mine = runs.filter((run) => run.requested.name === name)
  const paths: string[][] = []
  for (const run of mine) {
    if (run.fallback_path.length === 0) continue
    if (paths.some((path) => path.join('→') === run.fallback_path.join('→'))) continue
    paths.push([...run.fallback_path])
  }
  return {
    requested: mine.length,
    fellBack: mine.filter((run) => run.fallback).length,
    paths,
  }
}
