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

// Remember the last provider overall, plus the last model used per provider, so
// both menus open on the previous choice.
export function rememberSelection(providerId, model) {
  const cur = loadState();
  saveState({
    ...cur,
    lastProvider: providerId,
    lastModelByProvider: { ...(cur.lastModelByProvider || {}), [providerId]: model ?? '' }
  });
}
