<script setup lang="ts">
import type { RunRecord } from '@kanban/contracts'

defineProps<{
  attempts: readonly RunRecord[]
}>()
</script>

<template>
  <section
    class="flex flex-col gap-3"
    data-testid="drawer-attempts"
  >
    <h3 class="font-display text-sm font-semibold tracking-tight text-ink">
      Historical attempts
    </h3>
    <ol
      v-if="attempts.length > 0"
      class="flex flex-col gap-2"
    >
      <li
        v-for="attempt in attempts"
        :key="attempt.id"
        class="rounded-control border border-line bg-surface/70 px-3 py-2 text-sm"
        :data-testid="`drawer-attempt-${attempt.id}`"
      >
        <div class="flex items-baseline justify-between gap-2">
          <span class="font-mono text-xs text-ink-muted tabular-nums">
            Run {{ attempt.id }}
          </span>
          <span class="text-xs tracking-wide text-ink-subtle uppercase">
            {{ attempt.status }}
          </span>
        </div>
        <p class="text-ink">
          {{ attempt.effective.name }}
          <span
            v-if="attempt.fallback"
            class="text-ink-muted"
          > (fallback)</span>
        </p>
      </li>
    </ol>
    <p
      v-else
      class="text-sm text-ink-subtle"
    >
      No attempts yet.
    </p>
  </section>
</template>
