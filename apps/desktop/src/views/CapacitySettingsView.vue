<script setup lang="ts">
// Capacity settings: the global defaults that constrain active runs
// by harness, model family, and usage pool, and the stricter caps
// plus maximum active Lane count one Project may impose
// (KAN-S7-US3, DR-EP-06, DR-EP-07). An empty field imposes no cap,
// and a Project cap may never relax a global default.
import { inject, onMounted } from 'vue'
import { kanbanTransportKey } from '../core/transport'
import { useCapacityStore } from '../stores/capacity-settings'

const transport = inject(kanbanTransportKey)
const capacity = useCapacityStore()

onMounted(() => {
  if (transport) {
    void capacity.refresh(transport)
  }
})

async function onProjectChange(event: Event) {
  if (!transport) {
    return
  }
  const target = event.target as HTMLSelectElement
  await capacity.selectProject(transport, Number(target.value))
}

async function saveDefaults() {
  if (transport) {
    await capacity.saveDefaults(transport)
  }
}

async function saveProjectCaps() {
  if (transport) {
    await capacity.saveProjectCaps(transport)
  }
}
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
      <span class="text-slate-900">Capacity settings</span>
    </nav>

    <h1 class="text-3xl font-semibold tracking-tight">
      Capacity
    </h1>

    <p
      v-if="capacity.error"
      data-testid="capacity-error"
      role="alert"
      class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
    >
      {{ capacity.error }}
    </p>

    <section
      v-if="capacity.loaded && capacity.defaults"
      data-testid="capacity-global-defaults"
      class="rounded border border-slate-200 p-4"
    >
      <h2 class="text-lg font-semibold text-slate-900">
        Global defaults
      </h2>
      <p class="mt-1 text-sm text-slate-500">
        The most active runs one family may carry across every
        Project. A Project may only impose stricter limits.
      </p>
      <div class="mt-4 grid gap-3 sm:grid-cols-3">
        <label class="flex flex-col gap-1 text-sm">
          <span>Per harness family</span>
          <input
            v-model.number="capacity.defaults.max_active_per_harness"
            data-testid="defaults-harness"
            type="number"
            min="1"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm">
          <span>Per model family</span>
          <input
            v-model.number="capacity.defaults.max_active_per_model"
            data-testid="defaults-model"
            type="number"
            min="1"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm">
          <span>Per usage pool</span>
          <input
            v-model.number="capacity.defaults.max_active_per_usage_pool"
            data-testid="defaults-usage-pool"
            type="number"
            min="1"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
      </div>
      <button
        type="button"
        data-testid="save-defaults"
        class="mt-4 rounded bg-slate-900 px-3 py-2 text-sm text-white"
        @click="saveDefaults"
      >
        Save global defaults
      </button>
    </section>

    <section
      v-if="capacity.loaded"
      data-testid="capacity-project-caps"
      class="rounded border border-slate-200 p-4"
    >
      <h2 class="text-lg font-semibold text-slate-900">
        Project caps
      </h2>
      <p class="mt-1 text-sm text-slate-500">
        Stricter ceilings on the same dimensions plus a maximum
        active Lane count. An empty field imposes no cap, and a cap
        above the global default is refused.
      </p>
      <label class="mt-4 flex flex-col gap-1 text-sm">
        <span>Project</span>
        <select
          data-testid="project-select"
          class="rounded border border-slate-300 px-2 py-1"
          :value="capacity.selectedProjectId ?? undefined"
          @change="onProjectChange"
        >
          <option
            v-for="project in capacity.projects"
            :key="project.id"
            :value="project.id"
          >
            {{ project.code }} · {{ project.name }}
          </option>
        </select>
      </label>

      <!-- Text inputs, not number inputs: the DOM blanks text it
        cannot parse, and blank means a deliberate clear of the cap.
        Bad text must reach the store's refusal intact. -->
      <div
        v-if="capacity.caps"
        class="mt-4 grid gap-3 sm:grid-cols-2"
      >
        <label class="flex flex-col gap-1 text-sm">
          <span>Per harness family</span>
          <input
            v-model="capacity.harness"
            data-testid="caps-harness"
            type="text"
            inputmode="numeric"
            min="1"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm">
          <span>Per model family</span>
          <input
            v-model="capacity.model"
            data-testid="caps-model"
            type="text"
            inputmode="numeric"
            min="1"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm">
          <span>Per usage pool</span>
          <input
            v-model="capacity.usagePool"
            data-testid="caps-usage-pool"
            type="text"
            inputmode="numeric"
            min="1"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm">
          <span>Maximum active Lanes</span>
          <input
            v-model="capacity.lanes"
            data-testid="caps-lanes"
            type="text"
            inputmode="numeric"
            min="1"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
      </div>

      <button
        type="button"
        data-testid="save-project-caps"
        class="mt-4 rounded bg-slate-900 px-3 py-2 text-sm text-white"
        @click="saveProjectCaps"
      >
        Save project caps
      </button>
    </section>
  </main>
</template>
