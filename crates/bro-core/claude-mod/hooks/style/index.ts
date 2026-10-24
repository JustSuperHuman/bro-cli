// The look: Claude Code wears the bro theme on screen. bro writes the colours
// (`<config>/themes/bro.json`, selected per launch, live on every change); this
// half gives the spinner and the turn line the theme's voice.
import { atom, read } from 'claude-code'
import type { EngineInterface, On, PluginOptions } from 'claude-code'

import { familyFor, pick } from './words'

/** bro's theme on screen; `look.ts` keeps it current. */
const look = atom({ plugin: 'bro', key: 'look' } as const, null)

type Pair = readonly [now: string, past: string]

/** A turn's line drawn this long after the turn ended is that turn's own. */
const FRESH_MS = 30_000

/** The main loop's running turn and its words. */
let turn: Pair | null = null
/** Turns that ended and whose closing line hasn't been drawn yet, newest last. */
const unclaimed: { pair: Pair; endedAt: number }[] = []
/** Each closing line's words, by its row, so a redraw reads the same. */
const claimed = new Map<string, Pair>()

export function registerStyle(on: On, options: PluginOptions): void {
  if (options.flair === false) {
    return
  }

  on('turn.start', async ($, e, next) => {
    if (e.agentId === undefined) {
      turn = await pairFor($, e.turnId)
    }
    return next(e)
  })

  on('turn.complete', async ($, e, next) => {
    if (e.agentId === undefined && turn !== null) {
      unclaimed.push({ pair: turn, endedAt: await $.clock.now() })
      unclaimed.splice(0, Math.max(0, unclaimed.length - 3))
      turn = null
    }
    return next(e)
  })

  // The desktop's spinner row names the step it runs ("Creating notes.md"):
  // only the terminal's animated word is the theme's to say.
  on('ui.render', { component: 'Spinner', surface: 'terminal' }, async ($, e, next) => {
    const pair = turn ?? (await pairFor($, e.props.word))
    return pair === null ? next(e) : next({ ...e, props: { ...e.props, word: pair[0] } })
  })

  // "Forging…" while it works, "Forged for 12s" when it's done: the line the
  // engine draws as a turn ends takes the words of the turn that just ended.
  on('ui.render', { component: 'TurnDuration' }, async ($, e, next) => {
    if (e.props.isExpanded === true || e.props.waitingFor !== undefined) {
      return next(e)
    }
    let pair = claimed.get(e.requestId) ?? null
    if (pair === null) {
      const now = await $.clock.now()
      const last = unclaimed.at(-1)
      if (last !== undefined && now - last.endedAt < FRESH_MS) {
        unclaimed.pop()
        pair = last.pair
      } else {
        pair = await pairFor($, `${e.props.word}:${e.props.durationMs}`)
      }
      if (pair !== null) {
        claimed.set(e.requestId, pair)
      }
    }
    return pair === null ? next(e) : next({ ...e, props: { ...e.props, word: pair[1] } })
  })
}

/** The theme's words for a seed; null while no bro theme is on screen. */
async function pairFor($: EngineInterface, seed: string): Promise<Pair | null> {
  const theme = await read($, look)
  return theme === null ? null : pick(familyFor(theme.id, theme.base), seed)
}
