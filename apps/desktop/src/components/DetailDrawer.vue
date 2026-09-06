<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'
import AppButton from './AppButton.vue'
import { getFocusableElements, trapTabKey } from './focus-trap'

const { open, title, number = undefined, size = 'primary' } = defineProps<{
  open: boolean
  title: string
  /**
   * The record's stored global number, `KAN-T12`. It prefixes the title
   * inside the same heading, so the dialog is announced by the identity
   * a reader would quote and there is no second copy elsewhere.
   */
  number?: string
  /**
   * Secondary is the narrow drawer for stacking, primary the everyday
   * one. Wide exists for bodies that carry fixed-width content above
   * all: at primary they scroll from the first line, which makes them
   * unreadable rather than merely cropped.
   */
  size?: 'primary' | 'secondary' | 'wide'
}>()

const emit = defineEmits<{ close: [] }>()

const titleId = useId()
const panelRef = ref<HTMLElement | null>(null)
let previousFocus: HTMLElement | null = null
let trapActive = false

const PANEL_WIDTHS = {
  secondary: 'max-w-md',
  primary: 'max-w-2xl',
  wide: 'max-w-4xl',
} as const

const panelWidthClass = computed(() => PANEL_WIDTHS[size])

function onKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    emit('close')
    return
  }
  if (panelRef.value) {
    trapTabKey(event, panelRef.value)
  }
}

async function activateTrap(captureRestoreTarget: boolean): Promise<void> {
  if (trapActive) return
  if (captureRestoreTarget) {
    const active = document.activeElement
    previousFocus =
      active instanceof HTMLElement && active !== document.body ? active : null
  }

  window.addEventListener('keydown', onKeydown)
  trapActive = true
  await nextTick()
  const focusable = panelRef.value ? getFocusableElements(panelRef.value) : []
  ;(focusable[0] ?? panelRef.value)?.focus()
}

function detachTrap(): void {
  if (!trapActive) return
  window.removeEventListener('keydown', onKeydown)
  trapActive = false
}

function releaseTrapAndRestoreFocus(): void {
  detachTrap()
  const restore = previousFocus
  previousFocus = null
  if (restore && document.contains(restore)) {
    restore.focus()
  }
}

watch(
  () => open,
  (isOpen, wasOpen) => {
    if (isOpen) {
      // Capture the restore target only when the drawer newly opens.
      void activateTrap(!wasOpen)
      return
    }
    releaseTrapAndRestoreFocus()
  },
  { immediate: true },
)

onBeforeUnmount(() => {
  releaseTrapAndRestoreFocus()
})
</script>

<template>
  <Teleport to="body">
    <Transition
      enter-from-class="opacity-0"
      enter-active-class="transition-opacity duration-300 ease-out"
      leave-active-class="transition-opacity duration-200 ease-in"
      leave-to-class="opacity-0"
    >
      <div
        v-if="open"
        class="fixed inset-0 bg-ink/40 backdrop-blur-sm"
        data-testid="detail-drawer-backdrop"
        aria-hidden="true"
        @click="emit('close')"
      />
    </Transition>

    <div class="pointer-events-none fixed inset-0 overflow-hidden">
      <div class="absolute inset-y-0 right-0 flex max-w-full pl-10 sm:pl-16">
        <Transition
          enter-from-class="translate-x-full"
          enter-active-class="transform transition duration-300 ease-out"
          leave-active-class="transform transition duration-200 ease-in"
          leave-to-class="translate-x-full"
        >
          <aside
            v-if="open"
            ref="panelRef"
            class="pointer-events-auto flex h-full w-screen flex-col border-l border-line bg-surface shadow-drawer"
            :class="panelWidthClass"
            :data-drawer-size="size"
            role="dialog"
            aria-modal="true"
            :aria-labelledby="titleId"
            tabindex="-1"
          >
            <header
              class="flex items-start justify-between gap-3 border-b border-line px-5 py-5 sm:px-6"
            >
              <div class="min-w-0 flex flex-col gap-1">
                <h2
                  :id="titleId"
                  class="font-display text-xl font-semibold tracking-tight text-ink"
                >
                  <!-- The number span carries a real trailing space, so the
                       dialog is announced "KAN-T12 Title" rather than running
                       the two together. -->
                  <span
                    v-if="number"
                    class="mr-2 font-mono text-xl font-bold tracking-[0.02em] text-accent tabular-nums"
                    data-testid="record-number"
                  >{{ number }} </span>{{ title }}
                </h2>
                <p
                  v-if="$slots.subtitle"
                  class="font-mono text-xs text-ink-subtle"
                >
                  <slot name="subtitle" />
                </p>
              </div>

              <AppButton
                variant="ghost"
                size="icon"
                class="shrink-0"
                aria-label="Close panel"
                @click="emit('close')"
              >
                <svg
                  class="size-6 shrink-0"
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

            <div class="flex-1 overflow-y-auto px-5 py-5 sm:px-6">
              <slot />
            </div>
            <footer
              v-if="$slots.footer"
              class="flex items-center justify-end gap-2 border-t border-line px-5 py-4 sm:px-6"
            >
              <slot name="footer" />
            </footer>
          </aside>
        </Transition>
      </div>
    </div>
  </Teleport>
</template>
