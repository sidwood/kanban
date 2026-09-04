<script setup lang="ts">
// The Initiative management surface: list, create, rename, and
// archive through the initiatives store. Presentation only; every
// domain call goes through the generated client in the store, and
// no delete control exists (KAN-S1-US6).
import { inject, onMounted, reactive, ref } from 'vue'
import { kanbanTransportKey } from '../core/transport'
import { useInitiativesStore } from '../stores/initiatives'

const transport = inject(kanbanTransportKey)
const initiatives = useInitiativesStore()
const newName = ref('')
const renameDrafts = reactive<Record<number, string>>({})

onMounted(() => {
  if (transport) {
    void initiatives.refresh(transport)
  }
})

async function submitCreate() {
  if (!transport || !newName.value.trim()) {
    return
  }
  await initiatives.create(transport, newName.value)
  newName.value = ''
}

async function submitRename(id: number) {
  if (!transport || !renameDrafts[id]?.trim()) {
    return
  }
  await initiatives.rename(transport, id, renameDrafts[id])
  renameDrafts[id] = ''
}

async function submitArchive(id: number) {
  if (transport) {
    await initiatives.archive(transport, id)
  }
}
</script>

<template>
  <main class="mx-auto flex min-h-screen max-w-2xl flex-col gap-6 p-8">
    <nav class="text-sm text-slate-500">
      <RouterLink
        to="/"
        class="hover:text-slate-900"
      >
        Kanban
      </RouterLink>
      <span aria-hidden="true"> / </span>
      <span class="text-slate-900">Initiatives</span>
    </nav>

    <h1 class="text-3xl font-semibold tracking-tight">
      Initiatives
    </h1>

    <p
      v-if="initiatives.error"
      data-testid="initiative-error"
      role="alert"
      class="rounded border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700"
    >
      {{ initiatives.error }}
    </p>

    <ul
      v-if="initiatives.loaded"
      data-testid="initiative-list"
      class="flex flex-col divide-y divide-slate-200 rounded border border-slate-200"
    >
      <li
        v-for="initiative in initiatives.initiatives"
        :key="initiative.id"
        :data-testid="`initiative-row-${initiative.id}`"
        class="flex flex-wrap items-center gap-3 px-4 py-3"
      >
        <span
          :data-testid="`initiative-name-${initiative.id}`"
          class="font-medium"
        >
          {{ initiative.name }}
        </span>
        <span
          v-if="initiative.archived"
          :data-testid="`initiative-archived-${initiative.id}`"
          class="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-600"
        >
          Archived
        </span>
        <template v-else>
          <input
            v-model="renameDrafts[initiative.id]"
            :data-testid="`initiative-rename-${initiative.id}`"
            :aria-label="`New name for ${initiative.name}`"
            placeholder="New name"
            class="rounded border border-slate-300 px-2 py-1 text-sm"
          >
          <button
            :data-testid="`initiative-rename-submit-${initiative.id}`"
            class="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-50"
            @click="submitRename(initiative.id)"
          >
            Rename
          </button>
          <button
            :data-testid="`initiative-archive-${initiative.id}`"
            class="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-50"
            @click="submitArchive(initiative.id)"
          >
            Archive
          </button>
        </template>
      </li>
    </ul>
    <p
      v-else-if="!initiatives.error"
      data-testid="initiative-loading"
    >
      Loading Initiatives…
    </p>

    <form
      class="flex items-center gap-3"
      @submit.prevent="submitCreate"
    >
      <input
        v-model="newName"
        data-testid="initiative-new-name"
        aria-label="New Initiative name"
        placeholder="New Initiative name"
        class="rounded border border-slate-300 px-3 py-2 text-sm"
      >
      <button
        type="submit"
        data-testid="initiative-create"
        class="rounded bg-slate-900 px-3 py-2 text-sm font-medium text-white hover:bg-slate-700"
      >
        Create
      </button>
    </form>
  </main>
</template>
