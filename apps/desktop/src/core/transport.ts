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
  KanbanOperationName,
  KanbanTransport,
} from '@kanban/contracts'

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
  'timeline.query': 'timeline_query',
} as const satisfies Record<KanbanOperationName, string>

function shellInvokePayload<Request>(
  name: KanbanOperationName,
  request: Request,
): Record<string, unknown> {
  switch (name) {
    case 'timeline.query':
      return { request: request as Record<string, unknown> }
    default:
      return request as Record<string, unknown>
  }
}

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
    return invoke<Response>(COMMAND_FOR_OPERATION[name], shellInvokePayload(name, request))
  },
  async command<Request, Response>(
    name: KanbanOperationName,
    request: Request,
  ): Promise<Response> {
    return invoke<Response>(COMMAND_FOR_OPERATION[name], shellInvokePayload(name, request))
  },
  subscribe(handler: (event: EventEnvelope) => void): () => void {
    return listenTo(CORE_EVENT, (payload) => handler(payload as EventEnvelope))
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
