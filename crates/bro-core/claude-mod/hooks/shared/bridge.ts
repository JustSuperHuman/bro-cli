// bro's bridge: the loopback HTTP server inside bro behind `bro ctl`, `bro open`
// and the phone app. Loopback callers need no token.
import type { EngineInterface } from 'claude-code'

import type { BroEnv } from './env'

const JSON_HEADERS = { 'content-type': 'application/json', accept: 'application/json' }

export type Reply = { ok: boolean; status: number; text: string }

/**
 * One request to the bridge of the bro that launched this session. With none
 * (its bridge was off, or a second bro opened beside a running one) the link
 * stays quiet: another bro's bridge would answer for sessions that aren't these.
 */
export async function call(
  $: EngineInterface,
  env: BroEnv,
  method: string,
  path: string,
  body?: unknown,
): Promise<Reply> {
  const base = env.bridge
  if (base === undefined) {
    return { ok: false, status: 0, text: "this session's bro has no bridge running" }
  }
  try {
    const response = await $.http.fetch(`${base}${path}`, {
      method,
      headers: JSON_HEADERS,
      body: body === undefined ? undefined : JSON.stringify(body),
    })
    return { ok: response.ok, status: response.status, text: response.text }
  } catch (error) {
    return { ok: false, status: 0, text: `bro isn't reachable (${String(error)})` }
  }
}

/** GET a JSON document; undefined when bro is away or answers anything else. */
export async function getJson($: EngineInterface, env: BroEnv, path: string): Promise<unknown> {
  const reply = await call($, env, 'GET', path)
  return reply.ok ? parse(reply.text) : undefined
}

/** One of the bridge's MCP tools (what `bro ctl <tool>` runs), its text joined. */
export async function ctl(
  $: EngineInterface,
  env: BroEnv,
  tool: string,
  args: Record<string, unknown>,
): Promise<{ text: string; isError: boolean }> {
  const reply = await call($, env, 'POST', '/mcp', {
    jsonrpc: '2.0',
    id: 1,
    method: 'tools/call',
    params: { name: tool, arguments: args },
  })
  if (!reply.ok) {
    return { text: reply.text || `bro answered HTTP ${reply.status}`, isError: true }
  }
  const message = parse(reply.text) as
    | { result?: { content?: { text?: unknown }[]; isError?: boolean }; error?: { message?: unknown } }
    | undefined
  if (message?.error !== undefined) {
    return { text: String(message.error.message ?? 'bro refused the call'), isError: true }
  }
  const text = (message?.result?.content ?? [])
    .map(block => (typeof block.text === 'string' ? block.text : ''))
    .join('\n')
  return { text, isError: message?.result?.isError === true }
}

export function parse(text: string): unknown {
  try {
    return JSON.parse(text)
  } catch {
    return undefined
  }
}
