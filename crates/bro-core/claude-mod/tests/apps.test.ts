// The apps (hooks/apps): bro asks for MCP work, the mod does it with Claude
// Code's own connections and answers. Run with
// `claude plugin test crates/bro-core/claude-mod`.
import { expect, mock, test } from 'claude-code/testing'
import type { On } from 'claude-code'

const BRIDGE = 'http://127.0.0.1:10001'
const JOBS = `${BRIDGE}/api/mod/sid-1/jobs`
const URI = 'ui://widget/workout-v84.html'
const HTML = '<!doctype html><html><body>Leg day</body></html>'
const MCP_APP = 'text/html;profile=mcp-app'
const READ = { id: 'j1', kind: 'read', server: 'claude_ai_JustGains', uri: URI }

type Clock = ReturnType<typeof mock.clock>

/**
 * bro's bridge as the mod sees it: a poll takes what's queued, or waits out
 * the long poll; answers are kept. Away, it refuses the connection; closing,
 * it answers every poll empty at once.
 */
function bridge(on: On, clock: Clock) {
  const bro = {
    queued: [] as object[],
    polls: 0,
    isAway: false,
    isClosing: false,
    answers: [] as { id: string; body: unknown }[],
  }
  const reply = (status: number, body: unknown) => ({
    value: { status, ok: status < 300, headers: {}, text: JSON.stringify(body) },
  })
  on('http.fetch', async (_, e) => {
    const method = e.init?.method ?? 'GET'
    if (method === 'GET' && e.url === JOBS) {
      bro.polls += 1
      if (bro.isAway) {
        return { deny: 'connection refused' }
      }
      if (bro.queued.length === 0 && !bro.isClosing) {
        await clock.sleep(25_000)
      }
      const jobs = bro.queued
      bro.queued = []
      return reply(200, { jobs })
    }
    if (method === 'POST' && e.url.startsWith(`${JOBS}/`)) {
      bro.answers.push({ id: decodeURIComponent(e.url.slice(JOBS.length + 1)), body: JSON.parse(e.init?.body ?? 'null') })
      return reply(200, { ok: true })
    }
    return reply(404, { message: 'not here' })
  })
  return bro
}

/**
 * A Claude session bro started, with the engine beneath the mod and bro's
 * bridge behind it. The engine allows a resource read, as Claude Code does
 * for ReadMcpResourceTool unless a rule says otherwise; `verdict` changes that.
 */
function world(on: On, env: Record<string, string> = { BRO_SESSION_ID: 'sid-1', BRO_BRIDGE_URL: BRIDGE }) {
  const clock = mock.clock(on)
  mock.env(on, env)
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('command.register', () => ({ value: { command: 'bro' } }))
  on('ui.panes', () => ({ value: [] }))
  const engine = { verdict: { decision: 'allow' } as { decision: 'allow' | 'ask' | 'deny'; reason?: string }, reads: [] as string[] }
  on('tool.check', { tool: 'ReadMcpResourceTool' }, () => engine.verdict)
  on('tool.call', { tool: 'ReadMcpResourceTool' }, (_, e) => {
    engine.reads.push(e.uri)
    return e.uri === URI
      ? { result: { contents: [{ uri: e.uri, mimeType: MCP_APP, text: HTML }] } }
      : { result: { contents: [], error: 'Resource not found' } }
  })
  return { clock, engine, bro: bridge(on, clock) }
}

const START = { cwd: '/', surface: 'terminal', isInteractive: true } as const

test('a read goes through ReadMcpResourceTool and its contents go back to bro', async ($, on) => {
  const { clock, engine, bro } = world(on)
  bro.queued.push(READ)

  await $.session.start(START)
  await clock.settle()

  expect(engine.reads).toEqual([URI])
  expect(bro.answers).toEqual([
    { id: 'j1', body: { ok: true, result: { contents: [{ uri: URI, mimeType: MCP_APP, text: HTML }] } } },
  ])
  // it took the job and asked for more at once
  expect(bro.polls).toBe(2)
})

test('where Claude Code would ask first, it answers why and never prompts', async ($, on) => {
  const { clock, engine, bro } = world(on)
  engine.verdict = { decision: 'ask', reason: 'a rule asks' }
  bro.queued.push(READ, { ...READ, id: 'j2' })

  await $.session.start(START)
  await clock.settle()
  engine.verdict = { decision: 'deny', reason: 'a rule denies' }
  bro.queued.push({ ...READ, id: 'j3' })
  await clock.advance(25_000)

  expect(engine.reads).toEqual([])
  const errors = bro.answers.map(answer => (answer.body as { ok: boolean; error: string }).error)
  expect(errors[0]).toMatch(/would ask before reading ui:\/\/widget\/workout-v84\.html \(a rule asks\)/)
  expect(errors[2]).toMatch(/refuses to read .* \(a rule denies\)/)
  expect(bro.answers.every(answer => (answer.body as { ok: boolean }).ok === false)).toBe(true)
})

test("a widget's tool call goes to its server through $.mcp.call, the result as MCP gave it", async ($, on) => {
  const { clock, bro } = world(on)
  const calls: unknown[] = []
  const result = { content: [{ type: 'text', text: 'Leg day, 5 exercises' }], isError: false, structuredContent: { workoutId: 7 } }
  on('mcp.call', (_, e) => {
    calls.push(e)
    return { value: result }
  })
  bro.queued.push({ id: 'j2', kind: 'call', server: 'claude_ai_JustGains', tool: 'show_workout', arguments: { id: 7 } })

  await $.session.start(START)
  await clock.settle()

  expect(calls).toEqual([{ server: 'claude_ai_JustGains', tool: 'show_workout', args: { id: 7 } }])
  expect(bro.answers).toEqual([{ id: 'j2', body: { ok: true, result } }])
})

test("a job it can't do is answered with why", async ($, on) => {
  const { clock, bro } = world(on)
  on('mcp.call', () => ({ deny: 'claude_ai_Gone is not connected' }))
  bro.queued.push(
    { id: 'a', kind: 'paint' },
    { id: 'b', kind: 'read', server: 'claude_ai_JustGains', uri: 'ui://widget/missing.html' },
    { id: 'c', kind: 'call', server: 'claude_ai_Gone', tool: 'show' },
    { id: 'd', kind: 'read', server: 'claude_ai_JustGains' },
  )

  await $.session.start(START)
  await clock.settle()

  const answered = Object.fromEntries(bro.answers.map(answer => [answer.id, answer.body as { ok: boolean; error?: string }]))
  expect(answered.a).toEqual({ ok: false, error: 'unknown job' })
  expect(answered.b).toEqual({ ok: false, error: 'Resource not found' })
  expect(answered.c?.ok).toBe(false)
  expect(answered.c?.error).toMatch(/not connected/)
  expect(answered.d).toEqual({ ok: false, error: 'unknown job' })
})

test('while bro is away it asks again every five seconds, and picks up where it left off', async ($, on) => {
  const { clock, bro } = world(on)
  bro.isAway = true

  await $.session.start(START)
  await clock.settle()
  expect(bro.polls).toBe(1)
  await clock.advance(4_999)
  expect(bro.polls).toBe(1)
  await clock.advance(1)
  expect(bro.polls).toBe(2)

  bro.isAway = false
  bro.queued.push({ id: 'back', kind: 'paint' })
  await clock.advance(5_000)
  // one poll took the job, the next waits for more
  expect(bro.polls).toBe(4)
  expect(bro.answers.map(answer => answer.id)).toEqual(['back'])
})

test('an empty answer straight back (bro closing) is not asked again at once', async ($, on) => {
  const { clock, bro } = world(on)
  bro.isClosing = true

  await $.session.start(START)
  await clock.settle()
  expect(bro.polls).toBe(1)
  await clock.advance(5_000)
  expect(bro.polls).toBe(2)
})

test('without a bridge to ask, it never polls', async ($, on) => {
  const { clock, bro } = world(on, { BRO_SESSION_ID: 'sid-1' })

  await $.session.start(START)
  await clock.advance(60_000)

  expect(bro.polls).toBe(0)
})
