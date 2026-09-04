<script setup lang="ts">
// The boot surface: everything the operator sees before the board
// views land, bound to connection state from the generated client.
import { computed, inject, onMounted } from 'vue'
import TimelineSurface from '../components/TimelineSurface.vue'
import { kanbanTransportKey } from '../core/transport'
import { useConnectionStore } from '../stores/connection'

const transport = inject(kanbanTransportKey)
const connection = useConnectionStore()

onMounted(() => {
  if (transport) {
    void connection.boot(transport)
  }
})

const status = computed(() => {
  switch (connection.phase) {
    case 'connected':
      return `Core connected · v${connection.serviceVersion ?? 'unknown'}`
    case 'disconnected':
      return 'Core unreachable'
    default:
      return 'Connecting to the core…'
  }
})

const eventStream = computed(() =>
  connection.lastEventSequence === null
    ? 'Event stream idle'
    : `Event stream live · sequence ${connection.lastEventSequence}`,
)
</script>

<template>
  <main class="flex min-h-screen flex-col items-center justify-center gap-3">
    <h1 class="text-4xl font-semibold tracking-tight">
      Kanban
    </h1>
    <p
      data-testid="connection-status"
      class="text-sm text-slate-600"
      aria-live="polite"
    >
      {{ status }}
    </p>
    <p
      data-testid="event-stream"
      class="text-xs text-slate-400"
    >
      {{ eventStream }}
    </p>
    <RouterLink
      to="/initiatives"
      class="text-sm text-slate-500 underline-offset-4 hover:text-slate-900 hover:underline"
    >
      Manage Initiatives
    </RouterLink>
    <TimelineSurface
      v-if="connection.phase === 'connected'"
      project-id="kan"
      entity-kind="ticket"
      entity-id="kan-t9"
      class="mt-6"
    />
  </main>
</template>
