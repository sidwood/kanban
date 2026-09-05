// The one bridge between the WebView and the core: the generated
// client's transport, implemented over the shell's typed commands
// and ordered events. Hand-written request or event types never
// appear here; everything speaks the generated contract (ADR-0004).
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { InjectionKey } from 'vue'
import type {
  ApiError,
  EventEnvelope,
  KanbanLiveEvent,
  KanbanOperationName,
  KanbanTransport,
} from '@kanban/contracts'
import { parseKanbanLiveEvent } from '@kanban/contracts'

// The shell's Tauri event names. These are shell-level plumbing,
// not domain contracts; `core://event` payloads are the generated
// `EventEnvelope` values.
const CORE_EVENT = 'core://event'
const CONNECTION_EVENT = 'core://connection'

// The shell's announcement of its own connection, mirroring the
// shell crate's `ConnectionState`.
export type ShellConnectionState = 'connected' | 'disconnected'

// The generated operations map onto the shell's typed commands,
// one per operation; the map must stay complete over the generated
// catalog or typecheck fails.
const COMMAND_FOR_OPERATION = {
  'health.get': 'health_get',
  'initiative.archive': 'initiative_archive',
  'initiative.create': 'initiative_create',
  'initiative.list': 'initiative_list',
  'initiative.rename': 'initiative_rename',
  'project.archive': 'project_archive',
  'project.list': 'project_list',
  'project.register': 'project_register',
  'plan.create': 'plan_create',
  'plan.spec.add': 'plan_spec_add',
  'plan.spec.remove': 'plan_spec_remove',
  'plan.spec.move': 'plan_spec_move',
  'plan.edge.add': 'plan_edge_add',
  'plan.edge.remove': 'plan_edge_remove',
  'plan.activate': 'plan_activate',
  'plan.replan': 'plan_replan',
  'plan.complete': 'plan_complete',
  'plan.cancel': 'plan_cancel',
  'plan.archive': 'plan_archive',
  'plan.list': 'plan_list',
  'plan.get': 'plan_get',
  'spec.create': 'spec_create',
  'spec.content.update': 'spec_content_update',
  'spec.version.approve': 'spec_version_approve',
  'spec.version.supersede': 'spec_version_supersede',
  'spec.plan.join': 'spec_plan_join',
  'spec.execution.move': 'spec_execution_move',
  'spec.list': 'spec_list',
  'spec.get': 'spec_get',
  'spec.version.get': 'spec_version_get',
  'timeline.query': 'timeline_query',
  'comment.create': 'comment_create',
  'comment.edit': 'comment_edit',
  'comment.revisions': 'comment_revisions',
  'ruling.record': 'ruling_record',
  'ruling.supersede': 'ruling_supersede',
  'ruling.list': 'ruling_list',
  'deferral.record': 'deferral_record',
  'deferral.supersede': 'deferral_supersede',
  'deferral.list': 'deferral_list',
  'evidence.attach': 'evidence_attach',
  'evidence.list': 'evidence_list',
  'herdr.settings.get': 'herdr_settings_get',
  'herdr.settings.update': 'herdr_settings_update',
  'herdr.defaults.get': 'herdr_defaults_get',
  'herdr.defaults.update': 'herdr_defaults_update',
  'workspace.register': 'workspace_register',
  'workspace.observe': 'workspace_observe',
  'workspace.list': 'workspace_list',
} as const satisfies Record<KanbanOperationName, string>

// The transport the generated client runs on, plus the shell's
// connection announcements.
export interface ShellTransport extends KanbanTransport {
  onConnectionChange(handler: (state: ShellConnectionState) => void): () => void
}

export const tauriTransport: ShellTransport = {
  async query<Request, Response>(
    name: KanbanOperationName,
    request: Request,
  ): Promise<Response> {
    return invoke<Response>(COMMAND_FOR_OPERATION[name], { request })
  },
  async command<Request, Response>(
    name: KanbanOperationName,
    request: Request,
  ): Promise<Response> {
    return invoke<Response>(COMMAND_FOR_OPERATION[name], { request })
  },
  subscribe(handler: (event: KanbanLiveEvent) => void): () => void {
    return listenTo(CORE_EVENT, (payload) => handler(parseKanbanLiveEvent(payload as EventEnvelope)))
  },
  onConnectionChange(handler: (state: ShellConnectionState) => void): () => void {
    return listenTo(CONNECTION_EVENT, (payload) => {
      handler((payload as { state: ShellConnectionState }).state)
    })
  },
}

// Listen to one shell event until the returned stopper is called;
// a stop before the listener attached stops it on arrival instead.
function listenTo(eventName: string, deliver: (payload: unknown) => void): () => void {
  let unlisten: () => void = () => undefined
  let active = true
  void listen<unknown>(eventName, (entry) => deliver(entry.payload)).then((stop) => {
    if (active) {
      unlisten = stop
    } else {
      stop()
    }
  })
  return () => {
    active = false
    unlisten()
  }
}

// Shape anything the shell rejects with into the generated
// `ApiError`; unknown failures keep the internal code.
export function asApiError(failure: unknown): ApiError {
  if (isApiError(failure)) {
    return failure
  }
  return { code: 'internal', message: String(failure) }
}

function isApiError(value: unknown): value is ApiError {
  return (
    typeof value === 'object' &&
    value !== null &&
    'code' in value &&
    'message' in value &&
    typeof (value as { message?: unknown }).message === 'string'
  )
}

// The injection key the app provides the transport under.
export const kanbanTransportKey: InjectionKey<ShellTransport> = Symbol('kanban-transport')
