// Tools Claude calls to reach the user through bro. They answer from bro's
// bridge on loopback; none of them changes the user's code.
import type { EngineInterface, ToolSpec } from 'claude-code'

import { call, ctl, getJson } from '../shared/bridge'
import { type BroEnv, join } from '../shared/env'
import { toSessions } from './data'

export const SHOW_FILE = 'mcp__bro__show_file'
export const NOTIFY = 'mcp__bro__notify_user'
export const SESSIONS = 'mcp__bro__bro_sessions'

export const SPECS: Record<'show_file' | 'notify_user' | 'bro_sessions', ToolSpec> = {
  show_file: {
    name: 'show_file',
    description:
      "Put a file in front of the user in bro's file preview (on their desktop, the web and their phone), scrolled to a line when you give one. Use it when you point them at something specific: a change to review, the answer to \"where is X\". At most once per reply. Prefer it to running `bro open`.",
    inputSchema: {
      type: 'object',
      properties: {
        path: { type: 'string', description: 'The file: absolute, or relative to the working directory.' },
        line: { type: 'integer', minimum: 1, description: 'The 1-based line to scroll to.' },
      },
      required: ['path'],
    },
    isDeferred: false,
  },
  notify_user: {
    name: 'notify_user',
    description:
      "Send the user a notification through bro: a toast on their desktop and a push in the bro app on their phone. Only for something they need to know or act on while away from the screen (a long job finished, you are blocked on them). Never for routine progress.",
    inputSchema: {
      type: 'object',
      properties: {
        title: { type: 'string', description: 'A few words: what happened.' },
        body: { type: 'string', description: 'One sentence of detail.' },
      },
      required: ['title'],
    },
  },
  bro_sessions: {
    name: 'bro_sessions',
    description:
      "The user's other agent sessions in bro. With no `session`: one line per session (title, project, state, what it waits on or last did). With `session` (a title, a prefix of one, or an id): the last lines of that session's screen. Use it when your work depends on another session's, e.g. before touching files it is editing.",
    inputSchema: {
      type: 'object',
      properties: {
        session: { type: 'string', description: "Which session to read: its title, a prefix of it, or its id." },
        lines: { type: 'integer', minimum: 10, maximum: 400, description: 'How many screen lines to read (default 80).' },
      },
    },
  },
}

export type Answer = { text: string; isError: boolean }

export async function showFile($: EngineInterface, env: BroEnv, input: Record<string, unknown>): Promise<Answer> {
  const raw = typeof input.path === 'string' ? input.path.trim() : ''
  if (raw === '') {
    return { text: 'show_file needs a path.', isError: true }
  }
  const path = isAbsolute(raw) ? raw : join(await $.session.cwd(), raw)
  const line = typeof input.line === 'number' && input.line >= 1 ? Math.floor(input.line) : undefined
  const reply = await call($, env, 'POST', `/api/sessions/${encodeURIComponent(env.sessionId)}/open`, {
    path,
    line,
    source: 'agent',
  })
  const where = line === undefined ? raw : `${raw}:${line}`
  if (reply.ok) {
    return { text: `Showing ${where} in bro's preview.`, isError: false }
  }
  if (reply.status === 404) {
    return {
      text: `bro can't show ${where}: it must be an existing file inside this session's folder or project. Mention the path instead.`,
      isError: true,
    }
  }
  return { text: `bro didn't open ${where} (${reply.text || `HTTP ${reply.status}`}). Mention the path instead.`, isError: true }
}

export async function notifyUser($: EngineInterface, env: BroEnv, input: Record<string, unknown>): Promise<Answer> {
  const title = typeof input.title === 'string' ? input.title.trim() : ''
  if (title === '') {
    return { text: 'notify_user needs a title.', isError: true }
  }
  const body = typeof input.body === 'string' ? input.body.trim() : undefined
  const reply = await call($, env, 'POST', '/api/notify', { title, body, sound: 'done', sessionId: env.sessionId })
  return reply.ok
    ? { text: 'Sent: the user sees it on their desktop and phone.', isError: false }
    : { text: `bro couldn't send it (${reply.text || `HTTP ${reply.status}`}).`, isError: true }
}

export async function broSessions($: EngineInterface, env: BroEnv, input: Record<string, unknown>): Promise<Answer> {
  const target = typeof input.session === 'string' ? input.session.trim() : ''
  if (target !== '') {
    const lines = typeof input.lines === 'number' ? Math.min(400, Math.max(10, Math.floor(input.lines))) : 80
    return ctl($, env, 'read_session', { sessionId: target, lines })
  }
  const rows = await getJson($, env, '/api/sessions')
  if (rows === undefined) {
    return { text: "bro's bridge isn't answering.", isError: true }
  }
  const list = toSessions(rows, env.sessionId).filter(one => !one.isSelf)
  if (list.length === 0) {
    return { text: 'No other agent sessions are open in bro.', isError: false }
  }
  const words = { needs: 'waiting on the user', working: 'working', done: 'finished', idle: 'idle', exited: 'exited' }
  const text = list
    .map(one => {
      const role = one.isOrchestrator ? ' (orchestrator)' : ''
      const detail = one.detail === undefined ? '' : `: ${one.detail}`
      return `- ${one.title}${role} [${one.project}] ${words[one.state]}${detail} (id ${one.id})`
    })
    .join('\n')
  return { text, isError: false }
}

function isAbsolute(path: string): boolean {
  return /^([A-Za-z]:[\\/]|[\\/]{1,2})/.test(path)
}
