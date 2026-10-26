/** One of the user's bro sessions, as the pane shows it. */
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

/** The question on the clock: its dialog (the call's tool_use_id) and its seconds, all and left. */
export type BroQuestion = { id: string; left: number; total: number }

declare module 'claude-code' {
  interface PluginState {
    bro: {
      sessions: BroSession[]
      limits: BroLimit[]
      /** bro answered the last poll. */
      isOnline: boolean
      /** The question timer's countdown, while a question waits on it. */
      question: BroQuestion | null
    }
  }
}
