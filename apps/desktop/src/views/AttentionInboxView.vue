<script setup lang="ts">
// The global Attention Inbox composed into the shell (KAN-S11-US4,
// KAN-T140-AC6): every consolidated source class, each carrying the
// typed subject it names and an action that opens exactly that object.
// Opening a source is a read — the shell navigates and nothing is
// acknowledged — and acknowledgement stays the operator's own named
// command (DR-SA-12). Presentation only; every call goes through the
// generated client.
import { computed, inject, onBeforeUnmount, onMounted, ref, shallowRef } from 'vue'
import { watch } from 'vue'
import { KanbanClient } from '@kanban/contracts'
import type { AttentionItemRecord, AttentionState, ProjectRecord } from '@kanban/contracts'
import { asApiError, kanbanTransportKey } from '../core/transport'
import { adoptScope, emptyScope, releaseScope, scopeHolds } from '../core/scope-authority'
import { destinationForAttentionItem } from '../stores/attention-navigation'
import AppButton from '../components/AppButton.vue'
import EmptyState from '../components/EmptyState.vue'
import InlineAlert from '../components/InlineAlert.vue'
import NotificationSettings from '../components/NotificationSettings.vue'
import SectionHeader from '../components/SectionHeader.vue'
import SkeletonBlock from '../components/SkeletonBlock.vue'
import StatusBadge from '../components/StatusBadge.vue'
import type { StatusTone } from '../components/StatusBadge.vue'

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
// The inbox is global, so its one scope never changes; the reads
// issued in it still supersede each other, so only the latest listing
// writes (KAN-T145).
const scope = emptyScope()
let polling: ReturnType<typeof setInterval> | undefined

const labels: Record<AttentionState, string> = {
  blocker: 'Blocker', missing_result: 'Missing result', human_decision: 'Human decision',
  review_request: 'Review request', failed_schedule: 'Missed or failed schedule',
  invalid_approval: 'Invalid approval', disconnected_session: 'Disconnected session', stale_run: 'Stale run',
}

// The tone each class carries: a broken approval or a lost result is
// critical, work merely waiting is caution, and an observation that
// stopped reporting is progress stalled rather than a verdict.
const tones: Record<AttentionState, StatusTone> = {
  blocker: 'caution', missing_result: 'critical', human_decision: 'caution',
  review_request: 'caution', failed_schedule: 'critical', invalid_approval: 'critical',
  disconnected_session: 'progress', stale_run: 'progress',
}

function projectLabel(id: number): string {
  const project = projects.value.find((entry) => entry.id === id)
  return project ? `${project.code} — ${project.name}` : `Project ${id}`
}

// What each item's action opens, resolved once per rendered list.
const destinations = computed(() =>
  Object.fromEntries(items.value.map((item) => [item.id, destinationForAttentionItem(item)])),
)

async function refresh(): Promise<void> {
  if (!transport) return
  const claim = adoptScope(scope, 'attention:global')
  loading.value = true
  error.value = null
  try {
    const client = new KanbanClient(transport)
    const [inbox, register] = await Promise.all([
      client.queryAttentionList({ include_acknowledged: includeAcknowledged.value, include_inactive: includeInactive.value }),
      client.queryProjectList({}),
    ])
    if (!scopeHolds(scope, claim)) return
    items.value = inbox.items
    projects.value = register.projects
  } catch (failure) {
    if (scopeHolds(scope, claim)) error.value = asApiError(failure).message
  } finally {
    if (scopeHolds(scope, claim)) loading.value = false
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
  releaseScope(scope)
  if (polling !== undefined) clearInterval(polling)
})
</script>

<template>
  <main class="animate-rise flex flex-col gap-6 px-6 py-8 lg:px-8">
    <SectionHeader
      eyebrow="Pipeline"
      title="Attention inbox"
      summary="Every class of work waiting on a human, consolidated from its own source. A source fact is not a verdict: opening one only reads it, and only a named operator acknowledges."
    >
      <template #actions>
        <AppButton
          data-testid="attention-refresh"
          size="sm"
          :aria-disabled="loading"
          @click="refresh"
        >
          Refresh
        </AppButton>
        <AppButton
          data-testid="attention-notifications"
          size="sm"
          :aria-expanded="showNotifications"
          @click="showNotifications = !showNotifications"
        >
          Notification preferences
        </AppButton>
      </template>
    </SectionHeader>

    <InlineAlert
      v-if="error"
      data-testid="attention-error"
    >
      {{ error }}
    </InlineAlert>

    <NotificationSettings
      v-if="showNotifications"
      :projects="projects"
    />

    <div class="flex flex-wrap items-end gap-5">
      <label class="flex items-center gap-2 text-sm text-ink-muted">
        <input
          v-model="includeAcknowledged"
          data-testid="attention-show-ack"
          type="checkbox"
        > Show acknowledged
      </label>
      <label class="flex items-center gap-2 text-sm text-ink-muted">
        <input
          v-model="includeInactive"
          data-testid="attention-show-inactive"
          type="checkbox"
        > Show inactive sources
      </label>
      <label class="ml-auto flex min-w-56 flex-col gap-1 text-sm text-ink-muted">
        Operator name for acknowledgement
        <input
          v-model="who"
          data-testid="attention-who"
          autocomplete="name"
          class="rounded-control border border-line bg-surface px-3 py-2 text-ink"
          placeholder="Your name"
        >
      </label>
    </div>

    <div
      v-if="loading && items.length === 0"
      class="flex flex-col gap-2"
      role="status"
      aria-busy="true"
      aria-label="Loading attention"
    >
      <SkeletonBlock class="h-20" />
      <SkeletonBlock class="h-20" />
    </div>

    <EmptyState
      v-if="!loading && !error && items.length === 0"
      data-testid="attention-empty"
      :message="includeAcknowledged || includeInactive ? 'No items match this view.' : 'No current unacknowledged items.'"
      hint="Sources raise items on their own; nothing here is created by hand."
    />

    <section
      v-for="item in items"
      :key="item.id"
      data-testid="attention-item"
      class="flex flex-col gap-3 rounded-panel border border-line bg-surface p-4"
    >
      <div class="flex flex-wrap items-start justify-between gap-3">
        <div class="flex min-w-0 flex-col gap-2">
          <div class="flex flex-wrap items-center gap-2">
            <StatusBadge :tone="tones[item.kind]">
              {{ labels[item.kind] }}
            </StatusBadge>
            <span class="text-xs text-ink-subtle">{{ projectLabel(item.project_id) }}</span>
          </div>
          <h2 class="max-w-2xl text-sm font-semibold whitespace-pre-line text-ink">
            {{ item.summary }}
          </h2>
          <p
            :data-testid="`attention-source-${item.id}`"
            class="font-mono text-xs text-ink-subtle"
          >
            {{ item.subject_kind }} {{ item.subject_id }}
          </p>
        </div>
        <div class="flex shrink-0 items-center gap-2">
          <RouterLink
            v-if="destinations[item.id]?.route"
            :to="destinations[item.id]!.route!"
            data-testid="attention-open"
            :aria-label="`Open ${destinations[item.id]!.label}`"
            class="inline-flex h-8 items-center rounded-control border border-line-strong px-3 text-xs font-medium text-ink transition-colors hover:border-accent/40 hover:bg-accent/8"
          >
            Open {{ destinations[item.id]!.label }}
          </RouterLink>
          <AppButton
            data-testid="attention-ack"
            size="sm"
            :disabled="!who.trim() || acknowledging || loading || item.acknowledged_by != null"
            @click="acknowledge(item)"
          >
            Acknowledge
          </AppButton>
        </div>
      </div>
      <p
        v-if="!destinations[item.id]?.route"
        data-testid="attention-no-surface"
        class="text-xs text-ink-subtle"
      >
        This item names {{ destinations[item.id]?.label }}; its source details stay below.
      </p>
      <p
        v-if="item.acknowledged_by != null"
        data-testid="attention-acknowledged"
        class="text-sm text-accent"
      >
        Acknowledged by {{ item.acknowledged_by }} at {{ item.acknowledged_at }}.
      </p>
      <p
        v-if="!item.active"
        class="text-sm text-ink-subtle"
      >
        Source no longer current. This is not an acknowledgement.
      </p>
      <details class="text-sm">
        <summary class="cursor-pointer text-ink-muted">
          Source details
        </summary>
        <pre class="mt-2 overflow-x-auto rounded-control bg-rail p-3 text-xs whitespace-pre-wrap text-ink-muted">{{ JSON.stringify(item.detail, null, 2) }}</pre>
      </details>
    </section>
  </main>
</template>
