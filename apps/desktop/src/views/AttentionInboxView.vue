<script setup lang="ts">
import { inject, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import { KanbanClient } from '@kanban/contracts'
import type { AttentionItemRecord, AttentionState, ProjectRecord } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'
import NotificationSettings from '../components/NotificationSettings.vue'

const transport = inject(kanbanTransportKey)
const items = shallowRef<AttentionItemRecord[]>([])
const projects = ref<ProjectRecord[]>([])
const who = ref('')
const error = ref<string | null>(null)
const loading = ref(false)
const acknowledging = ref(false)
const includeAcknowledged = ref(false)
const includeInactive = ref(false)
const showNotifications = ref(false)
let refreshGeneration = 0
let polling: ReturnType<typeof setInterval> | undefined
const labels: Record<AttentionState, string> = {
  blocker: 'Blocker', missing_result: 'Missing result', human_decision: 'Human decision',
  review_request: 'Review request', failed_schedule: 'Missed or failed schedule',
  invalid_approval: 'Invalid approval', disconnected_session: 'Disconnected session', stale_run: 'Stale run',
}

function projectLabel(id: number): string {
  const project = projects.value.find((entry) => entry.id === id)
  return project ? `${project.code} — ${project.name}` : `Project ${id}`
}

async function refresh(): Promise<void> {
  if (!transport) return
  const generation = ++refreshGeneration
  loading.value = true
  error.value = null
  try {
    const client = new KanbanClient(transport)
    const [inbox, register] = await Promise.all([
      client.queryAttentionList({ include_acknowledged: includeAcknowledged.value, include_inactive: includeInactive.value }),
      client.queryProjectList({}),
    ])
    if (generation !== refreshGeneration) return
    items.value = inbox.items
    projects.value = register.projects
  } catch (failure) {
    if (generation === refreshGeneration) error.value = asApiError(failure).message
  } finally {
    if (generation === refreshGeneration) loading.value = false
  }
}
async function acknowledge(item: AttentionItemRecord): Promise<void> {
  if (!transport || !who.value.trim() || acknowledging.value || item.acknowledged_by != null) return
  acknowledging.value = true
  error.value = null
  try {
    await new KanbanClient(transport).commandAttentionAcknowledge({
      mutation: { optimistic_version: item.version, idempotency_key: crypto.randomUUID() },
      item_id: item.id, who: who.value,
    })
    await refresh()
  } catch (failure) {
    await refresh()
    error.value = asApiError(failure).message
  } finally {
    acknowledging.value = false
  }
}

watch([includeAcknowledged, includeInactive], () => { void refresh() })
onMounted(() => {
  void refresh()
  polling = setInterval(() => { if (!loading.value && !acknowledging.value) void refresh() }, 5000)
})
onBeforeUnmount(() => {
  refreshGeneration += 1
  if (polling !== undefined) clearInterval(polling)
})
</script>

<template>
  <main class="mx-auto flex min-h-screen max-w-5xl flex-col gap-6 p-8">
    <nav class="text-sm text-slate-500">
      <RouterLink to="/">
        Kanban
      </RouterLink> / Attention Inbox
    </nav>
    <header class="flex items-start justify-between gap-4">
      <div>
        <h1 class="text-3xl font-semibold tracking-tight">
          Attention Inbox
        </h1>
        <p class="mt-2 text-sm text-slate-600">
          Source facts are not verdicts. Only an explicit operator action acknowledges an item.
        </p>
      </div>
      <button
        type="button"
        data-testid="attention-refresh"
        :disabled="loading"
        class="rounded border px-3 py-2 text-sm"
        @click="refresh"
      >
        Refresh
      </button>
    </header>
    <button
      type="button"
      data-testid="attention-notifications"
      :aria-expanded="showNotifications"
      class="self-start rounded border px-3 py-2 text-sm"
      @click="showNotifications = !showNotifications"
    >
      Notification preferences and receipts
    </button>
    <NotificationSettings
      v-if="showNotifications"
      :projects="projects"
    />
    <div class="flex flex-wrap gap-5 text-sm">
      <label class="flex items-center gap-2"><input
        v-model="includeAcknowledged"
        data-testid="attention-show-ack"
        type="checkbox"
      > Show acknowledged</label>
      <label class="flex items-center gap-2"><input
        v-model="includeInactive"
        data-testid="attention-show-inactive"
        type="checkbox"
      > Show inactive sources</label>
    </div>
    <label class="grid max-w-md gap-1 text-sm">
      Operator name for acknowledgement
      <input
        v-model="who"
        data-testid="attention-who"
        autocomplete="name"
        class="rounded border border-slate-300 px-3 py-2"
        placeholder="Your name"
      >
    </label>
    <p
      v-if="error"
      data-testid="attention-error"
      role="alert"
      class="rounded border border-red-200 bg-red-50 p-3 text-sm text-red-700"
    >
      {{ error }}
    </p>
    <p
      v-if="loading"
      role="status"
      class="text-sm text-slate-600"
    >
      Loading attention…
    </p>
    <p
      v-if="!loading && !error && items.length === 0"
      data-testid="attention-empty"
      class="text-slate-600"
    >
      {{ includeAcknowledged || includeInactive ? 'No items match this view.' : 'No current unacknowledged items.' }}
    </p>
    <section
      v-for="item in items"
      :key="item.id"
      data-testid="attention-item"
      class="rounded-lg border border-slate-200 bg-white p-4"
    >
      <div class="flex items-start justify-between gap-4">
        <div>
          <p class="text-xs font-semibold uppercase tracking-wide text-slate-500">
            {{ projectLabel(item.project_id) }} · {{ labels[item.kind] }}
          </p>
          <h2 class="mt-2 whitespace-pre-line text-base font-semibold">
            {{ item.summary }}
          </h2>
          <p class="mt-1 text-sm text-slate-600">
            {{ item.subject_kind }} {{ item.subject_id }}
          </p>
        </div>
        <button
          type="button"
          data-testid="attention-ack"
          :disabled="!who.trim() || acknowledging || loading || item.acknowledged_by != null"
          class="rounded border border-slate-300 px-3 py-2 text-sm disabled:opacity-50"
          @click="acknowledge(item)"
        >
          Acknowledge
        </button>
      </div>
      <p
        v-if="item.acknowledged_by != null"
        data-testid="attention-acknowledged"
        class="mt-2 text-sm text-emerald-700"
      >
        Acknowledged by {{ item.acknowledged_by }} at {{ item.acknowledged_at }}.
      </p>
      <p
        v-if="!item.active"
        class="mt-2 text-sm text-slate-500"
      >
        Source no longer current. This is not an acknowledgement.
      </p>
      <details class="mt-3 text-sm">
        <summary class="cursor-pointer text-slate-600">
          Source details
        </summary>
        <pre class="mt-2 overflow-x-auto whitespace-pre-wrap rounded bg-slate-50 p-3 text-xs">{{ JSON.stringify(item.detail, null, 2) }}</pre>
      </details>
    </section>
  </main>
</template>
