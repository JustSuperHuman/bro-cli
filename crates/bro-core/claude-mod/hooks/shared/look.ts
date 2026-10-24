// bro's theme, as the Claude Code theme bro writes for the session carries it.
// bro rewrites `<config>/themes/bro.json` whenever its theme changes (previews
// included); Claude Code recolours from it live, and the mod follows the same
// file for what a theme can't say: the theme's id, label and marker.
import { atom, update } from 'claude-code'
import type { EngineInterface } from 'claude-code'

import type { BroLook, BroPalette } from '../../types'
import { type BroEnv, join } from './env'

const look = atom({ plugin: 'bro', key: 'look' } as const, null)

/** How often the theme file is checked; a stat, so cheap. */
const POLL_MS = 1500

export function themeFile(env: BroEnv): string {
  return join(env.configDir, 'themes', 'bro.json')
}

/** Keeps the `look` value in step with the theme file for the session's life. */
export async function followLook($: EngineInterface, env: BroEnv): Promise<void> {
  const file = themeFile(env)
  let seen = Number.NaN
  let isBusy = false
  const check = async () => {
    if (isBusy) {
      return
    }
    isBusy = true
    try {
      const stat = await $.fs.stat(file).catch(() => undefined)
      const mtime = stat?.mtimeMs ?? 0
      if (mtime === seen) {
        return
      }
      seen = mtime
      const text = stat === undefined ? undefined : await $.fs.read(file).catch(() => undefined)
      const found = text === undefined ? null : parseLook(text)
      await update($, look, () => found)
    } finally {
      isBusy = false
    }
  }
  await check()
  $.clock.every(POLL_MS, () => void check())
}

export function parseLook(text: string): BroLook | null {
  let doc: unknown
  try {
    doc = JSON.parse(text)
  } catch {
    return null
  }
  const meta = (doc as { bro?: unknown } | null)?.bro
  if (typeof meta !== 'object' || meta === null) {
    return null
  }
  const m = meta as Record<string, unknown>
  const palette = parsePalette(m.palette)
  if (typeof m.id !== 'string' || palette === null) {
    return null
  }
  return {
    id: m.id,
    label: typeof m.label === 'string' ? m.label : m.id,
    base: typeof m.base === 'string' ? m.base : m.id,
    isLight: m.light === true,
    marker: typeof m.marker === 'string' && m.marker !== '' ? m.marker : '▶',
    palette,
  }
}

function parsePalette(value: unknown): BroPalette | null {
  if (typeof value !== 'object' || value === null) {
    return null
  }
  const p = value as Record<string, unknown>
  const pick = (key: string) => (typeof p[key] === 'string' && p[key] !== '' ? (p[key] as string) : undefined)
  const accent = pick('accent')
  if (accent === undefined) {
    return null
  }
  return {
    accent,
    shine: pick('shine') ?? accent,
    good: pick('good') ?? 'green',
    warn: pick('warn') ?? 'yellow',
    danger: pick('danger') ?? 'red',
    muted: pick('muted') ?? 'gray',
    frame: pick('frame') ?? 'gray',
    fg: pick('fg'),
    bg: pick('bg'),
  }
}

/** A colour for the mod's own drawings: the theme's, else a Claude theme key that bro also themed. */
export function ink(l: BroLook | null, slot: keyof BroPalette): string {
  const fromTheme = l?.palette[slot]
  if (fromTheme !== undefined) {
    return fromTheme
  }
  const keys: Record<keyof BroPalette, string> = {
    accent: 'claude',
    shine: 'permission',
    good: 'success',
    warn: 'warning',
    danger: 'error',
    muted: 'inactive',
    frame: 'subtle',
    fg: 'text',
    bg: 'inverseText',
  }
  return keys[slot]
}
