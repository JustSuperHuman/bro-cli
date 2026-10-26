// The questions: when Claude asks you something (AskUserQuestion), bro chimes,
// and with the timer on, a countdown runs above the question and ticks for its
// last fifteen seconds; at zero the question closes and Claude goes with its
// best judgement. bro plays the sounds itself (`bro hook sound`), so they're the
// same on every OS.
import { atom, read, update } from 'claude-code'
import type { EngineInterface, On, PluginOptions } from 'claude-code'

import type { BroQuestion } from '../../types'
import { broEnv } from '../shared/env'

const question = atom({ plugin: 'bro', key: 'question' } as const, null)

/** The countdown ticks for this many seconds before the question closes. */
const TICKS_FROM = 15
/** bro is started this much ahead of the first tick, and waits for it itself. */
const TICKS_LEAD_MS = 2000
/** Cells in the countdown's bar. */
const BAR = 16

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
    const closesAt = (await $.clock.now()) + total * 1000
    let close = () => {}
    const closed = new Promise<null>(resolve => {
      close = () => resolve(null)
    })
    let stopTicks: (() => void) | undefined
    const startTicks = () => {
      if (hasTicks && env.bin !== undefined) {
        stopTicks = ticks($, env.bin, closesAt, Math.min(TICKS_FROM, total))
      }
    }

    await update($, question, () => ({ id, left: total, total }))
    // Rounded, not floored: a timer that fires a little late still shows the
    // second it was meant for.
    const tick = async () => {
      const left = Math.max(0, Math.round((closesAt - (await $.clock.now())) / 1000))
      await update($, question, q => (q?.id === id ? { ...q, left } : q))
    }
    const counting = $.clock.every(1000, () => void tick().catch(() => {}))
    const deadline = $.clock.after(total * 1000, close)
    const ticking = $.clock.after(Math.max(0, (total - TICKS_FROM) * 1000 - TICKS_LEAD_MS), startTicks)
    next.signal.addEventListener('abort', close, { once: true })

    const answered = next(e)
    // Once the clock wins, the dialog beneath is abandoned and may reject.
    answered.catch(() => {})
    try {
      const result = await Promise.race([answered, closed])
      if (result !== null) {
        return result
      }
      return {
        result: { questions: e.questions, answers: {}, afkTimeoutMs: total * 1000 },
        context: [
          `The user didn't answer within ${spoken(total)}, so bro's question timer closed the question. ` +
            'Go with your best judgement on each question and carry on rather than asking again; ' +
            'say in a line which way you went so they can correct it.',
        ],
      }
    } finally {
      counting.cancel()
      deadline.cancel()
      ticking.cancel()
      stopTicks?.()
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
    const isEnding = q.left <= 5
    // Claude Code's own colours, by their theme keys
    const color = isEnding ? 'warning' : 'claude'
    const filled = Math.round((q.left / q.total) * BAR)
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
