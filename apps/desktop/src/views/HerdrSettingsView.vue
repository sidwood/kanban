<script setup lang="ts">
// Herdr observation settings: reconciliation, fallback polling,
// deadlines, and connection diagnostics (KAN-S8-US1, DR-HB-11).
import { inject, onMounted } from 'vue'
import { kanbanTransportKey } from '../core/transport'
import { useHerdrSettingsStore } from '../stores/herdr-settings'

const transport = inject(kanbanTransportKey)
const herdr = useHerdrSettingsStore()

onMounted(() => {
  if (transport) {
    void herdr.refresh(transport)
  }
})

async function onProjectChange(event: Event) {
  if (!transport) {
    return
  }
  const target = event.target as HTMLSelectElement
  await herdr.selectProject(transport, Number(target.value))
}

async function saveProjectSettings() {
  if (transport) {
    await herdr.saveProjectSettings(transport)
  }
}

async function saveDefaults() {
  if (transport) {
    await herdr.saveDefaults(transport)
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
      <span class="text-slate-900">Herdr settings</span>
    </nav>

    <h1 class="text-3xl font-semibold tracking-tight">
      Herdr observation
    </h1>

    <p
      v-if="herdr.error"
      data-testid="herdr-error"
      role="alert"
      class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
    >
      {{ herdr.error }}
    </p>

    <section
      v-if="herdr.loaded && herdr.defaults"
      data-testid="herdr-global-defaults"
      class="rounded border border-slate-200 p-4"
    >
      <h2 class="text-lg font-semibold text-slate-900">
        Global defaults
      </h2>
      <div class="mt-4 grid gap-3 sm:grid-cols-3">
        <label class="flex flex-col gap-1 text-sm">
          <span>Reconciliation interval (seconds)</span>
          <input
            v-model.number="herdr.defaults.reconciliation_interval_secs"
            data-testid="defaults-reconciliation"
            type="number"
            min="1"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm">
          <span>Stall deadline (seconds)</span>
          <input
            v-model.number="herdr.defaults.stall_deadline_secs"
            data-testid="defaults-stall"
            type="number"
            min="1"
            class="rounded border border-slate-300 px-2 py-1"
          >
        </label>
        <label class="flex flex-col gap-1 text-sm">
          <span>Missing-result deadline (seconds)</span>
          <input
            v-model.number="herdr.defaults.missing_result_deadline_secs"
            data-testid="defaults-missing-result"
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
      v-if="herdr.loaded"
      data-testid="herdr-project-settings"
      class="rounded border border-slate-200 p-4"
    >
      <h2 class="text-lg font-semibold text-slate-900">
        Project settings
      </h2>
      <label class="mt-4 flex flex-col gap-1 text-sm">
        <span>Project</span>
        <select
          data-testid="project-select"
          class="rounded border border-slate-300 px-2 py-1"
          :value="herdr.selectedProjectId ?? undefined"
          @change="onProjectChange"
        >
          <option
            v-for="project in herdr.projects"
            :key="project.id"
            :value="project.id"
          >
            {{ project.code }} · {{ project.herdr_session }}
          </option>
        </select>
      </label>

      <template v-if="herdr.settings && herdr.diagnostics">
        <div class="mt-4 grid gap-3 sm:grid-cols-2">
          <label class="flex flex-col gap-1 text-sm">
            <span>Reconciliation interval (seconds)</span>
            <input
              v-model.number="herdr.settings.reconciliation_interval_secs"
              data-testid="settings-reconciliation"
              type="number"
              min="1"
              class="rounded border border-slate-300 px-2 py-1"
            >
          </label>
          <label class="flex items-center gap-2 text-sm">
            <input
              v-model="herdr.settings.polling_fallback_enabled"
              data-testid="settings-polling-enabled"
              type="checkbox"
            >
            Enable polling fallback
          </label>
          <label class="flex flex-col gap-1 text-sm">
            <span>Polling fallback interval (seconds)</span>
            <input
              v-model.number="herdr.settings.polling_fallback_interval_secs"
              data-testid="settings-polling-interval"
              type="number"
              min="1"
              class="rounded border border-slate-300 px-2 py-1"
            >
          </label>
          <label class="flex flex-col gap-1 text-sm">
            <span>Stall deadline (seconds)</span>
            <input
              v-model.number="herdr.settings.stall_deadline_secs"
              data-testid="settings-stall"
              type="number"
              min="1"
              class="rounded border border-slate-300 px-2 py-1"
            >
          </label>
          <label class="flex flex-col gap-1 text-sm sm:col-span-2">
            <span>Missing-result deadline (seconds)</span>
            <input
              v-model.number="herdr.settings.missing_result_deadline_secs"
              data-testid="settings-missing-result"
              type="number"
              min="1"
              class="rounded border border-slate-300 px-2 py-1"
            >
          </label>
        </div>

        <button
          type="button"
          data-testid="save-project-settings"
          class="mt-4 rounded bg-slate-900 px-3 py-2 text-sm text-white"
          @click="saveProjectSettings"
        >
          Save project settings
        </button>

        <section
          data-testid="herdr-diagnostics"
          class="mt-6 rounded border border-slate-100 bg-slate-50 p-4 text-sm"
        >
          <h3 class="font-medium text-slate-900">
            Connection diagnostics
          </h3>
          <dl class="mt-3 grid gap-2 sm:grid-cols-2">
            <div>
              <dt class="text-slate-500">
                Session
              </dt>
              <dd data-testid="diagnostics-session">
                {{ herdr.diagnostics.session_name }}
              </dd>
            </div>
            <div>
              <dt class="text-slate-500">
                Connected
              </dt>
              <dd data-testid="diagnostics-connected">
                {{ herdr.diagnostics.connected ? 'yes' : 'no' }}
              </dd>
            </div>
            <div>
              <dt class="text-slate-500">
                Product workspace
              </dt>
              <dd data-testid="diagnostics-product-workspace">
                {{ herdr.diagnostics.product_workspace }}
              </dd>
            </div>
            <div>
              <dt class="text-slate-500">
                Herdr workspace
              </dt>
              <dd data-testid="diagnostics-herdr-workspace">
                {{ herdr.diagnostics.herdr_workspace ?? 'unknown' }}
              </dd>
            </div>
            <div>
              <dt class="text-slate-500">
                Last snapshot
              </dt>
              <dd data-testid="diagnostics-last-snapshot">
                {{ herdr.diagnostics.last_snapshot_at ?? 'never' }}
              </dd>
            </div>
            <div>
              <dt class="text-slate-500">
                Last error
              </dt>
              <dd data-testid="diagnostics-last-error">
                {{ herdr.diagnostics.last_error ?? 'none' }}
              </dd>
            </div>
          </dl>
        </section>
      </template>
    </section>
  </main>
</template>
