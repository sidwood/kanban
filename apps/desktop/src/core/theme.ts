// The class-based theme control the presentation tokens swap on:
// daylight by default, forest night when the `dark` class lands on
// the root element. A presentation choice, stored beside the board's
// other local preferences.
export type ThemeName = 'light' | 'dark'

export const THEME_STORAGE_KEY = 'kanban.theme.v1'

export type ThemeStore = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>

// Read the stored theme, falling back on daylight when nothing or
// something unknown is stored.
export function loadTheme(store: ThemeStore = localStore()): ThemeName {
  try {
    const text = store.getItem(THEME_STORAGE_KEY)
    return text === 'dark' ? 'dark' : 'light'
  } catch {
    return 'light'
  }
}

// Keep the theme beside the other presentation choices; a storage
// that refuses must not break the theme control.
export function saveTheme(theme: ThemeName, store: ThemeStore = localStore()): void {
  try {
    store.setItem(THEME_STORAGE_KEY, theme)
  } catch {
    // Private mode or quota must not break the theme control.
  }
}

// Swap the class the `html.dark` token block answers to.
export function applyTheme(theme: ThemeName): void {
  document.documentElement.classList.toggle('dark', theme === 'dark')
}

function localStore(): ThemeStore {
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
