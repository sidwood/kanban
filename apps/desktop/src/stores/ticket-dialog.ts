// Where the Ticket editor is open, and on what. The editor has no
// destination of its own: it is a dialog the operator reaches from
// New Ticket, from a Spec's uncovered Story, and from the drawer's
// Edit, and quick Bug capture is its own small dialog off a global
// shortcut (KAN-S4-US1, KAN-S4-US3). This store holds only where each
// dialog is pointed; every rule about what a kind may carry stays in
// the core, and every save goes through a production command.
import { defineStore } from 'pinia'
import type { TicketKind } from '@kanban/contracts'

/** Whether the dialog is minting a Ticket or revising one. */
export type TicketEditorMode = 'create' | 'edit'

/** What one entry point points the editor at. */
export interface TicketEditorRequest {
  mode: TicketEditorMode
  /** The Project the Ticket belongs to; null lets the dialog ask. */
  projectId: number | null
  /** The kind the entry point presets. */
  kind: TicketKind
  /** Whether the entry point fixes the kind: an uncovered Story and
   * a standing Ticket both do, New Ticket does not. */
  kindLocked: boolean
  /** The Spec an Implementation attaches to, when the entry point
   * names one. */
  specId: number | null
  /** The User Story an uncovered-Story entry point starts the first
   * criterion from. */
  story: string | null
  /** The Ticket being revised, in edit mode. */
  ticketId: number | null
}

export const useTicketDialogStore = defineStore('ticket-dialog', {
  state: () => ({
    /** Where the editor is pointed, or null while it is shut. */
    editor: null as TicketEditorRequest | null,
    /** The Project a quick capture belongs to while the small dialog
     * is open; null asks the operator which. */
    quickBugProjectId: null as number | null,
    quickBugOpen: false,
  }),
  getters: {
    editorOpen: (state): boolean => state.editor !== null,
  },
  actions: {
    /** New Ticket: the kind is the operator's to pick. */
    openCreate(request: { projectId: number | null; kind?: TicketKind }): void {
      this.editor = {
        mode: 'create',
        projectId: request.projectId,
        kind: request.kind ?? 'implementation',
        kindLocked: false,
        specId: null,
        story: null,
        ticketId: null,
      }
    },
    /** A Spec's uncovered Story: only an Implementation covers one,
     * and it covers it on that Spec (DR-TK-02, DR-TK-04). */
    openForStory(request: { projectId: number; specId: number; story: string }): void {
      this.editor = {
        mode: 'create',
        projectId: request.projectId,
        kind: 'implementation',
        kindLocked: true,
        specId: request.specId,
        story: request.story,
        ticketId: null,
      }
    },
    /** Drawer Edit: the standing Ticket fixes the kind, and the
     * dialog reads the record itself rather than trusting a summary. */
    openEdit(request: { projectId: number; ticketId: number; kind?: TicketKind }): void {
      this.editor = {
        mode: 'edit',
        projectId: request.projectId,
        kind: request.kind ?? 'implementation',
        kindLocked: true,
        specId: null,
        story: null,
        ticketId: request.ticketId,
      }
    },
    closeEditor(): void {
      this.editor = null
    },
    openQuickBug(request: { projectId: number | null }): void {
      this.quickBugProjectId = request.projectId
      this.quickBugOpen = true
    },
    closeQuickBug(): void {
      this.quickBugOpen = false
      this.quickBugProjectId = null
    },
  },
})
