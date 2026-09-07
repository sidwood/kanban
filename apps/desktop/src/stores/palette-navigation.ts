// The navigation vocabulary the command palette may open without
// touching workflow. Every entry is a client-side route; none
// issues a mutating command.
import type { SearchGlobalHit, SearchHitKind } from '@kanban/contracts'

export type PaletteItemKind = 'navigation' | SearchHitKind

export interface PaletteItem {
  /** Stable identity for keyboard selection. */
  id: string
  kind: PaletteItemKind
  /** The line the palette leads with. */
  label: string
  /** The identifier the operator would quote, when one exists. */
  identifier?: string
  /** Where the shell navigates when the item is chosen. */
  route: string
}

/** Surfaces the operator can jump to without searching. */
export const PALETTE_NAVIGATION: readonly PaletteItem[] = [
  { id: 'nav-home', kind: 'navigation', label: 'Home', route: '/' },
  { id: 'nav-board', kind: 'navigation', label: 'Global board', route: '/board' },
  { id: 'nav-register', kind: 'navigation', label: 'Register a Project', route: '/register' },
  { id: 'nav-initiatives', kind: 'navigation', label: 'Manage Initiatives', route: '/initiatives' },
  { id: 'nav-planning', kind: 'navigation', label: 'Plan the Work', route: '/planning' },
  { id: 'nav-specs', kind: 'navigation', label: 'Author Specs', route: '/planning/specs' },
  { id: 'nav-tickets', kind: 'navigation', label: 'Create Tickets', route: '/planning/tickets' },
  {
    id: 'nav-dependencies',
    kind: 'navigation',
    label: 'Wire Dependencies',
    route: '/planning/dependencies',
  },
  { id: 'nav-herdr', kind: 'navigation', label: 'Herdr settings', route: '/settings/herdr' },
  {
    id: 'nav-profiles',
    kind: 'navigation',
    label: 'Execution profiles',
    route: '/settings/profiles',
  },
  {
    id: 'nav-capacity',
    kind: 'navigation',
    label: 'Capacity settings',
    route: '/settings/capacity',
  },
]

/** Filter navigation entries by the operator's text. */
export function filterNavigation(query: string): PaletteItem[] {
  const needle = query.trim().toLowerCase()
  if (!needle) {
    return [...PALETTE_NAVIGATION]
  }
  return PALETTE_NAVIGATION.filter((item) => item.label.toLowerCase().includes(needle))
}

/** Turn one search hit into a palette row the router can open. */
export function paletteItemFromHit(hit: SearchGlobalHit): PaletteItem {
  return {
    id: `search-${hit.kind}-${hit.id}`,
    kind: hit.kind,
    label: hit.label,
    identifier: hit.identifier,
    route: routeForSearchHit(hit),
  }
}

/** The route one search hit should open. */
export function routeForSearchHit(hit: SearchGlobalHit): string {
  switch (hit.kind) {
    case 'initiative':
      return '/initiatives'
    case 'project':
      return hit.project_id === undefined ? '/register' : `/projects/${hit.project_id}/board`
    case 'plan':
      return '/planning'
    case 'spec':
      return '/planning/specs'
    case 'ticket':
      return hit.project_id === undefined ? '/board' : `/projects/${hit.project_id}/board`
    default:
      return '/'
  }
}

/** Merge navigation rows and search hits for one palette view. */
export function mergePaletteItems(navigation: PaletteItem[], hits: SearchGlobalHit[]): PaletteItem[] {
  return [...navigation, ...hits.map(paletteItemFromHit)]
}
