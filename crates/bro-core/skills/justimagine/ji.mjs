#!/usr/bin/env node
// ji.mjs — a dependency-free client for the local JustImagine server. Installed by bro
// (`bro imagine`, `bro install justimagine`) into the generate-images, generate-video and
// generate-voice skills; the copies are identical.
//
//   node ji.mjs up                          is it running? (starts its service if not)
//   node ji.mjs cast [filter] [--full]      the cast: ids, style variants, refs, voices
//   node ji.mjs models image|video|voice [query] [--sort speed|quality|cost|new]
//   node ji.mjs plan <spec>                 validate + estimate, queues nothing
//   node ji.mjs run  <spec> [--max-cost 3] [--yes] [--timeout s]   queue, wait, print file paths
//   node ji.mjs wait <id,id,...> [--timeout s]                      resume waiting on job ids
//   node ji.mjs cast-new <name> <description>
//   node ji.mjs cast-ref <id> <imagePath | folder/file>
//   node ji.mjs cast-sheet <id> [count]     draw reference candidates, wait, print their paths
//   node ji.mjs cast-keep <id> <file...>    promote candidates to references
//   node ji.mjs voices [filter]             the ElevenLabs voices, and which cast member has which
//   node ji.mjs voice-design <description> [--text "sample line"] [--out dir]
//   node ji.mjs voice-save <generatedVoiceId> [characterId] [--name N]
//   node ji.mjs voice-assign <characterId> <voiceId>
//
// <spec> is a file path, '-' for stdin, or inline JSON. Same shape as POST /api/batch
// ({defaults, items}) plus: `cast` (alias of characters; ids, names or first names),
// `style` (picks a cast style variant), `@mentions` in prompts, `enhance: true`,
// and for video `keyframe: true` (draw a still with the cast, then animate it).
//
// It finds the server itself: JUSTIMAGINE_URL, else the port its service recorded, else 8793
// (standalone) or 8791 (bro v1). It signs in with JUSTIMAGINE_TOKEN or the token in
// ~/.justimagine/auth.token (JUSTIMAGINE_HOME moves that folder).

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const HOME = process.env.JUSTIMAGINE_HOME || path.join(os.homedir(), '.justimagine');
const CONFIG_DIR = process.env.JUSTIMAGINE_CONFIG_PATH ? path.dirname(process.env.JUSTIMAGINE_CONFIG_PATH) : HOME;
const BUILDER = (process.env.BUILDER_CONTROL_URL || 'http://localhost:6767').replace(/\/$/, '');
const KEYFRAME_MODEL = 'google/gemini-3.1-flash-image';
const MAX_COUNT = { image: 12, video: 4, voice: 4 };

const readText = (f) => { try { return fs.readFileSync(f, 'utf8'); } catch { return ''; } };
const readJson = (f) => { try { return JSON.parse(readText(f)); } catch { return null; } };
const TOKEN = (process.env.JUSTIMAGINE_TOKEN || readText(path.join(CONFIG_DIR, 'auth.token'))).trim();
let BASE = '';

// ---------- plumbing ----------

// Never process.exit() here: on Windows it can abort Node while fetch sockets are
// still closing. Throw instead and let the top level set the exit code.
class Exit extends Error { constructor(msg, code = 1) { super(msg); this.code = code; } }
const die = (msg, code = 1) => { throw new Exit(msg, code); };
const stop = (code) => { throw new Exit('', code); };
const out = (obj) => console.log(JSON.stringify(obj, null, 1));
const money = (n) => `$${n.toFixed(2)}`;

async function api(route, body, { timeoutMs = 30000 } = {}) {
  const headers = TOKEN ? { authorization: `Bearer ${TOKEN}` } : {};
  const init = body === undefined
    ? { headers, signal: AbortSignal.timeout(timeoutMs) }
    : { method: 'POST', headers: { ...headers, 'content-type': 'application/json' }, body: JSON.stringify(body), signal: AbortSignal.timeout(timeoutMs) };
  const res = await fetch(BASE + route, init);
  const text = await res.text();
  let json;
  try { json = JSON.parse(text); } catch { json = { error: text.slice(0, 300) }; }
  if (res.status === 401) die(`JustImagine wants its sign-in token and ${TOKEN ? 'rejected the one' : 'found none'} at ${path.join(CONFIG_DIR, 'auth.token')} (or set JUSTIMAGINE_TOKEN)`);
  if (!res.ok) throw new Error(`${route} → ${res.status} ${json.error || ''}`.trim());
  return json;
}

/** Where a running JustImagine answers, or ''. */
async function findBase() {
  const recorded = readJson(path.join(HOME, 'service', 'service.json'));
  const bases = process.env.JUSTIMAGINE_URL
    ? [process.env.JUSTIMAGINE_URL]
    : [recorded?.url, recorded?.port && `http://127.0.0.1:${recorded.port}`, 'http://127.0.0.1:8793', 'http://127.0.0.1:8791'];
  for (const b of [...new Set(bases.filter(Boolean).map((x) => String(x).replace(/\/$/, '')))]) {
    try {
      const r = await fetch(`${b}/health`, { signal: AbortSignal.timeout(1500) });
      if (r.ok && (await r.json()).service === 'justimagine') return b;
    } catch { /* not this one */ }
  }
  return '';
}

/** bro's install of JustImagine, when there is one. */
function installEntry() {
  const dirs = [process.env.JUSTIMAGINE_INSTALL_DIR,
    process.platform === 'win32' && path.join(process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local'), 'Programs', 'JustImagine'),
    path.join(process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local', 'share'), 'justimagine')];
  return dirs.filter(Boolean).map((d) => path.join(d, 'bin', 'justimagine.js')).find((f) => fs.existsSync(f));
}

async function ensureUp() {
  BASE = await findBase();
  if (!BASE) {
    // its own service first (what `bro install justimagine` set up), then JustGains Builder
    const entry = installEntry();
    if (entry) spawnSync(process.execPath, [entry, 'service', 'start'], { stdio: 'ignore', timeout: 30000, windowsHide: true });
    else await fetch(`${BUILDER}/api/services/justimagine/start`, { method: 'POST', signal: AbortSignal.timeout(5000) }).catch(() => null);
    for (let i = 0; i < 20 && !BASE; i++) {
      await new Promise((r) => setTimeout(r, 1000));
      BASE = await findBase();
    }
  }
  if (!BASE) die('JustImagine is not running and could not be started. Install it as a service with `bro install justimagine`, or start it with `bro install justimagine --force`.');
  return api('/api/state', undefined, { timeoutMs: 15000 });
}

function readSpec(arg) {
  if (!arg) die('missing <spec> (file, - for stdin, or inline JSON)');
  const raw = arg === '-' ? fs.readFileSync(0, 'utf8') : /^\s*[[{]/.test(arg) ? arg : fs.readFileSync(arg, 'utf8');
  try { return JSON.parse(raw); } catch (e) { die(`spec is not valid JSON: ${e.message}`); }
}

function flags(argv) {
  const f = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--yes' || a === '-y') f.yes = true;
    else if (a === '--full' || a === '--verbose') f[a.slice(2)] = true;
    else if (a.startsWith('--')) f[a.slice(2)] = argv[++i];
    else f._.push(a);
  }
  return f;
}

// ---------- cast ----------

const norm = (s) => String(s || '').toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim();
// "CPF Hector Colon (Mystery)" → variant "mystery", person "cpf hector colon". No parens = base look.
const variantOf = (c) => norm((c.name.match(/\(([^)]*)\)\s*$/) || [])[1]);
const personOf = (c) => norm(c.name.replace(/\([^)]*\)\s*$/, ''));
const plainName = (c) => c.name.replace(/\s*\([^)]*\)\s*$/, '').trim();
const BASE_STYLES = new Set(['', 'base', 'default', 'normal', 'standard', 'none']);
const styleKey = (s) => (BASE_STYLES.has(norm(s)) ? '' : norm(s));

/** Where the cast's files live (standalone keeps them in its home, bro v1 under ~/.bro): the
 *  folder holding most of the characters `ids` names. */
function castDir(ids = []) {
  const dirs = [path.join(HOME, 'characters'), path.join(os.homedir(), '.bro', 'justimagine', 'characters')];
  const held = (d) => [].concat(ids).filter((id) => id && fs.existsSync(path.join(d, id))).length;
  const best = dirs.reduce((a, b) => (held(b) > held(a) ? b : a));
  return held(best) ? best : dirs.find((d) => fs.existsSync(d)) || dirs[0];
}

function makeResolver(cast) {
  return function resolve(token, style) {
    const t = norm(String(token).replace(/^@/, ''));
    if (!t) return { error: 'empty cast name' };
    const exact = cast.filter((c) => norm(c.id) === t || norm(c.name) === t);
    if (exact.length === 1) {
      // An exact id still honours an explicit style: "cpf-hector-colon" + style "mystery"
      // means that person's Mystery variant, not the base look.
      const hit = exact[0];
      if (style === undefined || variantOf(hit) === styleKey(style)) return { c: hit };
      const twin = cast.find((c) => personOf(c) === personOf(hit) && variantOf(c) === styleKey(style));
      // No such variant: a single-look character simply ignores a batch-wide style.
      return { c: twin || hit };
    }
    let pool = exact.length ? exact : cast.filter((c) => ` ${personOf(c)} `.includes(` ${t} `) || ` ${norm(c.id)} `.includes(` ${t} `));
    if (!pool.length) return { error: `no cast member matches "${token}"` };
    if (style !== undefined && pool.length > 1) {
      const want = styleKey(style);
      const styled = pool.filter((c) => variantOf(c) === want);
      if (styled.length) pool = styled;
    }
    if (pool.length === 1) return { c: pool[0] };
    const people = new Set(pool.map(personOf));
    const opts = pool.map((c) => `${c.id}${variantOf(c) ? ` [style: ${variantOf(c)}]` : ' [style: base]'}`).join(', ');
    return {
      error: people.size === 1
        ? `"${token}" has several style variants — set "style" or use an id: ${opts}`
        : `"${token}" is ambiguous — use an id: ${opts}`
    };
  };
}

// Resolve an item's cast from `cast`/`characters` and @mentions in the prompt.
// Unambiguous names resolve first; their shared style then settles ambiguous ones.
function resolveItemCast(item, resolve, errors, warnings, label) {
  const tokens = [...(item.cast || []), ...(item.characters || [])].map(String);
  const mentions = [...String(item.prompt || '').matchAll(/(^|[^\w@.])@([A-Za-z0-9][\w-]*)/g)].map((m) => m[2]);
  const picked = new Map();
  const mentionMap = new Map();
  const pending = [];
  const tryResolve = (tok, isMention, style) => {
    const r = resolve(tok, style);
    if (r.c) { picked.set(r.c.id, r.c); if (isMention) mentionMap.set(tok, r.c); return true; }
    return r;
  };
  for (const [tok, isMention] of [...tokens.map((t) => [t, false]), ...mentions.map((t) => [t, true])]) {
    const r = tryResolve(tok, isMention, item.style);
    if (r !== true) pending.push([tok, isMention, r]);
  }
  const styles = new Set([...picked.values()].map(variantOf));
  const inferred = item.style === undefined && styles.size === 1 ? [...styles][0] : undefined;
  for (const [tok, isMention, first] of pending) {
    const r = inferred !== undefined ? tryResolve(tok, isMention, inferred) : first;
    if (r === true) continue;
    // A mention that names nobody is probably just an @ in the prose; an explicit cast entry is a mistake.
    if (isMention && /no cast member/.test(r.error)) warnings.push(`${label}: @${tok} is not a cast member — left as text`);
    else errors.push(`${label}: ${r.error}`);
  }
  let prompt = String(item.prompt || '');
  for (const [tok, c] of mentionMap) prompt = prompt.replace(new RegExp(`@${tok.replace(/[-]/g, '\\-')}\\b`, 'g'), plainName(c));
  const cast = [...picked.values()];
  const persons = new Map();
  for (const c of cast) {
    if (persons.has(personOf(c))) errors.push(`${label}: ${persons.get(personOf(c))} and ${c.id} are two style variants of one person — pick one`);
    persons.set(personOf(c), c.id);
  }
  if (item.kind !== 'voice') {
    const castStyles = new Set(cast.map(variantOf));
    if (castStyles.size > 1) warnings.push(`${label}: cast mixes styles (${[...castStyles].map((s) => s || 'base').join(', ')}) — the shot will fight itself; use one style`);
    for (const c of cast) if (!c.refs.length) warnings.push(`${label}: ${c.id} has no reference images — likeness comes from its description only (fix: cast-sheet ${c.id})`);
  }
  return { prompt, cast };
}

/** Cast members who speak a line of a voice script ("Nora: Where were you?"). */
function sceneSpeakers(prompt, cast) {
  const found = new Map();
  for (const line of String(prompt).split('\n')) {
    const m = line.match(/^\s*([^:\n]{1,40}):\s/);
    if (!m) continue;
    const n = norm(m[1]);
    const c = cast.find((x) => norm(plainName(x)) === n || norm(x.id) === n || norm(plainName(x)).split(' ')[0] === n);
    if (c) found.set(c.id, c);
  }
  return [...found.values()];
}

// ---------- planning ----------

async function plan(spec, state) {
  const body = Array.isArray(spec) ? { items: spec } : spec;
  const defaults = body.defaults || {};
  const rawItems = body.items || (body.prompt ? [body] : []);
  if (!rawItems.length) die('spec has no items');
  const kinds = new Set(rawItems.map((r) => (typeof r === 'string' ? defaults.kind : r.kind || defaults.kind) || 'image'));
  const [imageModels, videoModels, voiceModels, castRes] = await Promise.all([
    api('/api/models?kind=image'),
    kinds.has('video') ? api('/api/models?kind=video') : { models: [] },
    kinds.has('voice') ? api('/api/models?kind=voice') : { models: [] },
    api('/api/characters')
  ]);
  const cast = castRes.characters;
  const resolve = makeResolver(cast);
  const errors = [], warnings = [], items = [];
  let cost = 0, unknownCost = false, credits = 0;

  rawItems.forEach((raw, i) => {
    const item = { ...defaults, ...(typeof raw === 'string' ? { prompt: raw } : raw) };
    const label = `items[${i}]`;
    const kind = ['video', 'voice'].includes(item.kind) ? item.kind : 'image';
    item.kind = kind;
    if (!String(item.prompt || '').trim()) { errors.push(`${label}: prompt is required`); return; }
    const count = Math.max(1, Number(item.count) || 1);
    if (count > MAX_COUNT[kind]) errors.push(`${label}: count ${count} exceeds ${MAX_COUNT[kind]} per item`);

    const { prompt, cast: people } = resolveItemCast(item, resolve, errors, warnings, label);
    item.prompt = prompt;
    item.characters = people.map((c) => c.id);
    delete item.cast; delete item.style;

    if (kind === 'voice') {
      const catalogue = voiceModels.models || [];
      const m = item.model ? catalogue.find((x) => x.id === item.model) : catalogue[0];
      if (item.model && !m) errors.push(`${label}: unknown voice model "${item.model}" (see: node ji.mjs models voice)`);
      const speakers = sceneSpeakers(prompt, cast);
      for (const c of [...people, ...speakers]) {
        if (!c.voice) errors.push(`${label}: ${plainName(c)} has no voice yet — give them one first (node ji.mjs voice-design … / voice-assign ${c.id} <voiceId>)`);
      }
      if (!item.voice && !people.some((c) => c.voice) && !speakers.length) {
        errors.push(`${label}: no voice — set "voice" (an id from: node ji.mjs voices) or a cast member who has one`);
      }
      if (speakers.length > 1 && item.model && item.model !== 'eleven_v3') warnings.push(`${label}: a scene with several speakers always runs on eleven_v3`);
      if (m?.maxChars && prompt.length > m.maxChars) errors.push(`${label}: ${prompt.length} characters is over ${m.id}'s ${m.maxChars} — split it into several items`);
      credits += prompt.length * count * (Number(m?.credits) || 1);
      items.push({ ...item, count });
      return;
    }

    const handRefs = (item.images || []).length;
    for (const ref of [...(item.images || []), ...(item.keyframeImages || [])]) {
      if (/[\\/]|^[a-zA-Z]:/.test(ref) && !fs.existsSync(ref)) errors.push(`${label}: reference image not found: ${ref}`);
    }
    const catalogue = (kind === 'video' ? videoModels : imageModels).models;
    const matches = catalogue.filter((m) => m.id === item.model && (!item.api || m.api === item.api));
    const m = matches.find((x) => x.ready) || matches[0];
    if (!item.model) errors.push(`${label}: set "model" (see: node ji.mjs models ${kind})`);
    else if (!m) errors.push(`${label}: unknown ${kind} model "${item.model}"`);
    else if (!m.ready) errors.push(`${label}: ${item.model} has no provider key configured`);
    if (m && kind === 'image' && !item.api && m.api) item.api = m.api;

    if (kind === 'image') {
      const castRefs = people.reduce((n, c) => n + c.refs.length, 0);
      if (handRefs + castRefs > 8) {
        let room = 8 - handRefs;
        const cut = people.filter((c) => { const lost = c.refs.length > room; room = Math.max(0, room - c.refs.length); return lost; }).map((c) => c.id);
        warnings.push(`${label}: ${handRefs} attached + ${castRefs} cast references > 8 — refs of ${cut.join(', ')} get truncated; list the most important character first or attach fewer images`);
      }
      if (m?.pricing?.perImage) cost += m.pricing.perImage * count; else if (m) unknownCost = true;
    } else if (m) {
      const bad = (k, list, v) => v !== undefined && Array.isArray(list) && !list.map(String).includes(String(v)) &&
        errors.push(`${label}: ${k} ${JSON.stringify(v)} not accepted by ${m.id} — allowed: ${list.join(', ')}`);
      bad('duration', m.durations, item.duration);
      bad('resolution', m.resolutions, item.resolution);
      bad('aspectRatio', m.aspectRatios, item.aspectRatio);
      bad('size', m.sizes, item.size);
      if (item.audio && !m.audio) errors.push(`${label}: ${m.id} does not generate audio — drop "audio"`);
      if (item.seed !== undefined && !m.seed) errors.push(`${label}: ${m.id} does not take a seed`);
      const takesFirst = (m.frames || []).includes('first_frame');
      if (item.lastFrame && !(m.frames || []).includes('last_frame')) errors.push(`${label}: ${m.id} has no last_frame support`);
      if (item.keyframe) {
        if (!takesFirst) errors.push(`${label}: keyframe needs a model with first_frame support; ${m.id} has none`);
        if (handRefs && item.firstFrame !== false) errors.push(`${label}: keyframe draws frame one itself — move hand references to "keyframeImages"`);
      } else if (people.length) {
        if (handRefs && item.firstFrame !== false && takesFirst) warnings.push(`${label}: first attached image becomes frame one, so the server drops the cast references — use keyframe:true, or firstFrame:false`);
        else warnings.push(`${label}: cast goes only as reference guidance (input_references), which many video models weight lightly — keyframe:true gives much stronger likeness`);
      }
      const secs = Number(item.duration) || (m.durations || [])[0];
      if (!item.duration && secs) warnings.push(`${label}: no duration given — costed at ${secs}s; the model default may differ`);
      if (m.pricing?.perSecond && secs) cost += m.pricing.perSecond * secs * count; else unknownCost = true;
      if (item.keyframe) {
        const km = imageModels.models.find((x) => x.id === (item.keyframeModel || KEYFRAME_MODEL) && x.ready);
        if (!km) errors.push(`${label}: keyframe model ${item.keyframeModel || KEYFRAME_MODEL} unavailable`);
        else cost += km.pricing?.perImage || 0;
      }
    }
    items.push({ ...item, count });
  });
  return { items, errors, warnings, cost, unknownCost, credits, root: state.root };
}

function summary(p) {
  const n = (k) => p.items.filter((i) => i.kind === k).reduce((a, i) => a + i.count, 0);
  return {
    images: n('image') || undefined, videos: n('video') || undefined, voiceLines: n('voice') || undefined,
    estCost: money(p.cost) + (p.unknownCost ? ' + unpriced models' : ''),
    elevenLabsCredits: p.credits ? Math.round(p.credits) : undefined,
    keyframes: p.items.filter((i) => i.keyframe).length || undefined,
    cast: p.items.some((i) => i.characters.length)
      ? Object.fromEntries(p.items.map((i, k) => [`items[${k}]`, i.characters]).filter(([, c]) => c.length))
      : undefined,
    warnings: p.warnings.length ? p.warnings : undefined,
    errors: p.errors.length ? p.errors : undefined
  };
}

// ---------- running ----------

const toServerItem = (i) => {
  const { keyframe, keyframeModel, keyframePrompt, keyframeImages, enhance, ...rest } = i;
  return rest;
};

async function enhanceAll(items) {
  await Promise.all(items.filter((i) => i.enhance).map(async (i) => {
    try {
      const r = await api('/api/enhance', { prompt: i.prompt, kind: i.kind, model: i.model, characters: i.characters, images: i.images }, { timeoutMs: 60000 });
      if (r.prompt) i.prompt = r.prompt;
    } catch (e) { console.error(`warn: enhance failed, keeping original prompt: ${e.message}`); }
  }));
}

async function waitJobs(ids, timeoutS, root) {
  const deadline = Date.now() + timeoutS * 1000;
  let last;
  while (true) {
    const left = Math.max(1, Math.min(60, Math.round((deadline - Date.now()) / 1000)));
    last = await api(`/api/jobs?ids=${ids.join(',')}&wait=${left}`, undefined, { timeoutMs: (left + 30) * 1000 });
    if (last.settled || Date.now() >= deadline) break;
    const running = last.results.filter((r) => r.status === 'running' || r.status === 'queued');
    console.error(`… ${last.items.length} done, ${running.length} in progress (${running.map((r) => r.phase || r.status).join(', ')})`);
  }
  return {
    settled: last.settled,
    files: last.items.map((it) => path.join(root, it.folder || '', it.file)),
    cost: last.items.some((it) => it.cost) ? money(last.items.reduce((a, it) => a + (it.cost || 0), 0)) : undefined,
    failed: last.failed.map((f) => ({ id: f.id, prompt: String(f.prompt).slice(0, 80), error: f.error })),
    cancelled: last.cancelled.length || undefined,
    pending: last.settled ? undefined : last.pending.map((p) => p.id || p),
    resume: last.settled ? undefined : `node ji.mjs wait ${ids.join(',')}`
  };
}

async function submit(items) {
  const r = await api('/api/batch', { items: items.map(toServerItem) }, { timeoutMs: 120000 });
  if (r.warning) console.error(`server: ${r.warning}`);
  for (const w of r.warnings || []) console.error(`server: ${w}`);
  return r.jobs;
}

async function run(spec, f) {
  const state = await ensureUp();
  const p = await plan(spec, state);
  const s = summary(p);
  if (p.errors.length) { out(s); stop(2); }
  const cap = Number(f['max-cost'] ?? 3);
  if (p.cost > cap && !f.yes) { out({ ...s, refused: `estimate ${money(p.cost)} exceeds --max-cost ${money(cap)}; confirm with the user, then re-run with --yes` }); stop(3); }
  for (const w of p.warnings) console.error(`warn: ${w}`);
  await enhanceAll(p.items);
  const hasVideo = p.items.some((i) => i.kind === 'video');
  const timeout = Number(f.timeout) || (hasVideo ? 1500 : 300);

  // Phase 1 — keyframes: one still per keyframe video item, drawn with the cast attached.
  const keyed = p.items.filter((i) => i.keyframe);
  let keyframeReport;
  if (keyed.length) {
    const stills = keyed.map((v) => ({
      kind: 'image',
      model: v.keyframeModel || KEYFRAME_MODEL,
      folder: [v.folder, 'keyframes'].filter(Boolean).join('/'),
      prompt: `${v.keyframePrompt || v.prompt}\n\nThis is the opening frame of a ${v.aspectRatio || '16:9'} video: compose for a ${v.aspectRatio || '16:9'} frame, subjects clearly visible, mid-action pose.`,
      characters: v.characters,
      images: v.keyframeImages
    }));
    const ids = await submit(stills);
    keyframeReport = await waitJobs(ids, 300, p.root);
    const rows = (await api(`/api/jobs?ids=${ids.join(',')}`)).results;
    keyed.forEach((v, k) => {
      const row = rows.find((r) => r.id === ids[k]);
      const done = row?.item;
      if (!done) { v.skip = `keyframe failed: ${row?.error || row?.status || 'unknown'}`; return; }
      v.images = [path.join(p.root, done.folder || '', done.file)];
      v.firstFrame = true;
      // The frame now carries the likeness; sending the cast too would only be dropped upstream.
      v.characters = [];
    });
  }

  const todo = p.items.filter((i) => !i.skip);
  const ids = todo.length ? await submit(todo) : [];
  console.error(`queued ${ids.length} job(s); est ${s.estCost}${s.elevenLabsCredits ? ` + ~${s.elevenLabsCredits} ElevenLabs credits` : ''}; waiting up to ${timeout}s`);
  const res = ids.length ? await waitJobs(ids, timeout, p.root) : { settled: true, files: [], failed: [] };
  const skipped = p.items.filter((i) => i.skip).map((i) => ({ prompt: i.prompt.slice(0, 80), error: i.skip }));
  out({ ...res, keyframes: keyframeReport?.files, skipped: skipped.length ? skipped : undefined, warnings: s.warnings, gallery: BASE });
  if (res.failed.length || skipped.length) stop(4);
}

// ---------- voices ----------

async function voiceName(id) {
  const v = (await api('/api/voices')).voices.find((x) => x.id === id);
  if (!v) die(`no voice "${id}" on the ElevenLabs account (see: node ji.mjs voices)`);
  return v.name;
}

// ---------- commands ----------

const [cmd, ...rest] = process.argv.slice(2);
const f = flags(rest);

try {
  switch (cmd) {
    case 'up': {
      const s = await ensureUp();
      const cfg = await api('/api/config');
      out({ base: BASE, signedIn: Boolean(TOKEN), root: s.root, castDir: castDir((s.characters || []).map((c) => c.id)), cast: s.characters?.length,
            providers: cfg.apis.map((a) => `${a.id}${a.hasKey ? '' : ' (no key)'}${a.video ? ' +video' : ''}`),
            running: (s.jobs || []).filter((j) => j.status === 'running' || j.status === 'queued').length });
      break;
    }
    case 'cast': {
      await ensureUp();
      const q = norm(f._[0]);
      const all = (await api('/api/characters')).characters.filter((c) => !q || norm(`${c.id} ${c.name} ${c.description}`).includes(q));
      const groups = new Map();
      for (const c of all) groups.set(personOf(c), [...(groups.get(personOf(c)) || []), c]);
      for (const g of groups.values()) {
        console.log(plainName(g[0]));
        for (const c of g) {
          const voice = c.voice ? `voice=${c.voice.name || c.voice.id}` : 'voice=-';
          console.log(`  ${c.id.padEnd(34)} style=${(variantOf(c) || 'base').padEnd(10)} refs=${c.refs.length} ${voice}${c.candidates?.length ? ` candidates=${c.candidates.length}` : ''}  ${c.description.replace(/\s+/g, ' ').slice(0, f.full ? 2000 : 110)}`);
        }
      }
      console.log(`\n${all.length} characters. Refs on disk: ${path.join(castDir(all.map((c) => c.id)), '<id>', 'refs')}`);
      break;
    }
    case 'models': {
      await ensureUp();
      const kind = ['video', 'voice'].includes(f._[0]) ? f._[0] : 'image';
      if (kind === 'voice') {
        const r = await api(`/api/models?kind=voice${f._[1] ? `&q=${encodeURIComponent(f._[1])}` : ''}`);
        console.log('ElevenLabs speech models (credits = cost per character, relative to 1)');
        for (const m of r.models) {
          const bits = [`credits=${m.credits ?? '?'}`, m.maxChars ? `max=${m.maxChars} chars` : '', m.languages ? `${m.languages} languages` : '', m.tags ? 'audio-tags' : '', m.dialogue ? 'dialogue' : ''];
          console.log(`${m.id.padEnd(28)} ${bits.filter(Boolean).join('  ')}  ${String(m.description || '').slice(0, 90)}`);
        }
        break;
      }
      // The server does the ordering (unknowns last); cheapest first by default.
      const sort = f.sort || 'cost';
      const r = await api(`/api/models?kind=${kind}&sort=${encodeURIComponent(sort)}${f._[1] ? `&q=${encodeURIComponent(f._[1])}` : ''}`);
      console.log(`sorted by ${sort} (--sort speed|quality|cost|new)`);
      for (const m of r.models.filter((x) => x.ready)) {
        const p = m.pricing?.perImage ? `${(m.pricing.perImage * 100).toFixed(1)}¢/img` : m.pricing?.perSecond ? `${(m.pricing.perSecond * 100).toFixed(1)}¢/s` : 'unpriced';
        const speed = m.speedMs ? `${Math.round(m.speedMs / 1000)}s` : '?s';
        const rank = m.rank ? `#${m.rank}` : '#-';
        const bits = kind === 'video'
          ? [`dur=${(m.durations || []).length > 6 ? `${m.durations[0]}-${m.durations.at(-1)}` : (m.durations || ['?']).join('/')}`, `res=${(m.resolutions || ['?']).join('/')}`,
             `ar=${(m.aspectRatios || ['?']).join('/')}`, `frames=${(m.frames || []).map((x) => x.replace('_frame', '')).join('+') || 'none'}`, m.audio ? 'audio' : '']
          : [`api=${m.api}`];
        console.log(`${m.id.padEnd(40)} ${p.padEnd(12)} ${speed.padStart(5)} ${rank.padEnd(4)} ${bits.filter(Boolean).join('  ')}`);
      }
      break;
    }
    case 'plan': {
      const state = await ensureUp();
      const p = await plan(readSpec(f._[0]), state);
      out({ ...summary(p), items: f.verbose ? p.items : undefined });
      if (p.errors.length) stop(2);
      break;
    }
    case 'run':
      await run(readSpec(f._[0]), f);
      break;
    case 'wait': {
      const s = await ensureUp();
      if (!f._[0]) die('usage: wait <id,id,...>');
      out(await waitJobs(f._[0].split(','), Number(f.timeout) || 1500, s.root));
      break;
    }
    case 'cast-new': {
      await ensureUp();
      const [name, ...desc] = f._;
      if (!name || !desc.length) die('usage: cast-new <name> <description>');
      out((await api('/api/characters', { name, description: desc.join(' ') })).character);
      break;
    }
    case 'cast-ref': {
      const s = await ensureUp();
      const [id, src] = f._;
      if (!id || !src) die('usage: cast-ref <id> <imagePath | folder/file>');
      const abs = path.resolve(src);
      const body = fs.existsSync(abs) ? { id, path: abs } : { id, folder: path.posix.dirname(src.replace(/\\/g, '/')).replace(/^\.$/, ''), file: path.basename(src) };
      if (!body.path && !fs.existsSync(path.join(s.root, body.folder, body.file))) die(`not found: ${src}`);
      const r = await api('/api/characters/refs', body);
      out({ id, refs: (r.character || r).refs?.length ?? r });
      break;
    }
    case 'cast-sheet': {
      await ensureUp();
      const [id, count = '5'] = f._;
      if (!id) die('usage: cast-sheet <id> [count]');
      const r = await api('/api/characters/refs/generate', { id, count: Number(count) });
      console.error(`drawing ${r.count} reference shots on ${r.model} (~${money(0.067 * r.count)})…`);
      await waitJobs([r.job], 600, '');
      const c = (await api('/api/characters')).characters.find((x) => x.id === id);
      out({ id, candidates: (c?.candidates || []).map((file) => path.join(castDir(id), id, 'candidates', file)),
            next: `look at them, then: node ji.mjs cast-keep ${id} <file> [file...]` });
      break;
    }
    case 'cast-keep': {
      await ensureUp();
      const [id, ...files] = f._;
      if (!id || !files.length) die('usage: cast-keep <id> <file...>');
      out(await api('/api/characters/refs/keep', { id, files: files.map((x) => path.basename(x)) }));
      break;
    }
    case 'voices': {
      await ensureUp();
      const q = norm(f._[0]);
      const r = await api('/api/voices', undefined, { timeoutMs: 30000 });
      if (r.cast?.length) {
        console.log('cast voices');
        for (const c of r.cast) console.log(`  ${c.id.padEnd(30)} ${c.voice?.name || ''} (${c.voice?.id})`);
        console.log('');
      }
      const voices = r.voices.filter((v) => !q || norm(`${v.name} ${v.category} ${(v.labels || []).join(' ')} ${v.description || ''}`).includes(q));
      for (const v of voices) console.log(`${v.id.padEnd(24)} ${v.name.slice(0, 28).padEnd(28)} ${(v.category || '').padEnd(10)} ${(v.labels || []).join(', ')}`);
      console.log(`\n${voices.length} voices. Hear one before choosing: its previewUrl in GET /api/voices.`);
      break;
    }
    case 'voice-design': {
      await ensureUp();
      const description = f._.join(' ');
      if (description.length < 20) die('usage: voice-design "<at least 20 characters: age, accent, pitch, timbre, pace>" [--text "sample line"] [--out dir]');
      const r = await api('/api/voices/design', { description, text: f.text }, { timeoutMs: 120000 });
      const dir = f.out || fs.mkdtempSync(path.join(os.tmpdir(), 'voice-design-'));
      fs.mkdirSync(dir, { recursive: true });
      const previews = r.previews.map((p, i) => {
        const m = p.dataUrl.match(/^data:audio\/([a-z0-9.+-]+);base64,(.+)$/is);
        const file = path.join(dir, `candidate-${i + 1}.${m && m[1] === 'mpeg' ? 'mp3' : (m?.[1] || 'mp3')}`);
        fs.writeFileSync(file, Buffer.from(m ? m[2] : '', 'base64'));
        return { generatedVoiceId: p.id, file, seconds: p.duration };
      });
      out({ previews, text: r.text, next: 'let the user listen, then: node ji.mjs voice-save <generatedVoiceId> [characterId]' });
      break;
    }
    case 'voice-save': {
      await ensureUp();
      const [generatedVoiceId, characterId] = f._;
      if (!generatedVoiceId) die('usage: voice-save <generatedVoiceId> [characterId] [--name N] [--description D]');
      const r = await api('/api/voices/save', { generatedVoiceId, characterId, name: f.name, description: f.description }, { timeoutMs: 90000 });
      out({ voice: r.voice, character: r.character?.id });
      break;
    }
    case 'voice-assign': {
      await ensureUp();
      const [id, voiceId] = f._;
      if (!id || !voiceId) die('usage: voice-assign <characterId> <voiceId>');
      const r = await api('/api/characters/update', { id, voice: { id: voiceId, name: await voiceName(voiceId) } });
      out({ id: (r.character || r).id, voice: (r.character || r).voice });
      break;
    }
    default:
      console.log(fs.readFileSync(new URL(import.meta.url), 'utf8').split('\n').slice(1, 28).map((l) => l.replace(/^\/\/ ?/, '')).join('\n'));
      if (cmd) stop(1);
  }
} catch (e) {
  if (e.message) console.error(`error: ${e.message}`);
  process.exitCode = e instanceof Exit ? e.code : 1;
}
