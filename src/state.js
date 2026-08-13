import fs from 'node:fs';
import path from 'node:path';
import { BRO_DIR } from './config.js';

// Volatile UI state (last picks) — kept out of config.json so we never churn the
// user's hand-edited keys/providers.
const DEFAULT_STATE_PATH = path.join(BRO_DIR, 'state.json');
const statePath = () => process.env.BRO_STATE_PATH || DEFAULT_STATE_PATH;

function loadState() {
  try {
    return JSON.parse(fs.readFileSync(statePath(), 'utf8'));
  } catch {
    return {};
  }
}

function saveState(next) {
  try {
    const target = statePath();
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, JSON.stringify(next, null, 2));
  } catch {
    /* best-effort */
  }
}

export function lastProvider() {
  return loadState().lastProvider;
}

export function lastModelFor(providerId) {
  return (loadState().lastModelByProvider || {})[providerId];
}

export function lastHarness() {
  return loadState().lastHarness;
}

// Harness choice is persisted independently of provider/model completion so a
// picker selection survives a later login, install or launch failure.
export function rememberHarness(harness) {
  const cur = loadState();
  saveState({ ...cur, lastHarness: harness });
}

// The login profile last used with a provider that has several (Codex today),
// kept apart from the model so a switcher and a model menu don't overwrite
// each other's memory.
export function lastProfileFor(providerId) {
  return (loadState().lastProfileByProvider || {})[providerId];
}

export function rememberProfile(providerId, profile) {
  const cur = loadState();
  saveState({
    ...cur,
    lastProfileByProvider: { ...(cur.lastProfileByProvider || {}), [providerId]: profile ?? '' }
  });
}

// Remember only the per-provider pick, without making the provider the default
// — for flows like image gen that shouldn't steal the picker's start position.
export function rememberModelFor(providerId, model) {
  const cur = loadState();
  saveState({
    ...cur,
    lastModelByProvider: { ...(cur.lastModelByProvider || {}), [providerId]: model ?? '' }
  });
}

// Remember the last provider overall, the last model used per provider, and the
// last harness, so the menus open on the previous choices. `harness` is only
// stored when given (flows like image gen don't involve one).
export function rememberSelection(providerId, model, harness) {
  const cur = loadState();
  saveState({
    ...cur,
    lastProvider: providerId,
    lastModelByProvider: { ...(cur.lastModelByProvider || {}), [providerId]: model ?? '' },
    ...(harness ? { lastHarness: harness } : {})
  });
}
