// Where one Attention Item's source lives (KAN-T140-AC6, KAN-S11-US4).
// An item names a typed subject; the inbox's action opens that exact
// object, never the list it sits in, and never a surface the item
// does not actually name. Navigation only: opening a source is a read,
// and acknowledgement stays the operator's separate command
// (DR-SA-12).
import type { AttentionItemRecord, AttentionSubjectKind } from '@kanban/contracts'

/** Where one Attention Item's action leads, and what it opens. */
export interface AttentionDestination {
  /** The route the action opens, or null when the application holds
   * no surface for the object this source names. */
  route: string | null
  /** The typed source the action carries, in the item's own
   * vocabulary — what opens, or why nothing does. */
  label: string
}

/** The Ticket a source's detail names, when it names one. A Schedule
 * and a Run both record the Ticket they belong to, so their action
 * can open the object the operator actually has to decide about. */
function detailTicket(detail: unknown): number | null {
  if (typeof detail !== 'object' || detail === null) return null
  const value = (detail as Record<string, unknown>).ticket_id
  return typeof value === 'number' ? value : null
}

const SUBJECT_LABELS: Record<AttentionSubjectKind, string> = {
  ticket: 'the Ticket on its board',
  run: 'the Run in Workspaces & Lanes',
  spec: 'the Spec in planning',
  project: 'the Project board',
  deferral: 'the deferral on the Project activity',
  graph: 'the Ticket graph',
  schedule: 'the scheduled Ticket',
  role: 'the observed role’s own activity',
}

/** The exact object one Attention Item's source names. */
export function destinationForAttentionItem(item: AttentionItemRecord): AttentionDestination {
  const label = SUBJECT_LABELS[item.subject_kind]
  const project = item.project_id
  switch (item.subject_kind) {
    case 'ticket':
      return { route: `/projects/${project}/board?ticket=${item.subject_id}`, label }
    case 'project':
      return { route: `/projects/${item.subject_id}/board`, label }
    case 'run':
      return { route: `/projects/${project}/workspaces?run=${item.subject_id}`, label }
    case 'spec':
      return { route: `/planning?project=${project}&spec=${item.subject_id}`, label }
    case 'deferral':
      return {
        route: `/activity?project=${project}&deferral=${item.subject_id}`,
        label,
      }
    case 'schedule': {
      // A Schedule is configured on the Ticket it activates; without
      // that Ticket the source names nothing this application opens.
      const ticket = detailTicket(item.detail)
      return ticket === null
        ? { route: null, label: 'the Schedule, which names no Ticket to open' }
        : { route: `/projects/${project}/board?ticket=${ticket}`, label }
    }
    // An observed role is a Herdr identity, not a stored aggregate:
    // the only record this application holds of one is the telemetry
    // its Project's activity timeline carries (DR-HB-03, DR-HB-04),
    // so the action opens that Project's activity marked to this
    // exact role.
    case 'role':
      return {
        route: `/activity?project=${project}&role=${encodeURIComponent(item.subject_id)}`,
        label,
      }
    // A graph proposal is a real subject kind that nothing in this
    // application emits yet, and it has no object surface of its own.
    // Sending the operator to a Project list instead would lose
    // exactly the context the item carries, so the item states its
    // source and offers no destination.
    case 'graph':
      return { route: null, label: 'the Ticket graph, which has no surface of its own' }
  }
}
