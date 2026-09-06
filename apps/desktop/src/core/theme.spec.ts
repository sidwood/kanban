import { describe, expect, it } from 'vitest'
import { applyTheme, loadTheme, saveTheme, type ThemeStore } from './theme'

// A storage the tests steer; the production default is localStorage.
function memoryStore(initial: string | null = null): ThemeStore {
  let value = initial
  return {
    getItem: () => value,
    setItem: (_key: string, next: string) => {
      value = next
    },
    removeItem: () => {
      value = null
    },
  }
}

describe('theme', () => {
  it('defaults to the daylight theme', () => {
    expect(loadTheme(memoryStore())).toBe('light')
  })

  it('restores a saved theme', () => {
    const store = memoryStore()
    saveTheme('dark', store)

    expect(loadTheme(store)).toBe('dark')
  })

  it('falls back to daylight on an unknown stored value', () => {
    expect(loadTheme(memoryStore('"forest night"'))).toBe('light')
  })

  it('keeps saving when storage refuses', () => {
    const refusing: ThemeStore = {
      getItem: () => null,
      setItem: () => {
        throw new Error('quota')
      },
      removeItem: () => undefined,
    }

    expect(() => saveTheme('dark', refusing)).not.toThrow()
    expect(loadTheme(refusing)).toBe('light')
  })

  it('applies a theme by swapping the dark class', () => {
    document.documentElement.classList.remove('dark')

    applyTheme('dark')
    expect(document.documentElement.classList.contains('dark')).toBe(true)

    applyTheme('light')
    expect(document.documentElement.classList.contains('dark')).toBe(false)
  })
})
