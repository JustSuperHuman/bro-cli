import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { BRO_DIR } from './config.js';
import { stripHash } from './strip.js';
import { modelKey, imageTokensPerImage } from './model-info.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const BUNDLED = path.join(__dirname, '..', 'models.json');
const BUNDLED_VIDEO = path.join(__dirname, '..', 'video-models.json');
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
// selected so the model menu always shows everything currently available.
// Falls back to the last successful fetch (cached in
// ~/.bro/openrouter.cache.json), or null so the caller keeps its static list.
export const OPENROUTER_CACHE = path.join(BRO_DIR, 'openrouter.cache.json');
const OPENROUTER_URL = 'https://openrouter.ai/api/v1/models';

// The last catalogue fetched, with how old it is (ms), or null.
export function readOpenRouterCache() {
  const cached = readJson(OPENROUTER_CACHE);
  if (!Array.isArray(cached) || !cached.length) return null;
  let age = Infinity;
  try {
    age = Date.now() - fs.statSync(OPENROUTER_CACHE).mtimeMs;
  } catch {}
  return { models: cached, age };
}

// `maxAge` (ms) lets callers accept a recent cached copy without touching the
// network — the picker uses that so its rows appear instantly.
export async function loadOpenRouterModels({ maxAge = 0 } = {}) {
  if (maxAge > 0) {
    const cached = readOpenRouterCache();
    if (cached && cached.age <= maxAge) return cached.models;
  }
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), 5000);
  try {
    const res = await fetch(OPENROUTER_URL, { signal: ctrl.signal, headers: { accept: 'application/json', connection: 'close' } });
    if (!res.ok) throw new Error('HTTP ' + res.status);
    const json = await res.json();
    const models = mapOpenRouterModels(json?.data);
    if (!models.length) throw new Error('no models');
    fs.mkdirSync(BRO_DIR, { recursive: true });
    fs.writeFileSync(OPENROUTER_CACHE, JSON.stringify(models, null, 2));
    return models;
  } catch {
    return readOpenRouterCache()?.models || null;
  } finally {
    clearTimeout(timer);
  }
}

// ---- speed stats ----
// OpenRouter only reports throughput/latency per endpoint, one request per
// model, and only to an authenticated caller. The numbers are collected in the
// background, kept for a day per model (~/.bro/openrouter-stats.cache.json)
// and merged into the catalogue rows as `speed: { tps, ttft }`.
export const OPENROUTER_STATS_CACHE = path.join(BRO_DIR, 'openrouter-stats.cache.json');
export const STATS_MAX_AGE = 24 * 60 * 60 * 1000;

export function readOpenRouterStats() {
  const cached = readJson(OPENROUTER_STATS_CACHE);
  return cached && typeof cached === 'object' && !Array.isArray(cached) ? cached : {};
}

const median = (values) => {
  const sorted = values.filter((v) => Number.isFinite(v)).sort((a, b) => a - b);
  if (!sorted.length) return null;
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};

// One model's endpoints → its typical output speed (tokens/s) and time to
// first token (ms): the median of each endpoint's p50, so a single slow or
// fast host does not define the model. Null when nothing was measured.
export function summarizeEndpointStats(endpoints) {
  const list = Array.isArray(endpoints) ? endpoints : [];
  const tps = median(list.map((e) => e?.throughput_last_30m?.p50));
  const ttft = median(list.map((e) => e?.latency_last_30m?.p50));
  return tps == null && ttft == null ? null : { tps, ttft };
}

// Rows with their cached speed stats attached (stale entries included — an
// old measurement beats none; the refresher replaces them). Stats are keyed by
// OpenRouter id; a row matched from another provider carries that as
// `catalogueId`.
export const statsIdOf = (m) => m?.catalogueId || m?.id || '';
export function attachStats(models, stats = readOpenRouterStats()) {
  return (models || []).map((m) => {
    const s = stats[statsIdOf(m)];
    return s && (s.tps != null || s.ttft != null) ? { ...m, speed: { tps: s.tps ?? null, ttft: s.ttft ?? null } } : m;
  });
}

// Fetch stats for the ids whose cached entry is missing or older than
// `maxAge`, `concurrency` at a time, in the order given (put what the user
// sees first at the front). `onUpdate(stats)` fires after every write so a
// live picker can repaint; `signal` stops the run early. Resolves with the
// complete stats map. Without an API key this is a no-op.
export async function refreshOpenRouterStats({ ids, apiKey, concurrency = 4, maxAge = STATS_MAX_AGE, limit = Infinity, onUpdate = null, signal = null } = {}) {
  let stats = readOpenRouterStats();
  if (!apiKey || !Array.isArray(ids)) return stats;
  const now = Date.now();
  const queue = ids.filter((id) => id && !(stats[id] && now - (stats[id].at || 0) < maxAge)).slice(0, limit);
  if (!queue.length) return stats;
  let dirty = false;
  const flush = () => {
    if (!dirty) return;
    dirty = false;
    try {
      fs.mkdirSync(BRO_DIR, { recursive: true });
      // Re-read so two concurrent bro processes do not clobber each other.
      stats = { ...readOpenRouterStats(), ...stats };
      fs.writeFileSync(OPENROUTER_STATS_CACHE, JSON.stringify(stats));
    } catch {}
    onUpdate?.(stats);
  };
  const worker = async () => {
    while (queue.length && !signal?.aborted) {
      const id = queue.shift();
      try {
        const ctrl = new AbortController();
        const timer = setTimeout(() => ctrl.abort(), 8000);
        const stop = () => ctrl.abort();
        signal?.addEventListener('abort', stop, { once: true });
        let json;
        try {
          const res = await fetch(`${OPENROUTER_URL}/${id}/endpoints`, {
            signal: ctrl.signal,
            headers: { accept: 'application/json', connection: 'close', authorization: `Bearer ${apiKey}` }
          });
          if (!res.ok) throw new Error('HTTP ' + res.status);
          json = await res.json();
        } finally {
          clearTimeout(timer);
          signal?.removeEventListener('abort', stop);
        }
        const summary = summarizeEndpointStats(json?.data?.endpoints);
        stats[id] = { ...(summary || { tps: null, ttft: null }), at: Date.now() };
        dirty = true;
      } catch {
        if (signal?.aborted) return;
        // Remember the miss briefly so a dead id is not retried every run.
        stats[id] = { tps: null, ttft: null, at: Date.now() - maxAge + 60 * 60 * 1000 };
        dirty = true;
      }
    }
  };
  const ticker = setInterval(flush, 700);
  try {
    await Promise.all(Array.from({ length: Math.max(1, concurrency) }, worker));
  } finally {
    clearInterval(ticker);
    flush();
  }
  return stats;
}

// Live OpenRouter image-model catalogue — models whose output modalities include
// "image" (Gemini image / Nano Banana and friends). Used by JustImagine so
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
    const models = mapOpenRouterImageModels(json?.data);
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

// Live OpenRouter video-model catalogue. Video models are not in /models —
// they have their own endpoint, and each entry carries the knobs that model
// actually accepts (durations, resolutions, aspect ratios, audio, seed), which
// JustImagine turns into per-model controls instead of a fixed set that would
// be wrong for most of them.
export const OPENROUTER_VIDEO_CACHE = path.join(BRO_DIR, 'openrouter-video.cache.json');
const OPENROUTER_VIDEO_URL = process.env.JUSTIMAGINE_VIDEO_MODELS_URL || 'https://openrouter.ai/api/v1/videos/models';

export function mapOpenRouterVideoModels(data) {
  if (!Array.isArray(data)) return [];
  return data
    .filter((m) => m?.id)
    .sort((a, b) => (b.created || 0) - (a.created || 0) || String(a.id).localeCompare(String(b.id)))
    .map((m) => ({
      id: m.id,
      name: m.name || m.id,
      kind: 'video',
      created: m.created || undefined,
      pricing: videoPricing(m.pricing_skus),
      resolutions: m.supported_resolutions || null,
      aspectRatios: m.supported_aspect_ratios || null,
      sizes: m.supported_sizes || null,
      durations: m.supported_durations || null,
      frames: m.supported_frame_images || null,
      audio: !!m.generate_audio,
      seed: !!m.seed,
      upscale: m.upscale_factor != null || /upscale/i.test(m.id)
    }));
}

export async function loadOpenRouterVideoModels() {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), 6000);
  try {
    const res = await fetch(OPENROUTER_VIDEO_URL, { signal: ctrl.signal, headers: { accept: 'application/json', connection: 'close' } });
    if (!res.ok) throw new Error('HTTP ' + res.status);
    const models = mapOpenRouterVideoModels((await res.json())?.data);
    if (!models.length) throw new Error('no video models');
    fs.mkdirSync(BRO_DIR, { recursive: true });
    fs.writeFileSync(OPENROUTER_VIDEO_CACHE, JSON.stringify(models, null, 2));
    return models;
  } catch {
    const cached = readJson(OPENROUTER_VIDEO_CACHE);
    if (Array.isArray(cached) && cached.length) return cached;
    // Offline first run: the bundled snapshot still gives a usable menu.
    const bundled = readJson(BUNDLED_VIDEO);
    return Array.isArray(bundled?.models) && bundled.models.length ? bundled.models : null;
  } finally {
    clearTimeout(timer);
  }
}

// Keep every valid catalogue entry. New models appear first; ids break ties so
// the order remains deterministic if OpenRouter returns equal timestamps.
// Each row also carries the facts the picker rates: when the model appeared,
// its $/M-token prices, its context window and its Artificial Analysis
// benchmark indices (OpenRouter publishes those under `benchmarks`).
export function mapOpenRouterModels(data) {
  if (!Array.isArray(data)) return [];
  return data
    .filter((m) => m?.id)
    .sort((a, b) => (b.created || 0) - (a.created || 0) || String(a.id).localeCompare(String(b.id)))
    .map((m) => {
      const row = { id: m.id, name: m.name || m.id };
      if (m.created) row.created = m.created;
      if (m.context_length) row.context = m.context_length;
      const prompt = perMillion(m.pricing?.prompt);
      const completion = perMillion(m.pricing?.completion);
      if (prompt != null && completion != null) row.pricing = { prompt, completion };
      const aa = m.benchmarks?.artificial_analysis;
      if (aa && (aa.coding_index != null || aa.intelligence_index != null)) {
        row.quality = { coding: aa.coding_index ?? null, intelligence: aa.intelligence_index ?? null, agentic: aa.agentic_index ?? null };
      }
      if (m.reasoning || (m.supported_parameters || []).includes('reasoning')) row.reasoning = true;
      return row;
    });
}

// OpenRouter quotes $/token as strings; -1 means "varies" (routers), which is
// no price at all.
function perMillion(value) {
  const n = typeof value === 'string' ? parseFloat(value) : value;
  if (!Number.isFinite(n) || n < 0) return null;
  return Math.round(n * 1e6 * 1e4) / 1e4;
}

// Image-capable chat models (Gemini image / Nano Banana, GPT Image) with what
// the gallery rates them on: when they appeared, an estimated price per
// picture (output-token price × a family's tokens per image) and their Design
// Arena standing. A model without a score borrows its sibling's — the
// "-preview" release of the same model is the one that was benchmarked.
export function mapOpenRouterImageModels(data) {
  if (!Array.isArray(data)) return [];
  const models = data
    .filter((m) => m?.id && (m.architecture?.output_modalities || []).includes('image'))
    .sort((a, b) => (b.created || 0) - (a.created || 0) || String(a.id).localeCompare(String(b.id)))
    .map((m) => {
      const row = { id: m.id, name: m.name || m.id, via: 'chat', kind: 'image' };
      if (m.created) row.created = m.created;
      const imageOutput = perMillion(m.pricing?.image_output);
      const prompt = perMillion(m.pricing?.prompt);
      if (imageOutput != null) {
        row.pricing = {
          perImage: Math.round(imageOutput * imageTokensPerImage(m.id)) / 1e6,
          imageOutput,
          ...(prompt != null ? { prompt } : {})
        };
      }
      const arena = Array.isArray(m.benchmarks?.design_arena) ? m.benchmarks.design_arena : [];
      const score = arena.find((e) => e.category === 'image') || arena.find((e) => e.category === 'graphicdesign');
      if (score && score.win_rate != null) {
        row.quality = { arena: score.category, winRate: score.win_rate, rank: score.rank ?? null, elo: score.elo ?? null };
      }
      return row;
    });
  const scored = new Map();
  for (const m of models) if (m.quality) scored.set(modelKey(m.id), m);
  for (const m of models) {
    if (m.quality) continue;
    const sibling = scored.get(modelKey(m.id));
    if (sibling) m.quality = { ...sibling.quality, from: sibling.id };
  }
  return models;
}

// Video models are priced by the second, but every publisher spells it its own
// way in `pricing_skus`. Normalise to dollars per second of output at a
// standard tier — 720p when the model offers tiers, the plain rate otherwise,
// the cheapest of what is left — and say which tier that was.
const SEEDANCE_TOKENS_PER_SECOND = (1280 * 720 * 24) / 1024; // ByteDance's formula at 720p
export function videoPricing(skus) {
  if (!skus || typeof skus !== 'object') return undefined;
  const entries = Object.entries(skus)
    .map(([k, v]) => [k, parseFloat(v)])
    .filter(([, v]) => Number.isFinite(v) && v >= 0);
  if (!entries.length) return undefined;
  const perSecond = entries
    .map(([k, v]) => {
      if (/megapixel/.test(k)) return null; // upscalers: depends on the source
      if (/(image_input|reference_images|minimum_cents|continuation|with_video_input)/.test(k)) return null;
      if (/^video_tokens/.test(k)) return { k, v: v * SEEDANCE_TOKENS_PER_SECOND };
      if (/cents_per/.test(k)) return { k, v: v / 100 };
      if (/duration_seconds|per_second/.test(k)) return { k, v };
      return null;
    })
    .filter(Boolean);
  if (!perSecond.length) return undefined;
  const tiered = perSecond.filter((e) => /720p/.test(e.k));
  const plain = perSecond.filter((e) => !/\d{3,4}p|4k/i.test(e.k));
  const pool = tiered.length ? tiered : plain.length ? plain : perSecond;
  const best = pool.reduce((a, b) => (b.v < a.v ? b : a));
  const basis = /720p/.test(best.k) ? '720p' : (best.k.match(/(\d{3,4}p|4k)/i) || [])[1] || null;
  return { perSecond: Math.round(best.v * 1e4) / 1e4, ...(basis ? { basis } : {}) };
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
