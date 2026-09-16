<script setup lang="ts">
// The board's side sheet: the Columns and Filters flyouts share it.
// A modal dialog anchored to the right edge, closed by its button,
// its backdrop, or Escape, with focus held inside while it stands.
// The overlay teleports to body because the board's rise animation
// retains transform:translateY(0), which would otherwise become the
// containing block for this position:fixed sheet (KAN-T147).
import { nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'
import AppButton from '../AppButton.vue'
import { getFocusableElements, trapTabKey } from '../focus-trap'

const { open, title, summary, testid } = defineProps<{
  open: boolean
  title: string
  summary: string
  testid: string
}>()

const emit = defineEmits<{ close: [] }>()

const titleId = useId()
const panelRef = ref<HTMLElement | null>(null)
let previousFocus: HTMLElement | null = null
let trapActive = false

function onKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    event.preventDefault()
    emit('close')
    return
  }
  if (panelRef.value) trapTabKey(event, panelRef.value)
}

async function activateTrap(): Promise<void> {
  if (trapActive) return
  const active = document.activeElement
  previousFocus = active instanceof HTMLElement && active !== document.body ? active : null
  window.addEventListener('keydown', onKeydown)
  trapActive = true
  await nextTick()
  const focusable = panelRef.value ? getFocusableElements(panelRef.value) : []
  ;(focusable[0] ?? panelRef.value)?.focus()
}

function releaseTrap(): void {
  if (!trapActive) return
  window.removeEventListener('keydown', onKeydown)
  trapActive = false
  const restore = previousFocus
  previousFocus = null
  if (restore && document.contains(restore)) restore.focus()
}

watch(
  () => open,
  (isOpen) => {
    if (isOpen) {
      void activateTrap()
      return
    }
    releaseTrap()
  },
  { immediate: true },
)

onBeforeUnmount(releaseTrap)
</script>

<template>
  <Teleport to="body">
    <div
      v-if="open"
      class="fixed inset-0 z-45 flex justify-end"
    >
      <div
        class="absolute inset-0 bg-ink/30 backdrop-blur-[2px]"
        aria-hidden="true"
        @click="emit('close')"
      />
      <aside
        ref="panelRef"
        role="dialog"
        aria-modal="true"
        :aria-labelledby="titleId"
        :data-testid="testid"
        tabindex="-1"
        class="relative flex h-full w-[min(380px,92vw)] flex-col border-l border-line-strong bg-surface shadow-drawer"
      >
        <header class="flex shrink-0 items-center gap-2.5 border-b border-line px-3.5 py-3">
          <div class="flex min-w-0 flex-1 flex-col">
            <h2
              :id="titleId"
              class="font-display text-[0.95rem] font-semibold tracking-tight text-ink"
            >
              {{ title }}
            </h2>
            <p class="mt-0.5 text-[0.6rem] font-semibold tracking-[0.1em] text-ink-subtle uppercase">
              {{ summary }}
            </p>
          </div>
          <AppButton
            variant="ghost"
            size="iconSm"
            :data-testid="`${testid.replace('-flyout', '')}-close`"
            :aria-label="`Close ${title.toLowerCase()}`"
            @click="emit('close')"
          >
            <svg
              class="size-4"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
              stroke-linecap="round"
              aria-hidden="true"
            >
              <path d="M6 6l12 12M18 6L6 18" />
            </svg>
          </AppButton>
        </header>
        <div class="flex min-h-0 flex-1 flex-col gap-2.5 overflow-y-auto px-3.5 py-3">
          <slot />
        </div>
        <footer
          v-if="$slots.footer"
          class="flex shrink-0 items-center gap-2 border-t border-line bg-rail px-3.5 py-2.5"
        >
          <slot name="footer" />
        </footer>
      </aside>
    </div>
  </Teleport>
</template>
