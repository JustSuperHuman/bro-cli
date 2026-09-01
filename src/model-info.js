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
  const p = model?.pricing || {};
  if (kind === 'video' && p.perSecond != null) {
    const rating = videoCostRating(p.perSecond);
    const basis = p.basis ? ` at ${p.basis}` : '';
    facts.cost = {
      rating,
      label: `${formatCents(p.perSecond)}/s`,
      title: `About ${formatCents(p.perSecond * 5)} for a 5-second clip${basis} (OpenRouter list price)`
    };
  } else if (kind !== 'video' && p.perImage != null) {
    const rating = imageCostRating(p.perImage);
    facts.cost = {
      rating,
      label: rating === 0 ? 'free' : `≈${formatCents(p.perImage)}`,
      title: `About ${formatCents(p.perImage)} per image, estimated from ${formatPrice(p.imageOutput)}/M output tokens (OpenRouter list price)`
    };
  }
  if (timing?.median > 0) {
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
    facts.quality = {
      rating: arenaRating(q.winRate),
      label: q.rank ? `#${q.rank}` : `${Math.round(q.winRate)}%`,
      title: `Design Arena ${q.arena || 'image'} leaderboard: ${Math.round(q.winRate)}% win rate${q.rank ? `, rank ${q.rank}` : ''}${from}`
    };
  }
  return facts;
}
