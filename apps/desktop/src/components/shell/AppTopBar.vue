<script setup lang="ts">
// The top bar pinned inside the content column: the board scope, the
// search control that opens the command palette, the observed
// connection, and the theme. The connection chip reports what the
// generated health query and the shell's announcements say; it is
// a link to the health surface and never a switch.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { applyTheme, loadTheme, saveTheme } from '../../core/theme'
import type { ThemeName } from '../../core/theme'
import { useConnectionStore } from '../../stores/connection'
import { usePaletteStore } from '../../stores/palette'
import { useProjectRegisterStore } from '../../stores/project-register'
import { boardRouteFor, useScopeStore } from '../../stores/scope'
import type { BoardScope } from '../../stores/scope'
import ShellIcon from './ShellIcon.vue'

const router = useRouter()
const connection = useConnectionStore()
const palette = usePaletteStore()
const projects = useProjectRegisterStore()
const scope = useScopeStore()

// The scope menu.
const menuOpen = ref(false)
const menuRoot = ref<HTMLElement | null>(null)

const scopeCode = computed(() => {
  if (scope.projectId === null) return 'ALL'
  return projects.projects.find((entry) => entry.id === scope.projectId)?.code ?? `#${scope.projectId}`
})

const scopeName = computed(() => {
  if (scope.projectId === null) return 'All projects'
  return projects.projects.find((entry) => entry.id === scope.projectId)?.name ?? 'Project'
})

const scopeOptions = computed(() => [
  { value: 'all' as BoardScope, code: 'ALL', name: 'All projects', detail: 'Every Project' },
  ...projects.projects
    .filter((entry) => !entry.archived)
    .map((entry) => ({
      value: entry.id as BoardScope,
      code: entry.code,
      name: entry.name,
      detail: entry.repository,
    })),
])

async function pickScope(next: BoardScope): Promise<void> {
  menuOpen.value = false
  scope.set(next)
  await router.push(boardRouteFor(next))
}

function onWindowClick(event: MouseEvent): void {
  if (!menuOpen.value) return
  const target = event.target
  if (target instanceof Node && menuRoot.value?.contains(target)) return
  menuOpen.value = false
}

function onWindowKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape' && menuOpen.value) {
    menuOpen.value = false
  }
}

onMounted(() => {
  window.addEventListener('click', onWindowClick)
  window.addEventListener('keydown', onWindowKeydown)
})

onBeforeUnmount(() => {
  window.removeEventListener('click', onWindowClick)
  window.removeEventListener('keydown', onWindowKeydown)
})

// The observed connection.
const connectionLabel = computed(() => {
  switch (connection.phase) {
    case 'connected':
      return 'Service running'
    case 'disconnected':
      return 'Core unreachable'
    default:
      return 'Connecting…'
  }
})

const connectionClass = computed(() => {
  switch (connection.phase) {
    case 'connected':
      return 'border-line bg-surface text-ink-muted'
    case 'disconnected':
      return 'border-caution/40 bg-caution-soft text-caution'
    default:
      return 'border-line bg-surface text-ink-subtle'
  }
})

const connectionDotClass = computed(() => {
  switch (connection.phase) {
    case 'connected':
      return 'bg-accent-fill'
    case 'disconnected':
      return 'bg-caution'
    default:
      return 'bg-ink-subtle'
  }
})

// The theme.
const theme = ref<ThemeName>(loadTheme())

function toggleTheme(): void {
  theme.value = theme.value === 'dark' ? 'light' : 'dark'
  applyTheme(theme.value)
  saveTheme(theme.value)
}

const themeLabel = computed(() =>
  theme.value === 'dark' ? 'Switch to the daylight theme' : 'Switch to the night theme',
)
</script>

<template>
  <header
    data-testid="app-top-bar"
    class="flex h-12 shrink-0 items-center gap-3 border-b border-line bg-surface px-3"
  >
    <div
      ref="menuRoot"
      class="relative min-w-0"
    >
      <button
        type="button"
        data-testid="scope-menu"
        aria-haspopup="menu"
        :aria-expanded="menuOpen"
        aria-label="Scope the board to a Project"
        class="flex h-7 max-w-64 items-center gap-2 rounded-full border border-line bg-surface px-2.5 text-left transition-colors hover:border-line-strong"
        @click="menuOpen = !menuOpen"
      >
        <span class="font-mono text-[0.7rem] font-semibold text-accent">{{ scopeCode }}</span>
        <span class="hidden truncate text-xs font-medium text-ink sm:inline">{{ scopeName }}</span>
        <svg
          class="size-3 shrink-0 text-ink-subtle"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          aria-hidden="true"
        >
          <path d="M6 9l6 6 6-6" />
        </svg>
      </button>
      <div
        v-if="menuOpen"
        role="menu"
        aria-label="Board scope"
        class="absolute top-9 left-0 z-50 w-72 rounded-panel border border-line-strong bg-surface p-1.5 shadow-panel"
      >
        <p class="px-2.5 pt-1.5 pb-2 text-[0.6rem] font-bold tracking-[0.12em] text-ink-subtle uppercase">
          Board scope
        </p>
        <button
          v-for="option in scopeOptions"
          :key="String(option.value)"
          type="button"
          role="menuitemradio"
          :aria-checked="option.value === scope.scope"
          :data-testid="`scope-option-${option.value}`"
          class="flex w-full items-center gap-2.5 rounded-control border px-2.5 py-1.5 text-left transition-colors"
          :class="
            option.value === scope.scope
              ? 'border-accent/30 bg-accent/9'
              : 'border-transparent hover:bg-accent/6'
          "
          @click="pickScope(option.value)"
        >
          <span class="w-9 shrink-0 font-mono text-[0.65rem] font-semibold text-accent">
            {{ option.code }}
          </span>
          <span class="flex min-w-0 flex-col">
            <span class="truncate text-xs font-medium text-ink">{{ option.name }}</span>
            <span class="truncate text-[0.7rem] text-ink-subtle">{{ option.detail }}</span>
          </span>
        </button>
      </div>
    </div>

    <div class="flex min-w-0 flex-1 justify-center">
      <button
        type="button"
        data-testid="open-search"
        aria-label="Search tickets, specs and views"
        class="flex h-7 w-full max-w-105 items-center gap-2 rounded-control border border-line bg-rail px-2.5 text-left text-ink-subtle transition-colors hover:border-line-strong"
        @click="palette.openPalette()"
      >
        <ShellIcon name="search" />
        <span class="hidden min-w-0 flex-1 truncate text-xs sm:inline">Search tickets, specs, views…</span>
        <kbd class="hidden rounded-[4px] border border-line-strong px-1 font-mono text-[0.625rem] sm:inline">⌘K</kbd>
      </button>
    </div>

    <div class="flex shrink-0 items-center gap-2">
      <RouterLink
        to="/health"
        data-testid="connection-chip"
        :data-phase="connection.phase"
        :aria-label="`${connectionLabel}. Open health.`"
        class="flex h-6.5 items-center gap-1.5 rounded-full border px-2.5 text-[0.7rem] font-medium transition-colors hover:border-line-strong"
        :class="connectionClass"
        aria-live="polite"
      >
        <span
          class="size-1.5 rounded-full"
          :class="connectionDotClass"
          aria-hidden="true"
        />
        <span>{{ connectionLabel }}</span>
      </RouterLink>
      <button
        type="button"
        data-testid="theme-toggle"
        :aria-label="themeLabel"
        :aria-pressed="theme === 'dark'"
        :title="themeLabel"
        class="flex size-7 items-center justify-center rounded-control border border-line bg-surface text-ink-muted transition-colors hover:border-line-strong hover:text-ink"
        @click="toggleTheme"
      >
        <ShellIcon :name="theme === 'dark' ? 'sun' : 'moon'" />
      </button>
    </div>
  </header>
</template>
