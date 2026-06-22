import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { BRO_DIR } from './config.js';
import { stripHash } from './strip.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const BUNDLED = path.join(__dirname, '..', 'models.json');
export const CACHE = path.join(BRO_DIR, 'models.cache.json');
export const REMOTE_URL =
  process.env.BRO_MODELS_URL ||
  'https://raw.githubusercontent.com/JustSuperHuman/bro-cli/main/models.json';

function readJson(p) {
  try {
    return JSON.parse(fs.readFileSync(p, 'utf8'));
  } catch {
    return null;
  }
}

// Pull the list from REMOTE_URL and store it locally (~/.bro/models.cache.json).
async function fetchRemote() {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), 6000);
  try {
    // `connection: close` keeps undici from pooling a keep-alive socket, so the
    // process can exit promptly once the response is read.
    const res = await fetch(REMOTE_URL, { signal: ctrl.signal, headers: { accept: 'application/json', connection: 'close' } });
    if (!res.ok) throw new Error('HTTP ' + res.status);
    const json = await res.json();
    // Guard against a GitHub "blob" HTML page or any non-list response poisoning the cache.
    if (!json || !Array.isArray(json.providers)) throw new Error('response was not a models list (no "providers" array)');
    fs.mkdirSync(BRO_DIR, { recursive: true });
    fs.writeFileSync(CACHE, JSON.stringify(json, null, 2));
    return json;
  } finally {
    clearTimeout(timer);
  }
}

// Local-first: use the stored copy so normal runs are instant and work offline.
// The network is only touched to bootstrap the very first run; use `bro update`
// to refresh on demand.
export async function loadModels() {
  let data = readJson(CACHE);
  if (!data) {
    try {
      data = await fetchRemote();
    } catch {
      /* offline / not published yet — fall back to the bundled copy */
    }
  }
  if (!data) data = readJson(BUNDLED);
  if (!data) data = { providers: [] };
  return stripHash(data);
}

// Force a refresh from REMOTE_URL (used by `bro update`).
export async function updateModels() {
  const data = await fetchRemote();
  const providers = stripHash(data).providers || [];
  return {
    source: REMOTE_URL,
    cache: CACHE,
    providers: providers.length,
    models: providers.reduce((n, p) => n + (p.models?.length || 0), 0)
  };
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
