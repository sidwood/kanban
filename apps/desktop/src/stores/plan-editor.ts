// The planning editor state, driven entirely through the generated
// client: the Plans of one Project, the graph edits (membership,
// display order, and dependency edges as separate operations), the
// lifecycle moves, and the version switcher that keeps prior frozen
// versions visible. Terminal states stay listed but sit off the
// active surface (KAN-S3-US1, KAN-S3-US2, KAN-S3-US3).
import { defineStore } from 'pinia'
import { KanbanClient } from '@kanban/contracts'
import type { MutationContext, PlanRecord, PlanVersionRecord } from '@kanban/contracts'
import { asApiError } from '../core/transport'
import type { ShellTransport } from '../core/transport'

// One mutation's context: a fresh idempotency key per logical
// request, and the optimistic version the caller believes the
// aggregate is at.
function mutationFor(optimisticVersion: number): MutationContext {
  return { optimistic_version: optimisticVersion, idempotency_key: crypto.randomUUID() }
}

// The graph one plan or one frozen version carries.
export interface PlanGraph {
  spec_numbers: number[]
  edges: { from_spec: number; to_spec: number }[]
}

// Whether a state keeps a Plan on the active surface: draft and
// active are the working states; complete, cancelled, and archived
// are terminal.
function onActiveSurface(state: PlanRecord['state']): boolean {
  return state === 'draft' || state === 'active'
}

export const usePlanEditorStore = defineStore('plan-editor', {
  state: () => ({
    plans: [] as PlanRecord[],
    versions: [] as PlanVersionRecord[],
    selectedPlanId: null as number | null,
    selectedVersion: null as number | null,
    loaded: false,
    error: null as string | null,
  }),
  getters: {
    // The Plans still being worked: the active surface.
    activeSurface(state): PlanRecord[] {
      return state.plans.filter((plan) => onActiveSurface(plan.state))
    },
    // The terminal Plans, queryable but off the active surface.
    finished(state): PlanRecord[] {
      return state.plans.filter((plan) => !onActiveSurface(plan.state))
    },
    // The Plan the editor has open, if any.
    selectedPlan(state): PlanRecord | null {
      return state.plans.find((plan) => plan.id === state.selectedPlanId) ?? null
    },
    // The graph on display: the selected frozen version's shape, or
    // the working shape when no version is selected.
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
    // Load every Plan of one Project, terminal states included.
    async refresh(transport: ShellTransport, projectId: number): Promise<void> {
      try {
        const response = await new KanbanClient(transport).queryPlanList({ project_id: projectId })
        this.plans = response.plans
        this.loaded = true
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
      }
    },
    // Open one Plan: select it and load its record and frozen
    // versions, showing the working shape first.
    async open(transport: ShellTransport, planId: number): Promise<void> {
      this.selectedPlanId = planId
      this.selectedVersion = null
      try {
        const response = await new KanbanClient(transport).queryPlanGet({ plan_id: planId })
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
        this.error = asApiError(failure).message
      }
    },
    // Select a Plan from the list without loading its versions.
    select(planId: number): void {
      this.selectedPlanId = planId
      this.selectedVersion = null
    },
    // Show one frozen version's shape.
    showVersion(number: number): void {
      this.selectedVersion = number
    },
    // Show the working shape.
    showDraft(): void {
      this.selectedVersion = null
    },
    // Create a draft Plan under the Project; a fresh aggregate is
    // expected at version 0.
    async create(transport: ShellTransport, projectId: number): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandPlanCreate({ mutation: mutationFor(0), project_id: projectId }),
      )
    },
    // Add a Spec to the membership, appending it to the display
    // order.
    async addSpec(transport: ShellTransport, specNumber: number): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandPlanSpecAdd({
          mutation: mutationFor(this.versionOfSelected()),
          plan_id: this.selectedPlanId ?? 0,
          spec_number: specNumber,
        }),
      )
    },
    // Remove a Spec from the membership and the display order.
    async removeSpec(transport: ShellTransport, specNumber: number): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandPlanSpecRemove({
          mutation: mutationFor(this.versionOfSelected()),
          plan_id: this.selectedPlanId ?? 0,
          spec_number: specNumber,
        }),
      )
    },
    // Move a Spec within the display order; the edges stay put.
    async moveSpec(
      transport: ShellTransport,
      specNumber: number,
      position: number,
    ): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandPlanSpecMove({
          mutation: mutationFor(this.versionOfSelected()),
          plan_id: this.selectedPlanId ?? 0,
          spec_number: specNumber,
          position,
        }),
      )
    },
    // Add a dependency edge inside the Plan.
    async addEdge(transport: ShellTransport, fromSpec: number, toSpec: number): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandPlanEdgeAdd({
          mutation: mutationFor(this.versionOfSelected()),
          plan_id: this.selectedPlanId ?? 0,
          from_spec: fromSpec,
          to_spec: toSpec,
        }),
      )
    },
    // Remove a dependency edge.
    async removeEdge(transport: ShellTransport, fromSpec: number, toSpec: number): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandPlanEdgeRemove({
          mutation: mutationFor(this.versionOfSelected()),
          plan_id: this.selectedPlanId ?? 0,
          from_spec: fromSpec,
          to_spec: toSpec,
        }),
      )
    },
    // Freeze the shape into an immutable version.
    async activate(transport: ShellTransport): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandPlanActivate({
          mutation: mutationFor(this.versionOfSelected()),
          plan_id: this.selectedPlanId ?? 0,
        }),
      )
    },
    // Reopen the draft and reserve the replacement version.
    async replan(transport: ShellTransport): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandPlanReplan({
          mutation: mutationFor(this.versionOfSelected()),
          plan_id: this.selectedPlanId ?? 0,
        }),
      )
    },
    // Complete the Plan.
    async complete(transport: ShellTransport): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandPlanComplete({
          mutation: mutationFor(this.versionOfSelected()),
          plan_id: this.selectedPlanId ?? 0,
        }),
      )
    },
    // Cancel the Plan.
    async cancel(transport: ShellTransport): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandPlanCancel({
          mutation: mutationFor(this.versionOfSelected()),
          plan_id: this.selectedPlanId ?? 0,
        }),
      )
    },
    // Archive the Plan; archiving is terminal.
    async archive(transport: ShellTransport): Promise<void> {
      await this.mutate(transport, (client) =>
        client.commandPlanArchive({
          mutation: mutationFor(this.versionOfSelected()),
          plan_id: this.selectedPlanId ?? 0,
        }),
      )
    },
    // Run one command; a refusal is reported, a success refreshes the
    // plans and reopens the selection so new frozen versions show.
    async mutate(
      transport: ShellTransport,
      command: (client: KanbanClient) => Promise<PlanRecord>,
    ): Promise<void> {
      const plan = this.selectedPlan
      try {
        await command(new KanbanClient(transport))
        this.error = null
      } catch (failure) {
        this.error = asApiError(failure).message
        return
      }
      if (plan) {
        await this.refresh(transport, plan.project_id)
        await this.open(transport, plan.id)
      }
    },
    // The stored version of the selected Plan, or a reported error
    // when no Plan is open.
    versionOfSelected(): number {
      const plan = this.selectedPlan
      if (!plan) {
        throw new Error('no plan is selected')
      }
      return plan.version
    },
  },
})
