// The card chip presentation model: the closed, versioned vocabulary
// the generated contracts carry (DR-BP-16), resolved against a real
// Ticket and the facts the board holds beside it. The vocabulary
// names every chip a kind of card carries and the builders below are
// exhaustive over it, so a chip added to the schema has no renderer
// until one is written — the type errors rather than the board
// drifting (KAN-T26-AC4). Implementer, reviewer, and effective-profile
// values populate as KAN-S9 lands; the regions land here.
import type {
  ChipKind,
  LaneRecord,
  TicketPriority,
  TicketRecord,
  TicketReadinessBlocker,
  TicketSeverity,
} from '@kanban/contracts'
import { CHIP_VOCABULARY } from '@kanban/contracts'
import type { StatusTone } from '../components/StatusBadge.vue'

/** One chip on a card face: its vocabulary kind, its label, and the
 * value it wears. */
export interface CardChip {
  kind: ChipKind
  label: string
  value: string
  tone?: StatusTone
  /** Context the card keeps off its face: the full reviewer list
   * behind a `+N`, the profile a fallback replaced. */
  detail?: string
  /** The fallback indicator an effective profile wears when the run
   * fell back from its planned profile (DR-BP-13). */
  fallback?: boolean
}

/** The facts beside the Ticket that feed its chips: the Project code
 * its numbers render with, the Lane holding it, what the core's
 * readiness projection says holds it back, and the execution facts
 * that populate as KAN-S9 lands. */
export interface ChipSources {
  projectCode: string
  /** The Lane holding this Ticket, when one does. */
  lane?: LaneRecord | null
  /** The core's readiness projection for this Ticket. */
  blockers?: readonly TicketReadinessBlocker[]
  /** Ordered reviewer names; empty until KAN-S9 lands. */
  reviewers?: readonly string[]
  /** The run's effective profile snapshot; absent until KAN-S9. */
  execution?: { effective: string; fallback: boolean } | null
}

const PRIORITY_TONES: Record<TicketPriority, StatusTone> = {
  urgent: 'critical',
  high: 'caution',
  normal: 'neutral',
  low: 'neutral',
}

const SEVERITY_TONES: Record<TicketSeverity, StatusTone> = {
  critical: 'critical',
  high: 'caution',
  medium: 'neutral',
  low: 'neutral',
}

function chip(
  kind: ChipKind,
  label: string,
  value: string,
  tone?: StatusTone,
): CardChip {
  return tone === undefined ? { kind, label, value } : { kind, label, value, tone }
}

/** Capitalises one vocabulary word: `normal` becomes `Normal`. */
function sentence(word: string): string {
  return word.charAt(0).toUpperCase() + word.slice(1)
}

/** The UTC day an instant falls on, the shortest honest rendering of
 * a stored schedule or due date. */
function dayOf(instant: string): string {
  return instant.slice(0, 10)
}

/** The progress every card carries, resolved per kind: Acceptance
 * Criteria progress for Implementations and Bugs, completion progress
 * for Tasks (DR-BP-08). A Bug not yet qualified has no criteria to
 * count, so its progress names that state — off the card it would be
 * the only kind without one, and a count would invent criteria the
 * qualification has not defined. */
function progressChip(ticket: TicketRecord): CardChip | null {
  if (ticket.kind === 'implementation') {
    return chip('progress', 'Progress', `${ticket.criteria.length} criteria`)
  }
  if (ticket.kind === 'bug') {
    const criteria = ticket.bug?.qualification?.criteria
    return criteria
      ? chip('progress', 'Progress', `${criteria.length} criteria`)
      : chip('progress', 'Progress', 'Not yet qualified')
  }
  return chip('progress', 'Progress', `${ticket.completion.length} outcomes`)
}

/** The profile chip every executing kind wears — the Implementer of
 * an Implementation, the Profiles of a Bug, the Executor of an agent
 * Task: the planned profile before dispatch, the effective profile
 * with a fallback indicator during execution (DR-BP-12, DR-BP-13). */
function executionProfileChip(
  kind: ChipKind,
  label: string,
  ticket: TicketRecord,
  sources: ChipSources,
): CardChip | null {
  const execution = sources.execution ?? null
  if (ticket.state === 'active' && execution) {
    const fellBack = execution.fallback && ticket.profile !== undefined && ticket.profile !== null
    return {
      kind,
      label,
      value: execution.effective,
      fallback: fellBack || undefined,
      detail: fellBack
        ? `effective ${execution.effective}, fell back from the planned ${ticket.profile}`
        : `effective ${execution.effective}`,
    }
  }
  // Before dispatch — and while the run snapshot has not landed — the
  // record names only the planned profile.
  return ticket.profile
    ? { kind, label, value: ticket.profile, detail: 'planned profile' }
    : null
}

/** More than two reviewers collapse to `+N`; the full list stays in
 * the detail the drawer shows (DR-BP-14). */
export function reviewersChip(names: readonly string[]): CardChip | null {
  if (names.length === 0) return null
  const shown = names.slice(0, 2)
  const hidden = names.length - shown.length
  return hidden > 0
    ? {
        kind: 'reviewers',
        label: 'Reviewers',
        value: `${shown.join(', ')} +${hidden}`,
        detail: names.join(', '),
      }
    : { kind: 'reviewers', label: 'Reviewers', value: shown.join(', ') }
}

/** What holds the Ticket back, as the core's readiness projection
 * counts it: dependencies and explicit external blockers. */
function blockersChip(
  blockers: readonly TicketReadinessBlocker[],
): CardChip | null {
  if (blockers.length === 0) return null
  return chip(
    'blockers',
    'Blockers',
    `${blockers.length} ${blockers.length === 1 ? 'blocker' : 'blockers'}`,
    'caution',
  )
}

/** The Task's schedule or due date, whichever it carries. */
function scheduleChip(ticket: TicketRecord): CardChip | null {
  if (ticket.scheduled_for) {
    return chip('schedule', 'Scheduled', dayOf(ticket.scheduled_for))
  }
  if (ticket.due) {
    return chip('schedule', 'Due', dayOf(ticket.due))
  }
  return null
}

/** Who executes a Task: the operator by hand, or the named profile
 * under its assignment. */
function executorChip(
  ticket: TicketRecord,
  sources: ChipSources,
): CardChip | null {
  if (ticket.mode === 'human') {
    return chip('executor', 'Executor', 'Operator')
  }
  if (ticket.mode === 'agent') {
    return executionProfileChip('executor', 'Executor', ticket, sources)
  }
  return null
}

// One builder per chip kind, exhaustive over the closed vocabulary:
// a kind the schema adds finds no entry here until one is written.
const CHIP_BUILDERS: Record<
  ChipKind,
  (ticket: TicketRecord, sources: ChipSources) => CardChip | null
> = {
  priority: (ticket) =>
    chip('priority', 'Priority', sentence(ticket.priority), PRIORITY_TONES[ticket.priority]),
  progress: (ticket) => progressChip(ticket),
  spec: (ticket, sources) =>
    ticket.spec_id
      ? chip('spec', 'Spec', `${sources.projectCode}-S${ticket.spec_id}`)
      : null,
  implementer: (ticket, sources) =>
    executionProfileChip('implementer', 'Implementer', ticket, sources),
  reviewers: (_ticket, sources) => reviewersChip(sources.reviewers ?? []),
  lane: (_ticket, sources) =>
    sources.lane ? chip('lane', 'Lane', `Lane ${sources.lane.id}`) : null,
  blockers: (_ticket, sources) => blockersChip(sources.blockers ?? []),
  severity: (ticket) => {
    const severity = ticket.bug?.qualification?.severity
    return severity
      ? chip('severity', 'Severity', sentence(severity), SEVERITY_TONES[severity])
      : null
  },
  frequency: (ticket) => {
    const frequency = ticket.bug?.qualification?.frequency
    return frequency ? chip('frequency', 'Frequency', frequency) : null
  },
  origin: (ticket) => {
    const origin = ticket.bug?.reporter_evidence
    return origin ? chip('origin', 'Origin', origin) : null
  },
  profiles: (ticket, sources) =>
    executionProfileChip('profiles', 'Profiles', ticket, sources),
  subtype: (ticket) =>
    ticket.subtype ? chip('subtype', 'Subtype', sentence(ticket.subtype)) : null,
  mode: (ticket) => (ticket.mode ? chip('mode', 'Mode', sentence(ticket.mode)) : null),
  schedule: (ticket) => scheduleChip(ticket),
  executor: (ticket, sources) => executorChip(ticket, sources),
}

/** The chips one card carries: the vocabulary's set for the Ticket's
 * kind, each resolved against the Ticket and its sources, every chip
 * without a value left off the face. */
export function chipsFor(
  ticket: TicketRecord,
  sources: ChipSources,
): readonly CardChip[] {
  const set = CHIP_VOCABULARY.sets.find((entry) => entry.kind === ticket.kind)
  if (!set) return []
  const chips: CardChip[] = []
  for (const kind of set.chips) {
    const built = CHIP_BUILDERS[kind](ticket, sources)
    if (built !== null) chips.push(built)
  }
  return chips
}

/** The Lane holding a Ticket, when one does. */
export function laneFor(
  lanes: readonly LaneRecord[],
  ticketId: number,
): LaneRecord | undefined {
  return lanes.find((lane) => lane.ticket_id === ticketId)
}

/**
 * The surface a chip wears where it shares a card with the others.
 * Tone is the vocabulary, as with the status surface: a board
 * spelling its own chip colours would drift from the badges.
 */
export function chipSurfaceClass(tone: StatusTone = 'neutral'): string {
  switch (tone) {
    case 'critical':
      return 'border-critical/50 bg-critical/8 text-critical'
    case 'caution':
      return 'border-caution/50 bg-caution/8 text-caution'
    case 'positive':
      return 'border-accent/50 bg-accent/8 text-accent'
    case 'progress':
      return 'border-info/50 bg-info/8 text-info'
    default:
      return 'border-line bg-surface text-ink-muted'
  }
}
