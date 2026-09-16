import { expect, test } from 'bun:test';
import {
  USD_PER_RATIO_UNIT,
  mapNewApiModels,
  tiersFor,
  priceAt,
  withTier,
  modelById,
  isNewApi,
  tierKeyId,
  newApiKey,
  providerForModel,
  priceLabel,
  ratioLabel,
  tierRow,
  tierHeader
} from './newapi.js';

// A trimmed copy of what /api/pricing actually returns, with the real numbers
// for the models used in the examples.
const PAYLOAD = {
  success: true,
  group_ratio: {
    'Codex-Gpt-1': 0.03677,
    'Codex-Gpt-3': 0.07353,
    'Openai-Gpt-2': 1,
    'Anthropic-Claude-1': 1.1,
    'Claude-Code-1': 0.17647,
    Retired: undefined
  },
  usable_group: {
    'Codex-Gpt-1': 'Codex-Gpt-1',
    'Openai-Gpt-2': 'Openai-Gpt-2',
    'Claude-Code-1': '官转 Claude Code'
  },
  vendors: [
    { id: 52, name: 'OpenAI' },
    { id: 55, name: 'Anthropic' }
  ],
  data: [
    {
      model_name: 'gpt-6-astra',
      model_type: '对话',
      vendor_id: 52,
      quota_type: 0,
      model_ratio: 5,
      completion_ratio: 5,
      enable_groups: ['Openai-Gpt-2', 'Codex-Gpt-1', 'Codex-Gpt-3'],
      supported_endpoint_types: ['openai', 'openai-response'],
      sort_order: 601,
      usage_count: 10
    },
    {
      model_name: 'claude-opus-5',
      model_type: '对话',
      vendor_id: 55,
      quota_type: 0,
      model_ratio: 7.5,
      completion_ratio: 5,
      enable_groups: ['Anthropic-Claude-1', 'Claude-Code-1'],
      supported_endpoint_types: ['anthropic', 'openai'],
      sort_order: 700,
      usage_count: 5
    },
    {
      model_name: 'sora-2',
      model_type: '音视频',
      quota_type: 1,
      model_price: 0.4,
      enable_groups: ['Openai-Gpt-2'],
      supported_endpoint_types: ['OpenAI video format'],
      sort_order: 900
    },
    {
      model_name: 'orphan-model',
      model_type: '对话',
      quota_type: 0,
      model_ratio: 1,
      completion_ratio: 2,
      enable_groups: ['Retired'],
      supported_endpoint_types: ['openai'],
      sort_order: 999
    },
    { model_name: '', supported_endpoint_types: ['openai'] }
  ]
};

test('a ratio of 1 is $2 per million input tokens, the one-api convention every fork inherited', () => {
  expect(USD_PER_RATIO_UNIT).toBe(2);
  // gpt-4o is model_ratio 1.25 at these relays and costs $2.50/M in, $10/M out.
  expect(priceAt({ promptBase: 1.25 * 2, completionRatio: 4 }, 1)).toEqual({ prompt: 2.5, completion: 10 });
});

test('the group ratio scales both halves of the price', () => {
  const model = { promptBase: 10, completionRatio: 5 };
  expect(priceAt(model, 1)).toEqual({ prompt: 10, completion: 50 });
  expect(priceAt(model, 0.03677)).toEqual({ prompt: 0.3677, completion: 1.8385 });
});

test('a per-call model is priced per call, not per million tokens', () => {
  expect(priceAt({ perCallBase: 0.4 }, 0.5)).toEqual({ perCall: 0.2 });
  expect(priceAt({}, 1)).toBeNull();
});

test('the catalogue keeps chat models, drops media models and drops names it cannot use', () => {
  const models = mapNewApiModels(PAYLOAD);
  expect(models.map((m) => m.id)).toEqual(['claude-opus-5', 'gpt-6-astra']);
});

test('a model whose every tier has been retired is dropped rather than offered unreachably', () => {
  expect(mapNewApiModels(PAYLOAD).find((m) => m.id === 'orphan-model')).toBeUndefined();
});

test('models are ordered by the relay\'s own ranking, then by traffic', () => {
  const models = mapNewApiModels({
    ...PAYLOAD,
    data: [
      { model_name: 'b', model_ratio: 1, enable_groups: ['Openai-Gpt-2'], supported_endpoint_types: ['openai'], sort_order: 10, usage_count: 1 },
      { model_name: 'c', model_ratio: 1, enable_groups: ['Openai-Gpt-2'], supported_endpoint_types: ['openai'], sort_order: 10, usage_count: 99 },
      { model_name: 'a', model_ratio: 1, enable_groups: ['Openai-Gpt-2'], supported_endpoint_types: ['openai'], sort_order: 50 }
    ]
  });
  expect(models.map((m) => m.id)).toEqual(['a', 'c', 'b']);
});

test('each row is priced at its cheapest tier, so the model column shows the best available price', () => {
  const astra = modelById(mapNewApiModels(PAYLOAD), 'gpt-6-astra');
  expect(astra.tier).toBe('Codex-Gpt-1');
  expect(astra.pricing).toEqual({ prompt: 0.3677, completion: 1.8385 });
  // The list price is still reachable — it is just the expensive end.
  expect(astra.tiers.map((t) => t.id)).toEqual(['Codex-Gpt-1', 'Codex-Gpt-3', 'Openai-Gpt-2']);
  expect(astra.tiers.at(-1).pricing).toEqual({ prompt: 10, completion: 50 });
});

test('tiers carry the relay\'s own group description when it says more than the id', () => {
  const opus = modelById(mapNewApiModels(PAYLOAD), 'claude-opus-5');
  expect(opus.tiers.find((t) => t.id === 'Claude-Code-1').label).toBe('官转 Claude Code');
  expect(opus.tiers.find((t) => t.id === 'Anthropic-Claude-1').label).toBeUndefined();
});

test('a group with no published ratio is not offered as a tier', () => {
  const tiers = tiersFor({ groups: ['Openai-Gpt-2', 'Retired'], promptBase: 2, completionRatio: 1 }, PAYLOAD.group_ratio);
  expect(tiers.map((t) => t.id)).toEqual(['Openai-Gpt-2']);
});

test('withTier reprices the row and an unknown tier falls back to the cheapest', () => {
  const astra = modelById(mapNewApiModels(PAYLOAD), 'gpt-6-astra');
  expect(withTier(astra, 'Openai-Gpt-2').pricing).toEqual({ prompt: 10, completion: 50 });
  expect(withTier(astra, 'Openai-Gpt-2').tier).toBe('Openai-Gpt-2');
  expect(withTier(astra, 'no-such-tier').tier).toBe('Codex-Gpt-1');
});

test('the vendor is kept so a filter for "openai" finds the models OpenAI made', () => {
  expect(modelById(mapNewApiModels(PAYLOAD), 'gpt-6-astra').vendor).toBe('OpenAI');
});

test('an empty or unusable payload yields no models rather than throwing', () => {
  expect(mapNewApiModels(null)).toEqual([]);
  expect(mapNewApiModels({ data: [] })).toEqual([]);
});

// ---- routing ----

const OPENLUX = { id: 'openlux', name: 'OpenLux', catalogue: 'newapi', mode: 'anthropic', baseUrl: 'https://api.openlux.ai' };

test('only a provider with both the newapi marker and a base URL is treated as a relay', () => {
  expect(isNewApi(OPENLUX)).toBe(true);
  expect(isNewApi({ ...OPENLUX, catalogue: undefined })).toBe(false);
  expect(isNewApi({ ...OPENLUX, baseUrl: '' })).toBe(false);
  expect(isNewApi(null)).toBe(false);
});

test('a Claude model at a relay runs Claude Code straight against /v1/messages', () => {
  const models = mapNewApiModels(PAYLOAD);
  const routed = providerForModel(OPENLUX, modelById(models, 'claude-opus-5'));
  expect(routed.mode).toBe('anthropic');
  expect(routed.baseUrl).toBe('https://api.openlux.ai');
  expect(routed.id).toBe('openlux');
});

test('a model the relay only serves in OpenAI format goes through the proxy instead', () => {
  const models = mapNewApiModels(PAYLOAD);
  const routed = providerForModel(OPENLUX, modelById(models, 'gpt-6-astra'));
  expect(routed.mode).toBe('openai');
  expect(routed.baseUrl).toBe('https://api.openlux.ai/v1/chat/completions');
});

test('a trailing slash on the relay base URL does not double up in the routed URL', () => {
  const routed = providerForModel({ ...OPENLUX, baseUrl: 'https://api.openlux.ai/' }, { endpoints: ['openai'] });
  expect(routed.baseUrl).toBe('https://api.openlux.ai/v1/chat/completions');
});

test('a model row with no protocol information keeps the declared route', () => {
  // Offline, the static fallback list is all there is and its rows say nothing
  // about protocols — guessing would send a Claude model through the proxy.
  expect(providerForModel(OPENLUX, { id: 'claude-opus-5' })).toBe(OPENLUX);
  expect(providerForModel(OPENLUX, { id: 'claude-opus-5', endpoints: [] })).toBe(OPENLUX);
});

test('a provider that is not a relay is passed through untouched', () => {
  const zai = { id: 'zai', mode: 'anthropic', baseUrl: 'https://api.z.ai/api/anthropic' };
  expect(providerForModel(zai, { endpoints: ['openai'] })).toBe(zai);
});

// ---- keys ----

test('a tier gets its own key slot, with the provider key as the fallback', () => {
  expect(tierKeyId('openlux', 'Codex-Gpt-1')).toBe('openlux@Codex-Gpt-1');
  expect(tierKeyId('openlux', '')).toBe('openlux');

  const config = { keys: { openlux: 'sk-shared', 'openlux@Codex-Gpt-1': 'sk-codex' } };
  expect(newApiKey({ providerId: 'openlux', tier: 'Codex-Gpt-1', config })).toBe('sk-codex');
  expect(newApiKey({ providerId: 'openlux', tier: 'Openai-Gpt-2', config })).toBe('sk-shared');
});

test('the environment variable is the last resort, not an override of a saved key', () => {
  const env = { OPENLUX_API_KEY: 'sk-env' };
  expect(newApiKey({ providerId: 'openlux', tier: 'Codex-Gpt-1', config: {}, env, keyEnv: 'OPENLUX_API_KEY' })).toBe('sk-env');
  expect(
    newApiKey({ providerId: 'openlux', tier: 'Codex-Gpt-1', config: { keys: { openlux: 'sk-saved' } }, env, keyEnv: 'OPENLUX_API_KEY' })
  ).toBe('sk-saved');
  expect(newApiKey({ providerId: 'openlux', tier: 'x', config: {}, env: {} })).toBe('');
});

// ---- labels ----

test('a price reads as $/M in and out, or per call', () => {
  expect(priceLabel({ prompt: 0.3677, completion: 1.8385 })).toBe('$0.368/$1.84 per M');
  expect(priceLabel({ prompt: 10, completion: 50 })).toBe('$10/$50 per M');
  expect(priceLabel({ prompt: 5, completion: 25 })).toBe('$5/$25 per M');
  expect(priceLabel({ prompt: 0.1, completion: 0.4 })).toBe('$0.1/$0.4 per M');
  expect(priceLabel({ perCall: 0.2 })).toBe('$0.2/call');
  expect(priceLabel(null)).toBe('');
});

test('a tier ratio reads as the multiplier the relays themselves quote', () => {
  expect(ratioLabel(0.03677)).toBe('×0.037');
  expect(ratioLabel(1)).toBe('×1');
  expect(ratioLabel(1.65)).toBe('×1.65');
  expect(ratioLabel(NaN)).toBe('');
});

test('a tier row fits its width and marks the tiers a token is already saved for', () => {
  const tier = { id: 'Codex-Gpt-1', ratio: 0.03677, pricing: { prompt: 0.3677, completion: 1.8385 } };
  const row = tierRow(tier, { width: 70, keyed: true, best: 0.03677 });
  expect(row).toContain('Codex-Gpt-1');
  expect(row).toContain('×0.037');
  expect(row).toContain('$0.368/$1.84 per M');
  expect(row).toContain('(cheapest)');
  // The green dot only appears for a tier that has a token saved.
  expect(tierRow(tier, { width: 70, keyed: false, best: 0.03677 })).not.toContain('\x1b[32m');
  expect(tierHeader({ width: 70 })).toContain('$/M in·out');
});
