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

// Live OpenRouter catalogue — pulled fresh each time the OpenRouter provider is
// selected so the model menu always shows what's current. Only models from
// these authors are kept. Falls back to the last successful fetch (cached in
// ~/.bro/openrouter.cache.json), or null so the caller keeps its static list.
export const OPENROUTER_CACHE = path.join(BRO_DIR, 'openrouter.cache.json');
const OPENROUTER_URL = 'https://openrouter.ai/api/v1/models';
const OPENROUTER_AUTHORS = ['anthropic', 'openai', 'moonshotai', 'z-ai'];

export async function loadOpenRouterModels() {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), 5000);
  try {
    const res = await fetch(OPENROUTER_URL, { signal: ctrl.signal, headers: { accept: 'application/json', connection: 'close' } });
    if (!res.ok) throw new Error('HTTP ' + res.status);
    const json = await res.json();
    const models = pickOpenRouterModels(json?.data);
    if (!models.length) throw new Error('no matching models');
    fs.mkdirSync(BRO_DIR, { recursive: true });
    fs.writeFileSync(OPENROUTER_CACHE, JSON.stringify(models, null, 2));
    return models;
  } catch {
    const cached = readJson(OPENROUTER_CACHE);
    return Array.isArray(cached) && cached.length ? cached : null;
  } finally {
    clearTimeout(timer);
  }
}

// Live OpenRouter image-model catalogue — models whose output modalities include
// "image" (Gemini image / Nano Banana and friends). Used by the image-gen flow so
// its model menu always shows what's current. Falls back to the last successful
// fetch, or null so the caller keeps its static list.
export const OPENROUTER_IMAGE_CACHE = path.join(BRO_DIR, 'openrouter-image.cache.json');

export async function loadOpenRouterImageModels() {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), 5000);
  try {
    const res = await fetch(OPENROUTER_URL, { signal: ctrl.signal, headers: { accept: 'application/json', connection: 'close' } });
    if (!res.ok) throw new Error('HTTP ' + res.status);
    const json = await res.json();
    const models = (Array.isArray(json?.data) ? json.data : [])
      .filter((m) => m?.id && (m.architecture?.output_modalities || []).includes('image'))
      .sort((a, b) => (b.created || 0) - (a.created || 0))
      .map((m) => ({ id: m.id, name: m.name || m.id, via: 'chat' }));
    if (!models.length) throw new Error('no image models');
    fs.mkdirSync(BRO_DIR, { recursive: true });
    fs.writeFileSync(OPENROUTER_IMAGE_CACHE, JSON.stringify(models, null, 2));
    return models;
  } catch {
    const cached = readJson(OPENROUTER_IMAGE_CACHE);
    return Array.isArray(cached) && cached.length ? cached : null;
  } finally {
    clearTimeout(timer);
  }
}

// Keep the wanted authors, grouped in OPENROUTER_AUTHORS order, newest first
// within each group.
function pickOpenRouterModels(data) {
  if (!Array.isArray(data)) return [];
  const author = (m) => String(m.id).split('/')[0];
  return data
    .filter((m) => m?.id && OPENROUTER_AUTHORS.includes(author(m)))
    .sort(
      (a, b) =>
        OPENROUTER_AUTHORS.indexOf(author(a)) - OPENROUTER_AUTHORS.indexOf(author(b)) ||
        (b.created || 0) - (a.created || 0)
    )
    .map((m) => ({ id: m.id, name: m.name || m.id }));
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
