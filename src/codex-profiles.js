// Codex login profiles — the Codex half of what `~/.claude-max-pool/accounts`
// is for Claude.
//
// Codex keeps everything for one login under a single directory it calls
// CODEX_HOME: the ChatGPT credentials (auth.json), the rollouts (sessions/),
// settings (config.toml), history and caches. So a profile here is just
// another CODEX_HOME under ~/.bro/codex-profiles/<name>, and switching is
// setting that environment variable for the launch — the machine's own
// ~/.codex is never written to.
//
// A new profile is seeded with a copy of the machine's codex settings so it
// behaves like the codex you already configured, then goes its own way: its
// own login, its own sessions.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { BRO_DIR } from './config.js';
import { assertProfileName, listProfileDirs } from './profiles.js';
import { codexAuthStatus } from './codex-auth.js';
import { codexMeters, metersText } from './usage.js';

// Both roots are read when they're used, not when this module loads: the codex
// home is the user's own environment variable, which a launch may have changed.
export const profilesDir = () => process.env.BRO_CODEX_PROFILES_DIR || path.join(BRO_DIR, 'codex-profiles');

// The machine's own codex directory — the "local" login, and the source a new
// profile's settings are seeded from.
export const defaultCodexHome = () => process.env.CODEX_HOME || path.join(os.homedir(), '.codex');

// Where a launch should point CODEX_HOME. No name means the local login, which
// keeps CODEX_HOME exactly as the user's environment has it.
export function codexHomeFor(name) {
  return name ? path.join(profilesDir(), assertProfileName(name)) : defaultCodexHome();
}

// Settings worth carrying into a new profile: everything that makes codex feel
// like *your* codex, minus credentials, history and session state.
const SEEDED = ['config.toml', 'AGENTS.md', 'prompts', 'skills', 'hooks', 'hooks.json'];

function seedProfile(dir) {
  fs.mkdirSync(dir, { recursive: true });
  const from = defaultCodexHome();
  for (const name of SEEDED) {
    const source = path.join(from, name);
    const target = path.join(dir, name);
    if (!fs.existsSync(source) || fs.existsSync(target)) continue;
    try {
      fs.cpSync(source, target, { recursive: true });
    } catch {
      /* settings are a convenience — a profile without them still works */
    }
  }
  return dir;
}

export function createCodexProfile(name) {
  return seedProfile(codexHomeFor(assertProfileName(name)));
}

// Copy the machine's current Codex login into a profile, so an existing
// ChatGPT session can be pooled without signing in again.
export function importCodexProfile(name) {
  const dir = createCodexProfile(name);
  const source = path.join(defaultCodexHome(), 'auth.json');
  if (!fs.existsSync(source)) {
    throw new Error(`No Codex login found at ${source}. Run \`codex login\`, or \`bro codex login ${name}\` to sign this profile in directly.`);
  }
  fs.copyFileSync(source, path.join(dir, 'auth.json'));
  return dir;
}

export function removeCodexProfile(name) {
  const dir = codexHomeFor(assertProfileName(name));
  if (!fs.existsSync(dir)) return false;
  fs.rmSync(dir, { recursive: true, force: true });
  return true;
}

// Every profile with its login state. `plan` is the ChatGPT plan the stored
// token was issued for — the one piece of account detail Codex exposes without
// spending a request.
export function listCodexProfiles() {
  return listProfileDirs(profilesDir()).map(({ name, dir }) => {
    const status = codexAuthStatus(dir);
    return { name, dir, authenticated: status.loggedIn, plan: status.plan || null, identity: status.identity || null };
  });
}

// The local login is a profile too as far as the pickers are concerned: it has
// a home, a login state and a plan, it just has no name.
export function localCodexProfile() {
  const status = codexAuthStatus();
  return {
    name: '',
    dir: defaultCodexHome(),
    authenticated: status.loggedIn,
    plan: status.plan || null,
    identity: status.identity || null,
    // Whether the credentials live in the codex CLI's own directory or in
    // bro's fallback file decides whether the codex CLI can see them.
    source: status.source || ''
  };
}

const CODEX_METERS = [['5h', 'session'], ['wk', 'weekly']];

// One profile row: name, then dim state — plan when signed in, otherwise the
// reason it can't be used yet. Like a Claude account row, a signed-in profile
// shows its 5h and weekly meters once `usageStats` has them (null when they
// couldn't be read), with placeholders while `usagePending`.
export function codexProfileLabel(profile, { name = profile.name || 'local' } = {}) {
  const state = profile.authenticated ? profile.plan || 'ready' : 'logged out';
  if (!profile.authenticated) return `${name}  \x1b[2m${state}\x1b[0m`;
  const plan = ` \x1b[2m· ${state}\x1b[0m`;
  if (profile.usageStats) {
    const meters = codexMeters(profile.usageStats);
    return `${name}  ${metersText(CODEX_METERS.map(([label, key]) => [label, meters[key]]))}${plan}`;
  }
  if (profile.usagePending) return `${name}  ${metersText(CODEX_METERS, { pending: true })}${plan}`;
  if (profile.usageStats === null) return `${name}  \x1b[2musage unavailable\x1b[0m${plan}`;
  return `${name}  \x1b[2m${state}\x1b[0m`;
}

// The rollout files that have to exist inside the destination profile for it to
// fork another profile's session. Codex stores one file per session under
// sessions/<year>/<month>/<day>/, and finds a session by scanning that tree, so
// the copy only has to keep the same relative path.
export function codexSessionEntries(session, { sourceHome, targetHome }) {
  const id = String(session?.id || '');
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id)) {
    throw new Error(`Invalid Codex session id: ${id || '(missing)'}`);
  }
  if (!session?.file) throw new Error('That session has no rollout path. Reopen the switcher and try again.');

  const sourceRoot = path.resolve(sourceHome, 'sessions');
  const source = path.resolve(session.file);
  const relative = path.relative(sourceRoot, source);
  if (!relative || relative.startsWith('..') || path.isAbsolute(relative) || !source.includes(id)) {
    throw new Error(`The source rollout is unavailable or outside its Codex profile: ${source}`);
  }
  return [{ source, target: path.join(path.resolve(targetHome), 'sessions', relative) }];
}
