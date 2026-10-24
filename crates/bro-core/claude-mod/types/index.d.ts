/** bro's theme as the session's `themes/bro.json` carries it (bro writes it; see `look.ts`). */
export type BroLook = {
  /** bro's theme id (`tokyo-night`, or a custom theme's file name). */
  id: string
  /** Shown to people: `Tokyo Night`. */
  label: string
  /** The built-in theme a custom theme derives from; `id` for a built-in. */
  base: string
  isLight: boolean
  /** The theme's title marker in bro (`▶`, `✦`, `◆`). */
  marker: string
  /** Colours as Ink takes them: `#rrggbb`, or a terminal colour name. */
  palette: BroPalette
}

export type BroPalette = {
  accent: string
  shine: string
  good: string
  warn: string
  danger: string
  muted: string
  frame: string
  /** Absent where bro leaves the terminal's own colour. */
  fg?: string
  bg?: string
}

/** One of the user's bro sessions, as the radar and the dashboard show it. */
export type BroSession = {
  id: string
  title: string
  project: string
  state: 'needs' | 'working' | 'done' | 'idle' | 'exited'
  /** Why it waits, or what it last did. */
  detail?: string
  /** ISO time the state began, when bro knows it. */
  since?: string
  isSelf: boolean
  isOrchestrator: boolean
}

/** One Claude login's limits, from bro's usage report. */
export type BroLimit = {
  id: string
  name: string
  isSelf: boolean
  /** Percent used, 0 to 100, with the reset in epoch seconds. */
  fiveHour?: { used: number; resetsAt?: number }
  weekly?: { used: number; resetsAt?: number }
  isExhausted: boolean
}

declare module 'claude-code' {
  interface PluginState {
    bro: {
      look: BroLook | null
      hud: string | null
      sessions: BroSession[]
      limits: BroLimit[]
      /** bro answered the last poll. */
      isOnline: boolean
    }
  }
}
