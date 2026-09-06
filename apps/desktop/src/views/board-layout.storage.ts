// The board's local presentation choices: axis layouts, Done
// placement, Board/Register presentation, and Draft visibility, held
// in one stored record because they share a home — writing one must
// not drop the others. Saved Views will own these per perspective in
// the authoritative store once KAN-T28 lands; until then they are
// presentation state the browser keeps.
import {
  BOARD_LAYOUTS,
  BOARD_PRESENTATIONS,
  DEFAULT_BOARD_LAYOUTS,
  DEFAULT_BOARD_PRESENTATION,
  DEFAULT_DONE_PRESENTATION,
  DEFAULT_DRAFT_VISIBILITY,
  DONE_PRESENTATIONS,
  DRAFT_VISIBILITIES,
  type BoardLayoutState,
  type BoardPresentation,
  type DonePresentation,
  type DraftVisibility,
} from './board-layout'

export const BOARD_LAYOUT_STORAGE_VERSION = 1

export const BOARD_LAYOUT_STORAGE_KEY = `kanban.board.layout.v${BOARD_LAYOUT_STORAGE_VERSION}`

export type BoardChoiceStore = Pick<
  Storage,
  'getItem' | 'setItem' | 'removeItem'
>

/** Every board choice the operator keeps. */
export type BoardChoices = Readonly<{
  layouts: BoardLayoutState
  done: DonePresentation
  presentation: BoardPresentation
  draft: DraftVisibility
}>

export const DEFAULT_BOARD_CHOICES: BoardChoices = {
  layouts: DEFAULT_BOARD_LAYOUTS,
  done: DEFAULT_DONE_PRESENTATION,
  presentation: DEFAULT_BOARD_PRESENTATION,
  draft: DEFAULT_DRAFT_VISIBILITY,
}

export function loadBoardChoices(
  store: BoardChoiceStore = localStore(),
): BoardChoices {
  const payload = storedPayload(store)
  return {
    layouts: asLayouts(payload?.layouts),
    done: asDone(payload?.done),
    presentation: asPresentation(payload?.presentation),
    draft: asDraftVisibility(payload?.draft),
  }
}

export function saveBoardChoices(
  choices: BoardChoices,
  store: BoardChoiceStore = localStore(),
): void {
  try {
    store.setItem(
      BOARD_LAYOUT_STORAGE_KEY,
      JSON.stringify({ version: BOARD_LAYOUT_STORAGE_VERSION, ...choices }),
    )
  } catch {
    // Private mode or quota must not break the board layout controls.
  }
}

function storedPayload(
  store: BoardChoiceStore,
): Record<string, unknown> | undefined {
  try {
    const text = store.getItem(BOARD_LAYOUT_STORAGE_KEY)
    if (text === null) return undefined
    const parsed: unknown = JSON.parse(text)
    if (
      parsed === null ||
      typeof parsed !== 'object' ||
      !('version' in parsed) ||
      parsed.version !== BOARD_LAYOUT_STORAGE_VERSION
    ) {
      return undefined
    }
    return parsed as Record<string, unknown>
  } catch {
    return undefined
  }
}

/** Each choice falls back on its own, so one bad value keeps the rest. */
function asLayouts(value: unknown): BoardLayoutState {
  if (value === null || typeof value !== 'object') {
    return DEFAULT_BOARD_LAYOUTS
  }
  const stored = value as Record<string, unknown>
  const entries = (
    ['backlog', 'completion'] as const
  ).map((axis) => [
    axis,
    BOARD_LAYOUTS.find((layout) => layout === stored[axis]) ??
      DEFAULT_BOARD_LAYOUTS[axis],
  ])
  return Object.fromEntries(entries) as BoardLayoutState
}

function asDone(value: unknown): DonePresentation {
  return DONE_PRESENTATIONS.find((entry) => entry === value) ?? DEFAULT_DONE_PRESENTATION
}

function asPresentation(value: unknown): BoardPresentation {
  return (
    BOARD_PRESENTATIONS.find((entry) => entry === value) ?? DEFAULT_BOARD_PRESENTATION
  )
}

function asDraftVisibility(value: unknown): DraftVisibility {
  return (
    DRAFT_VISIBILITIES.find((entry) => entry === value) ?? DEFAULT_DRAFT_VISIBILITY
  )
}

function localStore(): BoardChoiceStore {
  try {
    return globalThis.localStorage
  } catch {
    return {
      getItem() {
        return null
      },
      setItem() {
        return
      },
      removeItem() {
        return
      },
    }
  }
}
