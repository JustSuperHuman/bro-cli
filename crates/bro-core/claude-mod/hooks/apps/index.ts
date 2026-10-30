// The apps: the MCP Apps (`ui://` widgets) a tool in this session answers
// with, which bro's desktop focus mode draws in the conversation. A widget's
// HTML, and the tools it calls, go through Claude Code's own MCP connections
// (claude.ai connectors, their sign-ins, stdio servers), which only the mod
// can reach. So bro asks and this part does the work: one long poll on bro's
// bridge for the session's life, each job run as it arrives and answered.
//
// A job never puts a permission prompt in front of the person. The jobs run
// from timers, outside any hook of the mod's, so the engine passes them by
// the mod's own hooks (a `tool.check` hook here would never see them). A read
// asks the engine's verdict first, which opens no dialog, and goes ahead only
// on an allow (what Claude Code answers for ReadMcpResourceTool unless a rule
// or another plugin says otherwise). A tool call goes through `$.mcp.call`,
// which never prompts.
import type { EngineInterface, McpToolResult } from 'claude-code'

import { call, parse } from '../shared/bridge'
import type { BroEnv } from '../shared/env'

/** bro didn't answer (it's away, or restarting): ask again after this. */
const RETRY_MS = 5000
/** A poll back empty sooner than this counts as a failed one, so the loop never spins. */
const QUICK_MS = 1000

/** A piece of work bro asks for: `read` a resource, or `call` a tool. */
type Job = { id: string; kind?: unknown; server?: unknown; uri?: unknown; tool?: unknown; arguments?: unknown }

/** What goes back to bro for a job. */
type Answer = { ok: true; result: unknown } | { ok: false; error: string }

/** Once the session is up: ask bro for work, for as long as the session lasts. */
export function startApps($: EngineInterface, env: BroEnv): void {
  const path = `/api/mod/${encodeURIComponent(env.sessionId)}/jobs`
  const loop = (): void => {
    void poll($, env, path)
      .catch(() => RETRY_MS)
      .then(wait => {
        if (wait === 0) {
          loop()
        } else {
          $.clock.after(wait, loop)
        }
      })
  }
  loop()
}

/** One long poll: starts the jobs it brought, and says how long to wait before the next (0, none). */
async function poll($: EngineInterface, env: BroEnv, path: string): Promise<number> {
  const asked = await $.clock.now()
  const reply = await call($, env, 'GET', path)
  const jobs = reply.ok ? jobsIn(parse(reply.text)) : undefined
  if (jobs === undefined) {
    return RETRY_MS
  }
  for (const job of jobs) {
    void run($, job).then(answer => call($, env, 'POST', `${path}/${encodeURIComponent(job.id)}`, answer))
  }
  // Empty and at once: the bridge is going down.
  return jobs.length === 0 && (await $.clock.now()) - asked < QUICK_MS ? RETRY_MS : 0
}

/** Does one job with Claude Code's own MCP connections. Never throws: a failure is the answer. */
export async function run($: EngineInterface, job: Job): Promise<Answer> {
  try {
    if (job.kind === 'read' && typeof job.server === 'string' && typeof job.uri === 'string') {
      return await read($, job.server, job.uri)
    }
    if (job.kind === 'call' && typeof job.server === 'string' && typeof job.tool === 'string') {
      const args = isRecord(job.arguments) ? job.arguments : undefined
      const result: McpToolResult = await $.mcp.call(job.server, job.tool, args)
      return { ok: true, result }
    }
    return { ok: false, error: 'unknown job' }
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : String(error) }
  }
}

/** Reads a resource with ReadMcpResourceTool, when Claude Code would let it run without asking anyone. */
async function read($: EngineInterface, server: string, uri: string): Promise<Answer> {
  const verdict = await $.tool.check({ tool: 'ReadMcpResourceTool', input: { server, uri } })
  if (verdict.decision !== 'allow') {
    const why = verdict.reason === undefined ? '' : ` (${verdict.reason})`
    return {
      ok: false,
      error:
        verdict.decision === 'deny'
          ? `Claude Code refuses to read ${uri}${why}`
          : `Claude Code would ask before reading ${uri}${why}, and the bro mod doesn't put prompts in front of you`,
    }
  }
  const done = await $.tool.call({ tool: 'ReadMcpResourceTool', server, uri })
  if (done.deny !== undefined) {
    return { ok: false, error: done.deny }
  }
  if (done.isError === true) {
    return { ok: false, error: done.text ?? `Claude Code couldn't read ${uri}` }
  }
  if (done.result.error !== undefined) {
    return { ok: false, error: done.result.error }
  }
  return { ok: true, result: { contents: done.result.contents } }
}

/** The jobs in a poll's answer; undefined when it isn't one. */
function jobsIn(body: unknown): Job[] | undefined {
  const jobs = isRecord(body) ? body.jobs : undefined
  if (!Array.isArray(jobs)) {
    return undefined
  }
  return jobs.filter((job): job is Job => isRecord(job) && typeof job.id === 'string')
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
