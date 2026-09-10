<script setup lang="ts">
// The application shell: the persistent rail, the top bar pinned
// inside the content column, and the routed surface between them,
// with the command palette over everything. Presentation only;
// domain rules never live in components, and the application talks
// to the core through the generated client.
import { inject, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import AppRail from './components/shell/AppRail.vue'
import AppTopBar from './components/shell/AppTopBar.vue'
import CommandPalette from './components/CommandPalette.vue'
import { applyTheme, loadTheme } from './core/theme'
import { kanbanTransportKey } from './core/transport'
import { useConnectionStore } from './stores/connection'
import { usePaletteStore } from './stores/palette'
import { usePreferencesStore } from './stores/preferences'
import { useProjectRegisterStore } from './stores/project-register'
import { useScopeStore } from './stores/scope'
import { ATTENTION_POLL_MS, NARROW_WIDTH_PX, useShellStore } from './stores/shell'

const transport = inject(kanbanTransportKey)
const route = useRoute()
const palette = usePaletteStore()
const connection = useConnectionStore()
const preferences = usePreferencesStore()
const projects = useProjectRegisterStore()
const scope = useScopeStore()
const shell = useShellStore()
const root = ref<HTMLElement | null>(null)
let observer: ResizeObserver | undefined
let poll: ReturnType<typeof setInterval> | undefined
let attentionInFlight = false

// The width the shell has to work with decides the rail and the
// board's stacking; measured on the shell itself, the window as the
// fallback.
function measure(): void {
  const width = root.value?.offsetWidth || document.documentElement.clientWidth || window.innerWidth
  shell.setNarrow(width < NARROW_WIDTH_PX)
}

async function refreshAttention(): Promise<void> {
  if (!transport || attentionInFlight || connection.phase !== 'connected') return
  attentionInFlight = true
  try {
    await shell.refreshAttention(transport)
  } finally {
    attentionInFlight = false
  }
}

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
  window.addEventListener('resize', measure)
  if (typeof ResizeObserver !== 'undefined' && root.value) {
    observer = new ResizeObserver(measure)
    observer.observe(root.value)
  }
  measure()
  applyTheme(loadTheme())
  if (transport) {
    void connection.boot(transport)
  }
  // No live event announces an Attention item, so the badge re-reads
  // the inbox on the inbox's own cadence.
  poll = setInterval(() => void refreshAttention(), ATTENTION_POLL_MS)
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onGlobalKeydown)
  window.removeEventListener('resize', measure)
  observer?.disconnect()
  if (poll !== undefined) clearInterval(poll)
})

// A connection — the first and every one after — reads the register,
// the inbox, and how the operator keeps the shell arranged; a scope
// naming a Project the register no longer offers falls back to every
// Project.
watch(
  () => connection.phase,
  (phase) => {
    if (phase === 'connected' && transport) {
      void projects.refresh(transport).then(() => scope.reconcile(projects.projects))
      void preferences.refresh(transport)
      void refreshAttention()
    }
  },
  { immediate: true },
)

// Arriving on a board adopts its scope; every arrival refreshes the
// inbox count.
watch(
  () => route.fullPath,
  () => {
    if (route.name === 'board' || route.name === 'global-board') {
      scope.adoptRoute(route.params as { projectId?: string | string[] })
    }
    void refreshAttention()
  },
  { immediate: true },
)
</script>

<template>
  <div
    ref="root"
    data-testid="app-shell"
    :data-rail-open="shell.railExpanded"
    :data-narrow="shell.narrow"
    class="flex h-screen min-h-160 overflow-hidden bg-canvas text-ink"
  >
    <AppRail />
    <div class="flex min-w-0 flex-1 flex-col overflow-hidden">
      <AppTopBar />
      <div
        data-testid="app-content"
        class="min-h-0 flex-1 overflow-auto"
      >
        <RouterView />
      </div>
    </div>
    <CommandPalette />
  </div>
</template>
