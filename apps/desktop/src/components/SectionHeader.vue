<script setup lang="ts">
defineProps<{
  eyebrow?: string
  title: string
  summary?: string
}>()
</script>

<template>
  <header class="flex flex-col gap-2">
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div class="flex min-w-0 flex-col gap-2">
        <p
          v-if="eyebrow"
          class="wordmark text-[0.6rem] text-ink-subtle"
        >
          {{ eyebrow }}
        </p>
        <h1 class="font-display text-3xl font-semibold tracking-tight text-ink">
          {{ title }}
        </h1>
      </div>
      <div
        v-if="$slots.actions"
        class="flex shrink-0 items-center gap-2"
      >
        <slot name="actions" />
      </div>
    </div>
    <p
      v-if="summary"
      class="max-w-2xl text-sm text-ink-muted"
    >
      {{ summary }}
    </p>
  </header>
</template>
