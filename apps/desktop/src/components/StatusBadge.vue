<script setup lang="ts">
/**
 * Visual tones only. Mapping a Kanban lifecycle state onto a tone
 * belongs with the feature that owns that vocabulary, so this
 * primitive never has to learn it.
 */
export type StatusTone =
  | 'neutral'
  | 'progress'
  | 'positive'
  | 'caution'
  | 'critical'

const TONE_CLASSES = {
  neutral: 'bg-line text-ink-muted',
  progress: 'bg-info/12 text-info',
  positive: 'bg-accent/12 text-accent',
  caution: 'bg-caution/12 text-caution',
  critical: 'bg-critical/12 text-critical',
} satisfies Record<StatusTone, string>

const DOT_CLASSES = {
  neutral: 'bg-ink-subtle',
  progress: 'bg-info',
  positive: 'bg-accent-fill',
  caution: 'bg-caution',
  critical: 'bg-critical',
} satisfies Record<StatusTone, string>

/** Pill is the board and register chip; square is the drawer's chip. */
export type StatusShape = 'pill' | 'square'

/** Compact is the dense chip a table cell spends. */
export type StatusDensity = 'default' | 'compact'

const SHAPE_CLASSES = {
  pill: {
    default: 'gap-1.5 rounded-full px-2.5 py-1 text-[0.6875rem]',
    compact: 'gap-1 rounded-full px-2 py-0.5 text-[0.625rem] leading-[1.2]',
  },
  square: {
    default: 'gap-1.5 rounded-[2px] px-2 py-0.5 font-mono text-[0.72rem]',
    compact:
      'gap-1 rounded-[2px] px-1.5 py-0.5 font-mono text-[0.6875rem] leading-[1.2]',
  },
} satisfies Record<StatusShape, Record<StatusDensity, string>>

const DOT_SIZE_CLASSES = {
  default: 'size-1.5',
  compact: 'size-1',
} satisfies Record<StatusDensity, string>

const {
  tone = 'neutral',
  shape = 'pill',
  density = 'default',
} = defineProps<{
  tone?: StatusTone
  shape?: StatusShape
  density?: StatusDensity
}>()
</script>

<template>
  <span
    class="inline-flex items-center font-semibold tracking-[0.08em] uppercase"
    :class="[TONE_CLASSES[tone], SHAPE_CLASSES[shape][density]]"
    :data-density="density"
    :data-shape="shape"
    :data-tone="tone"
  >
    <span
      class="shrink-0 rounded-full"
      :class="[DOT_CLASSES[tone], DOT_SIZE_CLASSES[density]]"
      aria-hidden="true"
    />
    <slot />
  </span>
</template>
