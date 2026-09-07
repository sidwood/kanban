<script setup lang="ts">
// The health dashboard surface: per-component detail and
// last-change times from the one health query (KAN-S13-US5).
import HealthDashboard from '../components/HealthDashboard.vue'
</script>

<template>
  <main class="mx-auto flex min-h-screen max-w-3xl flex-col gap-6 p-8">
    <nav class="text-sm text-slate-500">
      <RouterLink
        to="/"
        class="hover:text-slate-900"
      >
        Kanban
      </RouterLink>
      <span aria-hidden="true"> / </span>
      <span class="text-slate-900">Component health</span>
    </nav>

    <h1 class="text-3xl font-semibold tracking-tight">
      Component health
    </h1>

    <HealthDashboard />
  </main>
</template>
