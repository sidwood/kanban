<script setup lang="ts">
// Embedded rulings and deferrals with supersession provenance visible.
import { computed, inject, onMounted, watch } from 'vue'
import type { TimelineEntityKind } from '@kanban/contracts'
import { kanbanTransportKey } from '../core/transport'
import { useRulingsStore } from '../stores/rulings'

const props = defineProps<{
  projectId: string
  entityKind?: TimelineEntityKind | null
  entityId?: string
}>()

const transport = inject(kanbanTransportKey)
const rulings = useRulingsStore()

const entityRef = computed(() => {
  if (!props.entityKind || !props.entityId) {
    return null
  }
  return { kind: props.entityKind, id: props.entityId }
})

async function reload(): Promise<void> {
  if (transport) {
    await rulings.load(transport, props.projectId, entityRef.value)
  }
}

onMounted(() => {
  void reload()
})

watch(
  () => [props.projectId, props.entityKind, props.entityId],
  () => {
    void reload()
  },
)
</script>

<template>
  <section
    class="flex w-full max-w-2xl flex-col gap-4 rounded-lg border border-slate-200 bg-white p-4 shadow-sm"
    data-testid="rulings-surface"
  >
    <header class="flex flex-col gap-1">
      <h2 class="text-lg font-semibold text-slate-900">
        Rulings and deferrals
      </h2>
      <p class="text-sm text-slate-500">
        Project {{ projectId }}
      </p>
    </header>

    <p
      v-if="rulings.loading"
      data-testid="rulings-loading"
      class="text-sm text-slate-500"
    >
      Loading rulings…
    </p>
    <p
      v-else-if="rulings.error"
      data-testid="rulings-error"
      class="text-sm text-red-600"
    >
      {{ rulings.error }}
    </p>
    <div
      v-else
      class="grid gap-4 md:grid-cols-2"
    >
      <section>
        <h3 class="mb-2 text-sm font-medium text-slate-700">
          Rulings
        </h3>
        <ul
          data-testid="rulings-list"
          class="flex flex-col gap-2"
        >
          <li
            v-if="rulings.rulings.length === 0"
            class="text-sm text-slate-500"
          >
            No rulings recorded.
          </li>
          <li
            v-for="ruling in rulings.rulings"
            :key="ruling.id"
            class="rounded border border-slate-100 bg-slate-50 px-3 py-2 text-sm"
            :data-testid="`ruling-${ruling.id}`"
          >
            <p class="font-medium text-slate-900">
              {{ ruling.summary }}
            </p>
            <p
              v-if="ruling.supersedes_id"
              class="text-xs text-slate-600"
              :data-testid="`ruling-supersedes-${ruling.id}`"
            >
              Supersedes ruling {{ ruling.supersedes_id }}
            </p>
          </li>
        </ul>
      </section>

      <section>
        <h3 class="mb-2 text-sm font-medium text-slate-700">
          Deferrals
        </h3>
        <ul
          data-testid="deferrals-list"
          class="flex flex-col gap-2"
        >
          <li
            v-if="rulings.deferrals.length === 0"
            class="text-sm text-slate-500"
          >
            No deferrals recorded.
          </li>
          <li
            v-for="deferral in rulings.deferrals"
            :key="deferral.id"
            class="rounded border border-slate-100 bg-slate-50 px-3 py-2 text-sm"
            :data-testid="`deferral-${deferral.id}`"
          >
            <p class="font-medium text-slate-900">
              {{ deferral.reason }}
            </p>
            <p class="text-xs text-slate-600">
              Finding {{ deferral.finding_id }}
            </p>
            <p
              v-if="deferral.supersedes_id"
              class="text-xs text-slate-600"
              :data-testid="`deferral-supersedes-${deferral.id}`"
            >
              Supersedes deferral {{ deferral.supersedes_id }}
            </p>
          </li>
        </ul>
      </section>
    </div>
  </section>
</template>
