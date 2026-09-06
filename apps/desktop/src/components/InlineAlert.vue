<script setup lang="ts">
/**
 * The surface's one inline notice: a hairline panel that stays with the thing it
 * describes. A refusal the core returned is a state that is still true, so it
 * holds its place in the layout until the operator acts again.
 *
 * Critical and caution are announced assertively because both block the
 * operator; neutral is progress the operator asked for, so it stays polite.
 */
export type AlertTone = 'critical' | 'caution' | 'neutral'

const TONE_CLASSES = {
  critical: 'border-critical/30 bg-critical/8 text-critical',
  caution: 'border-caution/40 bg-caution-soft text-ink',
  neutral: 'border-line bg-surface text-ink-muted',
} satisfies Record<AlertTone, string>

const { tone = 'critical' } = defineProps<{ tone?: AlertTone }>()
</script>

<template>
  <div
    class="rounded-control border px-3 py-2 text-sm"
    :class="TONE_CLASSES[tone]"
    :role="tone === 'neutral' ? 'status' : 'alert'"
    :data-tone="tone"
  >
    <slot />
  </div>
</template>
