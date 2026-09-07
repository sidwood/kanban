<script setup lang="ts">
// The Project board: the SmokeFree Surface presentation — typography,
// colours, themes, spacing, shadows, grouped columns, collapsible
// axes, responsive layout, the Board/Register switch, the Done table
// option, the detail drawer, and the drag interaction language —
// speaking the Kanban domain (KAN-T24-AC1, KAN-T24-AC2). Every card
// is a real Ticket from the generated client wearing the chip
// vocabulary the application schema pins (KAN-T26-AC1 to
// KAN-T26-AC3), and a drag is one ticket.transition the core judges;
// a refusal — an agent-owned drag above all — surfaces as the core's
// own explanation (KAN-T24-AC3). The board belongs to one Project at
// a time: changing Project empties it before the next load settles
// (KAN-T125).
import { computed, inject, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import type { TicketRecord, TicketState } from '@kanban/contracts'
import AppButton from '../components/AppButton.vue'
import BoardRegister from '../components/BoardRegister.vue'
import ChevronIcon from '../components/ChevronIcon.vue'
import DetailDrawer from '../components/DetailDrawer.vue'
import EmptyState from '../components/EmptyState.vue'
import InlineAlert from '../components/InlineAlert.vue'
import SectionHeader from '../components/SectionHeader.vue'
import SkeletonBlock from '../components/SkeletonBlock.vue'
import StatusBadge from '../components/StatusBadge.vue'
import DoneBoardTable from '../components/DoneBoardTable.vue'
import { kanbanTransportKey } from '../core/transport'
import { applyTheme, loadTheme, saveTheme } from '../core/theme'
import type { ThemeName } from '../core/theme'
import { useBoardStore } from '../stores/board'
import { useProjectRegisterStore } from '../stores/project-register'
import {
  BOARD_PRESENTATIONS,
  type BoardColumnId,
  type BoardPresentation,
  type BoardLayoutAxis,
  type DonePresentation,
  boardColumnGroups,
  boardColumnLabel,
  boardColumnSubheading,
  boardLayoutAxisControls,
  columnForCard,
  columnHoldsManyStates,
  dropFor,
  isOnBoard,
  registerColumnsFor,
  resolveHiddenColumns,
  visibleColumnsFor,
} from './board-layout'
import {
  KIND_LABELS,
  STATUS_LABELS,
  STATUS_TONES,
  boardCardNumber,
  boardCardTitle,
  statusSurfaceClass,
} from './board-card'
import type { BoardRegisterColumn, BoardRegisterRow } from './board-card'
import { chipSurfaceClass, chipsFor, laneFor, specFor } from './board-chips'
import type { CardChip } from './board-chips'
import { useLanesStore } from '../stores/lanes'
import { useRunsStore } from '../stores/runs'
import {
  fallbackOwnedSet,
  ownedCopy,
  useSavedViewsStore,
  type ViewOwnedSet,
} from '../stores/saved-views'
import { orderCards } from './board-ordering'

/**
 * A group grows with the number of columns it holds, so a four-column
 * Backlog is not squeezed into the width Current gets. Written out
 * rather than interpolated: Tailwind generates the classes it can see.
 */
const GROUP_GROW_CLASSES: Record<number, string> = {
  1: 'md:grow-[1]',
  2: 'md:grow-[2]',
  3: 'md:grow-[3]',
  4: 'md:grow-[4]',
}

const PRESENTATION_LABELS: Record<BoardPresentation, string> = {
  board: 'Board',
  register: 'Register',
}

const THEME_LABELS: Record<ThemeName, string> = {
  light: 'Daylight',
  dark: 'Night',
}

const THEME_ENTRIES = Object.entries(THEME_LABELS) as [ThemeName, string][]

const transport = inject(kanbanTransportKey)
const route = useRoute()
const projects = useProjectRegisterStore()
const board = useBoardStore()
const lanes = useLanesStore()
const runs = useRunsStore()
const savedViews = useSavedViewsStore()

const projectId = computed(() => Number(route.params.projectId))

const project = computed(
  () => projects.projects.find((entry) => entry.id === projectId.value) ?? null,
)

const projectCode = computed(() => project.value?.code ?? '')

// The presentation choices the active view owns (KAN-T28): the
// Project's active Saved View supplies every property — expanded
// groups, hidden columns, mode, Done placement, sorting — and one
// choice writes through to the authoritative store, never to browser
// state. Until the views load, the everyday perspective stands in.
const activeProjectView = computed(() => savedViews.activeProjectView(projectId.value))
const owned = computed<ViewOwnedSet>(() =>
  activeProjectView.value ? ownedCopy(activeProjectView.value) : fallbackOwnedSet(),
)

function choose(next: Partial<ViewOwnedSet>): void {
  const view = activeProjectView.value
  if (view === null || transport === undefined) return
  void savedViews.reviseOwnedSet(transport, view.id, next)
}

const layouts = computed(() => ({
  backlog: owned.value.expanded_groups.includes('backlog') ? ('expanded' as const) : ('collapsed' as const),
  completion: owned.value.expanded_groups.includes('staged') ? ('expanded' as const) : ('collapsed' as const),
}))
const done = computed<DonePresentation>(() => owned.value.done_placement)
const presentation = computed<BoardPresentation>(() => owned.value.mode)

// The axis toggle the operator clicked, as the expanded group set the
// next view.update carries.
function toggleAxis(axis: BoardLayoutAxis): ViewOwnedSet['expanded_groups'] {
  const group = axis === 'backlog' ? 'backlog' : 'staged'
  const current = owned.value.expanded_groups
  return current.includes(group)
    ? current.filter((entry) => entry !== group)
    : [...current, group]
}

// The class-based theme the board header swaps.
const theme = ref(loadTheme())

onMounted(() => {
  applyTheme(theme.value)
})

function setTheme(next: ThemeName): void {
  theme.value = next
  applyTheme(next)
  saveTheme(next)
}

onMounted(() => {
  clearBoard()
  void load()
})

watch(projectId, () => {
  clearBoard()
  void load()
})

// A board arriving — mounted over a store that may still hold
// another Project's, or navigated from one — carries nothing of the
// Project before it: cards, counts, and the drawer leave before the
// next load settles, and a load that never settles leaves the board
// empty rather than showing the wrong Project (KAN-T125-AC1).
function clearBoard(): void {
  closeDrawer()
  board.clear()
}

async function load(): Promise<void> {
  if (!transport) return
  await projects.refresh(transport)
  // The active view owns the presentation this board renders; its
  // refresh lands before the tickets so the first frame is already
  // the operator's perspective.
  await savedViews.refresh(transport)
  if (project.value) {
    // The Lanes and the runs arrive beside the Tickets: the Lane chip
    // a card wears comes from the KAN-T32 contract, and its execution
    // chips from the run records the core owns — never from board
    // state.
    await Promise.all([
      board.refresh(transport, projectId.value),
      lanes.load(transport, projectId.value),
      runs.load(transport, projectId.value),
    ])
  }
}

// Only Tickets the board can place reach the columns; terminal
// states keep their history off the active board. The order is the
// deterministic one — priority, readiness, number — so every column,
// the register, and the Done table scan the same way, and no manual
// ordering exists anywhere (DR-LC-11).
const cards = computed(() =>
  orderCards(
    board.tickets.filter((ticket) => isOnBoard(ticket.state)),
    owned.value.sorting,
  ),
)

const draftCount = computed(
  () => board.tickets.filter((ticket) => ticket.state === 'draft').length,
)

// The hidden columns the view owns, resolved for rendering: Draft
// shows while cards sit in it, whatever the view says.
const hidden = computed(() => resolveHiddenColumns(owned.value.hidden_columns, draftCount.value))

function toggleDraft(): void {
  const current = owned.value.hidden_columns
  choose({
    hidden_columns: current.includes('draft')
      ? current.filter((group) => group !== 'draft')
      : [...current, 'draft'],
  })
}

const layoutControls = computed(() => boardLayoutAxisControls(layouts.value))

const groups = computed(() =>
  boardColumnGroups(layouts.value, done.value, hidden.value).map((group) => {
    const columns = group.columns.map((column) => ({
      id: column,
      label: boardColumnLabel(column),
      blurb: boardColumnSubheading(column),
      cards: cards.value.filter(
        (ticket) => columnForCard(ticket.state, layouts.value) === column,
      ),
    }))
    return {
      id: group.id,
      heading: group.heading,
      subheading: group.subheading,
      grouped: group.grouped,
      growClass: GROUP_GROW_CLASSES[columns.length] ?? 'md:grow-[1]',
      count: columns.reduce((total, column) => total + column.cards.length, 0),
      columns,
    }
  }),
)

/** As many placeholders as the presentation is about to fill. */
const loadingColumns = computed(() =>
  presentation.value === 'board'
    ? visibleColumnsFor(layouts.value, done.value, hidden.value)
    : registerColumnsFor(layouts.value, hidden.value),
)

const registerColumns = computed<readonly BoardRegisterColumn[]>(() =>
  registerColumnsFor(layouts.value, hidden.value).map((column) => ({
    id: column,
    label: boardColumnLabel(column),
    subheading: boardColumnSubheading(column),
    showsStatus: columnHoldsManyStates(column, layouts.value),
    rows: cards.value
      .filter((ticket) => columnForCard(ticket.state, layouts.value) === column)
      .map((ticket) => registerRow(ticket)),
  })),
)

function registerRow(ticket: TicketRecord): BoardRegisterRow {
  return {
    ticket,
    number: boardCardNumber(ticket, projectCode.value),
    title: boardCardTitle(ticket),
    kindLabel: KIND_LABELS[ticket.kind],
    statusLabel: STATUS_LABELS[ticket.state],
    statusTone: STATUS_TONES[ticket.state],
    moves: movesFor(ticket),
  }
}

// A register row is moved by naming its target; the drag that serves
// Task Tickets alone serves the register alone too.
function movesFor(ticket: TicketRecord): readonly { column: BoardColumnId; label: string }[] {
  if (ticket.kind !== 'task') return []
  return registerColumnsFor(layouts.value, hidden.value)
    .filter(
      (column) =>
        column !== 'draft' && columnForCard(ticket.state, layouts.value) !== column,
    )
    .map((column) => ({ column, label: boardColumnLabel(column) }))
}

const doneRows = computed<readonly BoardRegisterRow[]>(() =>
  done.value === 'table'
    ? cards.value
        .filter((ticket) => columnForCard(ticket.state, layouts.value) === 'done')
        .map((ticket) => registerRow(ticket))
    : [],
)

// The drag: only what the core may accept is highlighted, and the
// move itself is one command the core judges.
function onSwitchView(event: Event): void {
  const viewId = Number((event.target as HTMLSelectElement).value)
  savedViews.switchProjectView(projectId.value, viewId)
}

const drag = ref<TicketRecord | null>(null)
const dropTarget = ref<string | null>(null)
const moving = ref(false)

function canDrag(ticket: TicketRecord): boolean {
  return ticket.kind === 'task'
}

function canDropOn(target: BoardColumnId): boolean {
  return drag.value !== null && !moving.value &&
    dropFor(drag.value.state, target, layouts.value) !== undefined
}

function onDragStart(ticket: TicketRecord, event: DragEvent): void {
  drag.value = ticket
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', String(ticket.id))
  }
}

function onDragEnd(): void {
  dropTarget.value = null
  drag.value = null
}

function onDragOver(target: BoardColumnId, event: DragEvent): void {
  if (!canDropOn(target)) return
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
  if (card === null || moving.value) return
  const drop = dropFor(card.state, target, layouts.value)
  if (drop === undefined) return
  event.preventDefault()
  await applyMove(card, drop.state)
}

async function onRegisterMove(
  row: BoardRegisterRow,
  target: BoardColumnId,
): Promise<void> {
  if (moving.value) return
  const drop = dropFor(row.ticket.state, target, layouts.value)
  if (drop === undefined) return
  await applyMove(row.ticket, drop.state)
}

async function applyMove(ticket: TicketRecord, to: TicketState): Promise<void> {
  if (!transport) return
  moving.value = true
  try {
    await board.move(transport, ticket.id, to)
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

function cardLabel(ticket: TicketRecord, column: BoardColumnId): string {
  const parts = [boardCardTitle(ticket)]
  if (showsCardStatus(column)) parts.push(STATUS_LABELS[ticket.state])
  return parts.join(', ')
}

/** A Task Ticket keeps its dashed border wherever it sits. */
function cardChrome(ticket: TicketRecord, column: BoardColumnId): string {
  const dashed = ticket.kind === 'task'
  if (showsCardStatus(column)) {
    const surface = statusSurfaceClass(ticket.state)
    return dashed ? `border-dashed ${surface}` : surface
  }
  return dashed
    ? 'border-dashed border-line-strong bg-surface/70'
    : 'border-line'
}

// The chips one card wears, resolved from the vocabulary against the
// Ticket and the facts the board holds. The Spec identity is the
// number its record minted; a Spec the board did not load leaves the
// chip off rather than showing the row id. During execution the
// profile chips speak the run's frozen effective snapshot; before
// dispatch they show the planned profile the assignment names.
function cardChips(ticket: TicketRecord): readonly CardChip[] {
  return chipsFor(ticket, {
    projectCode: projectCode.value,
    spec: specFor(board.specs, ticket.spec_id),
    lane: laneFor(lanes.lanes, ticket.id),
    blockers: board.blockersFor(ticket.id),
    reviewers: [],
    execution: runs.executionFor(ticket.id),
  })
}

// The detail drawer.
const openTicketId = ref<number | null>(null)

const selectedTicket = computed(
  () => cards.value.find((ticket) => ticket.id === openTicketId.value) ?? null,
)

const drawerTitle = computed(() =>
  selectedTicket.value ? boardCardTitle(selectedTicket.value) : 'Ticket',
)

const drawerNumber = computed(() =>
  selectedTicket.value
    ? boardCardNumber(selectedTicket.value, projectCode.value)
    : undefined,
)

function openTicket(ticket: TicketRecord): void {
  openTicketId.value = ticket.id
}

function closeDrawer(): void {
  openTicketId.value = null
}

// The facts the drawer shows for the open Ticket; full history and
// the embedded timeline arrive with the drawer's own ticket. The Spec
// identity is the number its record minted — a Spec the board did not
// load states no identity at all rather than one built from the row
// id the Ticket carries.
const drawerFacts = computed(() => {
  const ticket = selectedTicket.value
  if (!ticket) return []
  const facts: { label: string; value: string }[] = [
    { label: 'Kind', value: KIND_LABELS[ticket.kind] },
    { label: 'State', value: STATUS_LABELS[ticket.state] },
    { label: 'Priority', value: ticket.priority },
    { label: 'Project', value: `${projectCode.value} — ${project.value?.name ?? ''}` },
  ]
  const spec = specFor(board.specs, ticket.spec_id)
  if (spec) {
    facts.push({ label: 'Spec', value: `${projectCode.value}-S${spec.number}` })
  }
  if (ticket.subtype) facts.push({ label: 'Subtype', value: ticket.subtype })
  if (ticket.mode) facts.push({ label: 'Mode', value: ticket.mode })
  if (ticket.scheduled_for) {
    facts.push({ label: 'Scheduled for', value: ticket.scheduled_for })
  }
  if (ticket.due) facts.push({ label: 'Due', value: ticket.due })
  return facts
})
</script>

<template>
  <main
    class="animate-rise flex min-h-screen flex-col gap-8 px-6 py-8 lg:px-8"
    :aria-busy="!board.loaded || undefined"
  >
    <SectionHeader
      eyebrow="Kanban"
      :title="project ? `${project.code} — ${project.name}` : 'Board'"
      summary="Task Tickets drag freely; Implementation and Bug transitions belong to their agents, and the core refuses an illegal drag."
    >
      <template #actions>
        <div
          class="flex flex-wrap items-center gap-x-3 gap-y-2"
          data-testid="board-layout-controls"
        >
          <div
            role="group"
            aria-label="Theme"
            class="inline-flex items-center gap-1 rounded-full border border-line bg-canvas/60 p-1"
            data-testid="board-theme"
          >
            <button
              v-for="[name, label] in THEME_ENTRIES"
              :key="name"
              type="button"
              class="rounded-full border px-3 py-1 text-xs font-medium transition-colors"
              :class="
                name === theme
                  ? 'border-accent bg-accent/12 font-semibold text-ink'
                  : 'border-transparent text-ink-muted hover:border-accent/40 hover:text-ink'
              "
              :aria-pressed="name === theme"
              :data-testid="`theme-${name}`"
              @click="setTheme(name)"
            >
              {{ label }}
            </button>
          </div>
          <div
            role="group"
            aria-label="Draft visibility"
          >
            <AppButton
              variant="secondary"
              size="sm"
              :aria-pressed="!hidden.includes('draft')"
              data-testid="toggle-draft"
              @click="toggleDraft"
            >
              {{ hidden.includes('draft') ? 'Show Draft' : 'Hide Draft' }}
              <span
                v-if="hidden.includes('draft')"
                class="rounded-full bg-line px-1.5 py-0.5 font-mono text-[0.625rem] text-ink-muted"
                aria-live="polite"
                data-testid="draft-count"
              >
                {{ draftCount }}
              </span>
            </AppButton>
          </div>
          <div
            role="group"
            aria-label="Saved view"
          >
            <select
              :value="activeProjectView?.id ?? ''"
              data-testid="board-view-select"
              class="rounded-full border border-line bg-canvas/60 px-3 py-1 text-xs text-ink"
              :aria-label="`Saved view for ${project?.code ?? 'the board'}`"
              @change="onSwitchView"
            >
              <option
                v-for="view in savedViews.projectViews(projectId)"
                :key="view.id"
                :value="view.id"
              >
                {{ view.name }}
              </option>
            </select>
          </div>
          <div
            role="group"
            aria-label="Board presentation"
            class="inline-flex items-center gap-1 rounded-full border border-line bg-canvas/60 p-1"
            data-testid="board-presentation"
          >
            <button
              v-for="option in BOARD_PRESENTATIONS"
              :key="option"
              type="button"
              class="rounded-full border px-3 py-1 text-xs font-medium transition-colors"
              :class="
                option === presentation
                  ? 'border-accent bg-accent/12 font-semibold text-ink'
                  : 'border-transparent text-ink-muted hover:border-accent/40 hover:text-ink'
              "
              :aria-pressed="option === presentation"
              :data-testid="`board-presentation-${option}`"
              @click="choose({ mode: option })"
            >
              {{ PRESENTATION_LABELS[option] }}
            </button>
          </div>
          <div
            v-for="control in layoutControls"
            :key="control.axis"
            role="group"
            :aria-label="`${control.collapsedLabel} layout`"
            class="inline-flex items-center gap-1 rounded-full border border-line bg-canvas/60 p-1"
            :data-testid="`layout-axis-${control.axis}`"
          >
            <button
              v-for="option in (['collapsed', 'expanded'] as const)"
              :key="option"
              type="button"
              class="rounded-full border px-3 py-1 text-xs font-medium transition-colors"
              :class="
                option === control.layout
                  ? 'border-accent bg-accent/12 font-semibold text-ink'
                  : 'border-transparent text-ink-muted hover:border-accent/40 hover:text-ink'
              "
              :aria-pressed="option === control.layout"
              :data-testid="`layout-axis-${control.axis}-${option}`"
              @click="choose({ expanded_groups: toggleAxis(control.axis) })"
            >
              {{ option === 'collapsed' ? control.collapsedLabel : control.expandedLabel }}
            </button>
          </div>
        </div>
      </template>
    </SectionHeader>

    <p
      v-if="projects.loaded && !project"
      data-testid="board-project-missing"
      class="text-sm text-critical"
    >
      Project {{ projectId }} is not registered.
    </p>

    <InlineAlert
      v-if="board.error || lanes.error || savedViews.error"
      data-testid="board-error"
    >
      {{ board.error ?? lanes.error ?? savedViews.error }}
    </InlineAlert>

    <div
      v-if="!board.loaded && !board.error"
      class="flex flex-col gap-3 pb-2"
      :class="presentation === 'board' ? 'overflow-x-auto md:flex-row' : undefined"
      data-testid="board-loading"
    >
      <div
        v-for="column in loadingColumns"
        :key="column"
        class="flex flex-1 flex-col gap-3 rounded-panel border border-line bg-surface/80 p-3"
        :class="presentation === 'board' ? 'min-h-[18rem] md:min-w-[14rem]' : ''"
      >
        <SkeletonBlock class="h-4 w-20" />
        <SkeletonBlock class="h-20" />
      </div>
    </div>

    <BoardRegister
      v-else-if="presentation === 'register'"
      :columns="registerColumns"
      :moving="moving"
      @select="(row) => openTicket(row.ticket)"
      @move="onRegisterMove"
    />

    <template v-else>
      <div
        class="flex flex-col items-stretch gap-3 overflow-x-auto pb-2 md:flex-row"
        data-testid="kanban-board"
        :data-backlog-layout="layouts.backlog"
        :data-completion-layout="layouts.completion"
        :data-hidden-columns="hidden.join(' ')"
        :data-done-presentation="done"
      >
        <div
          v-for="group in groups"
          :key="group.id"
          class="flex flex-col gap-3 md:basis-0"
          :class="[
            group.growClass,
            group.grouped ? 'rounded-panel bg-line-strong p-2' : '',
          ]"
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
              <span class="rounded-full bg-line px-2 py-0.5 font-mono text-xs text-ink-muted">
                {{ group.count }}
              </span>
            </div>
            <p class="text-[0.625rem] tracking-wide text-ink-subtle uppercase">
              {{ group.subheading }}
            </p>
          </header>

          <div class="flex flex-1 flex-col gap-3 md:flex-row">
            <section
              v-for="column in group.columns"
              :key="column.id"
              class="flex min-h-[18rem] flex-1 flex-col gap-3 rounded-panel border border-line bg-surface/80 p-3 transition-colors md:min-w-[14rem]"
              :class="{
                'border-accent/50 bg-accent/6': dropTarget === column.id || canDropOn(column.id),
              }"
              :data-testid="`kanban-column-${column.id}`"
              @dragover="onDragOver(column.id, $event)"
              @dragleave="onDragLeave(column.id)"
              @drop="onDrop(column.id, $event)"
            >
              <header class="flex flex-col gap-1 px-1">
                <div class="flex items-baseline justify-between gap-2">
                  <h2
                    :id="`kanban-heading-${column.id}`"
                    class="font-display text-sm font-semibold tracking-tight text-ink"
                  >
                    {{ column.label }}
                  </h2>
                  <div class="flex shrink-0 items-center gap-1">
                    <span class="rounded-full bg-line px-2 py-0.5 font-mono text-xs text-ink-muted">
                      {{ column.cards.length }}
                    </span>
                    <AppButton
                      v-if="column.id === 'done' && done === 'column'"
                      variant="ghost"
                      size="iconSm"
                      class="-my-1.5"
                      aria-label="Move Done below the board"
                      data-testid="move-done-below-board"
                      @click="choose({ done_placement: 'table' })"
                    >
                      <ChevronIcon direction="down" />
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
                  v-for="ticket in column.cards"
                  :key="ticket.id"
                >
                  <article
                    class="flex w-full flex-col gap-2 rounded-control border bg-surface px-3 py-3 text-left shadow-panel"
                    :class="cardChrome(ticket, column.id)"
                    :draggable="!moving && canDrag(ticket)"
                    :aria-label="cardLabel(ticket, column.id)"
                    :data-testid="`kanban-card-${ticket.id}`"
                    :data-kind="ticket.kind"
                    :data-state="ticket.state"
                    @dragstart="onDragStart(ticket, $event)"
                    @dragend="onDragEnd"
                  >
                    <div class="flex items-baseline justify-between gap-2">
                      <span
                        class="font-mono text-[0.6875rem] text-ink-subtle tabular-nums"
                        :data-testid="`card-number-${ticket.id}`"
                      >
                        {{ boardCardNumber(ticket, projectCode) }}
                      </span>
                      <span
                        class="text-[0.625rem] font-semibold tracking-[0.06em] text-ink-subtle uppercase"
                        :data-testid="`card-kind-${ticket.id}`"
                      >
                        {{ KIND_LABELS[ticket.kind] }}
                      </span>
                    </div>
                    <button
                      type="button"
                      class="self-start text-left text-sm font-medium text-ink underline-offset-2 transition-colors hover:text-accent hover:underline focus-visible:rounded-control focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
                      :data-testid="`open-ticket-${ticket.id}`"
                      @click="openTicket(ticket)"
                    >
                      {{ boardCardTitle(ticket) }}
                    </button>
                    <ul
                      class="flex flex-wrap gap-1"
                      :data-testid="`card-chips-${ticket.id}`"
                      :aria-label="`Chips for ${boardCardNumber(ticket, projectCode)}`"
                    >
                      <li
                        v-for="chip in cardChips(ticket)"
                        :key="chip.kind"
                        class="inline-flex max-w-full items-center gap-1 rounded-full border px-2 py-0.5 text-[0.625rem] leading-[1.3]"
                        :class="chipSurfaceClass(chip.tone)"
                        :data-tone="chip.tone ?? 'neutral'"
                        :data-testid="`card-chip-${chip.kind}-${ticket.id}`"
                        :title="chip.detail"
                      >
                        <span class="font-semibold tracking-[0.04em] uppercase">
                          {{ chip.label }}
                        </span>
                        <span class="truncate">{{ chip.value }}</span>
                        <!-- The fallback indicator an effective profile
                             wears (DR-BP-13). -->
                        <span
                          v-if="chip.fallback"
                          aria-label="fallback profile"
                          :data-testid="`card-fallback-${ticket.id}`"
                        >
                          ↺
                        </span>
                      </li>
                    </ul>
                    <StatusBadge
                      v-if="showsCardStatus(column.id)"
                      :tone="STATUS_TONES[ticket.state]"
                      :data-testid="`card-status-${ticket.id}`"
                    >
                      {{ STATUS_LABELS[ticket.state] }}
                    </StatusBadge>
                  </article>
                </li>
              </ul>
            </section>
          </div>
        </div>
      </div>

      <DoneBoardTable
        v-if="done === 'table'"
        :rows="doneRows"
        :drop-active="dropTarget === 'done' || canDropOn('done')"
        @select="(row) => openTicket(row.ticket)"
        @promote="choose({ done_placement: 'column' })"
        @dragover="onDragOver('done', $event)"
        @dragleave="onDragLeave('done')"
        @drop="onDrop('done', $event)"
      />
    </template>

    <DetailDrawer
      :open="selectedTicket !== null"
      :title="drawerTitle"
      :number="drawerNumber"
      size="wide"
      @close="closeDrawer"
    >
      <template #subtitle>
        {{ project ? `Project · ${project.code}` : '' }}
      </template>

      <dl
        v-if="selectedTicket"
        class="flex flex-col gap-3"
      >
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
            <StatusBadge :tone="STATUS_TONES[selectedTicket.state]">
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
    </DetailDrawer>
  </main>
</template>
