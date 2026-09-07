<script setup lang="ts">
// The health dashboard: the one health query rendered with
// per-component detail and last-change times (KAN-S13-US5,
// DR-RB-12). Every time shown is a time a component already
// records; nothing here observes anything itself.
import { computed, inject, onMounted } from 'vue'
import { kanbanTransportKey } from '../core/transport'
import { useHealthStore } from '../stores/health'

const transport = inject(kanbanTransportKey)
const healthState = useHealthStore()

async function recheck(): Promise<void> {
  if (transport) {
    await healthState.refresh(transport)
  }
}

onMounted(() => {
  void recheck()
})

const health = computed(() => healthState.health)

const census = computed(() => {
  const byHealth = health.value?.workspaces.by_health
  return byHealth
    ? ([
        ['available', byHealth.available],
        ['assigned', byHealth.assigned],
        ['dirty', byHealth.dirty],
        ['missing', byHealth.missing],
        ['retired', byHealth.retired],
        ['unobserved', byHealth.unobserved],
      ] as const)
    : []
})
</script>

<template>
  <p
    v-if="healthState.error"
    data-testid="health-error"
    role="alert"
    class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
  >
    {{ healthState.error }}
  </p>

  <button
    type="button"
    data-testid="health-recheck"
    class="w-fit rounded bg-slate-900 px-3 py-2 text-sm text-white"
    @click="recheck"
  >
    Recheck
  </button>

  <section
    v-if="health"
    data-testid="health-dashboard"
    class="flex w-full max-w-3xl flex-col gap-4"
  >
    <section
      data-testid="health-service"
      class="rounded-lg border border-slate-200 bg-white p-4 shadow-sm"
    >
      <h2 class="text-lg font-semibold text-slate-900">
        Service
      </h2>
      <dl class="mt-3 grid gap-2 sm:grid-cols-3">
        <div>
          <dt class="text-sm text-slate-500">
            Connected
          </dt>
          <dd
            data-testid="health-service-connected"
            class="text-sm text-slate-900"
          >
            {{ health.connected ? 'yes' : 'no' }}
          </dd>
        </div>
        <div>
          <dt class="text-sm text-slate-500">
            Version
          </dt>
          <dd
            data-testid="health-service-version"
            class="text-sm text-slate-900"
          >
            {{ health.service_version }}
          </dd>
        </div>
        <div>
          <dt class="text-sm text-slate-500">
            Started
          </dt>
          <dd
            data-testid="health-service-started"
            class="text-sm text-slate-900"
          >
            {{ health.service.started_at }}
          </dd>
        </div>
      </dl>
    </section>

    <section
      data-testid="health-database"
      class="rounded-lg border border-slate-200 bg-white p-4 shadow-sm"
    >
      <h2 class="text-lg font-semibold text-slate-900">
        Database
      </h2>
      <dl class="mt-3 grid gap-2 sm:grid-cols-3">
        <div>
          <dt class="text-sm text-slate-500">
            Journal mode
          </dt>
          <dd
            data-testid="health-database-journal"
            class="text-sm text-slate-900"
          >
            {{ health.database.journal_mode }}
          </dd>
        </div>
        <div>
          <dt class="text-sm text-slate-500">
            Schema version
          </dt>
          <dd
            data-testid="health-database-schema"
            class="text-sm text-slate-900"
          >
            {{ health.database.schema_version }}
          </dd>
        </div>
        <div>
          <dt class="text-sm text-slate-500">
            Last change
          </dt>
          <dd
            data-testid="health-database-last-change"
            class="text-sm text-slate-900"
          >
            {{ health.database.last_change_at ?? 'never' }}
          </dd>
        </div>
      </dl>
    </section>

    <section
      data-testid="health-scheduler"
      class="rounded-lg border border-slate-200 bg-white p-4 shadow-sm"
    >
      <h2 class="text-lg font-semibold text-slate-900">
        Scheduler
      </h2>
      <dl class="mt-3 grid gap-2 sm:grid-cols-2">
        <div>
          <dt class="text-sm text-slate-500">
            Last backup succeeded
          </dt>
          <dd
            data-testid="health-scheduler-last-backup"
            class="text-sm text-slate-900"
          >
            {{ health.scheduler.last_backup_success_at ?? 'never' }}
          </dd>
        </div>
      </dl>
    </section>

    <section
      data-testid="health-mcp"
      class="rounded-lg border border-slate-200 bg-white p-4 shadow-sm"
    >
      <h2 class="text-lg font-semibold text-slate-900">
        MCP
      </h2>
      <dl class="mt-3 grid gap-2 sm:grid-cols-2">
        <div>
          <dt class="text-sm text-slate-500">
            Exposed tools
          </dt>
          <dd
            data-testid="health-mcp-tools"
            class="text-sm text-slate-900"
          >
            {{ health.mcp.exposed_tools }}
          </dd>
        </div>
      </dl>
    </section>

    <section
      data-testid="health-herdr"
      class="rounded-lg border border-slate-200 bg-white p-4 shadow-sm"
    >
      <h2 class="text-lg font-semibold text-slate-900">
        Herdr
      </h2>
      <p
        v-if="health.herdr.sessions.length === 0"
        class="mt-2 text-sm text-slate-500"
      >
        No session is observed.
      </p>
      <ul
        v-else
        class="mt-3 flex flex-col gap-3"
      >
        <li
          v-for="session in health.herdr.sessions"
          :key="session.project_id"
          data-testid="health-herdr-session"
          class="rounded border border-slate-100 bg-slate-50 p-3"
        >
          <dl class="grid gap-2 sm:grid-cols-3">
            <div>
              <dt class="text-sm text-slate-500">
                Project
              </dt>
              <dd
                data-testid="health-herdr-project"
                class="text-sm text-slate-900"
              >
                {{ session.project_id }}
              </dd>
            </div>
            <div>
              <dt class="text-sm text-slate-500">
                Session
              </dt>
              <dd
                data-testid="health-herdr-name"
                class="text-sm text-slate-900"
              >
                {{ session.diagnostics.session_name ?? 'default session' }}
              </dd>
            </div>
            <div>
              <dt class="text-sm text-slate-500">
                Connected
              </dt>
              <dd
                data-testid="health-herdr-connected"
                class="text-sm text-slate-900"
              >
                {{ session.diagnostics.connected ? 'yes' : 'no' }}
              </dd>
            </div>
            <div>
              <dt class="text-sm text-slate-500">
                Last snapshot
              </dt>
              <dd
                data-testid="health-herdr-snapshot"
                class="text-sm text-slate-900"
              >
                {{ session.diagnostics.last_snapshot_at ?? 'never' }}
              </dd>
            </div>
            <div class="sm:col-span-2">
              <dt class="text-sm text-slate-500">
                Last error
              </dt>
              <dd
                data-testid="health-herdr-error"
                class="text-sm text-slate-900"
              >
                {{ session.diagnostics.last_error ?? 'none' }}
              </dd>
            </div>
          </dl>
        </li>
      </ul>
    </section>

    <section
      data-testid="health-workspaces"
      class="rounded-lg border border-slate-200 bg-white p-4 shadow-sm"
    >
      <h2 class="text-lg font-semibold text-slate-900">
        Workspaces
      </h2>
      <dl class="mt-3 grid gap-2 sm:grid-cols-3">
        <div
          v-for="[state, count] in census"
          :key="state"
        >
          <dt class="text-sm text-slate-500">
            {{ state }}
          </dt>
          <dd
            :data-testid="`health-workspace-${state}`"
            class="text-sm text-slate-900"
          >
            {{ count }}
          </dd>
        </div>
        <div class="sm:col-span-3">
          <dt class="text-sm text-slate-500">
            Last change
          </dt>
          <dd
            data-testid="health-workspaces-last-change"
            class="text-sm text-slate-900"
          >
            {{ health.workspaces.last_change_at ?? 'never' }}
          </dd>
        </div>
      </dl>
    </section>
  </section>

  <p
    v-else
    data-testid="health-loading"
    class="text-sm text-slate-500"
  >
    Reading component health…
  </p>
</template>
