<script setup lang="ts">
import { computed, inject, ref, watch } from 'vue'
import { KanbanClient } from '@kanban/contracts'
import type { SchedulePreviewQuery, SchedulePreviewResponse, TicketRecord } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'

const props = defineProps<{ tickets: TicketRecord[]; projectCode: string }>()
const emit = defineEmits<{ saved: [TicketRecord] }>()
const transport = inject(kanbanTransportKey)
const pickedTicketId = ref<number | null>(null)
const pickedTicket = computed(() => props.tickets.find((t) => t.id === pickedTicketId.value) ?? null)
const mode = ref<'one-time' | 'recurring'>('one-time')
const activation = ref('')
const cron = ref('0 9 * * *')
const timezone = ref('UTC')
const after = ref(new Date().toISOString())
const profile = ref('')
const preview = ref<SchedulePreviewResponse | null>(null)
const error = ref<string | null>(null)
const previewing = ref(false)
const saving = ref(false)
const saved = ref(false)
let previewGeneration = 0
let loadGeneration = 0
const loading = ref(false)
const loaded = ref(false)

watch(pickedTicket, (ticket, previous) => {
  if (ticket?.id === previous?.id && ticket?.version === previous?.version) return
  if (ticket?.id !== previous?.id) saved.value = false
  void loadSchedule(ticket)
}, { flush: 'sync' })

async function loadSchedule(ticket: TicketRecord | null): Promise<void> {
  const generation = ++loadGeneration
  invalidatePreview()
  loaded.value = false
  loading.value = Boolean(ticket && transport)
  error.value = null
  mode.value = 'one-time'
  activation.value = ticket?.scheduled_for ?? ''
  cron.value = '0 9 * * *'
  timezone.value = 'UTC'
  profile.value = ticket?.profile ?? ''
  if (!ticket || !transport) return
  try {
    const result = await new KanbanClient(transport).queryScheduleGet({ ticket_id: ticket.id })
    if (generation !== loadGeneration) return
    if (result.schedule) {
      mode.value = result.schedule.cron === null ? 'one-time' : 'recurring'
      activation.value = result.schedule.activation ?? ''
      cron.value = result.schedule.cron ?? '0 9 * * *'
      timezone.value = result.schedule.timezone
      profile.value = result.schedule.profile
    }
    loaded.value = true
  } catch (failure) {
    if (generation === loadGeneration) error.value = asApiError(failure).message
  } finally {
    if (generation === loadGeneration) loading.value = false
  }
}

function invalidatePreview(): void {
  previewGeneration += 1
  preview.value = null
  previewing.value = false
}

watch([pickedTicketId, mode, activation, cron, timezone, after, profile], () => {
  invalidatePreview()
  if (!loading.value) saved.value = false
  error.value = null
}, { flush: 'sync' })

watch(() => pickedTicket.value?.version, invalidatePreview, { flush: 'sync' })

function previewRequest(): SchedulePreviewQuery {
  return {
    ...(mode.value === 'recurring' ? { cron: cron.value } : { activation: activation.value }),
    timezone: timezone.value,
    after: after.value,
    count: 5,
  }
}

async function showPreview(): Promise<void> {
  if (!transport || !pickedTicket.value || !loaded.value || loading.value || saving.value) return
  const generation = ++previewGeneration
  previewing.value = true
  saved.value = false
  error.value = null
  try {
    const result = await new KanbanClient(transport).querySchedulePreview(previewRequest())
    if (generation === previewGeneration) preview.value = result
  } catch (failure) {
    if (generation === previewGeneration) {
      error.value = asApiError(failure).message
      preview.value = null
    }
  } finally {
    if (generation === previewGeneration) previewing.value = false
  }
}

async function save(): Promise<void> {
  const ticket = pickedTicket.value
  if (!transport || !ticket || !preview.value || !loaded.value || loading.value || previewing.value || saving.value || !profile.value) return
  saving.value = true
  error.value = null
  try {
    const result = await new KanbanClient(transport).commandTicketSchedule({
      mutation: { optimistic_version: ticket.version, idempotency_key: crypto.randomUUID() },
      ticket_id: ticket.id,
      ...(mode.value === 'recurring' ? { cron: cron.value, after: after.value } : { activation: activation.value }),
      timezone: timezone.value,
      profile: profile.value,
    })
    preview.value = null
    saved.value = true
    emit('saved', result)
  } catch (failure) {
    error.value = asApiError(failure).message
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <section
    data-testid="schedule-editor"
    aria-label="Schedule editor"
    class="rounded-lg border border-slate-200 bg-white p-4"
  >
    <h2 class="text-lg font-semibold">
      Schedule a Ticket
    </h2>
    <p class="mt-1 text-sm text-slate-600">
      Preview the core-calculated times and DST rules before saving.
    </p>
    <form
      data-testid="schedule-form"
      class="mt-4 grid gap-4 sm:grid-cols-2"
      @submit.prevent="save"
    >
      <label class="grid gap-1 text-sm">
        Ticket
        <select
          v-model.number="pickedTicketId"
          data-testid="schedule-ticket"
          :disabled="saving"
          class="rounded border border-slate-300 px-3 py-2"
        >
          <option :value="null">Choose a Ticket</option>
          <option
            v-for="ticket in tickets"
            :key="ticket.id"
            :value="ticket.id"
          >
            {{ projectCode }}-T{{ ticket.number }} — {{ ticket.title ?? ticket.slice }}
          </option>
        </select>
      </label>
      <label class="grid gap-1 text-sm">
        Schedule type
        <select
          v-model="mode"
          data-testid="schedule-mode"
          :disabled="!loaded || loading || saving"
          class="rounded border border-slate-300 px-3 py-2"
        >
          <option value="one-time">One-time activation</option>
          <option
            v-if="pickedTicket?.kind === 'task'"
            value="recurring"
          >Recurring Task</option>
        </select>
      </label>
      <label
        v-if="mode === 'recurring'"
        class="grid gap-1 text-sm"
      >
        Five-field cron
        <input
          v-model="cron"
          data-testid="schedule-cron"
          :disabled="!loaded || loading || saving"
          class="rounded border border-slate-300 px-3 py-2 font-mono"
          placeholder="0 9 * * *"
        >
      </label>
      <label
        v-else
        class="grid gap-1 text-sm"
      >
        Activation (RFC 3339, including UTC offset)
        <input
          v-model="activation"
          data-testid="schedule-activation"
          :disabled="!loaded || loading || saving"
          class="rounded border border-slate-300 px-3 py-2 font-mono"
          placeholder="2026-10-25T01:30:00+00:00"
        >
      </label>
      <label class="grid gap-1 text-sm">
        IANA timezone
        <input
          v-model="timezone"
          data-testid="schedule-timezone"
          :disabled="!loaded || loading || saving"
          class="rounded border border-slate-300 px-3 py-2 font-mono"
          placeholder="Europe/London"
        >
      </label>
      <label
        v-if="mode === 'recurring'"
        class="grid gap-1 text-sm"
      >
        Preview after (RFC 3339)
        <input
          v-model="after"
          data-testid="schedule-after"
          :disabled="!loaded || loading || saving"
          class="rounded border border-slate-300 px-3 py-2 font-mono"
        >
      </label>
      <label class="grid gap-1 text-sm">
        Eligible Execution Profile
        <input
          v-model="profile"
          data-testid="schedule-profile"
          :disabled="!loaded || loading || saving"
          class="rounded border border-slate-300 px-3 py-2"
          placeholder="Profile name"
        >
      </label>
      <div class="flex gap-3 sm:col-span-2">
        <button
          type="button"
          data-testid="schedule-preview"
          :disabled="previewing || !pickedTicket || !loaded || loading || saving"
          class="rounded border px-3 py-2 text-sm disabled:opacity-50"
          @click="showPreview"
        >
          {{ previewing ? 'Calculating…' : 'Preview activations' }}
        </button>
        <button
          type="submit"
          data-testid="schedule-save"
          :disabled="!preview || !pickedTicket || !loaded || loading || previewing || saving || !profile"
          class="rounded bg-slate-900 px-3 py-2 text-sm text-white disabled:opacity-50"
        >
          Save schedule
        </button>
      </div>
    </form>
    <p
      v-if="loading"
      role="status"
      class="mt-3 text-sm text-slate-600"
    >
      Loading standing schedule…
    </p>
    <button
      v-if="error && !loaded && pickedTicket"
      type="button"
      class="mt-3 rounded border px-3 py-2 text-sm"
      @click="loadSchedule(pickedTicket)"
    >
      Retry loading
    </button>
    <p
      v-if="saved"
      role="status"
      class="mt-3 text-sm text-emerald-700"
    >
      Schedule saved.
    </p>
    <p
      v-if="error"
      role="alert"
      class="mt-3 text-sm text-red-700"
    >
      {{ error }}
    </p>
    <div
      v-if="preview"
      data-testid="schedule-preview-results"
      class="mt-4 space-y-2"
    >
      <h3 class="font-semibold">
        Calculated activations
      </h3>
      <p class="text-sm">
        {{ preview.dst_behaviour.spring_forward }}
      </p>
      <p class="text-sm">
        {{ preview.dst_behaviour.fall_back }}
      </p>
      <ol class="list-decimal space-y-1 pl-5 text-sm">
        <li
          v-for="instant in preview.activations"
          :key="instant.utc"
        >
          <span class="font-mono">{{ instant.local }}</span>
          <span class="text-slate-600"> — UTC </span><time
            :datetime="instant.utc"
            class="font-mono"
          >{{ instant.utc }}</time>
        </li>
      </ol>
    </div>
  </section>
</template>
