<script setup lang="ts">
// The rail's one icon set: a stroke glyph per destination, drawn
// inline so the WebView loads nothing to show it.
export type ShellIconName =
  | 'board'
  | 'inbox'
  | 'plan'
  | 'activity'
  | 'lanes'
  | 'profiles'
  | 'projects'
  | 'initiatives'
  | 'herdr'
  | 'capacity'
  | 'health'
  | 'search'
  | 'menu'
  | 'sun'
  | 'moon'
  | 'chevrons'

const PATHS: Record<ShellIconName, string> = {
  board: 'M3 3h18v18H3zM9 3v18M15 3v18',
  inbox: 'M3 13h5l1.5 3h5L16 13h5M4 6h16l1 7v5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-5z',
  plan: 'M6 6m-2.5 0a2.5 2.5 0 1 0 5 0a2.5 2.5 0 1 0-5 0M18 12m-2.5 0a2.5 2.5 0 1 0 5 0a2.5 2.5 0 1 0-5 0M6 18m-2.5 0a2.5 2.5 0 1 0 5 0a2.5 2.5 0 1 0-5 0M8.5 7.2 15.5 11M8.5 16.8 15.5 13',
  activity: 'M3 12h4l3-8 4 16 3-8h4',
  lanes: 'M4 7h16M4 12h16M4 17h16M9 7m-1.6 0a1.6 1.6 0 1 0 3.2 0a1.6 1.6 0 1 0-3.2 0M15 12m-1.6 0a1.6 1.6 0 1 0 3.2 0a1.6 1.6 0 1 0-3.2 0',
  profiles: 'M12 3 4 7v6c0 4.2 3.4 7.2 8 8 4.6-.8 8-3.8 8-8V7zM9 12l2 2 4-4',
  projects: 'M3 7h6l2 2h10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
  initiatives: 'M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z',
  herdr: 'M4 6h16v10H4zM8 20h8M12 16v4',
  capacity: 'M4 20V10M10 20V4M16 20v-7M22 20H2',
  health: 'M12 21s-7-4.5-7-11a4 4 0 0 1 7-2.5A4 4 0 0 1 19 10c0 6.5-7 11-7 11z',
  search: 'M11 11m-7 0a7 7 0 1 0 14 0a7 7 0 1 0-14 0M20 20l-3.5-3.5',
  menu: 'M4 7h16M4 12h16M4 17h16',
  sun: 'M12 12m-4 0a4 4 0 1 0 8 0a4 4 0 1 0-8 0M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4',
  moon: 'M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z',
  chevrons: 'M13 6l-6 6 6 6M19 6l-6 6 6 6',
}

const { name } = defineProps<{ name: ShellIconName }>()
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
    <path :d="PATHS[name]" />
  </svg>
</template>
