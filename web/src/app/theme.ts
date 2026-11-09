import type { RuntimeSettingsState, ThemePaletteState } from '@/remote/protocol'

/** Design tokens as CSS variables so built-in modifiers follow light/dark mode. */
export const color = {
  bg: 'var(--bg)',
  sidebar: 'var(--sidebar)',
  field: 'var(--field)',
  navHover: 'var(--nav-hover)',
  navSelect: 'var(--nav-select)',
  sessionSelect: 'var(--session-select)',
  rowHover: 'var(--row-hover)',
  rowSelect: 'var(--row-select)',
  bubble: 'var(--bubble)',
  rule: 'var(--rule)',
  composer: 'var(--composer)',
  panel: 'var(--panel)',
  raised: 'var(--raised)',
  text: 'var(--text)',
  text2: 'var(--text2)',
  muted: 'var(--muted)',
  faint: 'var(--faint)',
  line: 'var(--line)',
  hover: 'var(--hover)',
  accent: 'var(--accent)',
  accentHover: 'var(--accent-hover)',
  accentInk: 'var(--accent-ink)',
  add: 'var(--add)',
  addBg: 'var(--add-bg)',
  del: 'var(--del)',
  delBg: 'var(--del-bg)',
  scrim: 'var(--scrim)',
}
export const mono = 'var(--mono)'

let lastTheme = ''
let applied: string[] = []

/** Rust resolves theme files/catalogs; the browser only maps the chosen palette. */
export function applyTheme(settings: RuntimeSettingsState | null | undefined) {
  const appearance = settings?.appearance || 'auto'
  const dark = appearance === 'dark' || (appearance === 'auto' && matchMedia('(prefers-color-scheme: dark)').matches)
  const palette = dark ? settings?.themeDarkPalette : settings?.themeLightPalette
  const name = dark ? settings?.themeDarkResolved : settings?.themeLightResolved
  const signature = JSON.stringify([appearance, dark, name, palette])
  if (signature === lastTheme) return
  lastTheme = signature
  const root = document.documentElement
  root.dataset.appearance = appearance
  applied.forEach(token => root.style.removeProperty(token))
  applied = []
  // The Rust palette is authoritative for the default theme too.
  if (!palette || !name) return
  const mapping: Record<string, keyof ThemePaletteState> = {
    bg: 'background', sidebar: 'surface', inspector: 'surface', composer: 'surface', panel: 'surface',
    field: 'surfaceRaised', raised: 'surfaceRaised', 'nav-hover': 'surfaceRaised', 'row-hover': 'surfaceRaised',
    'nav-select': 'selected', 'row-select': 'selected', 'session-select': 'selected', bubble: 'userSurface',
    text: 'text', text2: 'textDim', muted: 'textDim', faint: 'muted', rule: 'border', line: 'borderDim',
    accent: 'accent', 'accent-hover': 'accentHot', focus: 'accent', add: 'success', del: 'error',
  }
  const set = (token: string, value: string) => {
    if (!CSS.supports('color', value)) return
    root.style.setProperty(`--${token}`, value)
    applied.push(`--${token}`)
  }
  for (const [token, key] of Object.entries(mapping)) set(token, palette[key])
  // Choose readable ink on the theme's accent (palettes use #rrggbb).
  const hex = palette.accent.match(/^#([\da-f]{6})$/i)?.[1]
  if (hex) {
    const [r, g, b] = [0, 2, 4].map(offset => parseInt(hex.slice(offset, offset + 2), 16) / 255)
      .map(value => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4)
    set('accent-ink', 0.2126 * r! + 0.7152 * g! + 0.0722 * b! > 0.179 ? '#18181b' : '#ffffff')
  }
  set('hover', 'color-mix(in srgb, var(--text) 6%, transparent)')
  set('add-bg', 'color-mix(in srgb, var(--add) 12%, var(--bg))')
  set('del-bg', 'color-mix(in srgb, var(--del) 12%, var(--bg))')
}
