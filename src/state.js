import fs from 'node:fs';
import path from 'node:path';
import { BRO_DIR } from './config.js';

// Volatile UI state (last picks) — kept out of config.json so we never churn the
// user's hand-edited keys/providers.
const STATE_PATH = path.join(BRO_DIR, 'state.json');

function loadState() {
  try {
    return JSON.parse(fs.readFileSync(STATE_PATH, 'utf8'));
  } catch {
    return {};
  }
}

function saveState(next) {
  try {
    fs.mkdirSync(BRO_DIR, { recursive: true });
    fs.writeFileSync(STATE_PATH, JSON.stringify(next, null, 2));
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
