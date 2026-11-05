// The card chip presentation model: what one chip on a card face
// carries, and the surface its tone wears. The chips a card shows are
// built from its issue record in `linear-chips.ts`.
import type { ChipKind } from '@kanban/contracts'
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
  key?: string
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
