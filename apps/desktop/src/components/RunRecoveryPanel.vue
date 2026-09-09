<script setup lang="ts">
import { inject, onBeforeUnmount, ref, watch } from 'vue'
import { KanbanClient } from '@kanban/contracts'
import type { RunRecord, RunRecoveryAction, RunRecoveryListResponse, RunRecoveryRuleRequest } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'
import { useConnectionStore } from '../stores/connection'

const props = defineProps<{ runId: number; runStatus: RunRecord['status']; runVersion: number }>()
const emit = defineEmits<{ recovered: [runId: number] }>()
const transport = inject(kanbanTransportKey)
const connection = useConnectionStore()
const history = ref<RunRecoveryListResponse | null>(null)
const summary = ref('')
const error = ref<string | null>(null)
const refreshError = ref<string | null>(null)
const saving = ref(false)
let generation = 0
let refreshGeneration = 0
let refreshTimer: ReturnType<typeof setTimeout> | undefined
onBeforeUnmount(() => {
  generation += 1
  clearTimeout(refreshTimer)
})
let pending: { action: RunRecoveryAction; request: RunRecoveryRuleRequest } | null = null

watch(() => props.runId, () => {
  generation += 1
  history.value = null
  summary.value = ''
  error.value = null
  pending = null
  saving.value = false
  void refresh(generation)
}, { immediate: true })
watch(
  [() => props.runStatus, () => props.runVersion, () => connection.phase],
  () => { void refresh(generation) },
)
watch(summary, () => { if (!saving.value) pending = null })

async function refresh(expectedGeneration: number): Promise<void> {
  if (!transport || expectedGeneration !== generation) return
  clearTimeout(refreshTimer)
  const currentRefresh = ++refreshGeneration
  history.value = null
  refreshError.value = null
  try {
    const result = await new KanbanClient(transport).queryRunRecoveryList({ run_id: props.runId })
    if (expectedGeneration !== generation || currentRefresh !== refreshGeneration) return
    history.value = result
    if (result.pending_resume) refreshTimer = setTimeout(() => { void refresh(expectedGeneration) }, 5000)
  } catch (failure) {
    if (expectedGeneration === generation && currentRefresh === refreshGeneration) refreshError.value = asApiError(failure).message
  }
}

async function recover(action: RunRecoveryAction): Promise<void> {
  if (!transport || !history.value || saving.value || !summary.value.trim()) return
  if (action === 'resume' && !history.value.can_resume) return
  if (action === 'retry' && !history.value.can_retry) return
  const currentGeneration = generation
  if (!pending || pending.action !== action) pending = {
    action, request: {
      run_id: props.runId, summary: summary.value,
      mutation: { optimistic_version: history.value.version, idempotency_key: crypto.randomUUID() },
    },
  }
  saving.value = true
  error.value = null
  try {
    const client = new KanbanClient(transport)
    if (action === 'resume') await client.commandRunRecoveryResume(pending.request)
    else if (action === 'retry') await client.commandRunRecoveryRetry(pending.request)
    else await client.commandRunRecoveryRule(pending.request)
    if (currentGeneration !== generation) return
    pending = null
    summary.value = ''
    await refresh(currentGeneration)
    if (currentGeneration === generation) emit('recovered', props.runId)
  } catch (failure) {
    if (currentGeneration !== generation) return
    const apiError = asApiError(failure)
    error.value = apiError.message
    if (apiError.code === 'stale_version') {
      pending = null
      await refresh(currentGeneration)
    }
  } finally {
    if (currentGeneration === generation) saving.value = false
  }
}
</script>

<template>
  <div class="mt-3 border-t border-line pt-3">
    <p class="text-xs text-ink-muted">
      An exit, disconnect, deadline, or missing result is not a verdict.
      An operator ruling records your decision without changing the Ticket state.
      Resume requests the same run. Retry revokes its authority and queues a new attempt.
    </p>
    <form
      :data-testid="`run-recovery-form-${runId}`"
      class="mt-2 flex flex-col gap-2"
      @submit.prevent="recover('operator_ruling')"
    >
      <label
        :for="`run-recovery-summary-${runId}`"
        class="text-xs font-medium text-ink"
      >
        Recovery reason
      </label>
      <textarea
        :id="`run-recovery-summary-${runId}`"
        v-model="summary"
        :data-testid="`run-recovery-summary-${runId}`"
        :disabled="saving || !history"
        class="rounded-control border border-line bg-surface p-2 text-sm text-ink"
        required
      />
      <button
        type="submit"
        :disabled="saving || !history || !summary.trim()"
        class="self-start rounded-control border border-line px-3 py-1.5 text-xs font-medium disabled:opacity-50"
      >
        {{ saving ? 'Recording…' : 'Record operator ruling' }}
      </button>
      <div class="flex flex-wrap gap-2">
        <button
          type="button"
          :data-testid="`run-recovery-resume-${runId}`"
          :disabled="saving || !history?.can_resume || !summary.trim()"
          class="rounded-control border border-line px-3 py-1.5 text-xs font-medium disabled:opacity-50"
          @click="recover('resume')"
        >
          Request resume
        </button>
        <button
          type="button"
          :data-testid="`run-recovery-retry-${runId}`"
          :disabled="saving || !history?.can_retry || !summary.trim()"
          class="rounded-control border border-line px-3 py-1.5 text-xs font-medium disabled:opacity-50"
          @click="recover('retry')"
        >
          Retry in new run
        </button>
      </div>
    </form>
    <p
      v-if="history?.pending_resume"
      role="status"
      class="mt-2 text-sm text-ink-muted"
    >
      Resume requested; waiting for delivery. This is not an execution acknowledgement.
    </p>
    <p
      v-if="refreshError || error"
      role="alert"
      class="mt-2 text-sm text-red-700"
    >
      {{ refreshError || error }}
    </p>
    <ol
      :data-testid="`run-recovery-history-${runId}`"
      aria-label="Recovery history"
      class="mt-2 flex flex-col gap-2 text-xs text-ink-muted"
    >
      <li
        v-for="item in history?.records ?? []"
        :key="item.id"
      >
        {{ item.action === 'resume' ? 'Resume requested.' : item.action === 'retry' ? 'New attempt requested.' : 'Operator ruling.' }}
        Ruling #{{ item.ruling_id }}: {{ item.summary }}
        <span v-if="item.replacement_dispatch_request_id">Replacement Dispatch Request {{ item.replacement_dispatch_request_id }}.</span>
      </li>
    </ol>
  </div>
</template>
