<script setup lang="ts">
import { inject, onBeforeUnmount, ref, watch } from 'vue'
import { KanbanClient } from '@kanban/contracts'
import type { NotificationDeliveryRecord, NotificationPermissionRecord, NotificationSettingsRecord, ProjectRecord } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'

const props = defineProps<{ projects: ProjectRecord[] }>()
const transport = inject(kanbanTransportKey)
const selectedProjectId = ref<number | null>(props.projects.find((project) => !project.archived)?.id ?? null)
const settings = ref<NotificationSettingsRecord | null>(null)
const permission = ref<NotificationPermissionRecord | null>(null)
const deliveries = ref<NotificationDeliveryRecord[]>([])
const localEnabled = ref(false)
const mirrorEnabled = ref(false)
const mirrorRole = ref('')
const loading = ref(false)
const saving = ref(false)
const saved = ref(false)
const requestingPermission = ref(false)
const permissionRequested = ref(false)
const retrying = ref(false)
const error = ref<string | null>(null)
let generation = 0
let statusGeneration = 0
const statusLoading = ref(false)

watch(() => props.projects, (projects) => {
  if (!projects.some((project) => project.id === selectedProjectId.value && !project.archived)) {
    selectedProjectId.value = projects.find((project) => !project.archived)?.id ?? null
  }
})
watch(selectedProjectId, () => { void load() }, { immediate: true })

async function load(): Promise<void> {
  const id = selectedProjectId.value
  const current = ++generation
  statusGeneration += 1
  settings.value = null
  saved.value = false
  error.value = null
  if (!transport || id === null) return
  loading.value = true
  try {
    const client = new KanbanClient(transport)
    const [record, native, history] = await Promise.all([
      client.queryNotificationSettingsGet({ project_id: id }),
      client.queryNotificationPermissionGet({}),
      client.queryNotificationDeliveries({ project_id: id }),
    ])
    if (current !== generation) return
    settings.value = record
    permission.value = native
    deliveries.value = history.deliveries
    localEnabled.value = record.local_enabled
    mirrorEnabled.value = record.mirror_role != null
    mirrorRole.value = record.mirror_role ?? ''
  } catch (failure) {
    if (current === generation) error.value = asApiError(failure).message
  } finally {
    if (current === generation) loading.value = false
  }
}

async function refreshStatus(): Promise<void> {
  const id = selectedProjectId.value
  if (!transport || id === null) return
  const current = generation
  const status = ++statusGeneration
  statusLoading.value = true
  try {
    const client = new KanbanClient(transport)
    const [native, history] = await Promise.all([
      client.queryNotificationPermissionGet({}), client.queryNotificationDeliveries({ project_id: id }),
    ])
    if (current !== generation || status !== statusGeneration) return
    permission.value = native
    deliveries.value = history.deliveries
  } catch (failure) {
    if (current === generation && status === statusGeneration) error.value = asApiError(failure).message
  } finally {
    if (status === statusGeneration) statusLoading.value = false
  }
}
onBeforeUnmount(() => { generation += 1; statusGeneration += 1 })

async function requestPermission(): Promise<void> {
  if (!transport || permission.value?.state !== 'not_determined' || permission.value.request_pending || requestingPermission.value) return
  requestingPermission.value = true
  error.value = null
  try {
    const client = new KanbanClient(transport)
    const result = await client.commandNotificationPermissionRequest({
      mutation: { optimistic_version: 0, idempotency_key: crypto.randomUUID() },
    })
    permissionRequested.value = result.accepted
    await refreshStatus()
  } catch (failure) {
    error.value = asApiError(failure).message
  } finally {
    requestingPermission.value = false
  }
}

async function retryDelivery(delivery: NotificationDeliveryRecord): Promise<void> {
  if (!transport || delivery.status !== 'failed' || retrying.value) return
  retrying.value = true
  error.value = null
  try {
    await new KanbanClient(transport).commandNotificationRetry({
      mutation: { optimistic_version: delivery.version, idempotency_key: crypto.randomUUID() },
      delivery_id: delivery.id,
    })
    await refreshStatus()
  } catch (failure) {
    error.value = asApiError(failure).message
  } finally {
    retrying.value = false
  }
}

async function save(): Promise<void> {
  const record = settings.value
  if (!transport || !record || loading.value || saving.value || (mirrorEnabled.value && !mirrorRole.value.trim())) return
  saving.value = true
  saved.value = false
  error.value = null
  try {
    const updated = await new KanbanClient(transport).commandNotificationSettingsUpdate({
      mutation: { optimistic_version: record.version, idempotency_key: crypto.randomUUID() },
      project_id: record.project_id, local_enabled: localEnabled.value,
      mirror_role: mirrorEnabled.value ? mirrorRole.value : null,
    })
    if (selectedProjectId.value === updated.project_id) {
      settings.value = updated
      saved.value = true
    }
  } catch (failure) {
    error.value = asApiError(failure).message
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <section
    data-testid="notification-settings"
    class="rounded-lg border border-slate-200 bg-slate-50 p-4"
  >
    <div class="flex items-center justify-between gap-4">
      <h2 class="text-lg font-semibold">
        Notification settings
      </h2>
      <button
        type="button"
        data-testid="notification-refresh"
        :disabled="loading || saving || statusLoading || selectedProjectId === null"
        class="rounded border px-3 py-2 text-sm"
        @click="refreshStatus"
      >
        Refresh permission and receipts
      </button>
    </div>
    <p class="mt-1 text-sm text-slate-600">
      Notifications and mirrors only inform. Delivery never acknowledges an Inbox item.
    </p>
    <label class="mt-4 grid gap-1 text-sm">
      Project
      <select
        v-model.number="selectedProjectId"
        data-testid="notification-project"
        :disabled="saving"
        class="rounded border border-slate-300 px-3 py-2"
      >
        <option :value="null">Choose a Project</option>
        <option
          v-for="project in projects.filter((entry) => !entry.archived)"
          :key="project.id"
          :value="project.id"
        >{{ project.code }} — {{ project.name }}</option>
      </select>
    </label>
    <p
      v-if="loading"
      role="status"
      class="mt-3 text-sm"
    >
      Loading preferences…
    </p>
    <p
      v-if="error"
      role="alert"
      class="mt-3 text-sm text-red-700"
    >
      {{ error }}
    </p>
    <form
      v-if="settings"
      data-testid="notification-settings-form"
      class="mt-4 space-y-4"
      @submit.prevent="save"
    >
      <p
        v-if="permission"
        data-testid="notification-permission"
        class="text-sm"
      >
        macOS permission: {{ permission.state.replaceAll('_', ' ') }}. {{ permission.reason }}
      </p>
      <button
        v-if="permission?.state === 'not_determined'"
        type="button"
        data-testid="notification-request-permission"
        :disabled="requestingPermission || permission.request_pending"
        class="rounded border px-3 py-2 text-sm"
        @click="requestPermission"
      >
        Request macOS permission
      </button>
      <p
        v-if="permissionRequested && permission?.state !== 'granted'"
        class="text-sm text-slate-600"
      >
        Request accepted. Check the macOS permission dialog; this is not a grant.
      </p>
      <p
        v-if="permission?.state === 'denied'"
        class="text-sm text-slate-600"
      >
        Change notification permission in macOS System Settings.
      </p>
      <label class="flex items-center gap-2 text-sm"><input
        v-model="localEnabled"
        data-testid="notification-local"
        type="checkbox"
        :disabled="saving || (permission?.state !== 'granted' && !localEnabled)"
      > Local macOS notifications</label>
      <label class="flex items-center gap-2 text-sm"><input
        v-model="mirrorEnabled"
        data-testid="notification-mirror"
        type="checkbox"
        :disabled="saving"
      > Mirror to Herdr</label>
      <label
        v-if="mirrorEnabled"
        class="grid gap-1 text-sm"
      >
        Existing informational Herdr role
        <input
          v-model="mirrorRole"
          data-testid="notification-role"
          :disabled="saving"
          maxlength="128"
          class="rounded border border-slate-300 px-3 py-2"
          placeholder="Existing role name"
        >
        <span class="text-xs text-slate-600">Uses this Project’s configured session and workspace. Does not create a role or acknowledge attention.</span>
      </label>
      <button
        type="submit"
        :disabled="saving || (mirrorEnabled && !mirrorRole.trim())"
        class="rounded bg-slate-900 px-3 py-2 text-sm text-white disabled:opacity-50"
      >
        Save preferences
      </button>
      <p
        v-if="saved"
        role="status"
        class="text-sm text-emerald-700"
      >
        Preferences saved.
      </p>
    </form>
    <section
      v-if="deliveries.length"
      class="mt-5 space-y-2"
    >
      <h3 class="font-semibold">
        Delivery receipts
      </h3>
      <div
        v-for="delivery in deliveries"
        :key="delivery.id"
        data-testid="notification-delivery"
        class="rounded border bg-white p-3 text-sm"
      >
        Delivery #{{ delivery.id }} · {{ delivery.channel }} · {{ delivery.status }}
        <p
          v-if="delivery.last_error"
          class="text-red-700"
        >
          {{ delivery.last_error }}
        </p>
        <p
          v-if="delivery.status === 'prepared' || delivery.status === 'uncertain'"
          class="mt-1 text-slate-600"
        >
          Submission is not confirmed. Inspect the receiver; do not blindly repeat it.
        </p>
        <button
          v-if="delivery.status === 'failed'"
          type="button"
          data-testid="notification-retry"
          :disabled="retrying || saving"
          class="mt-2 rounded border px-2 py-1"
          @click="retryDelivery(delivery)"
        >
          Retry known unsent failure
        </button>
      </div>
    </section>
  </section>
</template>
