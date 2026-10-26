// The link: `/bro` opens every session the user has in bro, most urgent first
// and a press away from each, with each login's limits, in a pane; `/bro go`
// and `/bro title` jump to a session or rename this one.
import { atom, read } from 'claude-code'
import type { EngineInterface, On } from 'claude-code'

import { call, ctl } from '../shared/bridge'
import { type BroEnv, broEnv } from '../shared/env'
import { dashboard } from './dashboard'
import { firstLine } from './data'
import { PANE, refreshLimits, refreshSessions, startPolling } from './poll'

// What the pane reads; `poll.ts` writes them.
const sessions = atom({ plugin: 'bro', key: 'sessions' } as const, [])
const limits = atom({ plugin: 'bro', key: 'limits' } as const, [])
const isOnline = atom({ plugin: 'bro', key: 'isOnline' } as const, false)

const HELP = [
  '/bro                 open or close the bro pane: your sessions and limits',
  '/bro go <session>    bring another session to the front in bro',
  '/bro title <text>    rename this session in bro',
  "/bro limits          each login's 5-hour and weekly use",
].join('\n')

/** Before the first prompt: the `/bro` command. */
export async function wireLink($: EngineInterface): Promise<void> {
  await $.command.register({
    name: 'bro',
    description: 'bro: your sessions and limits in a pane, jump to a session, rename this one',
    argumentHint: '[go <session> | title <text> | limits]',
  })
}

/** Once the session is up: the pane's polling loop, and the orchestrator's sidebar. */
export async function startLink($: EngineInterface, env: BroEnv): Promise<void> {
  startPolling($, env)
  if (env.isOrchestrator) {
    await refreshSessions($, env)
    await refreshLimits($, env)
    // The engine seats a pane nobody asked for only where the terminal is
    // wide enough to dock it beside the chat.
    void $.ui.open({ id: PANE, title: 'bro' })
  }
}

export function registerLink(on: On): void {
  on('command.run', { command: 'bro' }, async ($, e) => {
    const env = await broEnv($)
    if (env === null) {
      return { text: 'This session was not started by bro, so there is nothing to link to.' }
    }
    const [verb = '', ...rest] = e.args.trim().split(/\s+/)
    const arg = rest.join(' ').trim()
    switch (verb.toLowerCase()) {
      case '':
      case 'pane':
      case 'sessions': {
        const isUp = (await $.ui.panes()).some(pane => pane.id === PANE)
        if (isUp && verb === '') {
          await $.ui.close({ id: PANE })
          return {}
        }
        await refreshSessions($, env)
        await refreshLimits($, env)
        await $.ui.open({ id: PANE, title: 'bro', focus: true })
        return {}
      }
      case 'go':
      case 'focus': {
        if (arg === '') {
          return { text: 'Which session? /bro go <title or id>' }
        }
        const done = await ctl($, env, 'focus_session', { sessionId: arg })
        return {
          text: done.isError ? `bro couldn't find "${arg}": ${firstLine(done.text)}` : `bro brought "${arg}" to the front.`,
        }
      }
      case 'title':
      case 'rename': {
        if (arg === '') {
          return { text: 'What should bro call this session? /bro title <text>' }
        }
        const reply = await call($, env, 'PATCH', `/api/sessions/${encodeURIComponent(env.sessionId)}`, { title: arg })
        return {
          text: reply.ok ? `bro calls this session "${arg}" now.` : `bro didn't rename it (${reply.text || `HTTP ${reply.status}`}).`,
        }
      }
      case 'limits':
      case 'usage': {
        const list = await refreshLimits($, env)
        if (list.length === 0) {
          return { text: 'bro has no Claude login usage to show yet.' }
        }
        const pct = (w?: { used: number }) => (w === undefined ? '—' : `${Math.round(w.used)}%`)
        const rows = list.map(
          one => `${one.isSelf ? '▸' : ' '} ${one.name}: 5h ${pct(one.fiveHour)} · week ${pct(one.weekly)}${one.isExhausted ? ' · out' : ''}`,
        )
        return { text: rows.join('\n') }
      }
      default:
        return { text: HELP }
    }
  })

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    const t = $.ui.resolve(e)
    const env = await broEnv($)
    if (env === null) {
      const { Text } = t
      return <Text dimColor>This session was not started by bro.</Text>
    }
    return dashboard($, t, {
      env,
      sessions: await read($, sessions),
      limits: await read($, limits),
      isOnline: await read($, isOnline),
      columns: e.props.bodyColumns,
      now: await $.clock.now(),
    })
  })

  on('ui.render', { component: 'SessionMode' }, async ($, e, next) => {
    const env = await broEnv($)
    return env?.isOrchestrator === true && !e.props.modes.includes('orchestrator')
      ? next({ ...e, props: { ...e.props, modes: [...e.props.modes, 'orchestrator'] } })
      : next(e)
  })
}
