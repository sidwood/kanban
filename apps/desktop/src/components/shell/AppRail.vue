<script setup lang="ts">
// The persistent rail: the brand mark, the grouped destinations the
// handover names — Pipeline, Execution, Authoring — and the Operator
// footer. The prototype's Authoring group held only the excluded
// Ticket editors destination, so the group holds the authored
// records instead: the Plans and Specs work is written from, and the
// Projects and Initiatives that hold them (KAN-T137-AC1, AC8). It
// collapses to icons, remembers that, and at narrow width keeps only
// its icons whatever it remembers. Navigation only; nothing here
// touches workflow. There is no sign-out: no local contract says
// what signing out of a single-user install would mean.
import { computed, inject } from 'vue'
import { useRoute } from 'vue-router'
import { kanbanTransportKey } from '../../core/transport'
import { usePreferencesStore } from '../../stores/preferences'
import { useScopeStore, boardRouteFor } from '../../stores/scope'
import { useShellStore } from '../../stores/shell'
import ShellIcon from './ShellIcon.vue'
import type { ShellIconName } from './ShellIcon.vue'

interface RailItem {
  id: string
  label: string
  to: string
  icon: ShellIconName
  active: boolean
  badge: number | null
}

interface RailGroup {
  id: string
  label: string
  items: RailItem[]
}

const transport = inject(kanbanTransportKey)
const route = useRoute()
const shell = useShellStore()
const preferences = usePreferencesStore()
const scope = useScopeStore()

const open = computed(() => shell.railExpanded)

const groups = computed<RailGroup[]>(() => {
  const path = route.path
  const workspacesRoute =
    scope.projectId === null ? '/workspaces' : `/projects/${scope.projectId}/workspaces`
  return [
    {
      id: 'pipeline',
      label: 'Pipeline',
      items: [
        {
          id: 'boards',
          label: 'Boards',
          to: boardRouteFor(scope.scope),
          icon: 'board',
          active: path === '/board' || /^\/projects\/\d+\/board$/.test(path),
          badge: null,
        },
        {
          id: 'attention',
          label: 'Attention inbox',
          to: '/attention',
          icon: 'inbox',
          active: path === '/attention',
          badge: shell.attentionCount,
        },
        {
          id: 'activity',
          label: 'Activity',
          to: '/activity',
          icon: 'activity',
          active: path === '/activity',
          badge: null,
        },
      ],
    },
    {
      id: 'execution',
      label: 'Execution',
      items: [
        {
          id: 'workspaces',
          label: 'Workspaces & Lanes',
          to: workspacesRoute,
          icon: 'lanes',
          active: path.endsWith('/workspaces'),
          badge: null,
        },
        {
          id: 'profiles',
          label: 'Execution profiles',
          to: '/settings/profiles',
          icon: 'profiles',
          active: path === '/settings/profiles',
          badge: null,
        },
        {
          id: 'herdr',
          label: 'Herdr settings',
          to: '/settings/herdr',
          icon: 'herdr',
          active: path === '/settings/herdr',
          badge: null,
        },
        {
          id: 'capacity',
          label: 'Capacity settings',
          to: '/settings/capacity',
          icon: 'capacity',
          active: path === '/settings/capacity',
          badge: null,
        },
        {
          id: 'health',
          label: 'Health',
          to: '/health',
          icon: 'health',
          active: path === '/health',
          badge: null,
        },
      ],
    },
    {
      id: 'authoring',
      label: 'Authoring',
      items: [
        {
          id: 'planning',
          label: 'Planning',
          to: '/planning',
          icon: 'plan',
          active: path.startsWith('/planning'),
          badge: null,
        },
        {
          id: 'projects',
          label: 'Projects',
          to: '/register',
          icon: 'projects',
          active: path === '/register',
          badge: null,
        },
        {
          id: 'initiatives',
          label: 'Initiatives',
          to: '/initiatives',
          icon: 'initiatives',
          active: path === '/initiatives',
          badge: null,
        },
      ],
    },
  ]
})

const toggleLabel = computed(() => (open.value ? 'Collapse sidebar' : 'Expand sidebar'))

// The rail is a shell control. Collapse and expand happen here;
// the core may remember the choice when it is reachable.
function toggleRail(): void {
  void preferences.setRailOpen(transport, !preferences.railOpen)
}
</script>

<template>
  <nav
    aria-label="Primary"
    data-testid="app-rail"
    class="flex h-full shrink-0 flex-col overflow-hidden border-r border-line bg-surface transition-[width] duration-200"
    :class="open ? 'w-62' : 'w-16'"
  >
    <div
      class="flex h-12 shrink-0 items-center gap-2.5"
      :class="open ? 'px-3' : 'justify-center px-0'"
    >
      <RouterLink
        to="/board"
        data-testid="brand"
        class="flex min-w-0 items-center gap-2.5 rounded-control focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
        aria-label="Kanban, open the board"
      >
        <svg
          class="size-7 shrink-0"
          viewBox="0 0 40 40"
          aria-hidden="true"
        >
          <defs>
            <linearGradient
              id="kanban-mark"
              x1="0"
              y1="0"
              x2=".4"
              y2="1"
            >
              <stop
                offset="0"
                stop-color="#a8dc70"
              />
              <stop
                offset=".55"
                stop-color="#88d048"
              />
              <stop
                offset="1"
                stop-color="#6cb62f"
              />
            </linearGradient>
          </defs>
          <circle
            cx="20"
            cy="20"
            r="18"
            fill="none"
            stroke="url(#kanban-mark)"
            stroke-width="4"
          />
          <g
            fill="none"
            stroke="#6cb62f"
            stroke-width="4.6"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <path d="M13.8 11v18" />
            <path d="M26.2 11 16.6 20.4" />
            <path d="M18.4 19.2 26.6 29" />
          </g>
        </svg>
        <span
          class="flex min-w-0 flex-col"
          :class="open ? '' : 'sr-only'"
        >
          <span class="wordmark text-[0.7rem] leading-tight text-ink">Kanban</span>
          <span class="text-[0.5rem] font-semibold tracking-[0.085em] text-ink-subtle uppercase">
            Control centre
          </span>
        </span>
      </RouterLink>
      <button
        v-if="open"
        type="button"
        data-testid="rail-toggle"
        class="ml-auto flex size-7 shrink-0 items-center justify-center rounded-control border border-transparent text-ink-subtle transition-colors hover:border-line hover:text-ink"
        :aria-expanded="open"
        :aria-label="toggleLabel"
        :title="toggleLabel"
        @click="toggleRail"
      >
        <ShellIcon name="chevrons" />
      </button>
    </div>
    <div
      v-if="!open"
      class="flex justify-center pt-2 pb-0.5"
    >
      <button
        type="button"
        data-testid="rail-toggle"
        class="flex size-7 items-center justify-center rounded-control border border-transparent text-ink-subtle transition-colors hover:border-line hover:text-ink"
        :aria-expanded="open"
        :aria-label="toggleLabel"
        :title="toggleLabel"
        @click="toggleRail"
      >
        <ShellIcon
          name="chevrons"
          class="rotate-180"
        />
      </button>
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto px-2 pt-1.5 pb-2.5">
      <div
        v-for="(group, index) in groups"
        :key="group.id"
        :data-testid="`rail-group-${group.id}`"
        :class="index === 0 ? 'mt-1' : open ? 'mt-4' : 'mt-0'"
      >
        <p
          class="mb-1 px-2 text-[0.6rem] font-bold tracking-[0.13em] text-ink-subtle uppercase"
          :class="open ? '' : 'sr-only'"
        >
          {{ group.label }}
        </p>
        <div
          v-if="!open && index > 0"
          class="mx-1.5 my-2 h-px bg-line"
          aria-hidden="true"
        />
        <ul class="flex flex-col gap-0.5">
          <li
            v-for="item in group.items"
            :key="item.id"
          >
            <RouterLink
              v-slot="{ href, navigate }"
              :to="item.to"
              custom
            >
              <a
                :href="href"
                :data-testid="`rail-link-${item.id}`"
                :aria-current="item.active ? 'page' : undefined"
                :title="item.label"
                class="relative flex h-8.5 w-full items-center gap-2.5 rounded-control text-[0.8rem] transition-colors"
                :class="[
                  open ? 'px-2.5' : 'justify-center px-0',
                  item.active
                    ? 'bg-accent/11 font-semibold text-accent'
                    : 'font-medium text-ink-muted hover:bg-accent/8 hover:text-ink',
                ]"
                @click="navigate"
              >
                <span
                  v-if="item.active"
                  class="absolute top-1/2 -left-2 h-4.5 w-0.75 -translate-y-1/2 rounded-r-sm bg-accent-fill"
                  aria-hidden="true"
                />
                <ShellIcon :name="item.icon" />
                <span
                  class="min-w-0 flex-1 truncate text-left"
                  :class="open ? '' : 'sr-only'"
                >{{ item.label }}</span>
                <span
                  v-if="item.badge !== null && item.badge > 0"
                  :data-testid="`rail-${item.id}-count`"
                  class="rounded-full border border-caution/35 bg-caution/10 px-1.5 py-px font-mono text-[0.625rem] text-caution"
                  :class="open ? '' : 'absolute -top-0.5 -right-0.5'"
                  :aria-label="`${item.badge} waiting`"
                >
                  {{ item.badge }}
                </span>
              </a>
            </RouterLink>
          </li>
        </ul>
      </div>
    </div>

    <div
      data-testid="rail-operator"
      class="flex shrink-0 items-center gap-2.5 border-t border-line py-2.5"
      :class="open ? 'px-3' : 'justify-center px-0'"
    >
      <span
        class="grid size-7 shrink-0 place-items-center rounded-full bg-accent-fill font-sans text-xs font-bold text-cta-ink"
        aria-hidden="true"
      >
        O
      </span>
      <span
        class="flex min-w-0 flex-col"
        :class="open ? '' : 'sr-only'"
      >
        <span class="text-[0.8rem] font-semibold text-ink">Operator</span>
        <span class="text-[0.7rem] text-ink-subtle">Local control plane</span>
      </span>
    </div>
  </nav>
</template>
