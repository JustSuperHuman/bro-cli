// What bro's bridge says about the user's sessions and logins, in the shapes
// the mod draws.
import type { BroLimit, BroSession } from '../../types'
import { baseName, loginName } from '../shared/env'

/** A `GET /api/sessions` row, as far as the mod reads it. */
type Summary = {
  id?: unknown
  title?: unknown
  cwd?: unknown
  agent?: unknown
  agentActivity?: unknown
  status?: unknown
  kind?: unknown
  attention?: { state?: unknown; reason?: unknown; since?: unknown }
  recap?: { text?: unknown }
}

export function toSessions(rows: unknown, selfId: string): BroSession[] {
  if (!Array.isArray(rows)) {
    return []
  }
  const out: BroSession[] = []
  for (const row of rows as Summary[]) {
    const one = toSession(row, selfId)
    if (one !== null) {
      out.push(one)
    }
  }
  return out.sort((a, b) => RANK[a.state] - RANK[b.state])
}

/** Most urgent first: what waits on the user, then what runs, then the rest. */
const RANK: Record<BroSession['state'], number> = { needs: 0, done: 1, working: 2, idle: 3, exited: 4 }

function toSession(row: Summary, selfId: string): BroSession | null {
  if (typeof row.id !== 'string') {
    return null
  }
  const isOrchestrator = row.kind === 'orchestrator'
  const attention = typeof row.attention?.state === 'string' ? row.attention.state : undefined
  const activity = typeof row.agentActivity === 'string' ? row.agentActivity : undefined
  // A plain shell has neither: it isn't an agent session.
  if (row.agent == null && attention === undefined && !isOrchestrator) {
    return null
  }
  const state: BroSession['state'] =
    row.status === 'exited'
      ? 'exited'
      : attention === 'blocked' || activity === 'awaiting'
        ? 'needs'
        : attention === 'working' || activity === 'working'
          ? 'working'
          : attention === 'done'
            ? 'done'
            : 'idle'
  const reason = typeof row.attention?.reason === 'string' ? row.attention.reason : undefined
  const recap = typeof row.recap?.text === 'string' ? firstLine(row.recap.text) : undefined
  const cwd = typeof row.cwd === 'string' ? row.cwd : ''
  return {
    id: row.id,
    title: typeof row.title === 'string' && row.title.trim() !== '' ? row.title.trim() : baseName(cwd),
    project: baseName(cwd),
    state,
    detail: state === 'needs' ? (reason ?? recap) : (recap ?? reason),
    since: typeof row.attention?.since === 'string' ? row.attention.since : undefined,
    isSelf: row.id === selfId,
    isOrchestrator,
  }
}

/** A `GET /api/usage` login. */
type Login = {
  id?: unknown
  app?: unknown
  name?: unknown
  fiveHour?: { usedPct?: unknown; resetsAt?: unknown } | null
  weekly?: { usedPct?: unknown; resetsAt?: unknown } | null
  exhausted?: unknown
}

export function toLimits(report: unknown, selfProfile: string | undefined): BroLimit[] {
  const logins = (report as { logins?: unknown } | undefined)?.logins
  if (!Array.isArray(logins)) {
    return []
  }
  const out: BroLimit[] = []
  for (const login of logins as Login[]) {
    if (typeof login.id !== 'string' || login.app !== 'claude') {
      continue
    }
    out.push({
      id: login.id,
      name: typeof login.name === 'string' && login.name !== '' ? login.name : loginName(login.id),
      isSelf: login.id === selfProfile,
      fiveHour: window(login.fiveHour),
      weekly: window(login.weekly),
      isExhausted: login.exhausted === true,
    })
  }
  return out.sort((a, b) => Number(b.isSelf) - Number(a.isSelf))
}

function window(w: Login['fiveHour']): BroLimit['fiveHour'] {
  if (w == null || typeof w.usedPct !== 'number') {
    return undefined
  }
  return { used: w.usedPct, resetsAt: typeof w.resetsAt === 'number' ? w.resetsAt : undefined }
}

export function firstLine(text: string): string {
  const line = text.split(/\r?\n/).find(part => part.trim() !== '') ?? ''
  return line.trim().replace(/^[#>*\-\s]+/, '')
}

/** `2h 10m`, `3d 4h`, `45s`: how long until (or since) a moment, roughly. */
export function span(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000))
  if (s < 60) {
    return `${s}s`
  }
  const m = Math.round(s / 60)
  if (m < 60) {
    return `${m}m`
  }
  const h = Math.floor(m / 60)
  if (h < 48) {
    return m % 60 === 0 ? `${h}h` : `${h}h ${m % 60}m`
  }
  const d = Math.floor(h / 24)
  return h % 24 === 0 ? `${d}d` : `${d}d ${h % 24}h`
}

/**
 * The radar's line: who waits on the user; undefined when nobody does. The
 * engine draws a plugin's status line as a warning (`⚠ bro: …`), so it is kept
 * for what needs the user now; finished sessions wait in the pane.
 */
export function radarLine(list: readonly BroSession[]): string | undefined {
  const needs = list.filter(one => !one.isSelf && one.state === 'needs')
  if (needs.length === 0) {
    return undefined
  }
  const [first] = needs
  if (needs.length === 1 && first !== undefined) {
    const why = first.detail === undefined ? '' : ` (${clip(first.detail, 48)})`
    return `${first.title} needs you${why} · /bro`
  }
  return `${needs.length} sessions need you: ${names(needs.map(one => one.title))} · /bro`
}

/** Cuts `text` to `max` characters, an ellipsis standing for the rest. */
export function clip(text: string, max: number): string {
  const chars = [...text]
  return chars.length <= max ? text : `${chars.slice(0, Math.max(1, max - 1)).join('').trimEnd()}…`
}

function names(list: readonly string[]): string {
  return list.length <= 3 ? list.join(', ') : `${list.slice(0, 3).join(', ')} +${list.length - 3}`
}
