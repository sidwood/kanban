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
// own explanation (KAN-T24-AC3).
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
  type DonePresentation,
  type DraftVisibility,
  boardColumnGroups,
  boardColumnLabel,
  boardColumnSubheading,
  boardLayoutAxisControls,
  columnForCard,
  columnHoldsManyStates,
  dropFor,
  isOnBoard,
  registerColumnsFor,
  resolveDraftVisibility,
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
import { chipSurfaceClass, chipsFor, laneFor } from './board-chips'
import type { CardChip } from './board-chips'
import { useLanesStore } from '../stores/lanes'
import { loadBoardChoices, saveBoardChoices } from './board-layout.storage'
import type { BoardChoices } from './board-layout.storage'
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

const projectId = computed(() => Number(route.params.projectId))

const project = computed(
  () => projects.projects.find((entry) => entry.id === projectId.value) ?? null,
)

const projectCode = computed(() => project.value?.code ?? '')

// The operator's kept presentation choices, saved together.
const choices = ref<BoardChoices>(loadBoardChoices())

function choose(next: Partial<BoardChoices>): void {
  choices.value = { ...choices.value, ...next }
  saveBoardChoices(choices.value)
}

const layouts = computed(() => choices.value.layouts)
const done = computed<DonePresentation>(() => choices.value.done)
const presentation = computed<BoardPresentation>(() => choices.value.presentation)

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

onMounted(load)

watch(projectId, () => {
  void load()
})

async function load(): Promise<void> {
  if (!transport) return
  await projects.refresh(transport)
  if (project.value) {
    // The Lanes arrive beside the Tickets: the Lane chip a card wears
    // comes from the KAN-T32 contract, never from board state.
    await Promise.all([
      board.refresh(transport, projectId.value),
      lanes.load(transport, projectId.value),
    ])
  }
}

// Only Tickets the board can place reach the columns; terminal
// states keep their history off the active board. The order is the
// deterministic one — priority, readiness, number — so every column,
// the register, and the Done table scan the same way, and no manual
// ordering exists anywhere (DR-LC-11).
const cards = computed(() =>
  orderCards(board.tickets.filter((ticket) => isOnBoard(ticket.state))),
)

const draftCount = computed(
  () => board.tickets.filter((ticket) => ticket.state === 'draft').length,
)

const draft = computed<DraftVisibility>(() =>
  resolveDraftVisibility(choices.value.draft, draftCount.value),
)

const layoutControls = computed(() => boardLayoutAxisControls(layouts.value))

const groups = computed(() =>
  boardColumnGroups(layouts.value, done.value, draft.value).map((group) => {
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
    ? visibleColumnsFor(layouts.value, done.value, draft.value)
    : registerColumnsFor(layouts.value, draft.value),
)

const registerColumns = computed<readonly BoardRegisterColumn[]>(() =>
  registerColumnsFor(layouts.value, draft.value).map((column) => ({
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
  return registerColumnsFor(layouts.value, draft.value)
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
// Ticket and the facts the board holds. Reviewer and effective-profile
// values arrive with KAN-S9's dispatch and run data; until then the
// planned profile the assignment names is the profile a card shows.
function cardChips(ticket: TicketRecord): readonly CardChip[] {
  return chipsFor(ticket, {
    projectCode: projectCode.value,
    lane: laneFor(lanes.lanes, ticket.id),
    blockers: board.blockersFor(ticket.id),
    reviewers: [],
    execution: null,
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
// the embedded timeline arrive with the drawer's own ticket.
const drawerFacts = computed(() => {
  const ticket = selectedTicket.value
  if (!ticket) return []
  const facts: { label: string; value: string }[] = [
    { label: 'Kind', value: KIND_LABELS[ticket.kind] },
    { label: 'State', value: STATUS_LABELS[ticket.state] },
    { label: 'Priority', value: ticket.priority },
    { label: 'Project', value: `${projectCode.value} — ${project.value?.name ?? ''}` },
  ]
  if (ticket.spec_id) {
    facts.push({ label: 'Spec', value: `${projectCode.value}-S${ticket.spec_id}` })
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
              :aria-pressed="draft === 'visible'"
              data-testid="toggle-draft"
              @click="choose({ draft: draft === 'hidden' ? 'visible' : 'hidden' })"
            >
              {{ draft === 'visible' ? 'Hide Draft' : 'Show Draft' }}
              <span
                v-if="draft === 'hidden'"
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
              @click="choose({ presentation: option })"
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
              @click="choose({ layouts: { ...layouts, [control.axis]: option } })"
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
      v-if="board.error || lanes.error"
      data-testid="board-error"
    >
      {{ board.error ?? lanes.error }}
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
        :data-draft-visibility="draft"
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
                      @click="choose({ done: 'table' })"
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
        @promote="choose({ done: 'column' })"
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
