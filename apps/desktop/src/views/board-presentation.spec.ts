// Snapshot-style pin of the Surface presentation tokens the board
// preserves: typography, colours, themes, spacing radii, and shadows
// (KAN-T24-AC1). The board views consume these as Tailwind utilities;
// this spec holds the token sheet itself against drift.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

// Read the sheet from disk: the test pins the authored tokens, not a
// transform pipeline's output. Desktop tests run from apps/desktop.
const tokens = readFileSync(resolve(process.cwd(), 'src/main.css'), 'utf8')

describe('the Surface presentation tokens', () => {
  it('carries the Surface typography', () => {
    expect(tokens).toContain('--font-sans:')
    expect(tokens).toContain('Archivo Variable')
    expect(tokens).toContain('--font-display:')
    expect(tokens).toContain('Bricolage Grotesque')
    expect(tokens).toContain('--font-mono:')
    expect(tokens).toContain('JetBrains Mono')
  })

  it('carries the semantic colour vocabulary the board speaks', () => {
    for (const token of [
      '--color-canvas',
      '--color-surface',
      '--color-raised',
      '--color-line',
      '--color-line-strong',
      '--color-ink',
      '--color-ink-muted',
      '--color-ink-subtle',
      '--color-accent',
      '--color-accent-fill',
      '--color-on-accent',
      '--color-caution',
      '--color-critical',
      '--color-info',
    ]) {
      expect(tokens, `${token} is part of the presentation`).toContain(token)
    }
  })

  it('keeps the brand ramp the accents sample', () => {
    expect(tokens).toContain('--color-brand-300')
    expect(tokens).toContain('--color-brand-500')
    expect(tokens).toContain('--color-brand-950')
  })

  it('fixes the control and panel radii', () => {
    expect(tokens).toContain('--radius-control: 0.5rem')
    expect(tokens).toContain('--radius-panel: 0.875rem')
  })

  it('fixes the panel and drawer shadows', () => {
    expect(tokens).toContain('--shadow-panel:')
    expect(tokens).toContain('--shadow-drawer:')
  })

  it('supplies both themes as class-swapped token values', () => {
    expect(tokens).toMatch(/:root\s*\{[^}]*color-scheme:\s*light/s)
    expect(tokens).toMatch(/html\.dark\s*\{[^}]*color-scheme:\s*dark/s)
    expect(tokens).toContain('@custom-variant dark')
  })

  it('paints the document from the tokens', () => {
    expect(tokens).toMatch(/body\s*\{[^}]*background-color:\s*var\(--canvas\)/s)
    expect(tokens).toMatch(/body\s*\{[^}]*color:\s*var\(--ink\)/s)
    expect(tokens).toMatch(/body\s*\{[^}]*font-family:\s*var\(--font-sans\)/s)
  })
})
