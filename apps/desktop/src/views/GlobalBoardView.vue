<script setup lang="ts">
// The global board: every Project's work on one surface, filtered by
// the ten axes the register fixes — Initiative, Project, Plan, Spec,
// kind, state, priority, Lane, execution profile, and attention
// state. One `board.global` query carries the filter in and the
// projection back: the cards arrive already grouped and already in
// the deterministic order, so this view renders what it is given and
// recomputes nothing — no client-side grouping, no client-side
// sorting. Lane, profile, and attention values populate as their
// feeds land; the axes themselves are all here.
import { computed, inject, onMounted } from 'vue'
import type {
  BoardFilterOption,
  TicketKind,
  TicketPriority,
  TicketState,
} from '@kanban/contracts'
import AppButton from '../components/AppButton.vue'
import EmptyState from '../components/EmptyState.vue'
import InlineAlert from '../components/InlineAlert.vue'
import SkeletonBlock from '../components/SkeletonBlock.vue'
import StatusBadge from '../components/StatusBadge.vue'
import { kanbanTransportKey } from '../core/transport'
import { useGlobalBoardStore } from '../stores/global-board'
import type { BoardIdAxis, BoardWordAxis } from '../stores/global-board'
import { KIND_LABELS, STATUS_LABELS, STATUS_TONES, boardCardNumber, boardCardTitle } from './board-card'
import {
  ATTENTION_LABELS,
  GLOBAL_BOARD_GROUPS,
  PRIORITY_LABELS,
  activeAxisCount,
  cardsOfGroup,
} from './global-board-filters'

const transport = inject(kanbanTransportKey)
const board = useGlobalBoardStore()

onMounted(() => {
  void load()
})

async function load(): Promise<void> {
  if (transport) await board.refresh(transport)
}

/** One selectable value of one axis, with its selected state. */
interface FilterChoice {
  value: string
  label: string
  checked: boolean
}

/** One identity axis the filter panel offers. */
interface IdFilterAxis {
  axis: BoardIdAxis
  label: string
  options: readonly FilterChoice[]
}

/** One vocabulary axis the filter panel offers. */
interface WordFilterAxis {
  axis: BoardWordAxis
  label: string
  options: readonly FilterChoice[]
}

/** Whether one word axis already holds one value. */
function wordSelected(axis: BoardWordAxis, value: string): boolean {
  return ((board.filter[axis] ?? []) as readonly unknown[]).includes(value)
}

// The identity axes: every one offers the values the core read, and a
// value toggles in and out of its own set.
const idAxes = computed<readonly IdFilterAxis[]>(() => {
  const sources: readonly {
    axis: BoardIdAxis
    label: string
    entries: readonly BoardFilterOption[]
  }[] = [
    { axis: 'initiatives', label: 'Initiative', entries: board.options?.initiatives ?? [] },
    { axis: 'projects', label: 'Project', entries: board.options?.projects ?? [] },
    { axis: 'plans', label: 'Plan', entries: board.options?.plans ?? [] },
    { axis: 'specs', label: 'Spec', entries: board.options?.specs ?? [] },
    { axis: 'lanes', label: 'Lane', entries: board.options?.lanes ?? [] },
  ]
  return sources.map((source) => ({
    axis: source.axis,
    label: source.label,
    options: source.entries.map((option) => ({
      value: String(option.id),
      label: option.label,
      checked: (board.filter[source.axis] ?? []).includes(option.id),
    })),
  }))
})

// The vocabulary axes: the closed sets the contracts carry, and the
// profile names the core listed.
const wordAxes = computed<readonly WordFilterAxis[]>(() => {
  const vocabulary = (
    axis: BoardWordAxis,
    label: string,
    entries: readonly { value: string; label: string }[],
  ): WordFilterAxis => ({
    axis,
    label,
    options: entries.map((entry) => ({
      value: entry.value,
      label: entry.label,
      checked: wordSelected(axis, entry.value),
    })),
  })
  return [
    vocabulary(
      'kinds',
      'Kind',
      (Object.entries(KIND_LABELS) as [TicketKind, string][]).map(([value, label]) => ({
        value,
        label,
      })),
    ),
    vocabulary(
      'states',
      'State',
      (Object.entries(STATUS_LABELS) as [TicketState, string][]).map(([value, label]) => ({
        value,
        label,
      })),
    ),
    vocabulary(
      'priorities',
      'Priority',
      (Object.entries(PRIORITY_LABELS) as [TicketPriority, string][]).map(([value, label]) => ({
        value,
        label,
      })),
    ),
    vocabulary(
      'attention',
      'Attention',
      (board.options?.attention ?? []).map((value) => ({
        value,
        label: ATTENTION_LABELS[value],
      })),
    ),
    vocabulary(
      'profiles',
      'Profile',
      (board.options?.profiles ?? []).map((value) => ({ value, label: value })),
    ),
  ]
})

function onToggleId(axis: BoardIdAxis, id: number): void {
  board.toggleId(axis, id)
  void load()
}

function onToggleWord(axis: BoardWordAxis, value: string): void {
  board.toggleWord(axis, value)
  void load()
}

function onClear(): void {
  board.resetFilter()
  void load()
}

const activeCount = computed(() => activeAxisCount(board.filter))

// The six fixed groups, each holding the cards the core placed in it,
// in the order they arrived.
const groups = computed(() =>
  GLOBAL_BOARD_GROUPS.map((entry) => ({
    ...entry,
    cards: cardsOfGroup(board.cards, entry.group),
  })),
)
</script>

<template>
  <main class="mx-auto flex min-h-screen w-full max-w-7xl flex-col gap-6 px-6 py-10">
    <header class="flex flex-wrap items-end justify-between gap-4">
      <div class="flex flex-col gap-1">
        <h1 class="text-2xl font-semibold tracking-tight">
          Global board
        </h1>
        <p class="text-sm text-ink-muted">
          Every Project's work, one filtered view
          <template v-if="activeCount > 0">
            · {{ activeCount }} filter{{ activeCount === 1 ? '' : 's' }} active
          </template>
        </p>
      </div>
      <div class="flex items-center gap-3">
        <AppButton
          data-testid="global-board-clear"
          :disabled="activeCount === 0"
          @click="onClear"
        >
          Clear filters
        </AppButton>
        <RouterLink
          to="/"
          class="text-sm text-ink-muted underline-offset-4 hover:text-ink hover:underline"
        >
          Home
        </RouterLink>
      </div>
    </header>

    <InlineAlert v-if="board.error">
      {{ board.error }}
    </InlineAlert>

    <section
      v-if="board.options"
      data-testid="global-board-filters"
      class="flex flex-wrap gap-x-6 gap-y-4 rounded-panel border border-line bg-surface px-4 py-4"
      aria-label="Board filters"
    >
      <fieldset
        v-for="axis in idAxes"
        :key="axis.axis"
        class="flex min-w-40 flex-col gap-1"
      >
        <legend class="text-xs font-semibold tracking-wide text-ink-muted uppercase">
          {{ axis.label }}
        </legend>
        <label
          v-for="option in axis.options"
          :key="`${axis.axis}:${option.value}`"
          class="flex items-center gap-2 text-sm text-ink"
        >
          <input
            type="checkbox"
            :value="`${axis.axis}:${option.value}`"
            :checked="option.checked"
            @change="onToggleId(axis.axis, Number(option.value))"
          >
          {{ option.label }}
        </label>
      </fieldset>
      <fieldset
        v-for="axis in wordAxes"
        :key="axis.axis"
        class="flex min-w-40 flex-col gap-1"
      >
        <legend class="text-xs font-semibold tracking-wide text-ink-muted uppercase">
          {{ axis.label }}
        </legend>
        <label
          v-for="option in axis.options"
          :key="`${axis.axis}:${option.value}`"
          class="flex items-center gap-2 text-sm text-ink"
        >
          <input
            type="checkbox"
            :value="`${axis.axis}:${option.value}`"
            :checked="option.checked"
            @change="onToggleWord(axis.axis, option.value)"
          >
          {{ option.label }}
        </label>
      </fieldset>
    </section>

    <div
      v-if="!board.loaded && !board.error"
      class="flex gap-4"
      aria-busy="true"
    >
      <SkeletonBlock
        v-for="group in groups"
        :key="group.group"
        class="h-64 flex-1"
      />
    </div>

    <EmptyState
      v-else-if="board.loaded && board.cards.length === 0"
      message="No work matches"
      hint="Every filter composes; clearing them brings the whole board back."
    />

    <div
      v-else
      class="grid grid-cols-1 gap-4 md:grid-cols-3 xl:grid-cols-6"
    >
      <section
        v-for="group in groups"
        :key="group.group"
        data-testid="global-board-group"
        :data-group="group.group"
        class="flex min-w-0 flex-col gap-3 rounded-panel border border-line bg-surface/60 p-3"
        :aria-label="group.label"
      >
        <header class="flex items-baseline justify-between gap-2">
          <h2 class="text-sm font-semibold tracking-wide uppercase">
            {{ group.label }}
          </h2>
          <span class="text-xs text-ink-subtle">{{ group.cards.length }}</span>
        </header>
        <ol class="flex flex-col gap-2">
          <li
            v-for="card in group.cards"
            :key="`${card.ticket.project_id}:${card.ticket.id}`"
          >
            <RouterLink
              :to="`/projects/${card.ticket.project_id}/board`"
              class="flex flex-col gap-1.5 rounded-control border border-line bg-surface px-3 py-2 text-left hover:border-line-strong"
            >
              <span class="flex items-center justify-between gap-2">
                <span class="font-mono text-xs text-ink-muted">
                  {{ boardCardNumber(card.ticket, card.project_code) }}
                </span>
                <StatusBadge
                  :tone="STATUS_TONES[card.ticket.state]"
                  density="compact"
                >
                  {{ STATUS_LABELS[card.ticket.state] }}
                </StatusBadge>
              </span>
              <span class="text-sm leading-snug font-medium">
                {{ boardCardTitle(card.ticket) }}
              </span>
              <span class="flex flex-wrap gap-x-2 gap-y-1 text-xs text-ink-subtle">
                <span>{{ KIND_LABELS[card.ticket.kind] }}</span>
                <span>· {{ PRIORITY_LABELS[card.ticket.priority] }}</span>
                <span v-if="card.spec_number !== null && card.spec_number !== undefined">
                  · {{ card.project_code }}-S{{ card.spec_number }}
                </span>
                <span v-if="card.lane_id !== null && card.lane_id !== undefined">
                  · Lane {{ card.lane_id }}
                </span>
              </span>
            </RouterLink>
          </li>
        </ol>
      </section>
    </div>
  </main>
</template>
