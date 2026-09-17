// Jev Router (https://github.com/gargpratyush/jev-router) — per-turn model
// routing for Claude Code and Codex.
//
// It is not another harness: `jev-claude` and `jev-codex` launch the real CLI
// with a loopback proxy in front of it, so every flag, session, permission and
// login bro already arranges still applies. That is why bro treats it as a
// switch on the claude and codex harnesses rather than a row in the harness
// toggle — turning it on only swaps the executable bro spawns.
//
// The proxy is also why it cannot be combined with everything: jev-claude sets
// ANTHROPIC_BASE_URL itself, and jev-codex installs its own codex provider, so
// a run that already points the CLI somewhere else (an Anthropic-compatible
// provider, the ccr proxy, the account pool, a Codex-backed bridge) would have
// two owners for the same setting. Those combinations are refused here, with
// the reason, instead of producing a session that silently ignores one of them.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { ensureGlobalPackage, globalBinDirs, which } from './proc.js';

export const JEV_PACKAGE = Object.freeze({
  label: 'Jev Router',
  command: 'jev-claude',
  packageName: 'jev-router',
  managers: ['npm', 'bun']
});

// One command per CLI jev-router fronts. Only these two harnesses can use it.
export const JEV_COMMANDS = Object.freeze({ claude: 'jev-claude', codex: 'jev-codex' });

// Where jev-router looks for its key, in its own precedence order. bro reports
// the same places so "no key" names a file the user can actually create.
export const jevEnvFiles = (cwd = process.cwd(), home = os.homedir()) => [
  path.join(cwd, '.env'),
  path.join(home, '.jev-router.env'),
  path.join(home, '.jev-claude.env')
];

const KEY_NAMES = ['JEV_API_KEY', 'TYPESAFE_API_KEY'];

function fileHasKey(file) {
  let text = '';
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch {
    return false;
  }
  return KEY_NAMES.some((name) => new RegExp(`^\\s*(?:export\\s+)?${name}\\s*=\\s*\\S`, 'm').test(text));
}

// Whether routing will actually happen. Without a key jev-router still starts
// the CLI, just with no routing at all — worth saying before the session opens
// rather than leaving the user to notice a one-line warning scroll past.
export function jevKeyStatus({ env = process.env, files = jevEnvFiles() } = {}) {
  const fromEnv = KEY_NAMES.find((name) => env[name]);
  if (fromEnv) return { found: true, source: fromEnv };
  const file = files.find((candidate) => fileHasKey(candidate));
  if (file) return { found: true, source: file };
  return { found: false, source: '', files };
}

// The routes jev-router can front. Claude Code must be running on its own
// login (this machine's, or one of bro's account profiles); codex must be on
// its ChatGPT login. Anything else already owns the base URL jev needs.
export function jevSupport({ harness = 'claude', provider = {} } = {}) {
  const mode = provider.mode || 'native';
  if (harness === 'claude') {
    if (mode === 'native' || mode === 'account') return { ok: true, command: JEV_COMMANDS.claude };
    const why = mode === 'pool'
      ? 'the account pool serves its own Anthropic-compatible endpoint'
      : mode === 'codex'
        ? 'Claude Code is already bridged to your ChatGPT login'
        : `${provider.name || provider.id} is reached through a base URL of its own`;
    return {
      ok: false,
      reason: `Jev Router needs Claude Code on its own login — ${why}.`,
      hint: 'Use a Claude provider or profile (bro account), or run it with --codex.'
    };
  }
  if (harness === 'codex') {
    if (mode === 'codex') return { ok: true, command: JEV_COMMANDS.codex };
    return {
      ok: false,
      reason: `Jev Router needs codex on your ChatGPT login — ${provider.name || provider.id} is a provider codex reaches directly.`,
      hint: 'Choose "Codex (ChatGPT subscription)", or drop --jev.'
    };
  }
  return {
    ok: false,
    reason: `Jev Router fronts Claude Code and codex; the ${harness} harness is not one of them.`,
    hint: 'Run it with --claude or --codex.'
  };
}

// Resolve the command without installing anything — for dry runs, which are
// meant to describe a launch, not perform half of one.
export const jevCommandPath = (command) => which(command, globalBinDirs()) || command;

export function ensureJev(harness = 'claude', options) {
  const command = JEV_COMMANDS[harness];
  if (!command) throw new Error(`No Jev Router command for the ${harness} harness.`);
  // Both commands ship in one package, so the install check can look for
  // whichever one this launch needs.
  const { executable, dirs } = ensureGlobalPackage({ ...JEV_PACKAGE, command }, options);
  return { executable, dirs };
}

// What a dry run should say about the routing layer.
export function describeJev(harness = 'claude') {
  const key = jevKeyStatus();
  return {
    via: `jev-router (${JEV_COMMANDS[harness]})`,
    routing: key.found ? `on (key from ${key.source})` : 'off (no JEV_API_KEY — the CLI still starts)',
    model: 'chosen per turn by Jev'
  };
}

// The line bro prints as a Jev-fronted session starts: what is routing, and —
// when nothing is — the exact file to put a key in.
export function jevNotice(harness = 'claude') {
  const key = jevKeyStatus();
  if (key.found) {
    return `\x1b[2mJev Router is picking the model each turn (${JEV_COMMANDS[harness]}; key from ${key.source}).\x1b[0m`;
  }
  return (
    `\x1b[2mJev Router found no JEV_API_KEY, so this session runs unrouted.\n` +
    `  Add JEV_API_KEY=… to ${key.files[1]} to enable routing.\x1b[0m`
  );
}
