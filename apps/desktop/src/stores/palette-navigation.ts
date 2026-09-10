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

/** Surfaces the operator can jump to without searching: the rail's
 * destinations first, then the planning surfaces the rail reaches
 * through Planning. */
export const PALETTE_NAVIGATION: readonly PaletteItem[] = [
  { id: 'nav-boards', kind: 'navigation', label: 'Boards', route: '/board' },
  { id: 'nav-attention', kind: 'navigation', label: 'Attention inbox', route: '/attention' },
  { id: 'nav-planning', kind: 'navigation', label: 'Planning', route: '/planning' },
  { id: 'nav-activity', kind: 'navigation', label: 'Activity', route: '/activity' },
  { id: 'nav-workspaces', kind: 'navigation', label: 'Workspaces & Lanes', route: '/workspaces' },
  {
    id: 'nav-profiles',
    kind: 'navigation',
    label: 'Execution profiles',
    route: '/settings/profiles',
  },
  { id: 'nav-projects', kind: 'navigation', label: 'Projects', route: '/register' },
  { id: 'nav-initiatives', kind: 'navigation', label: 'Initiatives', route: '/initiatives' },
  { id: 'nav-herdr', kind: 'navigation', label: 'Herdr settings', route: '/settings/herdr' },
  {
    id: 'nav-capacity',
    kind: 'navigation',
    label: 'Capacity settings',
    route: '/settings/capacity',
  },
  { id: 'nav-health', kind: 'navigation', label: 'Health', route: '/health' },
  { id: 'nav-specs', kind: 'navigation', label: 'Author Specs', route: '/planning/specs' },
  { id: 'nav-tickets', kind: 'navigation', label: 'Create Tickets', route: '/planning/tickets' },
  {
    id: 'nav-dependencies',
    kind: 'navigation',
    label: 'Wire Dependencies',
    route: '/planning/dependencies',
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

/** The route one search hit should open: the exact object it named,
 * never the list that object lives in (KAN-T140-AC2). A Ticket opens
 * on its Project's board with its drawer already open; a Plan and a
 * Spec open on the planning surface already scoped to their Project
 * and selected; an Initiative opens selected in the Initiative
 * register; a Project opens its own board. A hit naming no Project
 * can only be taken as far as its kind's surface. */
export function routeForSearchHit(hit: SearchGlobalHit): string {
  switch (hit.kind) {
    case 'initiative':
      return `/initiatives?initiative=${hit.id}`
    case 'project':
      return hit.project_id == null ? '/register' : `/projects/${hit.project_id}/board`
    case 'plan':
      return planningRoute('plan', hit)
    case 'spec':
      return planningRoute('spec', hit)
    case 'ticket':
      return hit.project_id == null ? '/board' : `/projects/${hit.project_id}/board?ticket=${hit.id}`
    default:
      return '/board'
  }
}

/** The planning route that opens one Plan or Spec: the Project first,
 * so the surface loads the Project the object belongs to before
 * selecting it. */
function planningRoute(object: 'plan' | 'spec', hit: SearchGlobalHit): string {
  const scope = hit.project_id == null ? '' : `project=${hit.project_id}&`
  return `/planning?${scope}${object}=${hit.id}`
}

/** Merge navigation rows and search hits for one palette view. */
export function mergePaletteItems(navigation: PaletteItem[], hits: SearchGlobalHit[]): PaletteItem[] {
  return [...navigation, ...hits.map(paletteItemFromHit)]
}
