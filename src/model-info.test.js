import { expect, test } from 'bun:test';
import {
  ageLabel,
  formatPrice,
  costRating,
  speedRating,
  qualityRating,
  topQuality,
  meter,
  modelKey,
  catalogueIndex,
  enrichFromCatalogue,
  modelRow,
  modelHeader,
  anySpeed,
  formatCents,
  formatDuration,
  imageCostRating,
  videoCostRating,
  generationSpeedRating,
  arenaRating,
  mediaModelFacts
} from './model-info.js';

const DAY = 86400000;
const NOW = Date.UTC(2026, 8, 1);
const at = (daysAgo) => Math.round((NOW - daysAgo * DAY) / 1000);

test('age reads as new, days, months or years and stays four characters or fewer', () => {
  expect(ageLabel(at(2), NOW)).toBe('new');
  expect(ageLabel(at(20), NOW)).toBe('20d');
  expect(ageLabel(at(65), NOW)).toBe('2mo');
  expect(ageLabel(at(330), NOW)).toBe('11mo');
  expect(ageLabel(at(400), NOW)).toBe('1.1y');
  expect(ageLabel(at(365.25 * 2), NOW)).toBe('2y');
  expect(ageLabel(undefined, NOW)).toBe('');
});

test('prices per million tokens are quoted the way people say them', () => {
  expect(formatPrice(0)).toBe('$0');
  expect(formatPrice(0.15)).toBe('$0.15');
  expect(formatPrice(0.3)).toBe('$0.3');
  expect(formatPrice(2)).toBe('$2');
  expect(formatPrice(2.5)).toBe('$2.5');
  expect(formatPrice(15)).toBe('$15');
  expect(formatPrice(150)).toBe('$150');
  expect(formatPrice(null)).toBe('');
});

test('cost is rated on a 3:1 input-weighted blend, free at zero', () => {
  expect(costRating({ prompt: 0, completion: 0 })).toBe(0);
  expect(costRating({ prompt: 0.07, completion: 0.2 })).toBe(1);
  expect(costRating({ prompt: 0.3, completion: 2.5 })).toBe(2);
  expect(costRating({ prompt: 2, completion: 10 })).toBe(4);
  expect(costRating({ prompt: 15, completion: 75 })).toBe(5);
  expect(costRating(null)).toBeNull();
});

test('speed is rated from median tokens per second', () => {
  expect(speedRating(10)).toBe(1);
  expect(speedRating(57)).toBe(3);
  expect(speedRating(95)).toBe(4);
  expect(speedRating(180)).toBe(5);
  expect(speedRating(null)).toBeNull();
});

test('quality is rated against the best coding score in the list', () => {
  const models = [
    { id: 'a', quality: { coding: 78, intelligence: 63 } },
    { id: 'b', quality: { coding: 44, intelligence: 40 } },
    { id: 'c', quality: { coding: null, intelligence: 20 } },
    { id: 'd' }
  ];
  const top = topQuality(models);
  expect(top).toBe(78);
  expect(qualityRating(models[0].quality, top)).toBe(5);
  expect(qualityRating(models[1].quality, top)).toBe(3);
  expect(qualityRating(models[2].quality, top)).toBe(1);
  expect(qualityRating(undefined, top)).toBeNull();
  expect(topQuality([{ id: 'x' }])).toBeNull();
});

test('a meter is five glyphs wide and blank for an unknown', () => {
  expect(meter(3, '★', '☆')).toBe('★★★☆☆');
  expect(meter(0, '$', '·')).toBe('·····');
  expect(meter(null, '●', '○')).toBe('     ');
});

test('provider model ids match their OpenRouter catalogue entry', () => {
  expect(modelKey('claude-haiku-4-5-20251001')).toBe(modelKey('anthropic/claude-haiku-4.5'));
  expect(modelKey('claude-opus-4-8')).toBe(modelKey('anthropic/claude-opus-4.8'));
  expect(modelKey('glm-5.3')).toBe(modelKey('z-ai/glm-5.3'));
  expect(modelKey('kimi-k2-0711-preview')).toBe(modelKey('moonshotai/kimi-k2'));
  expect(modelKey('gpt-4o')).toBe(modelKey('openai/gpt-4o'));
  expect(modelKey('deepseek/deepseek-chat:free')).toBe('deepseekchat');
  expect(modelKey('glm-5.3')).not.toBe(modelKey('z-ai/glm-5.3-flash'));
  expect(modelKey('')).toBe('');
});

test('catalogue facts flow onto a provider row without touching its id or name', () => {
  const catalogue = [
    { id: 'z-ai/glm-5.3', name: 'Z.AI: GLM 5.3', created: 5, pricing: { prompt: 1, completion: 3 }, quality: { coding: 74.8 } },
    { id: 'z-ai/glm-5.3:free', name: 'Z.AI: GLM 5.3 (free)', created: 6 }
  ];
  const index = catalogueIndex(catalogue);
  expect(enrichFromCatalogue({ id: 'glm-5.3', name: 'GLM 5.3' }, index)).toEqual({
    id: 'glm-5.3',
    name: 'GLM 5.3',
    catalogueId: 'z-ai/glm-5.3',
    created: 5,
    pricing: { prompt: 1, completion: 3 },
    quality: { coding: 74.8 }
  });
  const unknown = { id: 'fugu', name: 'Fugu' };
  expect(enrichFromCatalogue(unknown, index)).toBe(unknown);
  expect(enrichFromCatalogue({ name: 'Default (your login)' }, index)).toEqual({ name: 'Default (your login)' });
});

const sonnet = {
  id: 'anthropic/claude-sonnet-5',
  name: 'Anthropic: Claude Sonnet 5',
  created: at(60),
  pricing: { prompt: 2, completion: 10 },
  quality: { coding: 68, intelligence: 60 },
  speed: { tps: 57, ttft: 2050 }
};
const bare = { id: 'openrouter/fusion', name: 'OpenRouter: Fusion', created: at(90) };

test('a wide row shows every column with its numbers, aligned under the header', () => {
  const opts = { width: 110, now: NOW, top: 78 };
  const header = modelHeader(opts);
  const row = modelRow(sonnet, opts).replace(/\x1b\[[0-9;]*m/g, '');
  expect(header.length).toBe(110);
  expect(row.length).toBe(110);
  expect(row).toContain('anthropic/claude-sonnet-5');
  expect(row).toContain('  2mo  $$$$· $2·$10');
  expect(row).toContain('●●●○○ 57t/s');
  expect(row).toContain('★★★★☆ 68');
  expect(header.indexOf('age')).toBe(row.indexOf('2mo'));
  expect(header.indexOf('cost')).toBe(row.indexOf('$$$$·'));
  expect(header.indexOf('speed')).toBe(row.indexOf('●●●○○'));
  expect(header.indexOf('quality')).toBe(row.indexOf('★★★★☆'));
});

test('a medium row keeps the meters and drops the numbers; a narrow row keeps age and quality', () => {
  const medium = modelRow(sonnet, { width: 70, now: NOW, top: 78 });
  expect(medium.length).toBe(70);
  expect(medium.endsWith('2mo  $$$$·  ●●●○○  ★★★★☆')).toBe(true);
  const narrow = modelRow(sonnet, { width: 46, now: NOW, top: 78 });
  expect(narrow.length).toBe(46);
  expect(narrow.endsWith('2mo  ★★★★☆')).toBe(true);
  expect(narrow).not.toContain('$');
});

test('unknown facts leave their column blank rather than scoring low', () => {
  const row = modelRow(bare, { width: 70, now: NOW, top: 78 });
  expect(row).toBe('OpenRouter: Fusion  \x1b[2mopenrouter/fusion\x1b[0m         3mo                     ');
});

test('cost can be hidden and the speed column left out entirely', () => {
  const opts = { width: 110, now: NOW, top: 78, showCost: false, showSpeed: false };
  const header = modelHeader(opts);
  const row = modelRow(sonnet, opts).replace(/\x1b\[[0-9;]*m/g, '');
  expect(header).not.toContain('speed');
  expect(header).not.toContain('cost');
  expect(header.length).toBe(110);
  expect(row.length).toBe(110);
  expect(row).not.toContain('●');
  expect(row).not.toContain('$2');
  expect(row).toContain('★★★★☆ 68');
  expect(anySpeed([sonnet, bare])).toBe(true);
  expect(anySpeed([bare])).toBe(false);
});

test('a long name is truncated with an ellipsis instead of pushing the columns', () => {
  const long = { ...bare, name: 'A very long model name that goes on and on and on and on' };
  const row = modelRow(long, { width: 46, now: NOW, top: 78 });
  expect(row.length).toBe(46);
  expect(row).toContain('…');
  expect(row.endsWith('3mo       ')).toBe(true);
});

test('media prices and durations read the way people say them', () => {
  expect(formatCents(0.0387)).toBe('3.9¢');
  expect(formatCents(0.14)).toBe('14¢');
  expect(formatCents(1.2)).toBe('$1.20');
  expect(formatCents(0)).toBe('free');
  expect(formatDuration(12400)).toBe('12s');
  expect(formatDuration(100000)).toBe('1m 40s');
  expect(formatDuration(15 * 60000)).toBe('15m');
});

test('image, video, speed and arena ratings each use a scale that fits the medium', () => {
  expect(imageCostRating(0.008)).toBe(1);
  expect(imageCostRating(0.039)).toBe(2);
  expect(imageCostRating(0.134)).toBe(4);
  expect(videoCostRating(0.03)).toBe(1);
  expect(videoCostRating(0.14)).toBe(3);
  expect(videoCostRating(0.5)).toBe(5);
  expect(generationSpeedRating(6000, 'image')).toBe(5);
  expect(generationSpeedRating(45000, 'image')).toBe(2);
  expect(generationSpeedRating(45000, 'video')).toBe(5);
  expect(generationSpeedRating(300000, 'video')).toBe(2);
  expect(generationSpeedRating(0, 'video')).toBeNull();
  expect(arenaRating(65.1)).toBe(5);
  expect(arenaRating(55.6)).toBe(3);
  expect(arenaRating(null)).toBeNull();
});

test('a gallery model row gets display-ready facts, blank where nothing is known', () => {
  const image = {
    id: 'google/gemini-2.5-flash-image',
    created: at(60),
    pricing: { perImage: 0.0387, imageOutput: 30 },
    quality: { arena: 'image', winRate: 55.6, rank: 8 }
  };
  const facts = mediaModelFacts(image, { kind: 'image', now: NOW, timing: { median: 12400, n: 8 } });
  expect(facts.age.label).toBe('2mo');
  expect(facts.cost).toMatchObject({ rating: 2, label: '≈3.9¢' });
  expect(facts.cost.title).toContain('$30/M output tokens');
  expect(facts.speed).toMatchObject({ rating: 4, label: '12s' });
  expect(facts.speed.title).toContain('8 generations');
  expect(facts.quality).toMatchObject({ rating: 3, label: '#8' });

  const video = { id: 'google/veo-3.1-lite', created: at(3), pricing: { perSecond: 0.03, basis: '720p' } };
  const v = mediaModelFacts(video, { kind: 'video', now: NOW });
  expect(v.age.label).toBe('new');
  expect(v.cost).toMatchObject({ rating: 1, label: '3¢/s' });
  expect(v.cost.title).toContain('15¢ for a 5-second clip at 720p');
  expect(v.speed).toBeNull();
  expect(v.quality).toBeNull();

  expect(mediaModelFacts({ id: 'gpt-image-1' }, { kind: 'image', now: NOW })).toEqual({ age: null, cost: null, speed: null, quality: null });
});
