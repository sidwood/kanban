// Which announcements a mounted board has to read itself again for.
// The board is a projection of the core's records: once one of those
// records changes, what the board shows is out of date until it is
// re-queried (KAN-T137-AC2, KAN-T137-AC3). The map is exhaustive over
// the generated event catalogue, so an event kind added to the schema
// has no entry here until one is written — the type errors rather
// than the board silently ignoring it.
import type { KanbanEventName } from '@kanban/contracts'

const REFRESHES_BOARD: Readonly<Record<KanbanEventName, boolean>> = {
  // The Initiative and Project axes the filter offers, and the code
  // every card's number wears.
  'initiative.created': true,
  'initiative.renamed': true,
  'initiative.archived': true,
  'project.registered': true,
  'project.updated': true,
  'project.archived': true,
  // The Plan axis, and the Plan a Spec belongs to.
  'plan.created': true,
  'plan.activated': true,
  'plan.replanned': true,
  'plan.completed': true,
  'plan.cancelled': true,
  'plan.archived': true,
  // The Spec axis and the Spec number a card wears.
  'spec.created': true,
  'spec.planned': true,
  'spec.version.approved': true,
  'spec.version.superseded': true,
  'spec.execution.moved': true,
  // The Tickets themselves: membership, group, order, and every fact
  // a card face carries.
  'ticket.created': true,
  'ticket.assigned': true,
  'ticket.state.changed': true,
  'ticket.edited': true,
  'ticket.review.configured': true,
  'ticket.spec.moved': true,
  'ticket.pinned': true,
  'ticket.graph.approved': true,
  // The Profile axis and the profile chips.
  'profile.defined': true,
  'profile.updated': true,
  'profile.retired': true,
  // The Lane axis and the Lane chip a held Ticket wears.
  'lane.created': true,
  'lane.workspace.assigned': true,
  'lane.workspace.released': true,
  'lane.ticket.assigned': true,
  'lane.ticket.released': true,
  // Execution: the run snapshot the effective-profile chips read.
  'dispatch.requested': true,
  'dispatch.claimed': true,
  'run.acknowledged': true,
  // What a Ticket's criteria have reached: the bindings every card's
  // progress is counted from (DR-BP-08).
  'criterion.binding.changed': true,
  // Records the board face does not read. The drawer reads comments,
  // rulings, deferrals and evidence when it is open; Workspaces and
  // clones belong to their own surfaces.
  'comment.created': false,
  'comment.edited': false,
  'ruling.recorded': false,
  'ruling.superseded': false,
  'deferral.recorded': false,
  'deferral.superseded': false,
  'evidence.attached': false,
  'evidence.listed': false,
  'workspace.registered': false,
  'workspace.observed': false,
  'workspace.retired': false,
  'clone.created': false,
  'clone.removed': false,
}

/** Whether this announcement can change what a mounted board shows. */
export function refreshesBoard(event: KanbanEventName): boolean {
  return REFRESHES_BOARD[event] === true
}
