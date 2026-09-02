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
  mediaModelFacts,
  cleanDescription,
  bestResolution,
  modelChips,
  enrichMediaFacts,
  mediaModelDetail,
  costNote,
  qualityNote,
  mediaKey,
  arenaIndex,
  arenaQuality
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

test('a catalogue description becomes a couple of plain sentences', () => {
  // Markdown links keep their text and lose their target.
  expect(cleanDescription('Wan 3.0 Prime is a variant of [Wan 3.0](https://openrouter.ai/x) from Alibaba. It does image-to-video.')).toBe(
    'Wan 3.0 Prime is a variant of Wan 3.0 from Alibaba. It does image-to-video.'
  );
  // An abbreviation is not a sentence end, so the first sentence survives whole.
  const nano = cleanDescription('Gemini 3.1 Flash Image, a.k.a. "Nano Banana 2," is Google\'s latest model. It is fast. And a third.');
  expect(nano).toBe('Gemini 3.1 Flash Image, a.k.a. "Nano Banana 2," is Google\'s latest model. It is fast.');
  // A decimal point does not end a sentence either.
  expect(cleanDescription('Built on Gemini 2.5 Pro for speed.')).toBe('Built on Gemini 2.5 Pro for speed.');
  // Code fences and images go entirely; emphasis markers go but their words stay.
  expect(cleanDescription('A model. ```js\ncode\n``` ![pic](p.png)')).toBe('A model.');
  expect(cleanDescription('A **fast** model.')).toBe('A fast model.');
  expect(cleanDescription('')).toBe('');
  expect(cleanDescription(null)).toBe('');
  expect(cleanDescription(undefined)).toBe('');

  // A single sentence longer than the budget is cut on a word boundary.
  const long = cleanDescription('x'.repeat(400));
  expect(long.length).toBeLessThanOrEqual(260);
  expect(long.endsWith('…')).toBe(true);

  // The sentence budget is respected.
  expect(cleanDescription('One. Two. Three.', { sentences: 1 })).toBe('One.');
});

test('the tallest resolution a model offers is named the way people say it', () => {
  expect(bestResolution(['480p', '720p', '1080p'])).toBe('1080p');
  expect(bestResolution(['1080p', '4k'])).toBe('4K');
  expect(bestResolution(['720p'])).toBe('720p');
  expect(bestResolution([])).toBe('');
  expect(bestResolution(null)).toBe('');
});

test('capability chips are read off the catalogue, never guessed', () => {
  const video = {
    durations: [2, 5, 10],
    resolutions: ['480p', '1080p'],
    aspectRatios: ['16:9', '9:16'],
    audio: true,
    frames: ['first_frame', 'last_frame'],
    seed: true
  };
  expect(modelChips(video, 'video').map((c) => c.label)).toEqual(['to 10s', '1080p', 'audio', '1st frame', 'last frame', 'seed', '2 ratios']);
  // The duration chip's hover carries the full range, not just the maximum.
  expect(modelChips(video, 'video')[0].title).toContain('2–10 seconds');

  // Nothing known means no chips invented.
  expect(modelChips({}, 'video')).toEqual([]);
  expect(modelChips(null, 'video')).toEqual([]);

  // An upscaler leads with what it is, because a prompt alone cannot drive it.
  expect(modelChips({ upscale: true }, 'video')[0].label).toBe('upscaler');

  // Image models say whether the size/quality knobs apply to them.
  expect(modelChips({ via: 'chat' }, 'image').map((c) => c.label)).toEqual(['reads refs']);
  expect(modelChips({}, 'image').map((c) => c.label)).toEqual(['size & quality']);
});

test('a provider borrows display facts for the same model but never its routing', () => {
  const index = catalogueIndex([
    {
      id: 'google/gemini-3.1-flash-image',
      name: 'Google: Gemini 3.1 Flash Image',
      via: 'chat',
      kind: 'image',
      created: 1770000000,
      pricing: { perImage: 0.067 },
      quality: { winRate: 65.1, rank: 2 },
      description: 'Nano Banana 2.'
    }
  ]);

  // An aggregator serving the same model under a shorter id gets the numbers.
  const yunwu = enrichMediaFacts({ id: 'gemini-3.1-flash-image', name: 'Gemini 3.1 Flash Image' }, index);
  expect(yunwu.pricing.perImage).toBe(0.067);
  expect(yunwu.quality.rank).toBe(2);
  expect(yunwu.description).toBe('Nano Banana 2.');
  expect(yunwu.factsFrom).toBe('google/gemini-3.1-flash-image');
  // Crucially not `via` — borrowing it would route this provider's call at the
  // wrong endpoint.
  expect(yunwu.via).toBeUndefined();
  expect(yunwu.name).toBe('Gemini 3.1 Flash Image');

  // The provider's own facts win over the catalogue's.
  const own = enrichMediaFacts({ id: 'gemini-3.1-flash-image', description: 'Mine.' }, index);
  expect(own.description).toBe('Mine.');

  // Nothing matching, and the model whose facts these already are, are untouched.
  expect(enrichMediaFacts({ id: 'dall-e-3' }, index).factsFrom).toBeUndefined();
  expect(enrichMediaFacts({ id: 'google/gemini-3.1-flash-image' }, index).factsFrom).toBeUndefined();
});

test('a borrowed price says whose price it is', () => {
  const borrowed = { id: 'gemini-3.1-flash-image', factsFrom: 'google/gemini-3.1-flash-image', pricing: { perImage: 0.067, imageOutput: 120 }, quality: { winRate: 65, rank: 2 } };
  const f = mediaModelFacts(borrowed, { kind: 'image' });
  expect(f.cost.title).toContain('this provider may charge differently');
  expect(f.cost.title).toContain('google/gemini-3.1-flash-image');
  // Quality is measured, not charged — the note reads accordingly.
  expect(f.quality.title).toContain('measured on google/gemini-3.1-flash-image');
  expect(f.quality.title).not.toContain('charge differently');

  // A model's own numbers carry no such caveat.
  const own = mediaModelFacts({ id: 'x', pricing: { perImage: 0.067, imageOutput: 120 } }, { kind: 'image' });
  expect(own.cost.title).not.toContain('may charge differently');
});

test('the detail bundle carries the blurb, the chips and the attribution', () => {
  const d = mediaModelDetail(
    { id: 'x', via: 'chat', description: 'A model. It is good.', factsFrom: 'vendor/x' },
    { kind: 'image' }
  );
  expect(d.blurb).toBe('A model. It is good.');
  expect(d.chips.map((c) => c.label)).toEqual(['reads refs']);
  expect(d.borrowedFrom).toBe('vendor/x');

  // Nothing known is an empty bundle, not a fabricated one — but an unpriced
  // model still says why it is unpriced.
  expect(mediaModelDetail({ id: 'y' }, { kind: 'video' })).toEqual({
    blurb: '',
    chips: [],
    borrowedFrom: null,
    costNote: 'No published per-second price for this model.',
    qualityNote: 'Design Arena has not ranked this model — nobody has voted on it head to head yet.'
  });
});

test('a model with no price says why, when the reason is knowable', () => {
  // A router's price is whatever it routes to.
  expect(costNote({ id: 'openrouter/auto' })).toContain('router');
  expect(costNote({ id: 'openrouter/auto-beta' })).toContain('router');
  // An upscaler is billed against a source clip nobody has chosen yet.
  expect(costNote({ id: 'black-forest-labs/flux-video-upscale', upscale: true }, 'video')).toContain('per megapixel');
  // Otherwise: nothing published, said plainly and differently per kind.
  expect(costNote({ id: 'gpt-image-2' }, 'image')).toContain('per-image');
  expect(costNote({ id: 'x/y' }, 'video')).toContain('per-second');
  // A model that does have a price has nothing to explain.
  expect(costNote({ id: 'a', pricing: { perImage: 0.04 } }, 'image')).toBe('');
  expect(costNote({ id: 'b', pricing: { perSecond: 0.2 } }, 'video')).toBe('');
  // A router that somehow does carry a price is not second-guessed.
  expect(costNote({ id: 'openrouter/auto', pricing: { perImage: 0.01 } }, 'image')).toBe('');
});

test('a published per-image price explains what it assumes', () => {
  // Token-derived (OpenRouter): says so, and shows the rate behind it.
  const derived = mediaModelFacts({ id: 'a', pricing: { perImage: 0.067, imageOutput: 120 } }, { kind: 'image' });
  expect(derived.cost.title).toContain('estimated from $120/M output tokens');

  // Vendor-published (the first-party Images API models): the size and quality
  // the figure assumes, and whose figure it is. "≈4¢" alone invites "for what?".
  const listed = mediaModelFacts(
    { id: 'dall-e-3', pricing: { perImage: 0.04, basis: '1024×1024, standard quality', source: 'OpenAI list price' } },
    { kind: 'image' }
  );
  expect(listed.cost.label).toBe('≈4¢');
  expect(listed.cost.title).toBe('About 4¢ per image, at 1024×1024, standard quality (OpenAI list price)');
  expect(listed.cost.title).not.toContain('output tokens');
});

test('a leaderboard name is matched to the id we call the model by', () => {
  // The differences that are purely spelling.
  expect(mediaKey('wan-v3.0-t2v')).toBe(mediaKey('alibaba/wan-3.0'));
  expect(mediaKey('wan-v2.7-t2v')).toBe(mediaKey('alibaba/wan-2.7'));
  expect(mediaKey('kling-v3-pro')).toBe(mediaKey('kwaivgi/kling-v3.0-pro'));
  expect(mediaKey('happy-horse-1.1')).toBe(mediaKey('alibaba/happyhorse-1.1'));
  expect(mediaKey('seedance-1.5-pro')).toBe(mediaKey('bytedance/seedance-1-5-pro'));
  expect(mediaKey('dalle-3')).toBe(mediaKey('dall-e-3'));
  expect(mediaKey('gemini-3.1-flash-image-preview')).toBe(mediaKey('google/gemini-3.1-flash-image'));

  // And the differences that are not. Attributing another model's score is
  // worse than showing none, so these must stay apart.
  expect(mediaKey('veo-3')).not.toBe(mediaKey('google/veo-3.1'));
  expect(mediaKey('grok-imagine-video')).not.toBe(mediaKey('x-ai/grok-imagine-video-1.5'));
  expect(mediaKey('kling-v3-pro')).not.toBe(mediaKey('kwaivgi/kling-v3.0-std'));
  expect(mediaKey('wan-v3.0-t2v')).not.toBe(mediaKey('alibaba/wan-3.0-prime'));
  expect(mediaKey('seedance-2.0')).not.toBe(mediaKey('bytedance/seedance-2.0-fast'));
  expect(mediaKey('hailuo-2.3-pro')).not.toBe(mediaKey('minimax/hailuo-2.3'));
  expect(mediaKey('')).toBe('');
});

test('the leaderboard becomes ranks, keyed per category', () => {
  const board = {
    image: [
      { modelId: 'gpt-image-2', winRate: 72.1, elo: 1381, battles: 55968 },
      { modelId: 'dalle-3', winRate: 38.3, elo: 1086, battles: 134905 }
    ],
    video: [{ modelId: 'veo-3.1', winRate: 59.7, elo: 1186, battles: 18082 }]
  };
  const index = arenaIndex(board);

  const gpt = arenaQuality({ id: 'gpt-image-2' }, index, 'image');
  expect(gpt).toMatchObject({ arena: 'image', rank: 1, of: 2, winRate: 72.1, elo: 1381, battles: 55968, own: true });
  expect(arenaQuality({ id: 'dall-e-3' }, index, 'image').rank).toBe(2);

  // Categories do not leak into each other: a video model is ranked on the
  // video board or not at all.
  expect(arenaQuality({ id: 'google/veo-3.1' }, index, 'video').rank).toBe(1);
  expect(arenaQuality({ id: 'google/veo-3.1' }, index, 'image')).toBeNull();
  expect(arenaQuality({ id: 'gpt-image-2' }, index, 'video')).toBeNull();

  // Nothing on the board, and nothing to look in.
  expect(arenaQuality({ id: 'runway/gen-4.5' }, index, 'video')).toBeNull();
  expect(arenaQuality({ id: 'x' }, arenaIndex(null), 'image')).toBeNull();
  expect(arenaIndex(undefined).size).toBe(0);
});

test('a rank is reported with the size of the field behind it', () => {
  const facts = mediaModelFacts(
    { id: 'dall-e-3', quality: { arena: 'image', rank: 66, of: 76, winRate: 38.3, battles: 134905, own: true } },
    { kind: 'image' }
  );
  expect(facts.quality.label).toBe('#66');
  expect(facts.quality.rating).toBe(1);
  expect(facts.quality.title).toContain('rank 66 of 76');
  expect(facts.quality.title).toContain('134,905 head-to-head votes');

  // A score measured on this very model is not captioned as another listing's,
  // even when the row borrowed its price from one.
  const own = mediaModelFacts(
    { id: 'gemini-3.1-flash-image', factsFrom: 'google/gemini-3.1-flash-image', quality: { rank: 9, of: 76, winRate: 64.4, own: true } },
    { kind: 'image' }
  );
  expect(own.quality.title).not.toContain('measured on google/gemini-3.1-flash-image');
  // Whereas a score genuinely lent by the catalogue still says so.
  const lent = mediaModelFacts(
    { id: 'gemini-3.1-flash-image', factsFrom: 'google/gemini-3.1-flash-image', quality: { rank: 2, winRate: 65.1 } },
    { kind: 'image' }
  );
  expect(lent.quality.title).toContain('measured on google/gemini-3.1-flash-image');
});

test('an unranked model says it is unranked rather than showing a bare dash', () => {
  expect(qualityNote({ id: 'runway/gen-4.5' })).toContain('has not ranked');
  expect(qualityNote({ id: 'x', quality: { winRate: 50 } })).toBe('');
  // No model at all is not an unranked model — it says nothing, like costNote.
  expect(qualityNote(null)).toBe('');
});
