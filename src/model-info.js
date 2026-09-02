// Model facts for the pickers: how old a model is, what it costs, how fast it
// streams and how well it scores — each also folded into a five-step rating so
// a column of models can be compared at a glance. Everything here is pure; the
// data comes from OpenRouter's catalogue (see models.js) and the numbers are
// formatted for fixed-width terminal columns.

// ---------- age ----------

const DAY = 86400000;

// Compact age from a unix-seconds `created` stamp: "new" for the first week,
// then days, months and years — never more than four characters wide.
export function ageLabel(createdSeconds, now = Date.now()) {
  if (!createdSeconds) return '';
  const days = Math.max(0, (now - createdSeconds * 1000) / DAY);
  if (days < 7) return 'new';
  if (days < 45) return `${Math.round(days)}d`;
  const months = days / 30.44;
  if (months < 11.5) return `${Math.round(months)}mo`;
  const years = days / 365.25;
  return years < 9.95 ? `${years.toFixed(1).replace(/\.0$/, '')}y` : `${Math.round(years)}y`;
}

// ---------- cost ----------

// Per-million-token price as people quote it: "$2", "$0.15", "$1.25", "$150".
export function formatPrice(perMillion) {
  if (perMillion == null || !Number.isFinite(perMillion)) return '';
  if (perMillion === 0) return '$0';
  if (perMillion >= 100) return `$${Math.round(perMillion)}`;
  if (perMillion >= 10) return `$${perMillion.toFixed(1).replace(/\.0$/, '')}`;
  if (perMillion >= 1) return `$${perMillion.toFixed(2).replace(/\.?0+$/, '')}`;
  return `$${perMillion.toFixed(2).replace(/\.?0+$/, '')}`;
}

// Coding agents read far more than they write, so cost is blended 3:1
// input:output before it is rated.
export function blendedPrice(pricing) {
  if (!pricing || pricing.prompt == null || pricing.completion == null) return null;
  return (3 * pricing.prompt + pricing.completion) / 4;
}

// 0 = free, 1 = cents per million, 5 = frontier flagship money.
export function costRating(pricing) {
  const blended = blendedPrice(pricing);
  if (blended == null) return null;
  if (blended === 0) return 0;
  if (blended <= 0.25) return 1;
  if (blended <= 1) return 2;
  if (blended <= 3) return 3;
  if (blended <= 10) return 4;
  return 5;
}

// ---------- speed ----------

// Median output speed in tokens per second (from OpenRouter's endpoint stats).
export function speedRating(tokensPerSecond) {
  if (tokensPerSecond == null || !Number.isFinite(tokensPerSecond)) return null;
  if (tokensPerSecond >= 150) return 5;
  if (tokensPerSecond >= 90) return 4;
  if (tokensPerSecond >= 50) return 3;
  if (tokensPerSecond >= 25) return 2;
  return 1;
}

// ---------- quality ----------

// The score a coding tool cares about: Artificial Analysis' coding index,
// falling back to the general intelligence index.
export function qualityScore(quality) {
  if (!quality) return null;
  const score = quality.coding ?? quality.intelligence;
  return score == null || !Number.isFinite(score) ? null : score;
}

// Rated against the best score in the current catalogue rather than a fixed
// scale, so the stars keep meaning something as models improve.
export function qualityRating(quality, top) {
  const score = qualityScore(quality);
  if (score == null || !top) return null;
  const share = score / top;
  if (share >= 0.9) return 5;
  if (share >= 0.75) return 4;
  if (share >= 0.55) return 3;
  if (share >= 0.35) return 2;
  return 1;
}

export function topQuality(models) {
  let top = 0;
  for (const m of models || []) {
    const score = qualityScore(m?.quality);
    if (score != null && score > top) top = score;
  }
  return top || null;
}

// Five-step meter drawn from a filled and an empty glyph. `null` draws nothing
// (the column stays aligned but empty) — an unknown is not a low score.
export function meter(rating, filled, empty) {
  if (rating == null) return ' '.repeat(5);
  const n = Math.max(0, Math.min(5, Math.round(rating)));
  return filled.repeat(n) + empty.repeat(5 - n);
}

// ---------- cross-provider matching ----------

// Strip everything that varies between how a provider and OpenRouter spell the
// same model: the publisher prefix, punctuation, a ":variant" suffix, a
// "-latest"/"-preview" tag and a trailing date stamp (20251001 or 0711).
// "claude-haiku-4-5-20251001" and "anthropic/claude-haiku-4.5" both become
// "claudehaiku45".
export function modelKey(id) {
  if (!id) return '';
  let s = String(id).toLowerCase();
  s = s.replace(/^[^/]+\//, '');
  s = s.replace(/:.*$/, '');
  s = s.replace(/-(latest|preview)$/, '');
  s = s.replace(/-?(20\d{2})[-.]?(\d{2})[-.]?(\d{2})$/, '');
  s = s.replace(/-\d{4}$/, '');
  return s.replace(/[^a-z0-9]/g, '');
}

// Design Arena names a model the way its makers say it aloud; a catalogue names
// it the way you call it over HTTP. Normalise the differences that are purely
// spelling, so the leaderboard can be matched to what we serve:
//
//   wan-v3.0-t2v  → wan3       a `v` before the version, and the modality suffix
//   kling-v3-pro  → kling3pro
//   seedance-2.0  → seedance2  a trailing .0 is the same release as no .0
//
// Deliberately conservative: `veo-3` and `veo-3.1` stay different models, and
// so do `grok-imagine-video` and `grok-imagine-video-1.5`. A wrong match would
// put another model's score on a row, which is worse than leaving it blank.
export function mediaKey(id) {
  let s = String(id || '').toLowerCase();
  s = s.replace(/[-_](t2v|i2v|v2v)$/, ''); // text-to-video is a mode, not a model
  s = s.replace(/(^|[-_/])v(\d)/g, '$1$2'); // -v3.0 → -3.0
  s = s.replace(/(\d)\.0(?![0-9])/g, '$1'); // 3.0 → 3
  return modelKey(s);
}

// Lookup table from normalised key to catalogue entry; the first (newest)
// entry wins when several share a key.
export function catalogueIndex(models) {
  const index = new Map();
  for (const m of models || []) {
    const key = modelKey(m?.id);
    if (key && !index.has(key)) index.set(key, m);
  }
  return index;
}

// A provider's own model entry, enriched with what the catalogue knows about
// that same model. The provider's id and name always win; the catalogue's id
// is kept as `catalogueId` so speed stats (keyed by it) can still be found.
export function enrichFromCatalogue(model, index) {
  if (!model?.id || !index) return model;
  const hit = index.get(modelKey(model.id));
  if (!hit) return model;
  const { id: catalogueId, name: _name, ...facts } = hit;
  return { ...facts, catalogueId, ...model };
}

// The same trick for the gallery's image and video models — but copying only
// the four display facts and the blurb, never a routing field. A provider's
// entry says how to *call* it (`via`, urls, capability lists); borrowing
// `via: 'chat'` from OpenRouter's catalogue would quietly re-route a Yunwu or
// OpenAI model through the wrong endpoint. `factsFrom` records that the numbers
// describe the same model at a different vendor, so the UI can say so rather
// than presenting another shop's price as this one's.
const MEDIA_FACT_FIELDS = ['created', 'pricing', 'quality', 'description'];

export function enrichMediaFacts(model, index) {
  if (!model?.id || !index) return model;
  const hit = index.get(modelKey(model.id));
  if (!hit || hit.id === model.id) return model;
  const out = { ...model };
  let borrowed = false;
  for (const f of MEDIA_FACT_FIELDS) {
    if (out[f] == null && hit[f] != null) {
      out[f] = hit[f];
      borrowed = true;
    }
  }
  if (borrowed) out.factsFrom = hit.id;
  return out;
}

// ---------- Design Arena ----------

// The leaderboard keyed for matching, per category, carrying how many models
// the board holds so a rank can say what it is a rank *out of*.
export function arenaIndex(board) {
  const index = new Map();
  for (const category of Object.keys(board || {})) {
    const rows = Array.isArray(board[category]) ? board[category] : [];
    rows.forEach((row, i) => {
      // Takes either the mapped row or the leaderboard's own shape, and derives
      // the rank from position when it is not already carried — the board comes
      // back best-first either way.
      const key = mediaKey(row?.id ?? row?.modelId);
      // First wins: a preview and the release that shares its name resolve to
      // the higher-placed of the two.
      if (!key || index.has(`${category}:${key}`)) return;
      index.set(`${category}:${key}`, { category, ...row, rank: row?.rank ?? i + 1, of: rows.length });
    });
  }
  return index;
}

// A model's own standing on the board, in the shape `quality` takes elsewhere.
// `own` marks it as measured on this very model rather than lent by a sibling,
// so the facts do not misattribute it.
export function arenaQuality(model, index, kind = 'image') {
  if (!model?.id || !index) return null;
  const hit = index.get(`${kind === 'video' ? 'video' : 'image'}:${mediaKey(model.id)}`);
  if (!hit) return null;
  return {
    arena: hit.category,
    winRate: hit.winRate,
    rank: hit.rank,
    elo: hit.elo,
    of: hit.of,
    battles: hit.battles,
    own: true
  };
}

// ---------- rows ----------

const SEP = '  ';

// Which columns fit. The name column takes whatever is left.
function tier(width) {
  if (width >= 84) return 'wide';
  if (width >= 58) return 'medium';
  if (width >= 38) return 'narrow';
  return 'tiny';
}

const pad = (s, w) => {
  s = String(s ?? '');
  return s.length >= w ? s.slice(0, w) : s + ' '.repeat(w - s.length);
};
const padLeft = (s, w) => {
  s = String(s ?? '');
  return s.length >= w ? s.slice(0, w) : ' '.repeat(w - s.length) + s;
};

// Column widths per tier. Wide rows carry the numbers behind each rating.
const COLS = {
  wide: { age: 4, cost: 19, speed: 12, quality: 9 },
  medium: { age: 4, cost: 5, speed: 5, quality: 5 },
  narrow: { age: 4, cost: 0, speed: 0, quality: 5 },
  tiny: { age: 0, cost: 0, speed: 0, quality: 0 }
};

function costCell(model, w, wide) {
  if (!w) return '';
  if (!model.pricing) return ' '.repeat(w);
  const rating = costRating(model.pricing);
  const bars = meter(rating, '$', '·');
  if (!wide) return pad(bars, w);
  const numbers = rating === 0 ? 'free' : `${formatPrice(model.pricing.prompt)}·${formatPrice(model.pricing.completion)}`;
  return pad(`${bars} ${numbers}`, w);
}

function speedCell(model, w, wide) {
  if (!w) return '';
  const tps = model.speed?.tps;
  const rating = speedRating(tps);
  const bars = meter(rating, '●', '○');
  if (!wide) return pad(bars, w);
  return pad(rating == null ? bars : `${bars} ${Math.round(tps)}t/s`, w);
}

function qualityCell(model, w, wide, top) {
  if (!w) return '';
  const rating = qualityRating(model.quality, top);
  const stars = meter(rating, '★', '☆');
  if (!wide) return pad(stars, w);
  const score = qualityScore(model.quality);
  return pad(rating == null ? stars : `${stars} ${Math.round(score)}`, w);
}

// One picker row, laid out for `width` visible columns. `top` is the best
// quality score in the list (see topQuality); `showCost` hides the cost column
// where the price is not what the user pays (a subscription, a local model).
// `showSpeed` drops the speed column when no row in the list has a measurement.
export function modelRow(model, { width = 80, now = Date.now(), top = null, showCost = true, showSpeed = true } = {}) {
  const t = tier(width);
  const c = COLS[t];
  const wide = t === 'wide';
  const cells = [
    c.age ? padLeft(ageLabel(model.created, now), c.age) : '',
    showCost ? costCell(model, c.cost, wide) : '',
    showSpeed ? speedCell(model, c.speed, wide) : '',
    qualityCell(model, c.quality, wide, top)
  ].filter(Boolean);
  const facts = cells.length ? SEP + cells.join(SEP) : '';
  const nameW = Math.max(8, width - facts.length);
  const name = model.name || model.id || '(default)';
  let head = name;
  let visible = name.length;
  if (name.length > nameW) {
    head = name.slice(0, nameW - 1) + '…';
    visible = nameW;
  } else if (model.id && model.id !== name && name.length + 2 + model.id.length <= nameW) {
    // Show the id behind the name when it fits — it is what gets launched.
    head = `${name}  \x1b[2m${model.id}\x1b[0m`;
    visible = name.length + 2 + model.id.length;
  }
  return head + ' '.repeat(nameW - visible) + facts;
}

// Column headings aligned with modelRow for the same width and flags.
export function modelHeader({ width = 80, showCost = true, showSpeed = true } = {}) {
  const t = tier(width);
  const c = COLS[t];
  const wide = t === 'wide';
  const cells = [
    c.age ? padLeft('age', c.age) : '',
    showCost && c.cost ? pad(wide ? 'cost  $/M in·out' : 'cost', c.cost) : '',
    showSpeed && c.speed ? pad(wide ? 'speed  tok/s' : 'speed', c.speed) : '',
    c.quality ? pad(wide ? 'quality  AA' : 'qual.', c.quality) : ''
  ].filter(Boolean);
  const facts = cells.length ? SEP + cells.join(SEP) : '';
  const nameW = Math.max(8, width - facts.length);
  return pad('model', nameW) + facts;
}

// Whether any row in a list carries a speed measurement.
export const anySpeed = (models) => (models || []).some((m) => Number.isFinite(m?.speed?.tps));

// ---------- image & video models (JustImagine) ----------
// The gallery's picker shows the same four facts for generation models. Age
// and cost come from OpenRouter; quality from Design Arena (image models
// only); speed from the gallery's own history of how long each model took.

// Image models bill per output token; a picture is a roughly fixed number of
// them, which differs by family (Google documents 1290 for Gemini 2.5 Flash
// Image and 1120 for Gemini 3 Pro Image; OpenAI's medium 1024² is 1056).
export function imageTokensPerImage(id) {
  const s = String(id || '').toLowerCase();
  if (/gemini-3/.test(s)) return 1120;
  if (/gemini|banana/.test(s)) return 1290;
  if (/gpt|openai/.test(s)) return 1056;
  return 1290;
}

// Dollars → "4¢", "38¢", "$1.20".
export function formatCents(usd) {
  if (usd == null || !Number.isFinite(usd)) return '';
  if (usd === 0) return 'free';
  if (usd < 0.995) return `${usd < 0.095 ? (usd * 100).toFixed(1).replace(/\.0$/, '') : Math.round(usd * 100)}¢`;
  return `$${usd.toFixed(2)}`;
}

// Milliseconds → "12s", "1m 40s", "3m".
export function formatDuration(ms) {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return '';
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const rest = s % 60;
  return rest && m < 10 ? `${m}m ${rest}s` : `${m}m`;
}

// Estimated dollars per picture: 0 = free, 1 = a cent or two, 5 = the dear end.
export function imageCostRating(perImage) {
  if (perImage == null || !Number.isFinite(perImage)) return null;
  if (perImage === 0) return 0;
  if (perImage <= 0.02) return 1;
  if (perImage <= 0.05) return 2;
  if (perImage <= 0.1) return 3;
  if (perImage <= 0.2) return 4;
  return 5;
}

// Dollars per second of output video.
export function videoCostRating(perSecond) {
  if (perSecond == null || !Number.isFinite(perSecond)) return null;
  if (perSecond === 0) return 0;
  if (perSecond <= 0.05) return 1;
  if (perSecond <= 0.1) return 2;
  if (perSecond <= 0.2) return 3;
  if (perSecond <= 0.35) return 4;
  return 5;
}

// Wall-clock time a generation took, rated on a scale that fits its kind — a
// minute is quick for a clip and glacial for a picture.
export function generationSpeedRating(ms, kind = 'image') {
  if (ms == null || !Number.isFinite(ms) || ms <= 0) return null;
  const s = ms / 1000;
  const steps = kind === 'video' ? [60, 120, 240, 480] : [8, 15, 30, 60];
  if (s <= steps[0]) return 5;
  if (s <= steps[1]) return 4;
  if (s <= steps[2]) return 3;
  if (s <= steps[3]) return 2;
  return 1;
}

// Design Arena head-to-head win rate (percent) → stars.
export function arenaRating(winRate) {
  if (winRate == null || !Number.isFinite(winRate)) return null;
  if (winRate >= 62) return 5;
  if (winRate >= 56) return 4;
  if (winRate >= 50) return 3;
  if (winRate >= 44) return 2;
  return 1;
}

// Everything the gallery's picker needs for one row, ready to display. Each
// fact is { rating, label, title } or null when nothing is known. `timing` is
// the gallery's own record for this model: { median, n } in milliseconds.
export function mediaModelFacts(model, { kind = 'image', now = Date.now(), timing = null } = {}) {
  const facts = { age: null, cost: null, speed: null, quality: null };
  if (model?.created) {
    const label = ageLabel(model.created, now);
    facts.age = { label, title: `Published ${new Date(model.created * 1000).toISOString().slice(0, 10)}` };
  }
  // A fact borrowed from the same model on OpenRouter describes that listing,
  // not this vendor's bill — say so rather than quoting another shop's price
  // as if it were this one's.
  const borrowedPrice = model?.factsFrom
    ? ` — OpenRouter's list price for ${model.factsFrom}; this provider may charge differently`
    : '';
  const borrowedScore = model?.factsFrom ? ` — measured on ${model.factsFrom}, the same model at OpenRouter` : '';
  const p = model?.pricing || {};
  if (kind === 'video' && p.perSecond != null) {
    const rating = videoCostRating(p.perSecond);
    const basis = p.basis ? ` at ${p.basis}` : '';
    facts.cost = {
      rating,
      label: `${formatCents(p.perSecond)}/s`,
      title: `About ${formatCents(p.perSecond * 5)} for a 5-second clip${basis} (OpenRouter list price)${borrowedPrice}`
    };
  } else if (kind !== 'video' && p.perImage != null) {
    const rating = imageCostRating(p.perImage);
    // Two kinds of image price: one derived from a per-token rate in
    // OpenRouter's catalogue, and one the vendor publishes per picture (the
    // first-party Images API models, which no catalogue we read carries). Each
    // explains itself, because "≈4¢" invites the question of 4¢ for what.
    const how =
      p.imageOutput != null
        ? `estimated from ${formatPrice(p.imageOutput)}/M output tokens (OpenRouter list price)`
        : `${p.basis ? `at ${p.basis} ` : ''}(${p.source || 'list price'})`;
    facts.cost = {
      rating,
      label: rating === 0 ? 'free' : `≈${formatCents(p.perImage)}`,
      title: `About ${formatCents(p.perImage)} per image, ${how}${borrowedPrice}`
    };
  }
  // OpenRouter's own measurement first: it is there before you have generated
  // anything, and it is a p50 over everyone's requests in the last half hour
  // rather than a handful of your own. Your gallery's record is the fallback,
  // which is all there is for a model OpenRouter has no traffic for.
  const measured = model?.latency;
  if (measured?.p50 > 0) {
    facts.speed = {
      rating: generationSpeedRating(measured.p50, kind),
      label: formatDuration(measured.p50),
      title:
        `Typically ${formatDuration(measured.p50)} — OpenRouter's median over ` +
        `${measured.n ? measured.n.toLocaleString() + ' request' + (measured.n === 1 ? '' : 's') : 'recent traffic'} in the last 30 minutes${borrowedScore}`
    };
  } else if (timing?.median > 0) {
    const n = timing.n || 1;
    facts.speed = {
      rating: generationSpeedRating(timing.median, kind),
      label: formatDuration(timing.median),
      title: `Median of ${n} generation${n === 1 ? '' : 's'} in this gallery`
    };
  }
  const q = model?.quality;
  if (q?.winRate != null) {
    const from = q.from ? ` (measured on ${q.from})` : '';
    // A rank means little without the size of the field, and a win rate means
    // little without the number of votes behind it.
    const place = q.rank ? `rank ${q.rank}${q.of ? ` of ${q.of}` : ''}, ` : '';
    const votes = q.battles ? ` over ${q.battles.toLocaleString()} head-to-head votes` : '';
    facts.quality = {
      rating: arenaRating(q.winRate),
      label: q.rank ? `#${q.rank}` : `${Math.round(q.winRate)}%`,
      // Matched on this model's own name, so it is not another listing's score.
      title: `Design Arena ${q.arena || 'image'} leaderboard: ${place}${Math.round(q.winRate)}% win rate${votes}${from}${q.own ? '' : borrowedScore}`
    };
  }
  return facts;
}

// ---------- descriptions ----------

// OpenRouter ships a prose description with every image and video model. It is
// the honest answer to "what is this good at" — better than anything we could
// invent — but it is written for a docs page: markdown links, the odd code
// span, and several paragraphs. Reduce it to plain sentences that fit a picker.
export function cleanDescription(text, { sentences = 2, max = 260 } = {}) {
  if (!text || typeof text !== 'string') return '';
  let s = text
    .replace(/```[\s\S]*?```/g, ' ')
    .replace(/!\[[^\]]*\]\([^)]*\)/g, ' ')
    // "[Gemini 3 Pro](https://…)" → "Gemini 3 Pro"
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/[`*_#>]/g, '')
    .replace(/\s+/g, ' ')
    .trim();
  if (!s) return '';

  // Take whole sentences up to the budget. A decimal point ("Gemini 2.5") and
  // an abbreviation ("a.k.a.") are not sentence ends, so only break on a stop
  // that is followed by a space and a capital.
  // Two ordinary word characters must precede the stop, so "a.k.a." and
  // "Gemini 2.5" stay inside their sentence instead of ending it.
  const parts = s.split(/(?<=[a-z0-9)”"][a-z0-9)”"][.!?])\s+(?=[A-Z“"(])/);
  let out = '';
  let taken = 0;
  for (const part of parts) {
    if (out && (taken >= sentences || (out + ' ' + part).length > max)) break;
    out = out ? `${out} ${part}` : part;
    taken++;
  }
  if (!out) out = s;
  if (out.length > max) out = out.slice(0, max - 1).replace(/[\s,;:.]+\S*$/, '') + '…';
  return out;
}

// ---------- capabilities ----------

// The tallest resolution a video model offers, as a label people recognise.
export function bestResolution(resolutions) {
  if (!Array.isArray(resolutions) || !resolutions.length) return '';
  const height = (r) => {
    const s = String(r).toLowerCase();
    if (/4k/.test(s)) return 2160;
    const n = parseInt(s, 10);
    return Number.isFinite(n) ? n : 0;
  };
  const best = resolutions.reduce((a, b) => (height(b) > height(a) ? b : a));
  return height(best) >= 2160 ? '4K' : String(best);
}

// What a model can actually do, read off the fields the catalogue gives us
// rather than guessed. Each chip is { label, title } so the row stays short and
// the hover explains it. Nothing here is invented: a chip appears only when the
// data says so.
export function modelChips(model, kind = 'image') {
  if (!model) return [];
  const chips = [];
  const add = (label, title) => chips.push({ label, title });

  if (kind === 'video') {
    if (model.upscale) add('upscaler', 'Enlarges a video you give it — it needs a source clip, not a prompt');
    const durations = Array.isArray(model.durations) ? model.durations.filter((d) => Number.isFinite(d)) : [];
    if (durations.length) {
      const max = Math.max(...durations);
      const min = Math.min(...durations);
      add(`to ${max}s`, `Clip length ${min}–${max} seconds`);
    }
    const res = bestResolution(model.resolutions);
    if (res) add(res, `Renders up to ${res} (${model.resolutions.join(', ')})`);
    if (model.audio) add('audio', 'Generates a soundtrack with the picture');
    const frames = Array.isArray(model.frames) ? model.frames : [];
    if (frames.includes('first_frame')) add('1st frame', 'Animates a reference image as the opening frame');
    if (frames.includes('last_frame')) add('last frame', 'Takes a closing frame as well, so you can direct where the shot ends');
    if (model.seed) add('seed', 'Accepts a seed, so the same prompt can be reproduced');
    if (Array.isArray(model.aspectRatios) && model.aspectRatios.length) {
      add(`${model.aspectRatios.length} ratios`, `Aspect ratios: ${model.aspectRatios.join(', ')}`);
    }
  } else {
    if (model.via === 'chat') {
      add('reads refs', 'Chat-routed: it takes your reference images and edits as well as generates. Size and quality are described in the prompt, not set as knobs.');
    } else {
      add('size & quality', 'An /images/generations model: pick the size and quality with the knobs beside the prompt');
    }
    const res = bestResolution(model.resolutions);
    if (res) add(res, `Renders up to ${res}`);
  }
  return chips;
}

// The rest of what the picker's detail pane shows, kept apart from
// mediaModelFacts so the four-meter contract stays exactly four meters.
// `blurb` is the publisher's own description; `chips` are capabilities read off
// the catalogue; `borrowedFrom` names the listing any borrowed number came
// from, so the pane can attribute it.
export function mediaModelDetail(model, { kind = 'image' } = {}) {
  return {
    blurb: model?.description ? cleanDescription(model.description) : '',
    chips: modelChips(model, kind),
    borrowedFrom: model?.factsFrom || null,
    costNote: costNote(model, kind),
    qualityNote: qualityNote(model)
  };
}

// Design Arena only ranks what people have actually voted on, so plenty of
// models have no standing — including every brand-new release. Say that, rather
// than leaving a dash that reads like a failed lookup.
export function qualityNote(model) {
  if (!model || model.quality?.winRate != null) return '';
  return 'Design Arena has not ranked this model — nobody has voted on it head to head yet.';
}

// Some models have no price because none *can* be quoted, not because we failed
// to look it up. Saying which is which turns a bare dash into an answer.
export function costNote(model, kind = 'image') {
  if (!model || model.pricing?.perImage != null || model.pricing?.perSecond != null) return '';
  // A router charges whatever the model it picks charges.
  if (/(^|\/)auto(-|$)/.test(String(model.id || ''))) return 'A router — it costs whatever the model it picks costs.';
  // Upscalers are billed by the megapixel of the clip handed to them, so the
  // figure depends on a source video we have not seen yet.
  if (model.upscale) return 'Billed per megapixel of the clip you give it, so the cost depends on your source.';
  return kind === 'video'
    ? 'No published per-second price for this model.'
    : 'No published per-image price for this model — the provider sets it.';
}
