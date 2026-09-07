<script setup lang="ts">
// Presentation shell only. Domain rules never live in components; the
// application talks to the core through the generated client.
import { onBeforeUnmount, onMounted } from 'vue'
import CommandPalette from './components/CommandPalette.vue'
import { usePaletteStore } from './stores/palette'

const palette = usePaletteStore()

function onGlobalKeydown(event: KeyboardEvent): void {
  const key = event.key.toLowerCase()
  if ((event.metaKey || event.ctrlKey) && key === 'k') {
    event.preventDefault()
    if (palette.open) {
      palette.closePalette()
      return
    }
    palette.openPalette()
  }
}

onMounted(() => {
  window.addEventListener('keydown', onGlobalKeydown)
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onGlobalKeydown)
})
</script>

<template>
  <RouterView />
  <CommandPalette />
</template>
