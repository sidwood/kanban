<script setup lang="ts">
// The board, for every Project or one of them: the SmokeFree Surface
// presentation — typography, colours, themes, spacing, shadows, the
// six fixed groups, the collapsible axes, the responsive layout, the
// Board/Register switch, the Done table option, the detail drawer,
// and the drag interaction language — speaking the Kanban domain
// (KAN-T24-AC1, KAN-T24-AC2, KAN-T137). Every card is a real Ticket
// from the core's board projection wearing the chip vocabulary the
// application schema pins (KAN-T26-AC1 to KAN-T26-AC3), and a drag is
// one ticket.transition the core judges; a refusal — an agent-owned
// drag above all — surfaces as the core's own explanation
// (KAN-T24-AC3). The board belongs to one scope at a time: changing
// scope empties it before the next load settles (KAN-T125). The
// presentation the operator arranges is the active Saved View's
// working copy; Save writes it through, Reset lets it go.
import { computed, inject, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { KanbanClient } from '@kanban/contracts'
import type { BoardGlobalCard, TicketRecord, TicketState } from '@kanban/contracts'
import AppButton from '../components/AppButton.vue'
import AttemptHistory from '../components/AttemptHistory.vue'
import BoardCard from '../components/board/BoardCard.vue'
import BoardColumnsFlyout from '../components/board/BoardColumnsFlyout.vue'
import type { ColumnPreference } from '../components/board/BoardColumnsFlyout.vue'
import BoardFiltersFlyout from '../components/board/BoardFiltersFlyout.vue'
import BoardRegister from '../components/BoardRegister.vue'
import ChevronIcon from '../components/ChevronIcon.vue'
import DetailDrawer from '../components/DetailDrawer.vue'
import DoneBoardTable from '../components/DoneBoardTable.vue'
import EmptyState from '../components/EmptyState.vue'
import InlineAlert from '../components/InlineAlert.vue'
import SkeletonBlock from '../components/SkeletonBlock.vue'
import StatusBadge from '../components/StatusBadge.vue'
import TicketDetailSections from '../components/TicketDetailSections.vue'
import TicketDrawerActions from '../components/TicketDrawerActions.vue'
import TimelineSurface from '../components/TimelineSurface.vue'
import { kanbanTransportKey, asApiError } from '../core/transport'
import { useBoardStore } from '../stores/board'
import { usePreferencesStore } from '../stores/preferences'
import { useProjectRegisterStore } from '../stores/project-register'
import { scopeKeyOf, scopeOfRoute } from '../stores/scope'
import type { BoardScope } from '../stores/scope'
import { useSavedViewsStore } from '../stores/saved-views'
import type { ViewOwnedSet } from '../stores/saved-views'
import { useShellStore } from '../stores/shell'
import { useTicketDetailStore } from '../stores/ticket-detail'
import { useTicketDialogStore } from '../stores/ticket-dialog'
import {
  BOARD_GROUPS,
  BOARD_PRESENTATIONS,
  COLLAPSED_COLUMN_WIDTH_PX,
  COLLAPSED_ROW_HEIGHT_PX,
  type BoardColumnId,
  type BoardGroupId,
  type BoardLayout,
  type BoardLayoutAxis,
  type BoardPresentation,
  axisGroupId,
  boardColumnGroups,
  boardColumnLabel,
  boardColumnSubheading,
  boardLayoutAxisControls,
  canonicalGroups,
  collapsedCount,
  columnForGroupedCard,
  columnHoldsManyStates,
  columnsOfGroup,
  draftControl,
  dropFor,
  inboundStateForColumn,
  registerColumnsFor,
  registerTableLabel,
  resolveHiddenColumns,
  visibleColumnsFor,
} from './board-layout'
import {
  KIND_LABELS,
  STATUS_LABELS,
  STATUS_TONES,
  statusSurfaceClass,
  ticketTimelineId,
} from './board-card'
import type { BoardRegisterColumn, BoardRegisterRow } from './board-card'
import { refreshesBoard } from './board-events'
import { chipsFor } from './board-chips'
import type { CardChip } from './board-chips'
import {
  activeFilterCount,
  filterAxes,
  filterChips,
  withAxisValue,
  withoutValue,
} from './board-filters'
import type { FilterAxis } from './board-filters'
import { PRIORITY_LABELS, emptyFilter } from './global-board-filters'
import { orderGlobalCards } from './board-ordering'

const PRESENTATION_LABELS: Record<BoardPresentation, string> = {
  board: 'Board',
  register: 'Register',
}

const PRIORITY_TONES = {
  urgent: 'critical',
  high: 'caution',
  normal: 'neutral',
  low: 'neutral',
} as const

const transport = inject(kanbanTransportKey)
const route = useRoute()
const router = useRouter()
const projects = useProjectRegisterStore()
const board = useBoardStore()
const savedViews = useSavedViewsStore()
const collapse = usePreferencesStore()
const shell = useShellStore()
const detail = useTicketDetailStore()
const ticketDialog = useTicketDialogStore()

// The scope the route names: every Project, one Project, or none
// when the route carries something that is not a Project.
const scope = computed<BoardScope | null>(() =>
  scopeOfRoute(route.params as { projectId?: string | string[] }),
)
const scopeKey = computed(() => scopeKeyOf(scope.value ?? 'all'))
const project = computed(() =>
  scope.value === null || scope.value === 'all'
    ? null
    : (projects.projects.find((entry) => entry.id === scope.value) ?? null),
)
const projectMissing = computed(
  () => scope.value !== 'all' && projects.loaded && project.value === null,
)
const boardTitle = computed(() =>
  scope.value === 'all' ? 'All projects' : (project.value?.name ?? 'Board'),
)

// The presentation the active Saved View's working copy owns
// (KAN-T28, DR-BP-05): every property comes from it, a change edits
// the working copy, and Save writes the whole set through.
const owned = computed<ViewOwnedSet>(() => savedViews.workingFor(scopeKey.value))
const drifted = computed(() => savedViews.isDrifted(scopeKey.value))
const activeView = computed(() => savedViews.activeViewFor(scopeKey.value))
const viewOptions = computed(() => savedViews.viewsFor(scopeKey.value))

function revise(changes: Partial<ViewOwnedSet>): void {
  savedViews.revise(scopeKey.value, changes)
  if ('filter' in changes) void project_()
}

const layouts = computed(() => ({
  backlog: owned.value.expanded_groups.includes('backlog') ? ('expanded' as const) : ('collapsed' as const),
  completion: owned.value.expanded_groups.includes('staged') ? ('expanded' as const) : ('collapsed' as const),
}))
const done = computed(() => owned.value.done_placement)
const presentation = computed<BoardPresentation>(() => owned.value.mode)

/** The expanded set one axis takes when the operator picks a layout
 * for it: picking the layout already on show changes nothing. */
function axisSetTo(axis: BoardLayoutAxis, layout: BoardLayout): ViewOwnedSet['expanded_groups'] {
  const group = axisGroupId(axis)
  const current = owned.value.expanded_groups
  return canonicalGroups(
    layout === 'expanded' ? [...current, group] : current.filter((entry) => entry !== group),
  )
}

const layoutControls = computed(() => boardLayoutAxisControls(layouts.value))

// Loading and re-querying.
onMounted(() => {
  // The board on screen belongs to the route that mounted it. A
  // board the shared store still holds for another scope goes before
  // any prerequisite is awaited, so no previous scope's card is ever
  // rendered — or dragged — under this route (KAN-T125-AC1).
  if (board.scope !== scope.value) board.clear()
  void load()
})

watch(scope, () => {
  clearDrawer()
  board.clear()
  void load()
  void followLink(linkedTicketId.value)
})

// A change the core announces leaves the projection out of date, and
// a reconnection after the shell lost it dates everything the boot
// read — the Projects, the views, and the projection alike — so the
// whole load runs again rather than waiting for the operator to
// navigate (KAN-T137-AC2, KAN-T137-AC3).
const listening: Array<() => void> = []
onMounted(() => {
  if (!transport) return
  listening.push(
    transport.subscribe((event) => {
      if (refreshesBoard(event.event_type)) void project_()
    }),
    transport.onConnectionChange((state) => {
      if (state === 'connected') void load()
    }),
  )
})

onBeforeUnmount(() => {
  for (const stop of listening.splice(0)) stop()
})

async function load(): Promise<void> {
  if (!transport || scope.value === null) return
  await Promise.all([
    projects.refresh(transport),
    savedViews.refresh(transport),
    collapse.ensureLoaded(transport),
  ])
  if (scope.value !== 'all' && project.value === null) {
    // A scope with no Project holds no board; whatever the store
    // still carried for another one is not this route's to show.
    board.clear()
    return
  }
  await project_()
}

// Re-query the projection under the working filter; the views stand.
// The store's own request generation settles overlapping reads, so
// the projection on screen is always the newest answer.
async function project_(): Promise<void> {
  if (!transport || scope.value === null) return
  await board.refresh(transport, scope.value, owned.value.filter)
}

// The projection this route's scope holds. A board the store still
// carries for another scope renders nothing here (KAN-T125-AC1).
const held = computed(() =>
  board.scope !== null && board.scope === scope.value ? board.cards : [],
)
const settled = computed(() => board.loaded && board.scope === scope.value)

// The cards, in the order the working view reads them: the core's
// canonical order under Priority first, a stable re-key under
// Readiness first (DR-LC-11).
const cards = computed(() => orderGlobalCards(held.value, owned.value.sorting))
const draftCount = computed(() => cards.value.filter((card) => card.group === 'draft').length)
const hidden = computed(() => resolveHiddenColumns(owned.value.hidden_columns, draftCount.value))
const draft = computed(() => draftControl(owned.value.hidden_columns, draftCount.value))

function columnOf(card: BoardGlobalCard): BoardColumnId {
  return columnForGroupedCard(card.group, card.ticket.state, layouts.value)
}

const groups = computed(() =>
  boardColumnGroups(layouts.value, done.value, hidden.value).map((group) => {
    const columns = group.columns.map((column) => ({
      id: column,
      label: boardColumnLabel(column),
      blurb: boardColumnSubheading(column),
      collapsed: collapse.isCollapsed(scopeKey.value, column),
      cards: cards.value.filter((card) => columnOf(card) === column),
    }))
    return {
      id: group.id,
      heading: group.heading,
      subheading: group.subheading,
      grouped: group.grouped,
      count: columns.reduce((total, column) => total + column.cards.length, 0),
      columns,
    }
  }),
)

const visibleColumns = computed(() => visibleColumnsFor(layouts.value, done.value, hidden.value))
const collapsedTotal = computed(() =>
  collapsedCount(collapse.collapsedFor(scopeKey.value), visibleColumns.value),
)
const hiddenTotal = computed(() => hidden.value.length)
const columnsBadge = computed(() => {
  const parts: string[] = []
  if (collapsedTotal.value > 0) parts.push(`${collapsedTotal.value} collapsed`)
  if (hiddenTotal.value > 0) parts.push(`${hiddenTotal.value} hidden`)
  return parts.join(' · ')
})

/** As many placeholders as the presentation is about to fill. */
const loadingColumns = computed(() =>
  presentation.value === 'board' ? visibleColumns.value : registerColumnsFor(layouts.value, hidden.value),
)

// The Columns flyout.
const columnsOpen = ref(false)
const columnRows = computed<ColumnPreference[]>(() =>
  BOARD_GROUPS.map((group) => {
    // A group is one control over the columns it actually shows: the
    // group itself while it is aggregated, its state columns once it
    // opens (KAN-T137-AC3).
    const columns = columnsOfGroup(group.id, layouts.value)
    const collapsedColumns = columns.filter((column) =>
      collapse.isCollapsed(scopeKey.value, column),
    ).length
    return {
      group: group.id,
      label: group.label,
      count: cards.value.filter((card) => card.group === group.id).length,
      hidden: hidden.value.includes(group.id),
      collapsed: collapsedColumns === columns.length,
      collapsedColumns,
      columns: columns.length,
    }
  }),
)
const columnsSummary = computed(() => columnsBadge.value || 'All columns visible')

function toggleHidden(group: BoardGroupId): void {
  if (group === 'draft') {
    if (draft.value.disabled) return
    revise({ hidden_columns: draft.value.next })
    return
  }
  const current = owned.value.hidden_columns
  revise({
    hidden_columns: current.includes(group)
      ? current.filter((entry) => entry !== group)
      : canonicalGroups([...current, group]),
  })
}

// Collapse or expand every column the group shows, so the control
// and the board say the same thing.
function toggleGroupCollapsed(group: BoardGroupId): void {
  if (!transport) return
  const columns = columnsOfGroup(group, layouts.value)
  const collapsed = columns.every((column) => collapse.isCollapsed(scopeKey.value, column))
  void collapse.setCollapsed(transport, scopeKey.value, columns, !collapsed)
}

function toggleColumnCollapsed(column: BoardColumnId): void {
  if (!transport) return
  void collapse.toggle(transport, scopeKey.value, column)
}

function showAllColumns(): void {
  revise({ hidden_columns: owned.value.hidden_columns.includes('draft') ? ['draft'] : [] })
  if (transport) void collapse.expandAll(transport, scopeKey.value)
}

// The Filters flyout and the chips.
const filtersOpen = ref(false)
const axes = computed(() => filterAxes(owned.value.filter, board.options))
const chips = computed(() => filterChips(owned.value.filter, board.options, scope.value ?? 'all'))
const activeFilters = computed(() => activeFilterCount(owned.value.filter, scope.value ?? 'all'))
const scopedProjectLabel = computed(() =>
  project.value ? `${project.value.code} — ${project.value.name}` : null,
)

function onFilterChange(axis: FilterAxis, value: string | null): void {
  revise({ filter: withAxisValue(owned.value.filter, axis, value) })
}

function onRemoveChip(axis: FilterAxis, value: string): void {
  revise({ filter: withoutValue(owned.value.filter, axis, value) })
}

function clearFilters(): void {
  revise({ filter: emptyFilter() })
}

// The Saved View picker and its drift.
function onSwitchView(event: Event): void {
  const viewId = Number((event.target as HTMLSelectElement).value)
  if (savedViews.switchView(scopeKey.value, viewId)) void project_()
}

async function saveView(): Promise<void> {
  if (!transport) return
  await savedViews.saveWorking(transport, scopeKey.value)
}

function resetView(): void {
  savedViews.resetWorking(scopeKey.value)
  void project_()
}

const savingAs = ref(false)
const viewName = ref('')

async function saveViewAs(): Promise<void> {
  if (!transport || viewName.value.trim() === '') return
  const created = await savedViews.saveWorkingAs(transport, scopeKey.value, viewName.value.trim())
  if (created === null) return
  viewName.value = ''
  savingAs.value = false
}

// The register.
const registerColumns = computed<readonly BoardRegisterColumn[]>(() =>
  registerColumnsFor(layouts.value, hidden.value).map((column) => ({
    id: column,
    label: registerTableLabel(column),
    subheading: boardColumnSubheading(column),
    showsStatus: columnHoldsManyStates(column, layouts.value),
    rows: cards.value.filter((card) => columnOf(card) === column).map(registerRow),
  })),
)

function cardNumber(card: BoardGlobalCard): string {
  return `${card.project_code}-T${card.ticket.number}`
}

function cardTitle(ticket: TicketRecord): string {
  return ticket.slice ?? ticket.title ?? 'Untitled Ticket'
}

function progressOf(card: BoardGlobalCard): string {
  return cardChips(card).find((chip) => chip.kind === 'progress')?.value ?? '—'
}

function registerRow(card: BoardGlobalCard): BoardRegisterRow {
  const ticket = card.ticket
  return {
    ticket,
    number: cardNumber(card),
    title: cardTitle(ticket),
    kindLabel: KIND_LABELS[ticket.kind],
    statusLabel: STATUS_LABELS[ticket.state],
    statusTone: STATUS_TONES[ticket.state],
    projectCode: card.project_code,
    spec: card.spec_number != null ? `${card.project_code}-S${card.spec_number}` : null,
    priorityLabel: PRIORITY_LABELS[ticket.priority],
    priorityTone: PRIORITY_TONES[ticket.priority],
    progress: progressOf(card),
    moves: movesFor(card),
    agentOwned: ticket.kind !== 'task',
  }
}

// A register row is moved by naming its target, and the targets are
// the core's: it says where this Ticket may go now, and the register
// offers the visible columns that ask for one of those states
// (KAN-T137-AC5). No lifecycle rule is kept here — the column only
// says which state a move into it asks for.
function movesFor(card: BoardGlobalCard): readonly { column: BoardColumnId; label: string }[] {
  const targets = board.legalTargetsFor(card.ticket.id)
  if (targets.length === 0) return []
  return registerColumnsFor(layouts.value, hidden.value)
    .filter((column) => {
      const drop = dropFor(card.ticket.state, column, layouts.value)
      return drop !== undefined && targets.includes(drop.state)
    })
    .map((column) => ({ column, label: boardColumnLabel(column) }))
}

const doneRows = computed<readonly BoardRegisterRow[]>(() =>
  done.value === 'table'
    ? cards.value.filter((card) => columnOf(card) === 'done').map(registerRow)
    : [],
)

// Done relocation, with focus following the control that moved.
async function pushDoneDown(): Promise<void> {
  revise({ done_placement: 'table' })
  await nextTick()
  const target = document.querySelector<HTMLElement>('[data-testid="bring-done-back-to-board"]')
  target?.focus()
}

async function bringDoneBack(): Promise<void> {
  revise({ done_placement: 'column' })
  await nextTick()
  const target = document.querySelector<HTMLElement>('[data-testid="move-done-below-board"]')
  target?.focus()
}

// The drag: every surface accepts the drop event while a Task drag
// is live, so a refusal can be explained; only the surfaces the core
// might accept are highlighted, and the move itself is one command
// the core judges.
const drag = ref<BoardGlobalCard | null>(null)
const dropTarget = ref<string | null>(null)
const moving = ref(false)
const notice = ref<string | null>(null)

function canDrag(ticket: TicketRecord): boolean {
  return ticket.kind === 'task'
}

function dropHighlight(target: BoardColumnId): boolean {
  return (
    drag.value !== null &&
    !moving.value &&
    !collapse.isCollapsed(scopeKey.value, target) &&
    dropFor(drag.value.ticket.state, target, layouts.value) !== undefined
  )
}

function onDragStart(card: BoardGlobalCard, event: DragEvent): void {
  drag.value = card
  notice.value = null
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', String(card.ticket.id))
  }
}

function onDragEnd(): void {
  dropTarget.value = null
  drag.value = null
}

function onDragOver(target: BoardColumnId, event: DragEvent): void {
  if (drag.value === null || moving.value) return
  event.preventDefault()
  if (event.dataTransfer) event.dataTransfer.dropEffect = 'move'
  dropTarget.value = target
}

function onDragLeave(target: BoardColumnId): void {
  if (dropTarget.value === target) dropTarget.value = null
}

async function onDrop(target: BoardColumnId, event: DragEvent): Promise<void> {
  dropTarget.value = null
  const card = drag.value
  drag.value = null
  if (card === null || moving.value) return
  event.preventDefault()
  const number = cardNumber(card)
  const state = STATUS_LABELS[card.ticket.state]
  if (collapse.isCollapsed(scopeKey.value, target)) {
    notice.value = `Expand the ${boardColumnLabel(target)} column before dropping ${number}. The ticket stays in ${state}.`
    return
  }
  if (inboundStateForColumn(target, layouts.value) === undefined) {
    notice.value = `Nothing moves into ${boardColumnLabel(target)}; ${number} stays in ${state}.`
    return
  }
  const drop = dropFor(card.ticket.state, target, layouts.value)
  if (drop === undefined) return
  await applyMove(card.ticket.id, drop.state)
}

async function onRegisterMove(row: BoardRegisterRow, target: BoardColumnId): Promise<void> {
  if (moving.value) return
  const drop = dropFor(row.ticket.state, target, layouts.value)
  if (drop === undefined) return
  await applyMove(row.ticket.id, drop.state)
}

async function applyMove(ticketId: number, to: TicketState): Promise<void> {
  if (!transport) return
  moving.value = true
  notice.value = null
  try {
    // A landed move changes filter membership and deterministic
    // order, both of which the core owns: the projection is read
    // again rather than patched in place (KAN-T137-AC2, AC4).
    if (await board.move(transport, ticketId, to)) await project_()
  } finally {
    moving.value = false
  }
}

/**
 * A column holding several states cannot say which one a card is on by
 * position, so the card says it — in its accessible name as well as on a
 * badge.
 */
function showsCardStatus(column: BoardColumnId): boolean {
  return columnHoldsManyStates(column, layouts.value)
}

function cardLabel(card: BoardGlobalCard, column: BoardColumnId): string {
  const parts = [cardTitle(card.ticket)]
  if (showsCardStatus(column)) parts.push(STATUS_LABELS[card.ticket.state])
  return parts.join(', ')
}

/** A Task Ticket keeps its dashed border wherever it sits. */
function cardChrome(ticket: TicketRecord, column: BoardColumnId): string {
  const dashed = ticket.kind === 'task'
  if (showsCardStatus(column)) {
    const surface = statusSurfaceClass(ticket.state)
    return dashed ? `border-dashed ${surface}` : surface
  }
  return dashed ? 'border-dashed border-line-strong bg-surface/70' : 'border-line'
}

// The chips one card wears, resolved from the vocabulary against the
// Ticket and the facts the projection and the board hold beside it:
// the Spec number and Lane the core resolved, what its readiness
// projection holds back, the reviewers configured on it, the run
// executing it, and how far its criteria have actually got.
function cardChips(card: BoardGlobalCard): readonly CardChip[] {
  return chipsFor(card.ticket, {
    projectCode: card.project_code,
    specNumber: card.spec_number,
    laneId: card.lane_id,
    blockers: board.blockersFor(card.ticket.id),
    reviewers: board.reviewersFor(card.ticket.id),
    execution: board.executionFor(card.ticket.id),
    bindings: board.bindingsFor(card.ticket.id),
  })
}

// The detail drawer.
const openTicketId = ref<number | null>(null)
const drawerTicket = ref<TicketRecord | null>(null)
const drawerLoading = ref(false)
const drawerError = ref<string | null>(null)

const drawerCard = computed(() =>
  drawerTicket.value ? board.cardOf(drawerTicket.value.id) : undefined,
)
const drawerProject = computed(() =>
  drawerTicket.value
    ? (projects.projects.find((entry) => entry.id === drawerTicket.value?.project_id) ?? null)
    : null,
)
const drawerProjectCode = computed(
  () => drawerCard.value?.project_code ?? drawerProject.value?.code ?? '',
)
const drawerTitle = computed(() =>
  drawerTicket.value ? cardTitle(drawerTicket.value) : 'Ticket',
)
const drawerNumber = computed(() =>
  drawerTicket.value && drawerProjectCode.value
    ? `${drawerProjectCode.value}-T${drawerTicket.value.number}`
    : undefined,
)

// The drawer reads one Ticket at a time. Every read carries the
// generation it was issued under, so a read the operator has closed
// or replaced writes nothing over the record now on show — content,
// failure, and the loading state alike (KAN-T137-AC7).
let drawerRequest = 0

async function openTicket(ticketId: number): Promise<void> {
  openTicketId.value = ticketId
  // A link naming another Ticket would name a record the drawer is
  // not showing, so opening one from the board lets that link go.
  if (linkedTicketId.value !== null && linkedTicketId.value !== ticketId) dropTicketLink()
  await loadDrawer(ticketId)
}

async function loadDrawer(ticketId: number): Promise<void> {
  if (!transport) return
  drawerRequest += 1
  const attempt = drawerRequest
  drawerLoading.value = true
  drawerError.value = null
  try {
    const record = await new KanbanClient(transport).queryTicketGet({ ticket_id: ticketId })
    if (attempt !== drawerRequest) return
    drawerTicket.value = record
    drawerError.value = null
    // Everything the drawer says beyond the record itself is the
    // core's to answer, read for exactly this Ticket.
    await detail.load(transport, record)
  } catch (failure) {
    if (attempt !== drawerRequest) return
    drawerTicket.value = null
    drawerError.value = asApiError(failure).message
  } finally {
    if (attempt === drawerRequest) drawerLoading.value = false
  }
}

/** Empty the drawer and supersede whatever it still has on the wire. */
function clearDrawer(): void {
  drawerRequest += 1
  openTicketId.value = null
  drawerTicket.value = null
  drawerError.value = null
  drawerLoading.value = false
  detail.clear()
}

// Closing the drawer takes the Ticket out of the link as well, so
// the URL never names a record the operator is no longer looking at
// — and choosing that same Ticket again opens it again.
function closeDrawer(): void {
  clearDrawer()
  dropTicketLink()
}

function dropTicketLink(): void {
  if (route.query.ticket === undefined) return
  const query = { ...route.query }
  delete query.ticket
  void router.replace({ path: route.path, query })
}

// The Ticket the link names, when it names one.
const linkedTicketId = computed(() => {
  const raw = route.query.ticket
  const value = Array.isArray(raw) ? raw[0] : raw
  const ticketId = Number(value)
  return value && Number.isInteger(ticketId) && ticketId > 0 ? ticketId : null
})

// The Ticket the link last opened, so losing the link closes the
// drawer it opened and leaves one opened from a card alone.
let openedByLink: number | null = null

// The drawer follows the link: arriving with one opens it, changing
// it moves the drawer to the Ticket it now names, and clearing it
// closes the drawer (KAN-T137-AC7).
watch(linkedTicketId, (ticketId) => void followLink(ticketId), { immediate: true })

async function followLink(ticketId: number | null): Promise<void> {
  if (ticketId === null) {
    if (openedByLink !== null && openTicketId.value === openedByLink) clearDrawer()
    openedByLink = null
    return
  }
  openedByLink = ticketId
  if (openTicketId.value === ticketId) return
  await openTicket(ticketId)
}

const drawerAttempts = computed(() => detail.attempts)

async function refreshAttempts(): Promise<void> {
  if (transport && drawerTicket.value) {
    await detail.refreshRuns(transport, drawerTicket.value.project_id)
    await board.refreshRuns(transport, drawerTicket.value.project_id)
  }
}

// A command run from the drawer replaces the record the drawer is
// showing with the one the core returned, and the board re-reads the
// scope the move may have changed.
function drawerActed(record: TicketRecord): void {
  if (drawerTicket.value?.id !== record.id) return
  drawerTicket.value = record
  void load()
}

function openNewTicket(): void {
  ticketDialog.openCreate({
    projectId: scope.value === 'all' || scope.value === null ? null : scope.value,
  })
}

const drawerTimelineId = computed(() =>
  drawerTicket.value ? ticketTimelineId(drawerTicket.value.id) : '',
)

// A human review verdict appends its audit against the Ticket without
// moving the Ticket's own version, so the timeline is told to read
// again rather than inferring the change from a version that did not
// change (KAN-T139-AC5).
const drawerReviewTick = ref(0)

function drawerReviewed(): void {
  drawerReviewTick.value += 1
}

// The facts the drawer shows for the open Ticket; the Spec identity
// is the number the projection resolved — a Ticket the board does
// not hold states no Spec identity rather than one built from the
// row id it carries (KAN-T126-AC2).
const drawerFacts = computed(() => {
  const ticket = drawerTicket.value
  if (!ticket) return []
  const facts: { label: string; value: string }[] = [
    { label: 'Kind', value: KIND_LABELS[ticket.kind] },
    { label: 'State', value: STATUS_LABELS[ticket.state] },
    { label: 'Priority', value: PRIORITY_LABELS[ticket.priority] },
    {
      label: 'Project',
      value: drawerProject.value
        ? `${drawerProject.value.code} — ${drawerProject.value.name}`
        : drawerProjectCode.value,
    },
  ]
  const specNumber = drawerCard.value?.spec_number
  if (specNumber != null) {
    facts.push({ label: 'Spec', value: `${drawerProjectCode.value}-S${specNumber}` })
  }
  if (ticket.subtype) facts.push({ label: 'Subtype', value: ticket.subtype })
  if (ticket.mode) facts.push({ label: 'Mode', value: ticket.mode })
  if (ticket.scheduled_for) facts.push({ label: 'Scheduled for', value: ticket.scheduled_for })
  if (ticket.due) facts.push({ label: 'Due', value: ticket.due })
  return facts
})

// The empty states: a scope with nothing in it points at planning; a
// filter that matches nothing offers to clear itself.
const emptyScope = computed(
  () => settled.value && held.value.length === 0 && activeFilters.value === 0,
)
const emptyFiltered = computed(
  () => settled.value && held.value.length === 0 && activeFilters.value > 0,
)

const columnWidthStyle = (collapsed: boolean): Record<string, string> =>
  shell.narrow
    ? collapsed
      ? {
          flex: `0 0 ${COLLAPSED_ROW_HEIGHT_PX}px`,
          minWidth: '0',
          width: '100%',
          height: `${COLLAPSED_ROW_HEIGHT_PX}px`,
        }
      : { flex: '1 1 auto', minWidth: '0', width: '100%' }
    : collapsed
      ? {
          flex: `0 0 ${COLLAPSED_COLUMN_WIDTH_PX}px`,
          minWidth: `${COLLAPSED_COLUMN_WIDTH_PX}px`,
          width: `${COLLAPSED_COLUMN_WIDTH_PX}px`,
        }
      : { flex: '1 1 0', minWidth: '14rem' }
</script>

<template>
  <main
    class="animate-rise flex min-h-full flex-col"
    :aria-busy="!settled || board.loading || undefined"
  >
    <header class="flex flex-col gap-3 border-b border-line bg-surface px-4 pt-4 pb-3 lg:px-5">
      <div class="flex flex-wrap items-start gap-4">
        <div class="min-w-0 flex-1 basis-72">
          <p class="wordmark text-[0.6rem] text-ink-subtle">
            Pipeline
          </p>
          <h1
            class="mt-1 font-display text-3xl leading-[1.08] font-semibold tracking-tight text-ink"
            data-testid="board-title"
          >
            {{ boardTitle }}
          </h1>
          <p class="mt-1.5 max-w-[62ch] text-sm text-ink-muted">
            Implementation and Bug lifecycles are agent-owned; Task Tickets move freely. A refused
            move keeps the ticket in its authoritative state.
          </p>
          <p
            class="mt-1.5 text-xs text-ink-subtle"
            data-testid="board-count"
            aria-live="polite"
          >
            Showing
            <span class="font-mono text-ink-muted">{{ held.length }}</span> of
            <span class="font-mono text-ink-muted">{{ board.scopeTotal ?? held.length }}</span>
            tickets
          </p>
        </div>
        <div class="flex flex-wrap items-center justify-end gap-2">
          <div
            role="group"
            aria-label="Ordering"
            class="inline-flex items-center gap-1 rounded-full border border-line bg-surface p-0.75"
          >
            <button
              type="button"
              data-testid="sort-priority"
              aria-label="Order by priority, then readiness"
              :aria-pressed="owned.sorting === 'priority'"
              class="h-6.5 rounded-full border px-3 text-xs font-semibold transition-colors"
              :class="
                owned.sorting === 'priority'
                  ? 'border-accent bg-accent/12 text-accent'
                  : 'border-transparent text-ink-muted hover:text-ink'
              "
              @click="revise({ sorting: 'priority' })"
            >
              Priority first
            </button>
            <button
              type="button"
              data-testid="sort-readiness"
              aria-label="Order by readiness, then priority"
              :aria-pressed="owned.sorting === 'readiness'"
              class="h-6.5 rounded-full border px-3 text-xs font-semibold transition-colors"
              :class="
                owned.sorting === 'readiness'
                  ? 'border-accent bg-accent/12 text-accent'
                  : 'border-transparent text-ink-muted hover:text-ink'
              "
              @click="revise({ sorting: 'readiness' })"
            >
              Readiness first
            </button>
          </div>
          <button
            type="button"
            data-testid="columns-open"
            aria-label="Column visibility and collapse"
            :aria-expanded="columnsOpen"
            aria-haspopup="dialog"
            class="inline-flex h-7.5 items-center gap-1.5 rounded-full border border-line-strong bg-surface px-3 text-xs font-medium text-ink transition-colors hover:border-accent/40"
            @click="columnsOpen = true"
          >
            Columns
            <span
              v-if="columnsBadge"
              class="font-mono text-[0.625rem] text-accent"
            >{{ columnsBadge }}</span>
          </button>
          <button
            type="button"
            data-testid="filters-open"
            :aria-expanded="filtersOpen"
            aria-haspopup="dialog"
            class="inline-flex h-7.5 items-center gap-1.5 rounded-full border px-3 text-xs font-medium transition-colors"
            :class="
              activeFilters > 0
                ? 'border-accent/40 bg-accent/8 text-accent'
                : 'border-line-strong bg-surface text-ink hover:border-accent/40'
            "
            @click="filtersOpen = true"
          >
            Filters
            <span
              v-if="activeFilters > 0"
              class="rounded-full bg-accent-fill px-1.5 font-mono text-[0.625rem] font-semibold text-cta-ink"
              data-testid="filters-badge"
            >{{ activeFilters }}</span>
          </button>
          <button
            type="button"
            data-testid="new-ticket"
            class="inline-flex h-7.5 items-center rounded-full border border-brand-500 bg-brand-gradient px-3.5 text-xs font-semibold text-cta-ink shadow-panel hover:brightness-105"
            @click="openNewTicket"
          >
            New ticket
          </button>
        </div>
      </div>

      <div class="flex flex-wrap items-center gap-1.5">
        <label
          class="inline-flex h-7 items-center gap-1.5 rounded-full border border-line-strong bg-surface pr-2 pl-2.5"
        >
          <span class="text-[0.6rem] font-bold tracking-[0.11em] text-ink-subtle uppercase">View</span>
          <select
            :value="activeView?.id ?? ''"
            data-testid="board-view-select"
            class="max-w-44 border-0 bg-transparent text-xs font-semibold text-ink"
            aria-label="Saved view"
            @change="onSwitchView"
          >
            <option
              v-for="view in viewOptions"
              :key="view.id"
              :value="view.id"
            >
              {{ view.name }}
            </option>
          </select>
        </label>
        <span
          v-if="drifted"
          class="inline-flex items-center gap-1.5"
          data-testid="view-drift"
        >
          <button
            type="button"
            data-testid="view-save"
            class="h-7 rounded-full border border-accent/40 bg-accent/10 px-2.5 text-xs font-semibold text-accent"
            @click="saveView"
          >
            Save view
          </button>
          <button
            type="button"
            data-testid="view-reset"
            class="h-7 rounded-full border border-line-strong bg-surface px-2.5 text-xs font-medium text-ink-muted hover:text-ink"
            @click="resetView"
          >
            Reset
          </button>
        </span>
        <button
          type="button"
          data-testid="view-save-as"
          :aria-expanded="savingAs"
          class="h-7 rounded-full border border-line bg-surface px-2.5 text-xs font-medium text-ink-muted hover:border-line-strong hover:text-ink"
          @click="savingAs = !savingAs"
        >
          Save as…
        </button>
        <form
          v-if="savingAs"
          class="inline-flex items-center gap-1.5"
          @submit.prevent="saveViewAs"
        >
          <input
            v-model="viewName"
            data-testid="save-view-name"
            class="h-7 w-40 rounded-full border border-line bg-canvas/60 px-3 text-xs text-ink"
            placeholder="Name this view"
            aria-label="Name this view"
          >
          <AppButton
            variant="secondary"
            size="sm"
            type="submit"
            data-testid="save-view-create"
            :disabled="viewName.trim() === ''"
          >
            Save
          </AppButton>
        </form>
        <button
          v-for="chip in chips"
          :key="`${chip.axis}:${chip.value}`"
          type="button"
          :data-testid="`filter-chip-${chip.axis}-${chip.value}`"
          :aria-label="`Remove filter ${chip.label} ${chip.valueLabel}`"
          class="inline-flex h-7 items-center gap-1.5 rounded-full border border-accent/30 bg-accent/9 pr-2 pl-2.5 text-xs font-medium text-accent"
          @click="onRemoveChip(chip.axis, chip.value)"
        >
          <span class="text-[0.6rem] font-bold tracking-[0.1em] uppercase opacity-75">{{ chip.label }}</span>
          {{ chip.valueLabel }}
          <svg
            class="size-3"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            aria-hidden="true"
          >
            <path d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
        <span class="min-w-2 flex-1" />
        <div
          role="group"
          aria-label="Board presentation"
          class="inline-flex items-center gap-1 rounded-full border border-line bg-surface p-0.75"
          data-testid="board-presentation"
        >
          <button
            v-for="option in BOARD_PRESENTATIONS"
            :key="option"
            type="button"
            class="h-6.5 rounded-full border px-3 text-xs font-semibold transition-colors"
            :class="
              option === presentation
                ? 'border-accent bg-accent/12 text-accent'
                : 'border-transparent text-ink-muted hover:text-ink'
            "
            :aria-pressed="option === presentation"
            :data-testid="`board-presentation-${option}`"
            @click="revise({ mode: option })"
          >
            {{ PRESENTATION_LABELS[option] }}
          </button>
        </div>
        <span
          class="mx-0.5 h-4.5 w-px bg-line"
          aria-hidden="true"
        />
        <button
          type="button"
          data-testid="toggle-draft"
          :aria-pressed="draft.shown"
          :disabled="draft.disabled"
          :title="draft.disabled ? 'Draft shows while it holds tickets' : undefined"
          class="inline-flex h-7 items-center gap-2 rounded-full border border-line bg-surface px-3 text-xs font-medium text-ink transition-colors hover:border-line-strong disabled:cursor-default disabled:opacity-70"
          @click="revise({ hidden_columns: draft.next })"
        >
          {{ draft.label }}
          <span
            v-if="!draft.shown"
            class="rounded-full bg-tint px-1.5 py-px font-mono text-[0.625rem] text-ink-subtle"
            aria-live="polite"
            data-testid="draft-count"
          >{{ draftCount }}</span>
        </button>
        <div
          v-for="control in layoutControls"
          :key="control.axis"
          role="group"
          :aria-label="`${control.collapsedLabel} layout`"
          class="inline-flex items-center gap-1 rounded-full border border-line bg-surface p-0.75"
          :data-testid="`layout-axis-${control.axis}`"
        >
          <button
            v-for="option in (['collapsed', 'expanded'] as const)"
            :key="option"
            type="button"
            class="h-6.5 rounded-full border px-3 text-xs font-semibold transition-colors"
            :class="
              option === control.layout
                ? 'border-accent bg-accent/12 text-accent'
                : 'border-transparent text-ink-muted hover:text-ink'
            "
            :aria-pressed="option === control.layout"
            :data-testid="`layout-axis-${control.axis}-${option}`"
            @click="revise({ expanded_groups: axisSetTo(control.axis, option) })"
          >
            {{ option === 'collapsed' ? control.collapsedLabel : control.expandedLabel }}
          </button>
        </div>
      </div>
    </header>

    <div class="flex flex-1 flex-col gap-3 px-3 py-3 lg:px-4">
      <p
        v-if="projectMissing"
        data-testid="board-project-missing"
        class="text-sm text-critical"
      >
        Project {{ scope }} is not registered.
      </p>

      <InlineAlert
        v-if="board.error || savedViews.error"
        data-testid="board-error"
      >
        {{ board.error ?? savedViews.error }}
      </InlineAlert>

      <InlineAlert
        v-if="notice"
        tone="caution"
        data-testid="board-notice"
      >
        {{ notice }}
      </InlineAlert>

      <div
        v-if="!settled && !board.error && !projectMissing"
        class="flex flex-col gap-3 pb-2"
        :class="presentation === 'board' && !shell.narrow ? 'overflow-x-auto md:flex-row' : undefined"
        data-testid="board-loading"
      >
        <div
          v-for="column in loadingColumns"
          :key="column"
          class="flex flex-1 flex-col gap-3 rounded-panel border border-line bg-surface/80 p-3"
          :class="presentation === 'board' ? 'min-h-72 md:min-w-56' : ''"
        >
          <SkeletonBlock class="h-4 w-20" />
          <SkeletonBlock class="h-20" />
        </div>
      </div>

      <div
        v-else-if="emptyScope"
        data-testid="board-empty"
        class="mx-auto mt-12 flex max-w-lg flex-col items-center gap-4 rounded-panel border border-dashed border-line-strong bg-surface px-7 py-7 text-center"
      >
        <h2 class="font-display text-lg font-semibold text-ink">
          {{ scope === 'all' ? 'No tickets yet' : 'No tickets in this Project yet' }}
        </h2>
        <p class="text-sm leading-relaxed text-ink-muted">
          Write a Spec first, then generate Implementation Tickets from its user stories. Bugs and
          Tasks can be captured at any time.
        </p>
        <div class="flex gap-2">
          <RouterLink
            to="/planning"
            class="inline-flex h-7.5 items-center rounded-control border border-brand-500 bg-brand-gradient px-3 text-xs font-semibold text-cta-ink"
          >
            Open planning
          </RouterLink>
          <button
            type="button"
            data-testid="new-ticket-empty"
            class="inline-flex h-7.5 items-center rounded-control border border-line-strong bg-surface px-3 text-xs font-medium text-ink"
            @click="openNewTicket"
          >
            New ticket
          </button>
        </div>
      </div>

      <div
        v-else-if="emptyFiltered"
        data-testid="board-filtered-empty"
        class="mx-auto mt-12 flex max-w-lg flex-col items-center gap-3 rounded-panel border border-dashed border-line-strong bg-surface px-7 py-7 text-center"
      >
        <h2 class="font-display text-base font-semibold text-ink">
          No tickets match these filters
        </h2>
        <p class="text-sm text-ink-muted">
          {{ activeFilters }} filter{{ activeFilters === 1 ? '' : 's' }} active. Deterministic
          ordering and column preferences are unchanged.
        </p>
        <AppButton
          variant="secondary"
          size="sm"
          data-testid="filters-clear-inline"
          @click="clearFilters"
        >
          Clear filters
        </AppButton>
      </div>

      <BoardRegister
        v-else-if="presentation === 'register'"
        :columns="registerColumns"
        :moving="moving"
        @select="(row) => openTicket(row.ticket.id)"
        @move="onRegisterMove"
      />

      <template v-else>
        <div
          class="flex items-stretch gap-3 pb-2"
          :class="shell.narrow ? 'flex-col' : 'flex-row overflow-x-auto'"
          data-testid="kanban-board"
          :data-backlog-layout="layouts.backlog"
          :data-completion-layout="layouts.completion"
          :data-hidden-columns="hidden.join(' ')"
          :data-done-presentation="done"
        >
          <div
            v-for="group in groups"
            :key="group.id"
            class="flex flex-col gap-3"
            :class="[
              shell.narrow ? 'w-full' : 'basis-0',
              group.grouped ? 'rounded-panel border border-tint-line bg-tint p-2' : '',
              !shell.narrow && group.columns.every((column) => column.collapsed) ? 'grow-0' : 'grow',
            ]"
            :style="
              shell.narrow
                ? undefined
                : {
                  minWidth: `${group.columns.reduce(
                    (total, column) => total + (column.collapsed ? COLLAPSED_COLUMN_WIDTH_PX : 224),
                    group.grouped ? (group.columns.length - 1) * 12 + 16 : 0,
                  )}px`,
                }
            "
            :role="group.grouped ? 'group' : undefined"
            :aria-labelledby="group.grouped ? `kanban-group-heading-${group.id}` : undefined"
            :data-testid="`kanban-group-${group.id}`"
            :data-grouped="group.grouped ? 'true' : 'false'"
          >
            <header
              v-if="group.grouped"
              class="flex flex-col gap-1 px-1 pt-1"
              :data-testid="`kanban-group-header-${group.id}`"
            >
              <div class="flex items-baseline justify-between gap-2">
                <h2
                  :id="`kanban-group-heading-${group.id}`"
                  class="font-display text-sm font-semibold tracking-tight text-ink"
                >
                  {{ group.heading }}
                </h2>
                <span class="rounded-full bg-surface px-2 py-0.5 font-mono text-xs text-ink-muted">
                  {{ group.count }}
                </span>
              </div>
              <p class="text-[0.625rem] tracking-wide text-ink-subtle uppercase">
                {{ group.subheading }}
              </p>
            </header>

            <div
              class="flex flex-1 gap-3"
              :class="shell.narrow ? 'flex-col' : 'flex-row'"
            >
              <section
                v-for="column in group.columns"
                :key="column.id"
                class="flex flex-col rounded-panel border bg-surface/80 transition-colors"
                :class="[
                  column.collapsed ? 'items-stretch' : 'min-h-72 gap-3 p-3',
                  dropTarget === column.id && dropHighlight(column.id)
                    ? 'border-accent/60 bg-accent/8'
                    : dropHighlight(column.id)
                      ? 'border-accent/50 bg-accent/6'
                      : dropTarget === column.id
                        ? 'border-critical/50'
                        : 'border-line',
                ]"
                :style="columnWidthStyle(column.collapsed)"
                :data-testid="`kanban-column-${column.id}`"
                :data-collapsed="column.collapsed ? 'true' : 'false'"
                :aria-label="`${column.label} column, ${column.cards.length} ${column.cards.length === 1 ? 'ticket' : 'tickets'}${column.collapsed ? ', collapsed' : ''}`"
                @dragover="onDragOver(column.id, $event)"
                @dragleave="onDragLeave(column.id)"
                @drop="onDrop(column.id, $event)"
              >
                <div
                  v-if="column.collapsed"
                  class="flex h-full items-center gap-2"
                  :class="shell.narrow ? 'flex-row px-2.5' : 'flex-col py-2'"
                  :data-testid="`column-rail-${column.id}`"
                >
                  <button
                    type="button"
                    class="flex size-6.5 shrink-0 items-center justify-center rounded-control border border-line bg-surface text-ink-muted hover:text-ink"
                    :aria-expanded="false"
                    :aria-label="`Expand ${column.label} column`"
                    :data-testid="`column-expand-${column.id}`"
                    @click="toggleColumnCollapsed(column.id)"
                  >
                    <ChevronIcon :direction="shell.narrow ? 'down' : 'up'" />
                  </button>
                  <span
                    class="rounded-[5px] border border-line px-1 font-mono text-[0.625rem] font-semibold text-accent"
                    :data-testid="`column-rail-count-${column.id}`"
                  >{{ column.cards.length }}</span>
                  <span
                    class="flex flex-1 items-center justify-center font-display text-[0.72rem] font-semibold tracking-[0.04em] whitespace-nowrap text-ink-muted"
                    :class="shell.narrow ? '' : 'rotate-180 [writing-mode:vertical-rl]'"
                  >{{ column.label }}</span>
                </div>

                <template v-else>
                  <header class="flex flex-col gap-1 px-1">
                    <div class="flex items-baseline justify-between gap-2">
                      <h2
                        :id="`kanban-heading-${column.id}`"
                        class="font-display text-sm font-semibold tracking-tight text-ink"
                      >
                        {{ column.label }}
                      </h2>
                      <div class="flex shrink-0 items-center gap-1">
                        <span class="rounded-full bg-tint px-2 py-0.5 font-mono text-xs text-ink-muted">
                          {{ column.cards.length }}
                        </span>
                        <AppButton
                          v-if="column.id === 'done' && done === 'column'"
                          variant="ghost"
                          size="iconSm"
                          class="-my-1.5"
                          aria-label="Move Done below the board"
                          data-testid="move-done-below-board"
                          @click="pushDoneDown"
                        >
                          <ChevronIcon direction="down" />
                        </AppButton>
                        <AppButton
                          variant="ghost"
                          size="iconSm"
                          class="-my-1.5"
                          :aria-expanded="true"
                          :aria-label="`Collapse ${column.label} column`"
                          :data-testid="`column-collapse-${column.id}`"
                          @click="toggleColumnCollapsed(column.id)"
                        >
                          <svg
                            class="size-4"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="1.6"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                            aria-hidden="true"
                          >
                            <path d="M13 6l-6 6 6 6M19 6l-6 6 6 6" />
                          </svg>
                        </AppButton>
                      </div>
                    </div>
                    <p
                      v-if="column.blurb"
                      class="text-[0.625rem] tracking-wide text-ink-subtle uppercase"
                    >
                      {{ column.blurb }}
                    </p>
                  </header>

                  <EmptyState
                    v-if="column.cards.length === 0"
                    compact
                    class="flex-1"
                    message="Nothing here yet."
                  />

                  <ul
                    v-else
                    class="flex flex-col gap-3"
                    :aria-labelledby="`kanban-heading-${column.id}`"
                  >
                    <li
                      v-for="card in column.cards"
                      :key="`${card.ticket.project_id}:${card.ticket.id}`"
                    >
                      <BoardCard
                        :ticket-id="card.ticket.id"
                        :number="cardNumber(card)"
                        :title="cardTitle(card.ticket)"
                        :kind-label="KIND_LABELS[card.ticket.kind]"
                        :project-code="card.project_code"
                        :show-project="scope === 'all'"
                        :chips="cardChips(card)"
                        :status-label="STATUS_LABELS[card.ticket.state]"
                        :status-tone="STATUS_TONES[card.ticket.state]"
                        :shows-status="showsCardStatus(column.id)"
                        :draggable="!moving && canDrag(card.ticket)"
                        :dragging="drag?.ticket.id === card.ticket.id"
                        :chrome="cardChrome(card.ticket, column.id)"
                        :label="cardLabel(card, column.id)"
                        :kind="card.ticket.kind"
                        :state="card.ticket.state"
                        @open="openTicket(card.ticket.id)"
                        @dragstart="onDragStart(card, $event)"
                        @dragend="onDragEnd"
                      />
                    </li>
                  </ul>
                </template>
              </section>
            </div>
          </div>
        </div>

        <DoneBoardTable
          v-if="done === 'table' && !hidden.includes('done')"
          :rows="doneRows"
          :drop-active="dropTarget === 'done' || dropHighlight('done')"
          @select="(row) => openTicket(row.ticket.id)"
          @promote="bringDoneBack"
          @dragover="onDragOver('done', $event)"
          @dragleave="onDragLeave('done')"
          @drop="onDrop('done', $event)"
        />
      </template>
    </div>

    <BoardColumnsFlyout
      :open="columnsOpen"
      :rows="columnRows"
      :summary="columnsSummary"
      @close="columnsOpen = false"
      @toggle-hidden="toggleHidden"
      @toggle-collapsed="toggleGroupCollapsed"
      @show-all="showAllColumns"
    />

    <BoardFiltersFlyout
      :open="filtersOpen"
      :axes="axes"
      :active-count="activeFilters"
      :shown="held.length"
      :total="board.scopeTotal"
      :scoped-project-label="scopedProjectLabel"
      @close="filtersOpen = false"
      @change="onFilterChange"
      @clear="clearFilters"
    />

    <DetailDrawer
      :open="openTicketId !== null"
      :title="drawerTitle"
      :number="drawerNumber"
      size="wide"
      @close="closeDrawer"
    >
      <template #subtitle>
        {{ drawerProjectCode ? `Project · ${drawerProjectCode}` : '' }}
      </template>

      <p
        v-if="drawerLoading"
        class="text-sm text-ink-subtle"
        data-testid="drawer-loading"
      >
        Loading ticket detail…
      </p>
      <InlineAlert
        v-else-if="drawerError"
        data-testid="drawer-error"
      >
        {{ drawerError }}
      </InlineAlert>
      <template v-else-if="drawerTicket">
        <dl class="flex flex-col gap-3">
          <div
            v-for="fact in drawerFacts"
            :key="fact.label"
            class="flex items-baseline justify-between gap-4 border-b border-line pb-2 last:border-b-0"
          >
            <dt class="text-[0.625rem] font-semibold tracking-[0.06em] text-ink-subtle uppercase">
              {{ fact.label }}
            </dt>
            <dd
              v-if="fact.label === 'State'"
              data-testid="drawer-state"
            >
              <StatusBadge :tone="STATUS_TONES[drawerTicket.state]">
                {{ fact.value }}
              </StatusBadge>
            </dd>
            <dd
              v-else-if="fact.label === 'Spec'"
              data-testid="drawer-spec"
              class="text-sm text-ink"
            >
              {{ fact.value }}
            </dd>
            <dd
              v-else
              class="text-sm text-ink"
            >
              {{ fact.value }}
            </dd>
          </div>
        </dl>

        <TicketDetailSections
          class="mt-6"
          :ticket="drawerTicket"
        />

        <section
          v-if="drawerTicket.completion.length > 0"
          class="mt-6 flex flex-col gap-2"
          data-testid="drawer-completion"
        >
          <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
            Completion criteria
          </h3>
          <ul class="flex flex-col gap-2">
            <li
              v-for="(outcome, position) in drawerTicket.completion"
              :key="position"
              class="rounded-control border border-line bg-surface/70 px-3 py-2 text-sm text-ink"
            >
              {{ outcome }}
            </li>
          </ul>
        </section>

        <AttemptHistory
          class="mt-6"
          :attempts="drawerAttempts"
          @recovered="refreshAttempts"
        />

        <div
          class="mt-6"
          data-testid="drawer-timeline"
        >
          <!-- Keyed on the record's version: a command run from the
               footer appends its audit row, and the timeline re-reads
               rather than standing on the answer it had before
               (KAN-T139-AC5). -->
          <TimelineSurface
            :key="`${drawerTicket.id}:${drawerTicket.version}:${drawerReviewTick}`"
            :scope="{ project: drawerTicket.project_id }"
            entity-kind="ticket"
            :entity-id="drawerTimelineId"
          />
        </div>
      </template>

      <template
        v-if="drawerTicket"
        #footer
      >
        <TicketDrawerActions
          :ticket="drawerTicket"
          :legal-targets="board.legalTargetsFor(drawerTicket.id)"
          :state-labels="STATUS_LABELS"
          @acted="drawerActed"
          @reviewed="drawerReviewed"
        />
      </template>
    </DetailDrawer>
  </main>
</template>
