<script setup lang="ts">
/**
 * Absence, stated on purpose. A dashed hairline frame reads as "this panel is
 * real and its contents have not arrived", which a loose sentence of muted text
 * floating under a heading does not. On the board the same frame doubles as a
 * visible drop target.
 */
const { message, hint = undefined, compact = false } = defineProps<{
  message: string
  hint?: string
  /** For narrow hosts: a board column or a drawer section. */
  compact?: boolean
}>()
</script>

<template>
  <div
    class="flex flex-col items-center justify-center gap-1.5 rounded-panel border border-dashed border-line-strong text-center"
    :class="compact ? 'px-3 py-6' : 'px-6 py-10'"
  >
    <p
      class="text-ink-muted"
      :class="compact ? 'text-xs' : 'text-sm'"
    >
      {{ message }}
    </p>
    <p
      v-if="hint"
      class="max-w-sm text-ink-subtle"
      :class="compact ? 'text-xs' : 'text-sm'"
    >
      {{ hint }}
    </p>
  </div>
</template>
