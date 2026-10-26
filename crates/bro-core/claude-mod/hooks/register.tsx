// bro's Claude Code mod, loaded into every Claude session bro starts while it's
// on (bro passes `--plugin-dir`; see crates/bro-core/src/claude_mod.rs). Two
// parts:
//
//   link/       `/bro`: your sessions and limits in a pane, a press from each
//   questions/  Claude's questions: a chime, and the question timer's countdown
//
// Outside bro (no BRO_SESSION_ID) every hook passes straight through.
import type { Register } from 'claude-code'

import { registerLink, startLink, wireLink } from './link/index'
import { registerQuestions } from './questions/index'
import { broEnv } from './shared/env'

export const register: Register = (on, options) => {
  on('session.start', async ($, e, next) => {
    const env = e.isInteractive ? await broEnv($) : null
    if (env !== null) {
      // the command first: what session.start registers is listed by the first prompt
      await wireLink($)
    }
    const started = await next(e)
    if (env !== null) {
      await startLink($, env)
    }
    return started
  })

  registerLink(on)
  registerQuestions(on, options)
}
