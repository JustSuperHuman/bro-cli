// The `bro` pane: every agent session the user has open in bro, most urgent
// first, each a press away; then each Claude login's limits.
import type { ElementTable, EngineInterface, RenderSurface } from 'claude-code'

import type { BroLimit, BroSession } from '../../types'
import { ctl } from '../shared/bridge'
import type { BroEnv } from '../shared/env'
import { clip, span } from './data'

type Table = ElementTable<RenderSurface>

const GLYPH: Record<BroSession['state'], string> = {
  needs: '⚑',
  working: '●',
  done: '✓',
  idle: '○',
  exited: '×',
}

const STATE_WORDS: Record<BroSession['state'], string> = {
  needs: 'needs you',
  working: 'working',
  done: 'finished',
  idle: 'idle',
  exited: 'exited',
}

export type DashboardData = {
  env: BroEnv
  sessions: readonly BroSession[]
  limits: readonly BroLimit[]
  isOnline: boolean
  columns: number
  now: number
}

export function dashboard($: EngineInterface, t: Table, d: DashboardData) {
  const { Box, Text } = t
  const who = d.env.profile ?? d.env.backend
  return (
    <Box flexDirection="column" gap={1}>
      <Box flexDirection="row" justifyContent="space-between">
        <Text color="claude" weight="bold" wrap="truncate-end">
          ◆ bro
        </Text>
        {who !== undefined && <Text dimColor>{who}</Text>}
      </Box>
      {d.isOnline ? sessionList($, t, d) : <Text dimColor>bro's bridge isn't answering. Turn it on in bro's bridge view.</Text>}
      {limitList(t, d)}
    </Box>
  )
}

function sessionList($: EngineInterface, t: Table, d: DashboardData) {
  const { Box, Text, Button } = t
  const others = d.sessions.filter(one => !one.isSelf)
  // each session is one pressable line and one dim line of what it's on, cut to the pane
  const room = Math.max(12, d.columns - 2)
  return (
    <Box flexDirection="column">
      <Text weight="semibold">Sessions</Text>
      {others.length === 0 && <Text dimColor>Just this one. Open more from bro's launcher.</Text>}
      {others.map(one => {
        const state = stateWords(one, d.now)
        const title = clip(one.title, Math.max(8, room - state.length - 5))
        return (
          <Box key={`row:${one.id}`} flexDirection="column">
            <Button key={`go:${one.id}`} plain onPress={() => void focus($, d.env, one)}>
              <Text color={stateColor(one.state)}>{GLYPH[one.state]}</Text>
              {` ${title}`}
              <Text dimColor>{`  ${state}`}</Text>
            </Button>
            {one.detail !== undefined && <Text dimColor>{`  ${clip(one.detail, room - 2)}`}</Text>}
          </Box>
        )
      })}
    </Box>
  )
}

function limitList(t: Table, d: DashboardData) {
  const { Box, Text } = t
  if (d.limits.length === 0) {
    return null
  }
  const nameWidth = Math.min(14, Math.max(...d.limits.map(one => one.name.length)))
  const stacked = d.columns < nameWidth + 64
  return (
    <Box flexDirection="column">
      <Text weight="semibold">Limits</Text>
      {d.limits.map(one => (
        <Box key={`limit:${one.id}`} flexDirection={stacked ? 'column' : 'row'} columnGap={3}>
          <Box flexDirection="row" columnGap={1}>
            <Text color={one.isSelf ? 'claude' : undefined} wrap="truncate-end">
              {one.name.padEnd(nameWidth).slice(0, nameWidth)}
            </Text>
            {meter(t, d, '5h', one.fiveHour)}
          </Box>
          <Box flexDirection="row" columnGap={1}>
            {stacked && <Text>{' '.repeat(nameWidth)}</Text>}
            {meter(t, d, 'wk', one.weekly)}
          </Box>
        </Box>
      ))}
    </Box>
  )
}

const BAR = 12

function meter(t: Table, d: DashboardData, label: string, w: BroLimit['fiveHour']) {
  const { Text } = t
  if (w === undefined) {
    return <Text dimColor>{`${label} ${'─'.repeat(BAR)}   —`}</Text>
  }
  const used = Math.max(0, Math.min(100, w.used))
  const filled = Math.round((used / 100) * BAR)
  const color = used >= 85 ? 'error' : used >= 60 ? 'warning' : 'success'
  const reset = w.resetsAt === undefined ? '' : `  ${span(w.resetsAt * 1000 - d.now)}`
  return (
    <Text>
      <Text dimColor>{label} </Text>
      <Text color={color}>{'━'.repeat(filled)}</Text>
      <Text color="subtle">{'─'.repeat(BAR - filled)}</Text>
      {` ${String(Math.round(used)).padStart(3)}%`}
      <Text dimColor>{reset}</Text>
    </Text>
  )
}

/** Claude Code's own colours, by their theme keys. */
function stateColor(state: BroSession['state']): string {
  switch (state) {
    case 'needs':
      return 'warning'
    case 'working':
      return 'claude'
    case 'done':
      return 'success'
    default:
      return 'inactive'
  }
}

/** `needs you 3m`, `working`, `orchestrator · idle 1h`. */
function stateWords(one: BroSession, now: number): string {
  const since = one.since === undefined ? Number.NaN : Date.parse(one.since)
  const age = Number.isFinite(since) && one.state !== 'working' ? ` ${span(now - since)}` : ''
  return `${one.isOrchestrator ? 'orchestrator · ' : ''}${STATE_WORDS[one.state]}${age}`
}

async function focus($: EngineInterface, env: BroEnv, one: BroSession): Promise<void> {
  const done = await ctl($, env, 'focus_session', { sessionId: one.id })
  if (done.isError) {
    $.ui.toast(`bro couldn't bring up ${one.title}: ${done.text}`)
  }
}
