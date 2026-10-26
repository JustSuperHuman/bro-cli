// What bro tells a session it launched. Every part of the mod stays inert
// outside bro: no BRO_SESSION_ID, no hooks doing anything.
import type { EngineInterface } from 'claude-code'

export type BroEnv = {
  /** bro's id for this pane; the bridge, REST and `bro ctl` name the session by it. */
  sessionId: string
  /** Loopback URL of bro's bridge, when it ran at launch. */
  bridge?: string
  isOrchestrator: boolean
  /** The login the session runs as (`claude:work`), when it runs as one. */
  profile?: string
  /** Who serves the model: the login, a provider id, or `pool`. */
  backend?: string
  version?: string
  /** The bro that launched the session: it plays the mod's sounds (`bro hook sound`). */
  bin?: string
}

let known: Promise<BroEnv | null> | undefined

/** The launch's bro facts, read once per load; null outside bro. */
export function broEnv($: EngineInterface): Promise<BroEnv | null> {
  known ??= read($)
  return known
}

async function read($: EngineInterface): Promise<BroEnv | null> {
  const sessionId = clean(await $.env.get('BRO_SESSION_ID'))
  if (sessionId === undefined) {
    return null
  }
  const bridge = clean(await $.env.get('BRO_BRIDGE_URL'))?.replace(/\/+$/, '')

  return {
    sessionId,
    bridge,
    isOrchestrator: (await $.env.get('BRO_ORCHESTRATOR')) === '1',
    profile: clean(await $.env.get('BRO_PROFILE')),
    backend: clean(await $.env.get('BRO_BACKEND')),
    version: clean(await $.env.get('BRO_VERSION')),
    bin: clean(await $.env.get('BRO_BIN')),
  }
}

function clean(value: string | undefined): string | undefined {
  const trimmed = value?.trim()
  return trimmed === undefined || trimmed === '' ? undefined : trimmed
}

/** The last part of a path: a project's folder name. */
export function baseName(path: string): string {
  const parts = path.split(/[\\/]+/).filter(part => part !== '')
  return parts.at(-1) ?? path
}

/** `claude:work` → `work`; a provider id stays as it is. */
export function loginName(id: string): string {
  return id.replace(/^(claude|codex):/, '')
}
