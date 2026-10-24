// bro's Claude Code mod, loaded into every Claude session bro starts (bro
// passes `--plugin-dir`; see crates/bro-core/src/claude_mod.rs). Two halves:
//
//   style/  the look: the spinner and the turn line in the bro theme's voice,
//           beside the Claude Code theme bro writes from its own palette
//   link/   bro integration: limits in the hint line, a radar for sessions
//           that wait on you, the `/bro` pane, and tools for Claude
//
// Outside bro (no BRO_SESSION_ID) every hook passes straight through.
import type { Register } from 'claude-code'

import { registerLink, startLink, wireLink } from './link/index'
import { broEnv } from './shared/env'
import { followLook } from './shared/look'
import { registerStyle } from './style/index'

export const register: Register = (on, options) => {
  on('session.start', async ($, e, next) => {
    const env = e.isInteractive ? await broEnv($) : null
    if (env !== null) {
      // the command and the tools first: what session.start registers is
      // listed by the first prompt
      await wireLink($, env, options)
    }
    const started = await next(e)
    if (env !== null) {
      await followLook($, env)
      await startLink($, env, options)
    }
    return started
  })

  registerStyle(on, options)
  registerLink(on, options)
}
