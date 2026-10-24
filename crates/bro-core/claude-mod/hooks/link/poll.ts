// One quiet loop per session keeps the radar, the hint line and the dashboard
// current: bro's session list every few seconds, its usage report every half
// minute, Claude's own figures each tick. Loopback calls only.
import { atom, read, update } from 'claude-code'
import type { EngineInterface } from 'claude-code'

import type { BroLimit } from '../../types'
import { getJson } from '../shared/bridge'
import { type BroEnv, loginName } from '../shared/env'
import { radarLine, span, toLimits, toSessions } from './data'

// Written here, drawn by the pane and the hint line; `look.ts` writes `look`.
const look = atom({ plugin: 'bro', key: 'look' } as const, null)
const hud = atom({ plugin: 'bro', key: 'hud' } as const, null)
const sessions = atom({ plugin: 'bro', key: 'sessions' } as const, [])
const limits = atom({ plugin: 'bro', key: 'limits' } as const, [])
const isOnline = atom({ plugin: 'bro', key: 'isOnline' } as const, false)

export const PANE = 'bro'
const TICK_MS = 3000
/** Ticks between usage reads: bro itself reads each login every five minutes. */
const LIMIT_TICKS = 10
/** Past this, the user hears about it once, with when it resets. */
const WARN_AT = 90

export type Wants = { radar: boolean; hud: boolean }

export async function startPolling($: EngineInterface, env: BroEnv, wants: Wants): Promise<void> {
  let tick = 0
  let isBusy = false
  let shown: string | undefined
  const warned = new Set<string>()

  const run = async () => {
    if (isBusy) {
      return
    }
    isBusy = true
    try {
      const isPaneUp = (await $.ui.panes()).some(pane => pane.id === PANE && pane.isPlaced)
      if (wants.radar || isPaneUp) {
        const line = await refreshSessions($, env)
        const next = wants.radar ? line : undefined
        if (next !== shown) {
          shown = next
          $.ui.status(next)
        }
      }
      if ((wants.hud || isPaneUp) && tick % LIMIT_TICKS === 0) {
        await refreshLimits($, env)
      }
      if (wants.hud) {
        await refreshHud($, env, warned)
      }
      tick += 1
    } finally {
      isBusy = false
    }
  }
  await run()
  $.clock.every(TICK_MS, () => void run())
}

/** Pulls the session list; answers the radar's line for it. */
export async function refreshSessions($: EngineInterface, env: BroEnv): Promise<string | undefined> {
  const rows = await getJson($, env, '/api/sessions')
  await update($, isOnline, () => rows !== undefined)
  if (rows === undefined) {
    return undefined
  }
  const list = toSessions(rows, env.sessionId)
  await update($, sessions, () => list)
  return radarLine(list)
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

/**
 * The hint line's tail: `✦ work · 5h 23% · wk 41% · ctx 12%`. Claude's own
 * rate-limit reading wins (it is this session's account, live); bro's usage
 * report fills in until the first reply carries one.
 */
async function refreshHud($: EngineInterface, env: BroEnv, warned: Set<string>): Promise<void> {
  const usage = await $.session.usage().catch(() => undefined)
  const theme = await read($, look)
  const self = (await read($, limits)).find(one => one.isSelf)
  const who = env.profile ?? env.backend
  const five = usage?.rateLimits.find(r => r.kind === 'five_hour')
  const week = usage?.rateLimits.find(r => r.kind === 'seven_day')
  const fiveUsed = five?.percentUsed ?? self?.fiveHour?.used
  const weekUsed = week?.percentUsed ?? self?.weekly?.used

  const parts = [`${theme?.marker ?? '◆'} ${who === undefined ? 'bro' : loginName(who)}`]
  if (fiveUsed !== undefined) {
    parts.push(`5h ${Math.round(fiveUsed)}%`)
  }
  if (weekUsed !== undefined) {
    parts.push(`wk ${Math.round(weekUsed)}%`)
  }
  if (usage?.context.percent !== undefined) {
    parts.push(`ctx ${usage.context.percent}%`)
  }
  const text = parts.join(' · ')
  await update($, hud, () => text)

  const now = await $.clock.now()
  warn($, warned, '5-hour', fiveUsed, five?.resetsAt ?? self?.fiveHour?.resetsAt, now)
  warn($, warned, 'Weekly', weekUsed, week?.resetsAt ?? self?.weekly?.resetsAt, now)
}

function warn(
  $: EngineInterface,
  warned: Set<string>,
  name: string,
  used: number | undefined,
  resetsAt: string | number | undefined,
  now: number,
): void {
  if (used === undefined || used < WARN_AT) {
    warned.delete(name)
    return
  }
  if (warned.has(name)) {
    return
  }
  warned.add(name)
  const at = typeof resetsAt === 'number' ? resetsAt * 1000 : resetsAt === undefined ? Number.NaN : Date.parse(resetsAt)
  const when = Number.isFinite(at) ? `, resets in ${span(at - now)}` : ''
  $.ui.toast(`${name} limit ${Math.round(used)}% used${when}. bro's launcher can start the next session on a roomier login.`, {
    timeoutMs: 9000,
  })
}
