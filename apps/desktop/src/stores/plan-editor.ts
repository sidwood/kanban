// The Project list and selected Plan have separate authority: list refreshes do
// not cancel commands, while Project or Plan navigation does.
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { MutationContext, PlanRecord, PlanVersionRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'
import {
  adoptScope,
  emptyScope,
  issueCommand,
  projectScopeKey,
  releaseScope,
  scopeHolds,
} from '../core/scope-authority'
import type { ScopeClaim } from '../core/scope-authority'

// A fresh idempotency key per logical request, so a retried transport
// cannot apply one intent twice.
function mutationFor(optimisticVersion: number): MutationContext {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

export interface PlanGraph {
  spec_numbers: number[]
  edges: { from_spec: number; to_spec: number }[]
}

function onActiveSurface(state: PlanRecord['state']): boolean {
  return state === 'draft' || state === 'active'
}

export const usePlanEditorStore = defineStore('plan-editor', {
  state: () => ({
    ...emptyScope(),
    planScope: emptyScope(),
    projectId: null as number | null,
    plans: [] as PlanRecord[],
    versions: [] as PlanVersionRecord[],
    selectedPlanId: null as number | null,
    selectedVersion: null as number | null,
    loaded: false,
    error: null as string | null,
  }),
  getters: {
    activeSurface(state): PlanRecord[] {
      return state.plans.filter((plan) => onActiveSurface(plan.state))
    },
    finished(state): PlanRecord[] {
      return state.plans.filter((plan) => !onActiveSurface(plan.state))
    },
    selectedPlan(state): PlanRecord | null {
      return state.plans.find((plan) => plan.id === state.selectedPlanId) ?? null
    },
    openPlan(state): PlanRecord | null {
      const plan = this.selectedPlan
      if (!plan) return null
      return state.projectId === null || plan.project_id === state.projectId ? plan : null
    },
    displayed(state): PlanGraph | null {
      if (state.selectedVersion !== null) {
        const frozen = state.versions.find((version) => version.number === state.selectedVersion)
        return frozen
          ? { spec_numbers: [...frozen.spec_numbers], edges: [...frozen.edges] }
          : null
      }
      const plan = this.selectedPlan
      return plan ? { spec_numbers: [...plan.spec_numbers], edges: [...plan.edges] } : null
    },
  },
  actions: {
    async refresh(transport: ShellTransport, projectId: number): Promise<void> {
      const claim = adoptScope(this, projectScopeKey(projectId))
      this.projectId = projectId
      try {
        const response = await new KanbanClient(transport).queryPlanList({ project_id: projectId })
        if (!scopeHolds(this, claim)) return
        this.plans = response.plans
        this.loaded = true
        this.error = null
      } catch (failure) {
        if (!scopeHolds(this, claim)) return
        this.error = asApiError(failure).message
      }
    },
    clear(): void {
      releaseScope(this)
      releaseScope(this.planScope)
      this.projectId = null
      this.plans = []
      this.versions = []
      this.selectedPlanId = null
      this.selectedVersion = null
      this.loaded = false
      this.error = null
    },
    forgetSelection(): void {
      releaseScope(this.planScope)
      this.versions = []
      this.selectedPlanId = null
      this.selectedVersion = null
    },
    async open(transport: ShellTransport, planId: number): Promise<void> {
      const projectClaim = issueCommand(this)
      const claim = adoptScope(this.planScope, `plan:${planId}`)
      this.selectedPlanId = planId
      this.selectedVersion = null
      try {
        const response = await new KanbanClient(transport).queryPlanGet({ plan_id: planId })
        if (!scopeHolds(this, projectClaim) || !scopeHolds(this.planScope, claim)) return
        if (this.projectId === null) {
          adoptScope(this, projectScopeKey(response.plan.project_id))
          this.projectId = response.plan.project_id
        } else if (this.projectId !== response.plan.project_id) {
          return
        }
        this.versions = response.versions
        const known = this.plans.findIndex((plan) => plan.id === response.plan.id)
        if (known === -1) {
          this.plans = [...this.plans, response.plan]
        } else {
          this.plans = this.plans.map((plan) =>
            plan.id === response.plan.id ? response.plan : plan,
          )
        }
        this.error = null
      } catch (failure) {
        if (!scopeHolds(this, projectClaim) || !scopeHolds(this.planScope, claim)) return
        this.error = asApiError(failure).message
      }
    },
    select(planId: number): void {
      adoptScope(this.planScope, `plan:${planId}`)
      this.selectedPlanId = planId
      this.selectedVersion = null
    },
    showVersion(number: number): void {
      this.selectedVersion = number
    },
    showDraft(): void {
      this.selectedVersion = null
    },
    async create(transport: ShellTransport, projectId: number): Promise<void> {
      if (this.projectId !== null && this.projectId !== projectId) {
        this.error = `Project ${projectId} is not the Project on display.`
        return
      }
      if (this.projectId === null) {
        adoptScope(this, projectScopeKey(projectId))
        this.projectId = projectId
      }
      const projectClaim = issueCommand(this)
      const selectionClaim = issueCommand(this.planScope)
      let created: PlanRecord
      try {
        created = await new KanbanClient(transport).commandPlanCreate({
          mutation: mutationFor(0),
          project_id: projectId,
        })
      } catch (failure) {
        if (!scopeHolds(this, projectClaim)) return
        this.error = asApiError(failure).message
        return
      }
      if (!scopeHolds(this, projectClaim)) return
      this.error = null
      await this.refresh(transport, projectId)
      if (!scopeHolds(this, projectClaim) || !scopeHolds(this.planScope, selectionClaim)) return
      await this.open(transport, created.id)
    },
    async addSpec(transport: ShellTransport, specNumber: number): Promise<void> {
      await this.mutate(transport, (client, plan) =>
        client.commandPlanSpecAdd({
          mutation: mutationFor(plan.version),
          plan_id: plan.id,
          spec_number: specNumber,
        }),
      )
    },
    async removeSpec(transport: ShellTransport, specNumber: number): Promise<void> {
      await this.mutate(transport, (client, plan) =>
        client.commandPlanSpecRemove({
          mutation: mutationFor(plan.version),
          plan_id: plan.id,
          spec_number: specNumber,
        }),
      )
    },
    // The edges stay put: order and dependency are separate facts.
    async moveSpec(
      transport: ShellTransport,
      specNumber: number,
      position: number,
    ): Promise<void> {
      await this.mutate(transport, (client, plan) =>
        client.commandPlanSpecMove({
          mutation: mutationFor(plan.version),
          plan_id: plan.id,
          spec_number: specNumber,
          position,
        }),
      )
    },
    async addEdge(transport: ShellTransport, fromSpec: number, toSpec: number): Promise<void> {
      await this.mutate(transport, (client, plan) =>
        client.commandPlanEdgeAdd({
          mutation: mutationFor(plan.version),
          plan_id: plan.id,
          from_spec: fromSpec,
          to_spec: toSpec,
        }),
      )
    },
    async removeEdge(transport: ShellTransport, fromSpec: number, toSpec: number): Promise<void> {
      await this.mutate(transport, (client, plan) =>
        client.commandPlanEdgeRemove({
          mutation: mutationFor(plan.version),
          plan_id: plan.id,
          from_spec: fromSpec,
          to_spec: toSpec,
        }),
      )
    },
    async activate(transport: ShellTransport): Promise<void> {
      await this.mutate(transport, (client, plan) =>
        client.commandPlanActivate({
          mutation: mutationFor(plan.version),
          plan_id: plan.id,
        }),
      )
    },
    async replan(transport: ShellTransport): Promise<void> {
      await this.mutate(transport, (client, plan) =>
        client.commandPlanReplan({
          mutation: mutationFor(plan.version),
          plan_id: plan.id,
        }),
      )
    },
    async complete(transport: ShellTransport): Promise<void> {
      await this.mutate(transport, (client, plan) =>
        client.commandPlanComplete({
          mutation: mutationFor(plan.version),
          plan_id: plan.id,
        }),
      )
    },
    async cancel(transport: ShellTransport): Promise<void> {
      await this.mutate(transport, (client, plan) =>
        client.commandPlanCancel({
          mutation: mutationFor(plan.version),
          plan_id: plan.id,
        }),
      )
    },
    async archive(transport: ShellTransport): Promise<void> {
      await this.mutate(transport, (client, plan) =>
        client.commandPlanArchive({
          mutation: mutationFor(plan.version),
          plan_id: plan.id,
        }),
      )
    },
    async mutate(
      transport: ShellTransport,
      command: (client: KanbanClient, plan: PlanRecord) => Promise<PlanRecord>,
    ): Promise<void> {
      const plan = this.openPlan
      if (!plan) {
        this.error = 'The Project on display holds no open Plan for this change.'
        return
      }
      adoptScope(this.planScope, `plan:${plan.id}`)
      const planClaim = issueCommand(this.planScope)
      await this.submit(transport, planClaim, (client) => command(client, plan))
    },
    async submit(
      transport: ShellTransport,
      planClaim: ScopeClaim,
      command: (client: KanbanClient) => Promise<PlanRecord>,
    ): Promise<void> {
      const projectClaim = issueCommand(this)
      let landed: PlanRecord
      try {
        landed = await command(new KanbanClient(transport))
      } catch (failure) {
        if (!scopeHolds(this, projectClaim) || !scopeHolds(this.planScope, planClaim)) return
        this.error = asApiError(failure).message
        return
      }
      if (!scopeHolds(this, projectClaim) || !scopeHolds(this.planScope, planClaim)) return
      this.error = null
      await this.refresh(transport, this.projectId ?? landed.project_id)
      if (!scopeHolds(this, projectClaim) || !scopeHolds(this.planScope, planClaim)) return
      await this.open(transport, landed.id)
    },
  },
})
