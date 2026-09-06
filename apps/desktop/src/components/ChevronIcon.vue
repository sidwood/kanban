<script setup lang="ts">
// The one icon the board spends: a chevron that points where the
// control it names moves the thing it moves.
const { direction = 'down' } = defineProps<{
  direction?: 'down' | 'up'
}>()
</script>

<template>
  <svg
    class="size-4 shrink-0"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path
      :d="direction === 'down' ? 'M19.5 8.25 12 15.75 4.5 8.25' : 'M4.5 15.75 12 8.25 19.5 15.75'"
    />
  </svg>
</template>
