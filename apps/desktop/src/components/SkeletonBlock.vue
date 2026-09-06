<script setup lang="ts">
/**
 * One bar of not-yet-content, sized by the caller. The sweep is the same
 * gesture a section filling in reads as: the board is arriving.
 *
 * Purely decorative: the region being filled carries `aria-busy`, so assistive
 * tech is told the section is pending rather than read a wall of empty bars.
 */
</script>

<template>
  <span
    class="relative block overflow-hidden rounded-control bg-ink/6"
    aria-hidden="true"
  >
    <span
      class="animate-sweep absolute inset-y-0 left-0 w-1/3 bg-accent/12"
      data-motion="sweep"
    />
  </span>
</template>
