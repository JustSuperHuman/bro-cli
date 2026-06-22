import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { BRO_DIR } from './config.js';
import { stripHash } from './strip.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const BUNDLED = path.join(__dirname, '..', 'models.json');
const CACHE = path.join(BRO_DIR, 'models.cache.json');
const REMOTE_URL = process.env.BRO_MODELS_URL || 'https://m.justgains.com/models.json';

function readJson(p) {
  try {
    return JSON.parse(fs.readFileSync(p, 'utf8'));
  } catch {
    return null;
  }
}

async function fetchRemote() {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), 4000);
  try {
    const res = await fetch(REMOTE_URL, { signal: ctrl.signal });
    if (!res.ok) throw new Error('HTTP ' + res.status);
    const json = await res.json();
    try {
      fs.mkdirSync(BRO_DIR, { recursive: true });
      fs.writeFileSync(CACHE, JSON.stringify(json));
    } catch {
      /* cache is best-effort */
    }
    return json;
  } finally {
    clearTimeout(timer);
  }
}

// Remote first, then last-good cache, then the copy bundled with the package.
export async function loadModels() {
  let data = null;
  try {
    data = await fetchRemote();
  } catch {
    /* offline / not deployed yet — fall back */
  }
  if (!data) data = readJson(CACHE);
  if (!data) data = readJson(BUNDLED);
  if (!data) data = { providers: [] };
  return stripHash(data);
}

// Merge the user's custom providers into the remote list:
//   - same id  -> append models (and override baseUrl/mode/keyEnv/noKey if given)
//   - new id   -> add as a new provider
export function mergeProviders(remote, configProviders = []) {
  const providers = (remote.providers || []).map((p) => ({ ...p, models: [...(p.models || [])] }));
  const byId = new Map(providers.map((p) => [p.id, p]));
  for (const cp of configProviders) {
    if (!cp || !cp.id) continue;
    const existing = byId.get(cp.id);
    if (existing) {
      for (const f of ['baseUrl', 'mode', 'keyEnv', 'keyUrl', 'noKey', 'disable1mContext']) {
        if (cp[f] != null) existing[f] = cp[f];
      }
      for (const m of cp.models || []) existing.models.push(m);
    } else {
      const np = { ...cp, models: [...(cp.models || [])] };
      providers.push(np);
      byId.set(np.id, np);
    }
  }
  return providers;
}
