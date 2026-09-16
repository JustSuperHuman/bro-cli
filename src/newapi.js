// New-API relays — OpenLux, Yunwu and the many gateways built on the same
// software. They aggregate hundreds of models behind one OpenAI- and
// Anthropic-compatible endpoint, and publish the whole catalogue, unauthenticated,
// at {baseUrl}/api/pricing.
//
// The thing that makes them different from every other provider bro talks to is
// the *group*: one model is served through several upstream routes (an official
// API key, an Azure deployment, a Codex subscription, a reverse-engineered
// client) and each route is a named group with its own price multiplier. The
// same gpt-6-astra costs $10/M through Openai-Gpt-2 and $0.37/M through
// Codex-Gpt-1. bro calls a group a "tier", because that is what choosing one
// actually means.
//
// A tier is bound to the API token, not to the request: you create one token
// per tier in the relay's console. So bro stores a key per tier (see tierKeyId)
// and falls back to the provider's plain key when a tier has none of its own.

import fs from 'node:fs';
import path from 'node:path';
import { BRO_DIR } from './config.js';

// one-api's ratio convention, which every fork inherited: ratio 1 means
// $0.002 per 1K tokens, i.e. $2 per million. Verified against the relays' own
// posted numbers — gpt-4o is model_ratio 1.25 and costs $2.50/M in, gpt-5 is
// 0.625 and costs $1.25/M in, claude-sonnet-4-5 is 1.5 and costs $3/M in.
export const USD_PER_RATIO_UNIT = 2;

// Endpoint types that can carry a coding harness. The rest of a relay's
// catalogue is image, video, speech and rerank models, which have no harness to
// run — they belong in JustImagine, not in the provider menu.
const CHAT_ENDPOINTS = new Set(['openai', 'openai-response', 'anthropic', 'gemini']);

const round = (n, places = 4) => Math.round(n * 10 ** places) / 10 ** places;

export const newApiCachePath = (providerId) => path.join(BRO_DIR, `newapi-${providerId}.cache.json`);

function readJson(p) {
  try {
    return JSON.parse(fs.readFileSync(p, 'utf8'));
  } catch {
    return null;
  }
}

// $/M in and out for one model at one group ratio. `quota_type` 1 prices a
// whole call rather than its tokens, which has no per-million equivalent — the
// price is reported as `perCall` and the cost column stays empty rather than
// showing a number that means something else.
export function priceAt(model, groupRatio = 1) {
  const ratio = Number.isFinite(groupRatio) ? groupRatio : 1;
  if (model.perCallBase != null) return { perCall: round(model.perCallBase * ratio, 6) };
  if (model.promptBase == null) return null;
  const prompt = round(model.promptBase * ratio);
  return { prompt, completion: round(prompt * (model.completionRatio ?? 1)) };
}

// Every tier that serves this model, cheapest first, each with what it charges.
// `label` is the relay's own description of the group where it has one — Yunwu
// names its groups in Chinese ("官转" — official passthrough), OpenLux repeats
// the id.
export function tiersFor(model, groupRatio = {}, groupLabels = {}) {
  return (model.groups || [])
    .filter((g) => Number.isFinite(Number(groupRatio[g])))
    .map((g) => {
      const ratio = Number(groupRatio[g]);
      const label = groupLabels[g] && groupLabels[g] !== g ? String(groupLabels[g]) : '';
      return { id: g, ratio, ...(label ? { label } : {}), pricing: priceAt(model, ratio) };
    })
    .sort((a, b) => a.ratio - b.ratio || a.id.localeCompare(b.id));
}

// The catalogue as picker rows. Each row is priced at its cheapest tier, so the
// model column answers "what would this cost me at best" — the tier menu that
// follows shows what the other routes charge.
//
// Ordering follows the relay's own `sort_order` (how it ranks models on its
// site), then how much traffic a model actually gets, then the name, so the
// list is stable between fetches.
export function mapNewApiModels(payload) {
  const data = Array.isArray(payload?.data) ? payload.data : [];
  const groupRatio = payload?.group_ratio && typeof payload.group_ratio === 'object' ? payload.group_ratio : {};
  const groupLabels = payload?.usable_group && typeof payload.usable_group === 'object' ? payload.usable_group : {};
  const vendors = new Map((Array.isArray(payload?.vendors) ? payload.vendors : []).map((v) => [v?.id, v?.name]).filter(([id]) => id != null));

  const models = data
    .filter((m) => m?.model_name)
    .filter((m) => (m.supported_endpoint_types || []).some((e) => CHAT_ENDPOINTS.has(e)))
    .sort(
      (a, b) =>
        (b.sort_order || 0) - (a.sort_order || 0) ||
        (b.usage_count || 0) - (a.usage_count || 0) ||
        String(a.model_name).localeCompare(String(b.model_name))
    )
    .map((m) => {
      const perCall = m.quota_type === 1 && Number(m.model_price) > 0;
      const row = {
        id: m.model_name,
        name: m.model_name,
        endpoints: (m.supported_endpoint_types || []).filter((e) => CHAT_ENDPOINTS.has(e)),
        groups: [...new Set(m.enable_groups || [])].sort(),
        completionRatio: Number(m.completion_ratio) || 1
      };
      if (perCall) row.perCallBase = Number(m.model_price);
      else if (Number.isFinite(Number(m.model_ratio))) row.promptBase = round(Number(m.model_ratio) * USD_PER_RATIO_UNIT);
      const vendor = vendors.get(m.vendor_id);
      if (vendor) row.vendor = vendor;
      return row;
    });

  // A model nobody can reach — every group it lists has been retired — is not
  // something the picker should offer.
  for (const m of models) m.tiers = tiersFor(m, groupRatio, groupLabels);
  return models.filter((m) => m.tiers.length).map((m) => withTier(m, m.tiers[0].id));
}

// The same row priced at one named tier. Unknown tier → the cheapest, which is
// what an unset choice should mean.
export function withTier(model, tierId) {
  const tiers = model?.tiers || [];
  const tier = tiers.find((t) => t.id === tierId) || tiers[0];
  if (!tier) return model;
  const out = { ...model, tier: tier.id, tierRatio: tier.ratio };
  if (tier.pricing?.prompt != null) out.pricing = { prompt: tier.pricing.prompt, completion: tier.pricing.completion };
  else delete out.pricing;
  if (tier.pricing?.perCall != null) out.perCall = tier.pricing.perCall;
  else delete out.perCall;
  return out;
}

export const modelById = (models, id) => (models || []).find((m) => m.id === id) || null;

// ---- fetching ----

// The catalogue is public, so this needs no key and works before the user has
// created a token. A copy is kept per provider so the picker opens instantly
// and still works offline.
export async function fetchNewApiCatalogue({ id, baseUrl, timeout = 8000, signal = null } = {}) {
  if (!id || !baseUrl) throw new Error('a new-api provider needs an id and a baseUrl');
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeout);
  const stop = () => ctrl.abort();
  signal?.addEventListener('abort', stop, { once: true });
  try {
    const res = await fetch(`${String(baseUrl).replace(/\/+$/, '')}/api/pricing`, {
      signal: ctrl.signal,
      headers: { accept: 'application/json', connection: 'close' }
    });
    if (!res.ok) throw new Error('HTTP ' + res.status);
    const models = mapNewApiModels(await res.json());
    if (!models.length) throw new Error('no chat models in the catalogue');
    fs.mkdirSync(BRO_DIR, { recursive: true });
    fs.writeFileSync(newApiCachePath(id), JSON.stringify(models));
    return models;
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', stop);
  }
}

// The stored catalogue with its age in ms, or null.
export function readNewApiCache(providerId) {
  const cached = readJson(newApiCachePath(providerId));
  if (!Array.isArray(cached) || !cached.length) return null;
  let age = Infinity;
  try {
    age = Date.now() - fs.statSync(newApiCachePath(providerId)).mtimeMs;
  } catch {}
  return { models: cached, age };
}

// `maxAge` lets the picker accept a recent copy without touching the network.
// A failed fetch falls back to the last copy, then to whatever static list the
// provider carries in models.json.
export async function loadNewApiCatalogue({ id, baseUrl, maxAge = 0, fallback = null, signal = null } = {}) {
  if (maxAge > 0) {
    const cached = readNewApiCache(id);
    if (cached && cached.age <= maxAge) return cached.models;
  }
  try {
    return await fetchNewApiCatalogue({ id, baseUrl, signal });
  } catch {
    return readNewApiCache(id)?.models || (Array.isArray(fallback) && fallback.length ? fallback : null);
  }
}

export const isNewApi = (provider) => provider?.catalogue === 'newapi' && Boolean(provider?.baseUrl);

// ---- keys ----

// One token per tier, because that is how the relay works: the group is chosen
// when the token is created and cannot be overridden per request. `keys.openlux`
// stays the fallback, so a single token still launches everything it is allowed
// to reach.
export const tierKeyId = (providerId, tier) => (tier ? `${providerId}@${tier}` : providerId);

export function newApiKey({ providerId, tier, config = {}, env = process.env, keyEnv = '' } = {}) {
  const keys = config.keys || {};
  return (
    (tier && keys[tierKeyId(providerId, tier)]) ||
    keys[providerId] ||
    (keyEnv && env[keyEnv]) ||
    ''
  );
}

// ---- per-model routing ----

// A relay speaks both protocols, but not for every model: Claude models are
// served on /v1/messages, GPT and the rest only on /v1/chat/completions and
// /v1/responses. bro's provider carries a single mode, so the launcher is given
// a copy of the provider shaped for the model actually chosen — native Claude
// Code where the model speaks Anthropic, the OpenAI proxy where it does not.
// The id is left alone so saved keys and the proxy's route name still match.
export function providerForModel(provider, model) {
  if (!isNewApi(provider) || !model) return provider;
  const endpoints = model.endpoints;
  // Offline, the only model list left is the static fallback in models.json,
  // whose rows say nothing about protocols. Guessing there would route a Claude
  // model through the proxy; the provider's own declared mode is the better
  // answer, and it is what every non-relay provider uses anyway.
  if (!Array.isArray(endpoints) || !endpoints.length) return provider;
  const base = String(provider.baseUrl).replace(/\/+$/, '');
  if (endpoints.includes('anthropic')) return { ...provider, mode: 'anthropic', baseUrl: base };
  return { ...provider, mode: 'openai', baseUrl: `${base}/v1/chat/completions` };
}

// What a chosen row costs, for the launch summary and the tier menu.
export function priceLabel(pricing) {
  if (!pricing) return '';
  if (pricing.perCall != null) return `$${pricing.perCall}/call`;
  if (pricing.prompt == null) return '';
  // Three significant-ish figures, then trimmed: a tier that turns $10 into
  // $0.368 needs the decimals, and $5 should not read as "$5.0".
  const trim = (s) => (s.includes('.') ? s.replace(/0+$/, '').replace(/\.$/, '') : s);
  const fmt = (n) => trim(n.toFixed(n >= 10 ? 1 : n >= 1 ? 2 : 3));
  return `$${fmt(pricing.prompt)}/$${fmt(pricing.completion)} per M`;
}

// ---- tier menu rows ----

const pad = (s, w) => {
  s = String(s ?? '');
  return s.length >= w ? s.slice(0, Math.max(0, w)) : s + ' '.repeat(w - s.length);
};
const padLeft = (s, w) => {
  s = String(s ?? '');
  return s.length >= w ? s.slice(0, Math.max(0, w)) : ' '.repeat(w - s.length) + s;
};

// How much a tier multiplies the model's list price. Shown as "×0.04" rather
// than a percentage because that is how the relays themselves quote it, and
// because the interesting tiers are the ones an order of magnitude below 1.
export function ratioLabel(ratio) {
  if (!Number.isFinite(ratio)) return '';
  if (ratio >= 10) return `×${Math.round(ratio)}`;
  if (ratio >= 1) return `×${ratio.toFixed(2).replace(/\.?0+$/, '')}`;
  return `×${ratio.toFixed(3).replace(/0+$/, '').replace(/\.$/, '')}`;
}

// Whether a saved key exists for this exact tier — the menu says so, because a
// tier you have no token for is a tier bro will have to stop and ask about.
const TIER_COLS = { ratio: 7, price: 20 };

export function tierRow(tier, { width = 80, keyed = false, best = null } = {}) {
  const price = priceLabel(tier.pricing);
  const facts = `  ${padLeft(ratioLabel(tier.ratio), TIER_COLS.ratio)}  ${pad(price, TIER_COLS.price)}`;
  const nameW = Math.max(10, width - facts.length - 2);
  const mark = keyed ? '\x1b[32m•\x1b[0m ' : '  ';
  const cheapest = best != null && tier.ratio <= best ? ' \x1b[2m(cheapest)\x1b[0m' : '';
  const name = tier.label ? `${tier.id}  ${tier.label}` : tier.id;
  const shown = name.length > nameW ? name.slice(0, nameW - 1) + '…' : name;
  return mark + pad(shown, nameW) + facts + cheapest;
}

export function tierHeader({ width = 80 } = {}) {
  const facts = `  ${padLeft('price ×', TIER_COLS.ratio)}  ${pad('$/M in·out', TIER_COLS.price)}`;
  const nameW = Math.max(10, width - facts.length - 2);
  return '  ' + pad('tier (upstream route)', nameW) + facts;
}
