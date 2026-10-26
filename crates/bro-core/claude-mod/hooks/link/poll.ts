// While the `bro` pane is up, one quiet loop keeps it current: bro's session
// list every few seconds, its usage report every half minute. Loopback calls
// only, and none with the pane closed.
import { atom, read, update } from 'claude-code'
import type { EngineInterface } from 'claude-code'

import type { BroLimit } from '../../types'
import { getJson } from '../shared/bridge'
import type { BroEnv } from '../shared/env'
import { toLimits, toSessions } from './data'

// Written here, drawn by the pane.
const sessions = atom({ plugin: 'bro', key: 'sessions' } as const, [])
const limits = atom({ plugin: 'bro', key: 'limits' } as const, [])
const isOnline = atom({ plugin: 'bro', key: 'isOnline' } as const, false)

export const PANE = 'bro'
const TICK_MS = 3000
/** Ticks between usage reads: bro itself reads each login every five minutes. */
const LIMIT_TICKS = 10

export function startPolling($: EngineInterface, env: BroEnv): void {
  let tick = 0
  let isBusy = false
  const run = async () => {
    if (isBusy) {
      return
    }
    isBusy = true
    try {
      if (!(await $.ui.panes()).some(pane => pane.id === PANE && pane.isPlaced)) {
        return
      }
      await refreshSessions($, env)
      if (tick % LIMIT_TICKS === 0) {
        await refreshLimits($, env)
      }
      tick += 1
    } finally {
      isBusy = false
    }
  }
  $.clock.every(TICK_MS, () => void run())
}

export async function refreshSessions($: EngineInterface, env: BroEnv): Promise<void> {
  const rows = await getJson($, env, '/api/sessions')
  await update($, isOnline, () => rows !== undefined)
  if (rows !== undefined) {
    const list = toSessions(rows, env.sessionId)
    await update($, sessions, () => list)
  }
}

export async function refreshLimits($: EngineInterface, env: BroEnv): Promise<BroLimit[]> {
  const report = await getJson($, env, '/api/usage')
  if (report === undefined) {
    return read($, limits)
  }
  const list = toLimits(report, env.profile)
  await update($, limits, () => list)
  return list
}
