<script setup lang="ts">
// The modal dialog the authoring surfaces open over the shell: a
// focus-trapped panel with an accessible name, Escape and a backdrop
// click to leave, and three widths — small for a quick capture, wide
// for the kind-adaptive editor. Presentation only; it knows nothing
// about Tickets.
import { computed, nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'
import AppButton from './AppButton.vue'
import { getFocusableElements, trapTabKey } from './focus-trap'

const { open, title, size = 'medium', testid } = defineProps<{
  open: boolean
  title: string
  /** Small carries a capture; medium and wide carry a form. */
  size?: 'small' | 'medium' | 'wide'
  /** The identity the specs and the operator's tooling address the
   * dialog by. */
  testid: string
}>()

const emit = defineEmits<{ close: [] }>()

const titleId = useId()
const panelRef = ref<HTMLElement | null>(null)
let previousFocus: HTMLElement | null = null
let trapActive = false

const PANEL_WIDTHS = {
  small: 'max-w-md',
  medium: 'max-w-2xl',
  wide: 'max-w-4xl',
} as const

const panelWidthClass = computed(() => PANEL_WIDTHS[size])

// The open dialog owns Escape and Tab: it listens in the capture
// phase and stops those two keys, so a drawer underneath never also
// closes or steals the focus ring. Every other key passes through.
function onKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    event.stopPropagation()
    emit('close')
    return
  }
  if (event.key === 'Tab' && panelRef.value) {
    event.stopPropagation()
    trapTabKey(event, panelRef.value)
  }
}

async function activateTrap(captureRestoreTarget: boolean): Promise<void> {
  if (trapActive) return
  if (captureRestoreTarget) {
    const active = document.activeElement
    previousFocus = active instanceof HTMLElement && active !== document.body ? active : null
  }
  window.addEventListener('keydown', onKeydown, true)
  trapActive = true
  await nextTick()
  const focusable = panelRef.value ? getFocusableElements(panelRef.value) : []
  ;(focusable[0] ?? panelRef.value)?.focus()
}

function releaseTrapAndRestoreFocus(): void {
  if (trapActive) {
    window.removeEventListener('keydown', onKeydown, true)
    trapActive = false
  }
  const restore = previousFocus
  previousFocus = null
  if (restore && document.contains(restore)) restore.focus()
}

watch(
  () => open,
  (isOpen, wasOpen) => {
    if (isOpen) {
      void activateTrap(!wasOpen)
      return
    }
    releaseTrapAndRestoreFocus()
  },
  { immediate: true },
)

onBeforeUnmount(releaseTrapAndRestoreFocus)
</script>

<template>
  <div
    v-if="open"
    class="fixed inset-0 z-60 flex items-start justify-center overflow-y-auto p-6"
  >
    <div
      class="fixed inset-0 bg-ink/40 backdrop-blur-sm"
      :data-testid="`${testid}-backdrop`"
      aria-hidden="true"
      @click="emit('close')"
    />
    <section
      ref="panelRef"
      :data-testid="testid"
      :data-dialog-size="size"
      class="relative my-auto flex w-full flex-col rounded-panel border border-line bg-surface shadow-drawer"
      :class="panelWidthClass"
      role="dialog"
      aria-modal="true"
      :aria-labelledby="titleId"
      tabindex="-1"
    >
      <header class="flex items-start justify-between gap-3 border-b border-line px-5 py-4">
        <div class="flex min-w-0 flex-col gap-1">
          <h2
            :id="titleId"
            class="font-display text-lg font-semibold tracking-tight text-ink"
          >
            {{ title }}
          </h2>
          <p
            v-if="$slots.subtitle"
            class="text-xs text-ink-subtle"
          >
            <slot name="subtitle" />
          </p>
        </div>
        <AppButton
          variant="ghost"
          size="iconSm"
          class="shrink-0"
          :data-testid="`${testid}-close`"
          aria-label="Close dialog"
          @click="emit('close')"
        >
          <svg
            class="size-5 shrink-0"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <path d="M6 18 18 6M6 6l12 12" />
          </svg>
        </AppButton>
      </header>

      <div class="flex flex-col gap-4 px-5 py-5">
        <slot />
      </div>
    </section>
  </div>
</template>
