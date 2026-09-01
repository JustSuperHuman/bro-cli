import { expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { mapOpenRouterModels, mapOpenRouterVideoModels, mapOpenRouterImageModels, videoPricing, summarizeEndpointStats, attachStats } from './models.js';

test('OpenRouter mapping keeps models from every publisher and orders newest first', () => {
  const models = mapOpenRouterModels([
    { id: 'anthropic/claude-sonnet', name: 'Claude Sonnet', created: 20 },
    { id: 'google/gemini-pro', name: 'Gemini Pro', created: 30 },
    { id: 'meta/llama', name: 'Llama', created: 10 },
    { name: 'Missing id', created: 40 }
  ]);

  expect(models).toEqual([
    { id: 'google/gemini-pro', name: 'Gemini Pro', created: 30 },
    { id: 'anthropic/claude-sonnet', name: 'Claude Sonnet', created: 20 },
    { id: 'meta/llama', name: 'Llama', created: 10 }
  ]);
});

test('OpenRouter mapping falls back to the model id when its name is missing', () => {
  expect(mapOpenRouterModels([{ id: 'new/model' }])).toEqual([{ id: 'new/model', name: 'new/model' }]);
});

test('OpenRouter rows carry $/M prices, context and benchmark indices for the picker', () => {
  const [row] = mapOpenRouterModels([
    {
      id: 'anthropic/claude-sonnet-5',
      name: 'Anthropic: Claude Sonnet 5',
      created: 1782843083,
      context_length: 1000000,
      pricing: { prompt: '0.000002', completion: '0.00001', web_search: '0.01' },
      benchmarks: { artificial_analysis: { intelligence_index: 57.5, coding_index: 71.5, agentic_index: 58.2 } },
      supported_parameters: ['tools', 'reasoning']
    }
  ]);
  expect(row).toEqual({
    id: 'anthropic/claude-sonnet-5',
    name: 'Anthropic: Claude Sonnet 5',
    created: 1782843083,
    context: 1000000,
    pricing: { prompt: 2, completion: 10 },
    quality: { coding: 71.5, intelligence: 57.5, agentic: 58.2 },
    reasoning: true
  });
});

test('a router priced "-1" (varies) has no price, and models without benchmarks have no quality', () => {
  const [row] = mapOpenRouterModels([
    { id: 'openrouter/auto', name: 'Auto', pricing: { prompt: '-1', completion: '-1' }, benchmarks: { design_arena: [] } }
  ]);
  expect(row.pricing).toBeUndefined();
  expect(row.quality).toBeUndefined();
});

test('endpoint stats are summarised as the median p50 across hosts', () => {
  expect(
    summarizeEndpointStats([
      { provider_name: 'A', throughput_last_30m: { p50: 57 }, latency_last_30m: { p50: 2050 } },
      { provider_name: 'B', throughput_last_30m: { p50: 48 }, latency_last_30m: { p50: 2582 } },
      { provider_name: 'C', throughput_last_30m: { p50: 64.5 }, latency_last_30m: null }
    ])
  ).toEqual({ tps: 57, ttft: 2316 });
  expect(summarizeEndpointStats([{ throughput_last_30m: null, latency_last_30m: null }])).toBeNull();
  expect(summarizeEndpointStats(undefined)).toBeNull();
});

test('cached speed stats attach to matching rows and leave the rest untouched', () => {
  const rows = attachStats([{ id: 'a', name: 'A' }, { id: 'b', name: 'B' }], {
    a: { tps: 120, ttft: 900, at: 1 },
    b: { tps: null, ttft: null, at: 1 }
  });
  expect(rows).toEqual([{ id: 'a', name: 'A', speed: { tps: 120, ttft: 900 } }, { id: 'b', name: 'B' }]);
});

test("a row matched from another provider finds its stats under the catalogue's id", () => {
  const [row] = attachStats([{ id: 'glm-5.3', name: 'GLM 5.3', catalogueId: 'z-ai/glm-5.3' }], {
    'z-ai/glm-5.3': { tps: 31, ttft: 1200, at: 1 }
  });
  expect(row.speed).toEqual({ tps: 31, ttft: 1200 });
});

test('video models carry the capability lists that drive the JustImagine controls', () => {
  const models = mapOpenRouterVideoModels([
    {
      id: 'google/veo-3.1',
      name: 'Google: Veo 3.1',
      created: 30,
      supported_resolutions: ['720p', '1080p', '4K'],
      supported_aspect_ratios: ['16:9', '9:16'],
      supported_sizes: ['1280x720'],
      supported_durations: [4, 6, 8],
      supported_frame_images: ['first_frame', 'last_frame'],
      generate_audio: true,
      seed: true
    },
    { id: 'runway/aleph-2', created: 40, supported_aspect_ratios: ['1:1'], generate_audio: false },
    { name: 'no id', created: 50 }
  ]);

  expect(models.map((m) => m.id)).toEqual(['runway/aleph-2', 'google/veo-3.1']);
  expect(models[1]).toEqual({
    id: 'google/veo-3.1',
    name: 'Google: Veo 3.1',
    kind: 'video',
    created: 30,
    pricing: undefined,
    resolutions: ['720p', '1080p', '4K'],
    aspectRatios: ['16:9', '9:16'],
    sizes: ['1280x720'],
    durations: [4, 6, 8],
    frames: ['first_frame', 'last_frame'],
    audio: true,
    seed: true,
    upscale: false
  });
  // A model with no stated durations/resolutions becomes nulls, not empty
  // arrays, so the UI can tell "no constraint" from "no options".
  expect(models[0]).toMatchObject({ name: 'runway/aleph-2', durations: null, resolutions: null, audio: false, seed: false });
});

test('video prices are normalised to dollars per second at a standard tier', () => {
  // tiered by resolution: the 720p rate wins and is named
  expect(videoPricing({ duration_seconds_480p: '0.068', duration_seconds_720p: '0.14', duration_seconds_1080p: '0.28' })).toEqual({ perSecond: 0.14, basis: '720p' });
  // cents, with 720p and continuation variants
  expect(videoPricing({ cents_per_second_output: '17', cents_per_second_output_720p: '17', cents_per_second_output_1080p: '29', cents_per_second_video_continuation_720p: '41' })).toEqual({ perSecond: 0.17, basis: '720p' });
  // plain per-second rates: the cheapest untiered rate, 4K left aside
  expect(videoPricing({ duration_seconds_with_audio: '0.40', duration_seconds_with_audio_4k: '0.60', duration_seconds_without_audio: '0.20' })).toEqual({ perSecond: 0.2 });
  // Seedance bills per video token: converted with ByteDance's 720p formula
  expect(videoPricing({ video_tokens: '0.000007', video_tokens_with_video_input: '0.0000043' }).perSecond).toBeCloseTo(0.1512, 3);
  // image-input and minimum-charge SKUs are not per-second output prices
  expect(videoPricing({ cents_per_image_input: '1', cents_per_video_output_second_480p: '8', cents_per_video_output_second_720p: '14' })).toEqual({ perSecond: 0.14, basis: '720p' });
  expect(videoPricing({ cents_per_second_output: '28', minimum_cents_per_generation: '56' })).toEqual({ perSecond: 0.28 });
  // an upscaler is priced per megapixel of source: no comparable number
  expect(videoPricing({ cents_per_megapixel_second_precise: '7.5' })).toBeUndefined();
  expect(videoPricing(undefined)).toBeUndefined();
});

test('image models get an estimated price per picture and their Design Arena standing', () => {
  const models = mapOpenRouterImageModels([
    {
      id: 'google/gemini-2.5-flash-image',
      name: 'Google: Gemini 2.5 Flash Image',
      created: 20,
      architecture: { output_modalities: ['image', 'text'] },
      pricing: { prompt: '0.0000003', completion: '0.0000025', image_output: '0.00003' },
      benchmarks: { design_arena: [{ arena: 'models', category: 'graphicdesign', elo: 1191, win_rate: 56.9, rank: 8 }, { arena: 'models', category: 'image', elo: 1204, win_rate: 55.6, rank: 8 }] }
    },
    { id: 'openai/gpt-5-image', name: 'OpenAI: GPT-5 Image', created: 10, architecture: { output_modalities: ['image'] }, pricing: { prompt: '0.00001', image_output: '0.00004' } },
    { id: 'anthropic/claude-sonnet-5', name: 'text only', created: 30, architecture: { output_modalities: ['text'] } },
    { id: 'openrouter/auto', name: 'Auto', created: 1, architecture: { output_modalities: ['image', 'text'] }, pricing: { prompt: '-1', completion: '-1' } }
  ]);
  expect(models.map((m) => m.id)).toEqual(['google/gemini-2.5-flash-image', 'openai/gpt-5-image', 'openrouter/auto']);
  expect(models[0]).toEqual({
    id: 'google/gemini-2.5-flash-image',
    name: 'Google: Gemini 2.5 Flash Image',
    via: 'chat',
    kind: 'image',
    created: 20,
    pricing: { perImage: 0.0387, imageOutput: 30, prompt: 0.3 },
    quality: { arena: 'image', winRate: 55.6, rank: 8, elo: 1204 }
  });
  // OpenAI's medium 1024² picture is 1056 output tokens
  expect(models[1].pricing.perImage).toBeCloseTo(0.04224, 5);
  expect(models[1].quality).toBeUndefined();
  expect(models[2].pricing).toBeUndefined();
});

test('an image model without a score borrows the one measured on its preview release', () => {
  const models = mapOpenRouterImageModels([
    { id: 'google/gemini-3.1-flash-image', name: 'GA', created: 20, architecture: { output_modalities: ['image'] } },
    {
      id: 'google/gemini-3.1-flash-image-preview',
      name: 'Preview',
      created: 10,
      architecture: { output_modalities: ['image'] },
      benchmarks: { design_arena: [{ category: 'image', elo: 1294, win_rate: 65.1, rank: 2 }] }
    }
  ]);
  expect(models[0].quality).toEqual({ arena: 'image', winRate: 65.1, rank: 2, elo: 1294, from: 'google/gemini-3.1-flash-image-preview' });
  expect(models[1].quality.from).toBeUndefined();
});

test('an upscaling model is flagged so it is not offered length and audio knobs', () => {
  expect(mapOpenRouterVideoModels([{ id: 'black-forest-labs/flux-video-upscale', created: 1 }])[0].upscale).toBe(true);
  expect(mapOpenRouterVideoModels([{ id: 'x/y', created: 1, upscale_factor: 2 }])[0].upscale).toBe(true);
  expect(mapOpenRouterVideoModels([{ id: 'x/y', created: 1 }])[0].upscale).toBe(false);
});

test('the bundled video catalogue is a usable offline fallback', () => {
  const bundled = JSON.parse(readFileSync(new URL('../video-models.json', import.meta.url), 'utf8'));
  expect(Array.isArray(bundled.models)).toBe(true);
  expect(bundled.models.length).toBeGreaterThan(20);
  for (const m of bundled.models) {
    expect(typeof m.id).toBe('string');
    expect(m.kind).toBe('video');
  }
  // the headline models the help text promises are actually in there
  const ids = bundled.models.map((m) => m.id);
  for (const id of ['google/veo-3.1', 'openai/sora-2-pro', 'bytedance/seedance-2.0', 'alibaba/wan-3.0']) {
    expect(ids).toContain(id);
  }
});
