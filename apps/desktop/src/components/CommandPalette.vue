<script setup lang="ts">
// The command palette: global search and navigation without workflow
// mutation (DR-BP-17). Every search goes through `search.global`; a
// chosen row only navigates.
import { computed, inject, nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'
import { useRouter } from 'vue-router'
import { trapTabKey } from './focus-trap'
import { kanbanTransportKey } from '../core/transport'
import { usePaletteStore } from '../stores/palette'
import type { PaletteItem } from '../stores/palette-navigation'
import { useSearchStore } from '../stores/search'

const transport = inject(kanbanTransportKey)
const router = useRouter()
const palette = usePaletteStore()
const search = useSearchStore()

const titleId = useId()
const inputRef = ref<HTMLInputElement | null>(null)
const panelRef = ref<HTMLElement | null>(null)
let previousFocus: HTMLElement | null = null
let trapActive = false

const query = computed({
  get: () => search.query,
  set: (value: string) => {
    if (transport) {
      void palette.setQuery(transport, value)
    }
  },
})

const items = computed(() => palette.items)

function kindLabel(item: PaletteItem): string {
  if (item.kind === 'navigation') return 'Go to'
  return item.kind.replace('_', ' ')
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    palette.closePalette()
    return
  }
  if (event.key === 'ArrowDown') {
    event.preventDefault()
    palette.moveSelection(1)
    return
  }
  if (event.key === 'ArrowUp') {
    event.preventDefault()
    palette.moveSelection(-1)
    return
  }
  if (event.key === 'Enter') {
    event.preventDefault()
    void chooseSelected()
    return
  }
  if (panelRef.value) {
    trapTabKey(event, panelRef.value)
  }
}

async function chooseSelected(): Promise<void> {
  const item = palette.selectedItem()
  if (!item) return
  palette.closePalette()
  await router.push(item.route)
}

async function choose(item: PaletteItem): Promise<void> {
  palette.closePalette()
  await router.push(item.route)
}

async function activateTrap(): Promise<void> {
  if (trapActive) return
  const active = document.activeElement
  previousFocus =
    active instanceof HTMLElement && active !== document.body ? active : null
  window.addEventListener('keydown', onKeydown)
  trapActive = true
  await nextTick()
  inputRef.value?.focus()
}

function releaseTrap(): void {
  if (!trapActive) return
  window.removeEventListener('keydown', onKeydown)
  trapActive = false
  const restore = previousFocus
  previousFocus = null
  if (restore && document.contains(restore)) {
    restore.focus()
  }
}

watch(
  () => palette.open,
  (open) => {
    if (open) {
      void activateTrap()
      return
    }
    releaseTrap()
  },
)

onBeforeUnmount(releaseTrap)
</script>

<template>
  <div
    v-if="palette.open"
    class="fixed inset-0 z-50 flex items-start justify-center bg-slate-900/40 px-4 pt-[12vh]"
    data-testid="command-palette"
    @mousedown.self="palette.closePalette()"
  >
    <div
      ref="panelRef"
      role="dialog"
      aria-modal="true"
      :aria-labelledby="titleId"
      class="w-full max-w-xl overflow-hidden rounded-xl border border-line bg-surface shadow-2xl"
      @mousedown.stop
    >
      <h2
        :id="titleId"
        class="sr-only"
      >
        Command palette
      </h2>
      <label class="block border-b border-line px-4 py-3">
        <span class="sr-only">Search or jump to a surface</span>
        <input
          ref="inputRef"
          v-model="query"
          data-testid="palette-query"
          type="search"
          autocomplete="off"
          spellcheck="false"
          placeholder="Search or jump to a surface…"
          class="w-full border-0 bg-transparent text-base text-slate-900 outline-none placeholder:text-slate-400"
        >
      </label>
      <p
        v-if="search.error"
        data-testid="palette-error"
        class="border-b border-line px-4 py-2 text-sm text-critical"
      >
        {{ search.error }}
      </p>
      <p
        v-else-if="search.loading"
        data-testid="palette-loading"
        class="border-b border-line px-4 py-2 text-sm text-slate-500"
      >
        Searching…
      </p>
      <ul
        v-if="items.length > 0"
        data-testid="palette-items"
        class="max-h-80 overflow-y-auto py-1"
      >
        <li
          v-for="(item, index) in items"
          :key="item.id"
        >
          <button
            type="button"
            data-testid="palette-item"
            class="flex w-full items-baseline gap-3 px-4 py-2 text-left text-sm"
            :class="index === palette.selection ? 'bg-slate-100 text-slate-900' : 'text-slate-700'"
            @mouseenter="palette.selection = index"
            @click="choose(item)"
          >
            <span class="w-20 shrink-0 text-xs uppercase tracking-wide text-slate-400">
              {{ kindLabel(item) }}
            </span>
            <span class="min-w-0 flex-1">
              <span
                v-if="item.identifier"
                class="font-medium text-slate-900"
              >
                {{ item.identifier }}
              </span>
              <span
                v-if="item.identifier"
                class="text-slate-500"
              > · </span>
              <span>{{ item.label }}</span>
            </span>
          </button>
        </li>
      </ul>
      <p
        v-else
        data-testid="palette-empty"
        class="px-4 py-6 text-center text-sm text-slate-500"
      >
        No matches.
      </p>
    </div>
  </div>
</template>
