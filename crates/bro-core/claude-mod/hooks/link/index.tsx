// The link: Claude Code knows it runs in bro, and so does the person at the
// prompt. The hint line carries the login and its limits, a status line flags
// sessions that wait on them, `/bro` opens every session and limit in a pane,
// and Claude gets tools to show files, ping the phone and check other sessions.
import { atom, read } from 'claude-code'
import type { EngineInterface, On, PluginOptions } from 'claude-code'

import { call, ctl } from '../shared/bridge'
import { type BroEnv, broEnv } from '../shared/env'
import { ink } from '../shared/look'
import { dashboard } from './dashboard'
import { firstLine } from './data'
import { PANE, refreshLimits, refreshSessions, startPolling } from './poll'
import { type Answer, NOTIFY, SESSIONS, SHOW_FILE, SPECS, broSessions, notifyUser, showFile } from './tools'

// What the drawings here read; `poll.ts` and `look.ts` write them.
const look = atom({ plugin: 'bro', key: 'look' } as const, null)
const hud = atom({ plugin: 'bro', key: 'hud' } as const, null)
const sessions = atom({ plugin: 'bro', key: 'sessions' } as const, [])
const limits = atom({ plugin: 'bro', key: 'limits' } as const, [])
const isOnline = atom({ plugin: 'bro', key: 'isOnline' } as const, false)

const HELP = [
  '/bro                 open or close the bro pane: your sessions and limits',
  '/bro go <session>    bring another session to the front in bro',
  '/bro title <text>    rename this session in bro',
  "/bro limits          each login's 5-hour and weekly use",
].join('\n')

/** The tools this session registered (the orchestrator gets fewer). */
let registered: string[] = []

/** Before the first prompt: the `/bro` command and Claude's tools. */
export async function wireLink($: EngineInterface, env: BroEnv, options: PluginOptions): Promise<void> {
  await $.command.register({
    name: 'bro',
    description: 'bro: your sessions and limits in a pane, jump to a session, rename this one',
    argumentHint: '[go <session> | title <text> | limits]',
  })
  registered = []
  if (options.tools === false) {
    return
  }
  // The orchestrator's `bro` MCP server already serves notify_user and the
  // session tools under these very names.
  const specs = env.isOrchestrator ? [SPECS.show_file] : [SPECS.show_file, SPECS.notify_user, SPECS.bro_sessions]
  for (const spec of specs) {
    const { tool } = await $.tool.register(spec)
    registered.push(tool)
  }
}

/** Once the session is up: the polling loop, and the orchestrator's sidebar. */
export async function startLink($: EngineInterface, env: BroEnv, options: PluginOptions): Promise<void> {
  await startPolling($, env, { radar: options.radar !== false, hud: options.hud !== false })
  if (env.isOrchestrator) {
    // The engine seats a pane nobody asked for only where the terminal is
    // wide enough to dock it beside the chat.
    void $.ui.open({ id: PANE, title: 'bro' })
  }
}

const NOT_BRO = { deny: 'This session was not started by bro.' }

/** One of bro's tools' answers, as the model reads it: its text, or an error result with the reason. */
function reply(answer: Answer) {
  return answer.isError ? { deny: answer.text } : { result: answer.text }
}

const FAILED = reply({ text: "bro's tool failed before bro answered. Carry on without it.", isError: true })

/** What the model reads about bro: fixed for the session, so the prompt cache holds. */
function context(env: BroEnv, tools: readonly string[]): string | undefined {
  const lines: string[] = []
  if (tools.includes(SHOW_FILE)) {
    lines.push(
      `- ${SHOW_FILE} puts a file, optionally at a line, in front of the user in bro's preview. Use it when you point them at something specific; at most once per reply.`,
    )
  }
  if (tools.includes(NOTIFY)) {
    lines.push(`- ${NOTIFY} pings their desktop and phone. Only when they must act or know while away.`)
  }
  if (tools.includes(SESSIONS)) {
    lines.push(`- ${SESSIONS} lists the user's other agent sessions in bro, or reads one, when your work depends on theirs.`)
  }
  if (lines.length === 0) {
    return undefined
  }
  const role = env.isOrchestrator
    ? "You are bro's orchestrator."
    : 'Other agents may be working beside you in other bro sessions.'
  return [
    `You are running inside bro, the user's workspace for coding agents: their sessions side by side, on desktop, web and phone. ${role}`,
    ...lines,
  ].join('\n')
}

export function registerLink(on: On, options: PluginOptions): void {
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
      look: await read($, look),
      sessions: await read($, sessions),
      limits: await read($, limits),
      isOnline: await read($, isOnline),
      columns: e.props.bodyColumns,
      now: await $.clock.now(),
    })
  })

  if (options.hud !== false) {
    on('ui.render', { component: 'PromptHint' }, async ($, e, next) => {
      const tail = (await broEnv($)) === null ? null : await read($, hud)
      return tail === null || e.props.isDraft ? next(e) : next({ ...e, props: { ...e.props, tail } })
    })
  }

  on('ui.render', { component: 'SessionMode' }, async ($, e, next) => {
    const env = await broEnv($)
    return env?.isOrchestrator === true && !e.props.modes.includes('orchestrator')
      ? next({ ...e, props: { ...e.props, modes: [...e.props.modes, 'orchestrator'] } })
      : next(e)
  })

  if (options.tools === false) {
    return
  }

  on('prompt.compose', async ($, e, next) => {
    const composed = await next(e)
    const env = await broEnv($)
    const text = env === null ? undefined : context(env, e.tools)
    return text === undefined
      ? composed
      : { sections: [...composed.sections, { id: 'bro:context', text, scope: 'session' as const }] }
  })

  // bro's own tools change nothing on disk: no prompt for them in any mode.
  on('tool.check', { tool: [SHOW_FILE, NOTIFY, SESSIONS] }, async ($, e, next) => {
    return registered.includes(e.tool) ? { decision: 'allow' as const, reason: "bro's own tool" } : next(e)
  })

  on('tool.call', { tool: SHOW_FILE }, async ($, e) => {
    const env = await broEnv($)
    return env === null ? NOT_BRO : reply(await showFile($, env, e))
  }).catch(() => FAILED)
  on('tool.call', { tool: NOTIFY }, async ($, e) => {
    const env = await broEnv($)
    return env === null ? NOT_BRO : reply(await notifyUser($, env, e))
  }).catch(() => FAILED)
  on('tool.call', { tool: SESSIONS }, async ($, e) => {
    const env = await broEnv($)
    return env === null ? NOT_BRO : reply(await broSessions($, env, e))
  }).catch(() => FAILED)

  // The engine's row for an MCP call reads `bro - show_file (MCP)(path: …)`;
  // these say what happened, in the theme's colours.
  on('ui.render', { component: 'ToolUse', props: { tool: [SHOW_FILE, NOTIFY, SESSIONS] } }, async ($, e, next) => {
    if (e.props.isExpanded === true || e.props.isErrored || e.props.isInterrupted) {
      return next(e)
    }
    const { Box, Text } = $.ui.resolve(e)
    const theme = await read($, look)
    const input = (e.props.input ?? {}) as Record<string, unknown>
    const str = (key: string) => (typeof input[key] === 'string' ? (input[key] as string) : '')
    let verb: string
    let what: string
    if (e.props.tool === SHOW_FILE) {
      verb = e.props.isRunning ? 'Showing' : 'Showed'
      what = typeof input.line === 'number' ? `${str('path')}:${input.line}` : str('path')
    } else if (e.props.tool === NOTIFY) {
      verb = e.props.isRunning ? 'Pinging you' : 'Pinged you'
      what = `"${str('title')}"`
    } else {
      verb = str('session') === '' ? 'Checked your bro sessions' : 'Read'
      what = str('session')
    }
    return (
      <Box flexDirection="row">
        <Text wrap="truncate-end">
          <Text color={ink(theme, 'accent')}>{theme?.marker ?? '◆'} </Text>
          <Text weight="semibold">{verb}</Text>
          {what === '' ? '' : ` ${what}`}
          {e.props.tool === SHOW_FILE ? <Text dimColor> in bro</Text> : ''}
        </Text>
      </Box>
    )
  })
}
