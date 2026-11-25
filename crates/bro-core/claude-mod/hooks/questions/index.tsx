// The questions: when Claude asks you something (AskUserQuestion), bro chimes,
// and with the timer on, a countdown runs above the question and ticks for its
// last fifteen seconds; at zero the question closes and Claude goes with its
// best judgement. Start answering and the countdown stops: it runs again only
// once you've left the question a minute. bro plays the sounds itself (`bro
// hook sound`), so they're the same on every OS, and bro says when you last
// typed into the session, since the engine's dialog tells hooks nothing of it.
import { atom, read, update } from 'claude-code'
import type { EngineInterface, On, PluginOptions } from 'claude-code'

import type { BroQuestion } from '../../types'
import { call, parse } from '../shared/bridge'
import type { BroEnv } from '../shared/env'
import { broEnv } from '../shared/env'

const question = atom({ plugin: 'bro', key: 'question' } as const, null)

/** The countdown ticks for this many seconds before the question closes. */
const TICKS_FROM = 15
/** bro is started this much ahead of the first tick, and waits for it itself. */
const TICKS_LEAD_MS = 2000
/** Cells in the countdown's bar. */
const BAR = 16
/** Once you've touched the question, the countdown waits this long without a key before it runs again. */
const IDLE_MS = 60_000

export function registerQuestions(on: On, options: PluginOptions): void {
  const total = typeof options.timer === 'number' && options.timer > 0 ? Math.round(options.timer) : 0
  const hasChime = options.chime !== false
  const hasTicks = options.ticks !== false
  if (total === 0 && !hasChime) {
    return
  }

  on('tool.call', { tool: 'AskUserQuestion' }, async ($, e, next) => {
    const env = await broEnv($)
    if (env === null) {
      return next(e)
    }
    if (hasChime && env.bin !== undefined) {
      void $.process.run([env.bin, 'hook', 'sound', 'question'], { timeoutMs: 10_000 }).catch(() => {})
    }
    if (total === 0) {
      return next(e)
    }

    const id = e.tool_use_id
    const opened = await $.clock.now()
    let close = () => {}
    const closed = new Promise<null>(resolve => {
      close = () => resolve(null)
    })
    const typing = keys($, env, opened)

    // The countdown's clock: when it closes the question, or null while it's stopped for you.
    let closesAt: number | null = null
    let left = total
    let timers: { cancel: () => void }[] = []
    let stopTicks: (() => void) | undefined
    const disarm = () => {
      timers.forEach(timer => timer.cancel())
      timers = []
      stopTicks?.()
      stopTicks = undefined
    }
    const arm = (now: number, secs: number) => {
      const at = now + secs * 1000
      closesAt = at
      left = secs
      timers = [
        $.clock.after(secs * 1000, () => void closing().catch(close)),
        $.clock.after(Math.max(0, (secs - TICKS_FROM) * 1000 - TICKS_LEAD_MS), () => {
          if (hasTicks && env.bin !== undefined && closesAt === at) {
            stopTicks = ticks($, env.bin, at, Math.min(TICKS_FROM, secs))
          }
        }),
      ]
    }
    const show = () => update($, question, q => (q?.id === id ? { ...q, left, isPaused: closesAt === null } : q))

    // Each second: you touched the question, so the countdown stops; you left it a
    // minute, so it runs again, from where it stopped with at least the ticks to go.
    const step = async () => {
      const now = await $.clock.now()
      const answering = typing.isAnswering(now, await typing.heard())
      if (answering && closesAt !== null) {
        // Rounded, not floored: a timer that fires a little late still shows the
        // second it was meant for.
        left = Math.max(1, Math.round((closesAt - now) / 1000))
        closesAt = null
        disarm()
      } else if (!answering && closesAt === null) {
        arm(now, Math.min(total, Math.max(left, TICKS_FROM)))
      } else if (closesAt !== null) {
        left = Math.max(0, Math.round((closesAt - now) / 1000))
      }
      await show()
    }
    // At zero, one last look: a key in the last second still stops it.
    const closing = async () => {
      if (typing.isAnswering(await $.clock.now(), await typing.heard())) {
        await step()
      } else {
        close()
      }
    }
    let isStepping = false
    const counting = $.clock.every(1000, () => {
      if (isStepping) {
        return
      }
      isStepping = true
      void step()
        .catch(() => {})
        .finally(() => {
          isStepping = false
        })
    })

    await update($, question, () => ({ id, left: total, total, isPaused: false }))
    arm(opened, total)
    next.signal.addEventListener('abort', close, { once: true })

    const answered = next(e)
    // Once the clock wins, the dialog beneath is abandoned and may reject.
    answered.catch(() => {})
    try {
      const result = await Promise.race([answered, closed])
      if (result !== null) {
        return result
      }
      const now = await $.clock.now()
      const touched = typing.touched()
      return {
        result: { questions: e.questions, answers: {}, afkTimeoutMs: touched === null ? total * 1000 : Math.max(1, Math.round(now - touched)) },
        context: [
          (touched === null
            ? `The user didn't answer within ${spoken(total)}, so bro's question timer closed the question. `
            : "The user started on the question but left it for over a minute without finishing, so bro's " +
              'question timer closed it. ') +
            'Go with your best judgement on each question and carry on rather than asking again; ' +
            'say in a line which way you went so they can correct it.',
        ],
      }
    } finally {
      counting.cancel()
      disarm()
      await update($, question, q => (q?.id === id ? null : q))
    }
  }).catch(($, e, next) =>
    // The timer failed: the question waits for the person as it would without it.
    next(e),
  )

  if (total === 0) {
    return
  }

  // The countdown, above the question: the engine's dialog takes nothing below it.
  on('ui.render', { component: 'AskUserQuestion' }, async ($, e, next) => {
    const q: BroQuestion | null = await read($, question)
    if (q === null || q.id !== e.requestId) {
      return next(e)
    }
    const { Box, Text } = $.ui.resolve(e)
    const filled = Math.round((q.left / q.total) * BAR)
    if (q.isPaused === true) {
      return (
        <Box flexDirection="column">
          <Text>
            <Text color="inactive">{'━'.repeat(filled)}</Text>
            <Text color="subtle">{'─'.repeat(BAR - filled)}</Text>
            <Text color="subtle"> {clock(q.left)}</Text>
            <Text dimColor> paused while you answer · runs again after a minute idle</Text>
          </Text>
          {await next(e)}
        </Box>
      )
    }
    const isEnding = q.left <= 5
    // Claude Code's own colours, by their theme keys
    const color = isEnding ? 'warning' : 'claude'
    return (
      <Box flexDirection="column">
        <Text>
          <Text color={color}>{'━'.repeat(filled)}</Text>
          <Text color="subtle">{'─'.repeat(BAR - filled)}</Text>
          <Text color={color} weight={isEnding ? 'bold' : 'regular'}>
            {' '}
            {clock(q.left)}
          </Text>
          <Text dimColor> then Claude goes with its best judgement</Text>
        </Text>
        {await next(e)}
      </Box>
    )
  })
}

/**
 * Your keys on this question, as bro hears them: bro sees everything you type
 * into the session (keys, a paste, the phone) and says how long ago the last
 * was. A bro without that word, or without a bridge, never stops the countdown.
 */
function keys($: EngineInterface, env: BroEnv, opened: number) {
  const path = `/api/mod/${encodeURIComponent(env.sessionId)}/typed`
  let canHear = env.bridge !== undefined
  let last: number | null = null
  return {
    /** Asks bro; answers when you last typed since the question opened (mod clock), or null. */
    async heard(): Promise<number | null> {
      if (!canHear) {
        return last
      }
      const asked = await $.clock.now()
      const reply = await call($, env, 'GET', path)
      const body = reply.ok ? parse(reply.text) : undefined
      const said = typeof body === 'object' && body !== null && 'idleMs' in body ? (body as { idleMs: unknown }) : undefined
      if (reply.status === 404 || (reply.ok && said === undefined)) {
        // an older bro, or one that doesn't have this session: it can't say
        canHear = false
      }
      const idle = said?.idleMs
      if (typeof idle === 'number' && Number.isFinite(idle)) {
        const at = asked - idle
        if (at >= opened && (last === null || at > last)) {
          last = at
        }
      }
      return last
    },
    /** You touched the question less than a minute ago. */
    isAnswering(now: number, at: number | null): boolean {
      return at !== null && now - at < IDLE_MS
    },
    /** When you last touched it, if ever. */
    touched: (): number | null => last,
  }
}

/**
 * The countdown's last `secs` ticks, from a bro of their own that times them
 * to `closesAt` (epoch ms); answers the function that stops them.
 */
function ticks($: EngineInterface, bin: string, closesAt: number, secs: number): () => void {
  const player = $.process.spawn({ argv: [bin, 'hook', 'sound', 'ticks', String(Math.round(closesAt)), String(secs)] })
  void (async () => {
    try {
      for await (const _ of player) {
        // it writes nothing: the loop is the player's life
      }
    } catch {
      // no player: the countdown goes on silently
    }
  })()
  // Ending the stream kills the player, mid-wait or mid-tick.
  return () => void player.return({ code: null, signal: null }).catch(() => {})
}

/** `23s`, or `1:05` from a minute up. */
function clock(left: number): string {
  return left < 60 ? `${left}s` : `${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')}`
}

/** `30 seconds`, `2 minutes`, `1 minute 30 seconds`. */
function spoken(secs: number): string {
  const m = Math.floor(secs / 60)
  const s = secs % 60
  const part = (n: number, unit: string) => `${n} ${unit}${n === 1 ? '' : 's'}`
  return [m > 0 ? part(m, 'minute') : '', s > 0 || m === 0 ? part(s, 'second') : ''].filter(Boolean).join(' ')
}
