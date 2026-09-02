import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { pipeline } from 'node:stream';
import { fileURLToPath } from 'node:url';
import { ENHANCE_MODEL, enhancePrompt, generateImage, generateVideo, VIDEO_KEY_API } from './justimagine-gen.js';
import { arenaIndex, arenaQuality, catalogueIndex, enrichMediaFacts, mediaModelDetail, mediaModelFacts } from './model-info.js';
import { readMediaStats, refreshMediaStats } from './models.js';
import {
  CHARACTERS_DIR,
  addCandidate,
  addRefFromDataUrl,
  characterDir,
  characterShots,
  clearCandidates,
  composePrompt,
  createCharacter,
  deleteCharacter,
  deleteRef,
  ensureLibrary,
  getCharacter,
  keepCandidates,
  listCharacters,
  resolveCandidate,
  resolveCharacters,
  updateCharacter
} from './justimagine-characters.js';
import {
  contextDir,
  deleteContext,
  deleteFolder,
  deleteItem,
  createFolder,
  ensureRoot,
  listContext,
  listFolders,
  listItems,
  kindOf,
  modelTimings,
  mediaTypeOf,
  moveItems,
  renameFolder,
  resolveFile,
  resolveFolder,
  resolveRefs,
  tooSmallRefs,
  appendHistory,
  saveContext,
  saveThumb,
  thumbPath
} from './justimagine-store.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const UI_HTML = path.join(__dirname, 'justimagine.html');

// How many upstream calls may be in flight at once, per kind. Images are cheap
// and fast so the browser can fan out; video is minutes-long and metered, so a
// tighter gate keeps a stray "×8" from becoming eight concurrent paid jobs.
const LIMITS = { image: 8, video: 3 };

// How long a finished job stays readable, and how many are kept. The browser
// only needs the last few seconds — it is watching the event stream — but a
// script that submits fifty generations and polls for them must be able to
// collect every result, including ones that landed while it was asleep.
export const JOB_RETAIN_MS = 30 * 60 * 1000;
export const JOB_RETAIN_MAX = 500;

// One `POST /api/batch` may not become an unbounded queue: a typo in a loop
// should cost a rejection, not a hundred paid generations.
export const BATCH_MAX_ITEMS = 50;
export const BATCH_MAX_JOBS = 100;
// Longest a long-poll may hold a request open. Beyond this the caller gets what
// has finished so far and polls again, which survives proxies and Ctrl-C alike.
export const WAIT_MAX_SEC = 600;

export const isTerminal = (job) => job.status === 'done' || job.status === 'error' || job.status === 'cancelled';

// Nano Banana 2 — the strongest reference-driven image model on OpenRouter, and
// the reason a generated cast holds its likeness across angles.
export const CHARACTER_REF_MODEL = 'google/gemini-3.1-flash-image';
// How many of a reference set to have in flight at once. One job fans out
// rather than queueing five, so this is its own small gate.
const REF_FANOUT = 3;

// Run `work` over `items`, at most `limit` at a time, keeping the results in
// order and never rejecting — a failed shot is a gap in the set, not a dead job.
export async function mapLimit(items, limit, work) {
  const out = new Array(items.length);
  let next = 0;
  const runners = Array.from({ length: Math.min(limit, items.length) }, async () => {
    for (let i = next++; i < items.length; i = next++) {
      try {
        out[i] = { ok: true, value: await work(items[i], i) };
      } catch (e) {
        out[i] = { ok: false, error: e?.message || String(e) };
      }
    }
  });
  await Promise.all(runners);
  return out;
}

// Upstreams reject an undersized reference with a message about pixels that
// says nothing about *which* picture is at fault. Name it, and say how to fix
// it — re-adding runs the file back through the browser's resize, which now
// scales small images up as well as large ones down.
export function explainRefFailure(message, refs) {
  const text = String(message || '');
  if (!/\b(\d+)\s*px|width|height|dimension|resolution|too small/i.test(text)) return text;
  const small = tooSmallRefs(refs);
  if (!small.length) return text;
  const named = small.map((s) => `${s.file} (${s.width}×${s.height})`).join(', ');
  return `${text}\n\nToo small to use as a reference: ${named}. Remove it and add it again — it will be scaled up on the way in.`;
}

// Which references become frames, and which stay subject/style guidance.
//
// Attachment order is the interface: the first reference opens the clip, the
// second can close it — but only for a model that accepts those, and only when
// the user left frame conditioning on.
//
// `characterRefs` are the pictures of who is in the shot; `refs` are what the
// user attached by hand for this one generation. Only hand-attached references
// ever become frames — pinning a character portrait as frame one would force
// every clip to open on that exact photo. Character references go to
// input_references instead, which is the slot meant for "who/what this is".
// Upstream ignores input_references the moment a frame is present, so the two
// are kept mutually exclusive here rather than silently dropped there.
export function frameSelection({ refs = [], characterRefs = [], spec, firstFrame = true, lastFrame = false }) {
  const takes = (which) => !!spec?.frames?.includes(which);
  const first = firstFrame && refs[0] && takes('first_frame') ? refs[0] : null;
  const last = first && lastFrame && refs[1] && takes('last_frame') ? refs[1] : null;
  return {
    firstFrame: first,
    lastFrame: last,
    refs: first ? [] : [...refs, ...characterRefs],
    // Told to the caller so the UI can say why the cast was left out, instead
    // of the user wondering where their character went.
    droppedCharacters: !!(first && characterRefs.length)
  };
}

// ---------- naming ----------

function slug(text) {
  return (
    String(text)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 48) || 'gen'
  );
}

function stamp(d = new Date()) {
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}

export const outputName = (prompt, ext) => `${stamp()}-${slug(prompt)}-${crypto.randomBytes(2).toString('hex')}.${ext}`;

// ---------- http plumbing ----------

function readBody(req, max = 1024 * 1024) {
  return new Promise((resolve, reject) => {
    let size = 0;
    const chunks = [];
    req.on('data', (c) => {
      size += c.length;
      if (size > max) {
        reject(new Error('Body too large'));
        req.destroy();
        return;
      }
      chunks.push(c);
    });
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    req.on('error', reject);
  });
}

const readJsonBody = async (req, max) => JSON.parse((await readBody(req, max)) || '{}');

// Enough of a key to tell which one is saved, and never enough to use. Short
// strings give up nothing at all rather than most of themselves.
export function maskKey(key) {
  if (!key) return '';
  const s = String(key);
  if (s.length < 12) return '•'.repeat(8);
  return `${s.slice(0, 4)}…${s.slice(-4)}`;
}

// This server listens on the loopback interface, but any page in the browser can
// still POST to 127.0.0.1. A write must therefore come from this gallery's own
// origin: browsers set `Origin` on cross-origin writes and `Sec-Fetch-Site` on
// every modern request, and neither can be forged by page script. A same-origin
// fetch from our own HTML sends `Origin: http://127.0.0.1:<port>`; a form post
// from evil.example.com sends its own origin and is refused.
export function sameOrigin(req) {
  const site = req.headers['sec-fetch-site'];
  if (site) return site === 'same-origin' || site === 'none';
  const origin = req.headers.origin;
  // No Origin at all is a non-browser client (curl, a script) — the loopback
  // bind is the only guard there, and it is the one the CLI itself relies on.
  if (!origin) return true;
  return origin === `http://${req.headers.host}` || origin === `https://${req.headers.host}`;
}

function sendJson(res, code, obj) {
  const body = JSON.stringify(obj);
  res.writeHead(code, { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store' });
  res.end(body);
}

// Generated files never change under their name, so they can be cached hard.
// Range support is what lets a <video> seek instead of refetching the clip.
function sendFile(req, res, full, type, { immutable = true } = {}) {
  let stat;
  try {
    stat = fs.statSync(full);
  } catch {
    res.writeHead(404);
    res.end('Not found');
    return;
  }
  const headers = {
    'content-type': type || 'application/octet-stream',
    'accept-ranges': 'bytes',
    'last-modified': stat.mtime.toUTCString(),
    'cache-control': immutable ? 'public, max-age=31536000, immutable' : 'no-cache'
  };
  const range = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range || '');
  if (range && stat.size) {
    let start = range[1] === '' ? null : Number(range[1]);
    let end = range[2] === '' ? null : Number(range[2]);
    if (start === null) {
      // "bytes=-500" — the last N bytes.
      start = Math.max(0, stat.size - (end ?? 0));
      end = stat.size - 1;
    } else if (end === null || end >= stat.size) {
      end = stat.size - 1;
    }
    if (start > end || start >= stat.size) {
      res.writeHead(416, { 'content-range': `bytes */${stat.size}` });
      res.end();
      return;
    }
    res.writeHead(206, { ...headers, 'content-range': `bytes ${start}-${end}/${stat.size}`, 'content-length': end - start + 1 });
    if (req.method === 'HEAD') return res.end();
    sendBody(res, full, { start, end });
    return;
  }
  res.writeHead(200, { ...headers, 'content-length': stat.size });
  if (req.method === 'HEAD') return res.end();
  sendBody(res, full, { end: stat.size - 1 });
}

// Thumbnails, posters and character references are all small; reading one and
// writing it in a single call is both faster than a stream and free of the
// keep-alive race a piped response can lose on Windows, where the next request
// arrives while the pipe is still finishing and the socket resets. Anything
// large enough to be worth streaming (video) goes through pipeline(), which —
// unlike pipe() — sequences the end and destroys the response on a read error
// instead of leaving a half-written body behind.
const SMALL_FILE = 512 * 1024;

function sendBody(res, full, { start = 0, end }) {
  if (end - start + 1 <= SMALL_FILE) {
    let buf;
    try {
      buf = readRange(full, start, end);
    } catch {
      res.destroy();
      return;
    }
    res.end(buf);
    return;
  }
  pipeline(fs.createReadStream(full, { start, end }), res, () => {
    /* the client went away, or the file vanished mid-read */
  });
}

function readRange(full, start, end) {
  const fd = fs.openSync(full, 'r');
  try {
    const buf = Buffer.allocUnsafe(end - start + 1);
    let read = 0;
    while (read < buf.length) {
      const n = fs.readSync(fd, buf, read, buf.length - read, start + read);
      if (!n) break;
      read += n;
    }
    return read === buf.length ? buf : buf.subarray(0, read);
  } finally {
    fs.closeSync(fd);
  }
}

// ---------- job registry ----------

// Generations outlive the request that started them: the browser posts a job
// and then watches an event stream. That is what lets a five-minute video
// survive a page reload, and what keeps the UI from holding open sockets.
export function createJobs({ limits = LIMITS, retainMs = JOB_RETAIN_MS, retainMax = JOB_RETAIN_MAX } = {}) {
  const jobs = new Map();
  const clients = new Set();
  const waiters = new Set();
  const queues = { image: [], video: [] };
  const running = { image: 0, video: 0 };
  let seq = 0;

  const publicJob = (j) => ({
    id: j.id,
    kind: j.kind,
    folder: j.folder,
    prompt: j.prompt,
    model: j.model,
    characters: j.characters,
    // Set when the job is drawing a cast member's reference sheet, so the
    // editor can follow it and the gallery can leave it alone.
    characterId: j.characterId,
    status: j.status,
    phase: j.phase,
    refs: j.refs,
    startedAt: j.startedAt,
    queuedAt: j.queuedAt,
    finishedAt: j.finishedAt,
    error: j.error,
    item: j.item
  });

  // Finished jobs are kept so a poller can still collect them, but not forever
  // and not without a ceiling: oldest terminal jobs go first, and anything
  // still queued or running is never dropped.
  function prune(now = Date.now()) {
    const finished = [...jobs.values()].filter(isTerminal);
    for (const j of finished) if (now - (j.finishedAt || 0) > retainMs) jobs.delete(j.id);
    const left = [...jobs.values()].filter(isTerminal).sort((a, b) => (a.finishedAt || 0) - (b.finishedAt || 0));
    for (let i = 0; i < left.length - retainMax; i++) jobs.delete(left[i].id);
  }

  function emit(type, payload) {
    const frame = `data: ${JSON.stringify({ type, ...payload })}\n\n`;
    for (const res of clients) {
      try {
        res.write(frame);
      } catch {
        clients.delete(res);
      }
    }
  }

  const update = (job, patch) => {
    Object.assign(job, patch);
    if (isTerminal(job) && !job.finishedAt) job.finishedAt = Date.now();
    emit('job', { job: publicJob(job) });
    // A long-poll is settled by the same transition that feeds the event
    // stream, so `wait` returns the instant the last job lands.
    for (const w of [...waiters]) w();
  };

  function pump(kind) {
    while (running[kind] < limits[kind] && queues[kind].length) {
      const job = queues[kind].shift();
      if (job.status === 'cancelled') continue;
      running[kind]++;
      update(job, { status: 'running', phase: 'starting', startedAt: Date.now() });
      job
        .run(job)
        .then((item) => {
          if (job.status !== 'cancelled') update(job, { status: 'done', phase: '', item });
        })
        .catch((e) => {
          if (job.status === 'cancelled') return;
          update(job, { status: 'error', phase: '', error: e?.message || String(e) });
        })
        .finally(() => {
          running[kind]--;
          prune();
          pump(kind);
        });
    }
  }

  function cancelOne(id) {
    const job = jobs.get(id);
    if (!job || isTerminal(job)) return false;
    job.ctrl.abort();
    update(job, { status: 'cancelled', phase: '' });
    return true;
  }

  return {
    add({ kind, folder, prompt, model, refs, characters, characterId, run }) {
      const id = `${Date.now().toString(36)}-${(seq++).toString(36)}-${crypto.randomBytes(2).toString('hex')}`;
      const job = {
        id,
        kind,
        folder,
        prompt,
        model,
        refs,
        characters,
        characterId,
        run,
        status: 'queued',
        phase: 'queued',
        queuedAt: Date.now(),
        ctrl: new AbortController()
      };
      jobs.set(id, job);
      prune();
      queues[kind].push(job);
      emit('job', { job: publicJob(job) });
      pump(kind);
      return id;
    },
    progress(id, phase) {
      const job = jobs.get(id);
      if (job && job.status === 'running') update(job, { phase });
    },
    signal: (id) => jobs.get(id)?.ctrl.signal,
    cancel: cancelOne,
    // Stop everything still in flight — the "abort the batch I just queued"
    // button, and the same thing over HTTP.
    cancelAll: (kind) =>
      [...jobs.values()]
        .filter((j) => !isTerminal(j) && (!kind || j.kind === kind))
        .map((j) => j.id)
        .filter(cancelOne),
    active: () => [...jobs.values()].filter((j) => j.status === 'queued' || j.status === 'running').map(publicJob),
    snapshot: () => [...jobs.values()].map(publicJob),
    // Read specific jobs back by id. Ids the registry no longer holds come back
    // in `missing` rather than as a silent gap in the list, so a poller can tell
    // "not finished yet" apart from "waited too long and it has aged out".
    read(ids) {
      prune();
      const found = [];
      const missing = [];
      for (const id of ids) {
        const job = jobs.get(id);
        if (job) found.push(publicJob(job));
        else missing.push(id);
      }
      return { jobs: found, missing };
    },
    pendingIds: (kind) => [...jobs.values()].filter((j) => !isTerminal(j) && (!kind || j.kind === kind)).map((j) => j.id),
    // Long-poll until every named job has finished, or until the caller's
    // patience runs out. This is what lets a script submit a batch and collect
    // it with one more request instead of parsing an event stream.
    wait(ids, timeoutMs = 0) {
      const unfinished = () => ids.filter((id) => jobs.has(id) && !isTerminal(jobs.get(id)));
      if (!timeoutMs || !unfinished().length) return Promise.resolve(!unfinished().length);
      return new Promise((resolve) => {
        const settle = (ok) => {
          clearTimeout(timer);
          waiters.delete(check);
          resolve(ok);
        };
        const check = () => {
          if (!unfinished().length) settle(true);
        };
        const timer = setTimeout(() => settle(false), timeoutMs);
        timer.unref?.();
        waiters.add(check);
      });
    },
    prune,
    subscribe(res) {
      clients.add(res);
      return () => clients.delete(res);
    },
    emit,
    clientCount: () => clients.size,
    closeAll() {
      for (const res of clients) {
        try {
          res.end();
        } catch {
          /* already gone */
        }
      }
      clients.clear();
    }
  };
}

// ---------- server ----------

// `resolveKey(apiId)` is a function rather than a snapshot so a key saved after
// the server started (or rotated in the config) is picked up on the next call.
export function createServer({
  root,
  apis,
  videoModels = [],
  // Design Arena's leaderboard, both categories. The one quality scale the
  // picker rates on; absent, models simply have no rank.
  designArena = null,
  resolveKey,
  defaultApi,
  jobs = createJobs(),
  title = 'JustImagine',
  // The text model behind the ✨ button. Overridable so a user who prefers a
  // different writer — or a cheaper one — is not stuck with ours.
  enhanceModel = ENHANCE_MODEL,
  // The cast is global rather than per-gallery, so it is addressed separately
  // from the gallery root and every gallery sees the same characters.
  charactersRoot = CHARACTERS_DIR,
  // Writing a key back to the config file. Injected rather than imported so the
  // server stays testable without touching the real ~/.bro/config.json; when it
  // is absent the settings panel goes read-only rather than failing on save.
  saveKey = null,
  // Shown in the settings panel so the file is findable, and named in the error
  // when there is nothing to write to.
  configPath = '',
  // Collect OpenRouter's speed measurements in the background. Only the real
  // entry point turns this on, so tests never reach the network.
  statsRefresh = false
}) {
  ensureRoot(root);
  ensureLibrary(charactersRoot);

  const apiById = new Map(apis.map((a) => [a.id, a]));
  const keyOf = (id) => (resolveKey ? resolveKey(id) || '' : '');

  // `prompt` is what the user typed and what gets recorded; `composed` is that
  // plus the character block, and is what the model actually sees.
  async function runImageJob(job, { api, prompt, composed, model, size, quality, refs, castNames, folderFull, folderRel }) {
    const apiKey = keyOf(api.id);
    if (!apiKey) throw new Error(`No API key for ${api.name || api.id}. Add it to ~/.bro/config.json under keys.${api.id}.`);
    const started = Date.now();
    jobs.progress(job.id, 'generating');
    const { buf, ext, revisedPrompt } = await generateImage({
      api,
      apiKey,
      prompt: composed || prompt,
      model,
      size,
      quality,
      refs,
      signal: job.ctrl.signal
    }).catch((e) => {
      throw new Error(explainRefFailure(e.message, refs));
    });
    const file = outputName(prompt, ext);
    fs.mkdirSync(folderFull, { recursive: true });
    fs.writeFileSync(path.join(folderFull, file), buf);
    const entry = {
      file,
      kind: 'image',
      folder: folderRel,
      prompt,
      revisedPrompt: revisedPrompt || undefined,
      characters: castNames?.length ? castNames : undefined,
      api: api.id,
      model,
      size: size || 'auto',
      quality: quality || 'auto',
      images: refs.length ? refs.map((r) => r.file) : undefined,
      refs: refs.length || undefined,
      bytes: buf.length,
      ms: Date.now() - started,
      ts: Date.now()
    };
    appendHistory(folderFull, entry);
    return entry;
  }

  async function runVideoJob(job, { prompt, composed, model, params, refs, castNames, folderFull, folderRel }) {
    const apiKey = keyOf(VIDEO_KEY_API);
    if (!apiKey) {
      throw new Error('Video generation needs an OpenRouter key. Run `bro imagine -p openrouter`, or add keys.openrouter to ~/.bro/config.json.');
    }
    const started = Date.now();
    const sent = [params.firstFrame, params.lastFrame, ...(params.refs || [])].filter(Boolean);
    const { buf, ext, cost, generationId } = await generateVideo({
      apiKey,
      params: { model, prompt: composed || prompt, ...params },
      signal: job.ctrl.signal,
      onProgress: (p) => jobs.progress(job.id, p.phase)
    }).catch((e) => {
      throw new Error(explainRefFailure(e.message, sent));
    });
    const file = outputName(prompt, ext);
    fs.mkdirSync(folderFull, { recursive: true });
    fs.writeFileSync(path.join(folderFull, file), buf);
    const entry = {
      file,
      kind: 'video',
      folder: folderRel,
      prompt,
      characters: castNames?.length ? castNames : undefined,
      api: VIDEO_KEY_API,
      model,
      duration: params.duration || undefined,
      resolution: params.resolution && params.resolution !== 'auto' ? params.resolution : undefined,
      aspectRatio: params.aspectRatio && params.aspectRatio !== 'auto' ? params.aspectRatio : undefined,
      size: params.size && params.size !== 'auto' ? params.size : undefined,
      audio: params.generateAudio || undefined,
      seed: params.seed || undefined,
      images: refs.length ? refs.map((r) => r.file) : undefined,
      refs: refs.length || undefined,
      cost,
      generationId,
      bytes: buf.length,
      ms: Date.now() - started,
      ts: Date.now()
    };
    appendHistory(folderFull, entry);
    return entry;
  }

  // Build a character a set of reference shots. The first is the seed — either
  // a reference it already has, or one generated from its description — and
  // every other shot is drawn from that seed, which is what keeps them the same
  // character rather than five people who match the same sentence.
  async function runCharacterRefsJob(job, { id, count }) {
    const api = apiById.get('openrouter');
    const apiKey = api ? keyOf(api.id) : '';
    if (!api || !apiKey) {
      throw new Error('Generating references needs an OpenRouter key. Run `bro imagine -p openrouter`, or add keys.openrouter to ~/.bro/config.json.');
    }
    const character = getCharacter(charactersRoot, id);
    if (!character) throw new Error(`No character "${id}"`);
    if (!character.description && !character.refs.length) {
      throw new Error('Give the character a description first — there is nothing to draw from.');
    }

    const seeded = character.refs.length > 0;
    const shots = characterShots(character, seeded ? Number(count) + 1 : Number(count));
    const shoot = (prompt, refs) =>
      generateImage({ api, apiKey, prompt, model: CHARACTER_REF_MODEL, refs, signal: job.ctrl.signal });

    const files = [];
    let seedRef;
    if (seeded) {
      seedRef = resolveCharacters(charactersRoot, [id])[0]?.resolved?.[0];
    } else {
      jobs.progress(job.id, 'drawing the first shot');
      const { buf, ext } = await shoot(shots.seed, []);
      const file = addCandidate(charactersRoot, id, buf, mediaTypeOf(`x.${ext}`) || 'image/png');
      files.push(file);
      seedRef = { file, buf, type: 'image/png', ext, dataUrl: `data:image/png;base64,${buf.toString('base64')}` };
    }
    if (!seedRef) throw new Error('Could not read a reference to build the rest of the set from.');

    let done = files.length;
    const total = files.length + shots.variations.length;
    const results = await mapLimit(shots.variations, REF_FANOUT, async (prompt) => {
      const { buf, ext } = await shoot(prompt, [seedRef]);
      const file = addCandidate(charactersRoot, id, buf, mediaTypeOf(`x.${ext}`) || 'image/png');
      jobs.progress(job.id, `${++done} of ${total}`);
      return file;
    });
    for (const r of results) if (r.ok) files.push(r.value);

    const failed = results.filter((r) => !r.ok);
    // Every shot failing is a real failure; a couple missing is just a smaller
    // set, and the ones that landed are already on disk.
    if (!files.length) throw new Error(failed[0]?.error || 'No reference images were produced.');
    return {
      kind: 'character-refs',
      characterId: id,
      files,
      failed: failed.length || undefined,
      error: failed.length ? failed[0].error : undefined
    };
  }

  // Every model row, with display-ready facts. Built in one place because the
  // boot payload and the live refresh below have to agree exactly.
  function modelsPayload() {
    const timings = modelTimings(root);
    const now = Date.now();
    // Only OpenRouter's catalogue carries prices, publish dates, Design Arena
    // standings and the publisher's blurb. Aggregators and first-party APIs
    // serve the same models under shorter ids, so match them by normalised key
    // and lend them those facts — otherwise picking a Yunwu or OpenAI model
    // means choosing from a bare list of ids.
    const catalogue = catalogueIndex([...(apis.find((a) => a.id === 'openrouter')?.models || []), ...videoModels]);
    // How long a picture or a clip actually takes, measured by OpenRouter over
    // everyone's traffic — so a fresh gallery shows real speeds rather than
    // waiting for you to have generated something yourself.
    const latencies = readMediaStats();
    // One quality scale for the whole picker, from the board itself. OpenRouter
    // embeds a snapshot ranked among only the models it serves, which put two
    // different models at "#2" in the same list, left the first-party Images API
    // models unranked, and gave video no rating at all.
    const arena = arenaIndex(designArena);
    const withFacts = (raw, kind) => {
      const enriched = enrichMediaFacts(raw, catalogue);
      const ranked = arenaQuality(enriched, arena, kind);
      const m = ranked ? { ...enriched, quality: ranked } : enriched;
      const timing = timings[m.id] || (m.factsFrom ? timings[m.factsFrom] : null);
      const latency = latencies[m.id] || (m.factsFrom ? latencies[m.factsFrom] : null);
      const withLatency = latency?.p50 > 0 ? { ...m, latency } : m;
      return {
        ...withLatency,
        facts: mediaModelFacts(withLatency, { kind, now, timing }),
        detail: mediaModelDetail(withLatency, { kind })
      };
    };
    return {
      apis: apis.map((a) => ({
        id: a.id,
        name: a.name || a.id,
        video: !!a.video,
        models: (a.models || []).map((m) => withFacts(m, 'image')),
        hasKey: !!keyOf(a.id)
      })),
      videoModels: videoModels.map((m) => withFacts(m, 'video'))
    };
  }

  // Collect the speed numbers in the background and push the refreshed rows to
  // every open page, so the picker fills in without anyone reloading. Off by
  // default: only the real entry point turns it on, so tests never reach out.
  async function refreshModelStats() {
    const apiKey = keyOf('openrouter');
    if (!apiKey) return null;
    const wanted = [
      ...(apis.find((a) => a.id === 'openrouter')?.models || []).map((m) => ({ id: m.id, kind: 'image' })),
      ...videoModels.map((m) => ({ id: m.id, kind: 'video' }))
    ];
    try {
      await refreshMediaStats({ models: wanted, apiKey });
      if (jobs.clientCount()) jobs.emit('models', modelsPayload());
      return true;
    } catch {
      return false; // speeds stay blank; nothing else depends on them
    }
  }
  if (statsRefresh) setTimeout(refreshModelStats, 200).unref?.();

  // ---------- generation ----------

  // A reference is normally a name in the gallery's own `.context`, but a script
  // has files. Anything that looks like a path is registered on the way past and
  // swapped for its content-hash name, so `images: ["/photos/bottle.png"]` works
  // without a separate upload call. Registering is idempotent — the same bytes
  // keep the same name — so passing the same path to fifty items costs one write.
  const looksLikePath = (s) => /[\\/]/.test(s) || /^[a-zA-Z]:/.test(s);

  function registerRefPaths(images) {
    if (!Array.isArray(images)) return images;
    return images.map((entry) => {
      const name = String(entry || '');
      if (!name || !looksLikePath(name)) return name;
      const full = path.resolve(name);
      if (kindOf(full) !== 'image') throw Object.assign(new Error(`Not an image file: ${name}`), { status: 400 });
      let buf;
      try {
        buf = fs.readFileSync(full);
      } catch (e) {
        throw Object.assign(new Error(`Cannot read reference ${full}: ${e.code || e.message}`), { status: 400 });
      }
      return saveContext(root, `data:${mediaTypeOf(full)};base64,${buf.toString('base64')}`).file;
    });
  }

  // One generation spec, fully resolved but not yet queued. Splitting "can this
  // run?" from "run it" is what lets a batch reject its fortieth item before its
  // first has cost anything.
  function planGeneration(spec = {}) {
    const kind = spec.kind === 'video' ? 'video' : 'image';
    const prompt = String(spec.prompt || '').trim();
    if (!prompt) throw Object.assign(new Error('Prompt is required.'), { status: 400 });
    const { full: folderFull, rel: folderRel } = resolveFolder(root, spec.folder);
    const count = Math.min(Math.max(Number(spec.count) || 1, 1), kind === 'video' ? 4 : 12);
    const refs = resolveRefs(root, registerRefPaths(spec.images));

    // Picked characters bring their own reference images and are named in the
    // prompt, so "@Nora at a market stall" reaches the model as a described
    // person backed by pictures of her.
    const cast = resolveCharacters(charactersRoot, spec.characters);
    const characterRefs = cast.flatMap((c) => c.resolved);
    const common = {
      kind,
      count,
      prompt,
      composed: composePrompt(prompt, cast),
      castNames: cast.map((c) => c.name),
      folderFull,
      folderRel
    };

    if (kind === 'video') {
      const model = String(spec.model || videoModels[0]?.id || '').trim();
      if (!model) throw Object.assign(new Error('Pick a video model.'), { status: 400 });
      const modelSpec = videoModels.find((m) => m.id === model);
      const params = {
        duration: spec.duration ? Number(spec.duration) : undefined,
        resolution: spec.resolution,
        aspectRatio: spec.aspectRatio,
        size: spec.size,
        generateAudio: typeof spec.audio === 'boolean' ? spec.audio : undefined,
        seed: spec.seed,
        ...frameSelection({
          refs,
          characterRefs,
          spec: modelSpec,
          firstFrame: spec.firstFrame !== false,
          lastFrame: spec.lastFrame === true
        })
      };
      return { ...common, model, params, refs };
    }

    const api = apiById.get(spec.api) || apiById.get(defaultApi) || apis[0];
    if (!api) throw Object.assign(new Error('No image API configured.'), { status: 400 });
    const model = String(spec.model || api.models?.[0]?.id || '').trim();
    if (!model) throw Object.assign(new Error('Pick or type a model.'), { status: 400 });
    // A character's pictures ride alongside whatever was attached by hand,
    // capped the same way a manual attachment set is.
    return { ...common, api, model, size: spec.size, quality: spec.quality, refs: [...refs, ...characterRefs].slice(0, 8) };
  }

  function queueFromPlan(plan) {
    const { kind, count, prompt, composed, castNames, folderFull, folderRel, model, refs } = plan;
    const ids = [];
    for (let i = 0; i < count; i++) {
      ids.push(
        jobs.add({
          kind,
          folder: folderRel,
          prompt,
          model,
          characters: castNames,
          refs:
            kind === 'video'
              ? plan.params.refs.length + (plan.params.firstFrame ? 1 : 0) + (plan.params.lastFrame ? 1 : 0)
              : refs.length,
          run: (job) =>
            kind === 'video'
              ? runVideoJob(job, { prompt, composed, model, params: plan.params, refs, castNames, folderFull, folderRel })
              : runImageJob(job, {
                  prompt,
                  composed,
                  api: plan.api,
                  model,
                  size: plan.size,
                  quality: plan.quality,
                  refs,
                  castNames,
                  folderFull,
                  folderRel
                })
        })
      );
    }
    return {
      jobs: ids,
      kind,
      model,
      folder: folderRel,
      warning:
        kind === 'video' && plan.params.droppedCharacters
          ? 'Your attached frame takes precedence, so the character references were not sent.'
          : undefined
    };
  }

  const queueGeneration = (spec) => queueFromPlan(planGeneration(spec));

  // ---------- polling ----------

  const waitSeconds = (v) => Math.min(Math.max(Number(v) || 0, 0), WAIT_MAX_SEC);

  // Hold the request until every named job has finished (or the clock runs out),
  // then report them grouped the way a caller actually consumes them: what
  // landed, what failed, what is still going.
  async function collect(ids, seconds) {
    const settled = await jobs.wait(ids, seconds * 1000);
    const { jobs: rows, missing } = jobs.read(ids);
    return {
      settled,
      results: rows,
      items: rows.filter((j) => j.status === 'done' && j.item).map((j) => j.item),
      failed: rows
        .filter((j) => j.status === 'error')
        .map((j) => ({ id: j.id, prompt: j.prompt, model: j.model, error: j.error })),
      cancelled: rows.filter((j) => j.status === 'cancelled').map((j) => j.id),
      pending: rows.filter((j) => !isTerminal(j)).map((j) => j.id),
      missing: missing.length ? missing : undefined
    };
  }

  const routes = {
    async 'GET /api/state'() {
      // Every model row carries display-ready facts (age, cost, speed,
      // quality); speed is this gallery's own record of how long the model
      // has taken.
      return {
        title,
        root,
        defaultApi,
        ...modelsPayload(),
        videoReady: !!keyOf(VIDEO_KEY_API),
        // The ✨ button rides on the same OpenRouter key as video.
        enhanceReady: !!keyOf(VIDEO_KEY_API),
        enhanceModel,
        folders: listFolders(root),
        context: listContext(root),
        characters: listCharacters(charactersRoot),
        jobs: jobs.active()
      };
    },

    // What the settings panel shows: every provider, whether it has a key and
    // where that key came from. Never the key itself — only enough of it to
    // recognise which one is saved.
    'GET /api/config'() {
      return {
        configPath,
        root,
        writable: !!saveKey,
        videoApi: VIDEO_KEY_API,
        enhanceModel,
        apis: apis.map((a) => {
          const key = keyOf(a.id);
          const fromEnv = !!(!key ? false : a.keyEnv && process.env[a.keyEnv] === key);
          return {
            id: a.id,
            name: a.name || a.id,
            video: !!a.video,
            keyUrl: a.keyUrl || '',
            keyEnv: a.keyEnv || '',
            models: (a.models || []).length,
            hasKey: !!key,
            // An env var is the shell's to change, not ours — the panel says so
            // instead of offering a Remove that would not stick.
            source: key ? (fromEnv ? 'env' : 'config') : '',
            keyPreview: maskKey(key)
          };
        })
      };
    },

    // Save or clear one provider's key. An empty key removes it, which is how
    // the panel's Remove button works.
    async 'POST /api/config/key'(req) {
      if (!saveKey) throw Object.assign(new Error('This gallery cannot write the config file.'), { status: 400 });
      const { id, key } = await readJsonBody(req);
      if (!id || !apiById.has(id)) throw Object.assign(new Error(`Unknown API "${id}"`), { status: 400 });
      const trimmed = typeof key === 'string' ? key.trim() : '';
      // A pasted key with a stray newline or quote is a support ticket waiting
      // to happen; strip the obvious wrappers before it is written.
      const cleaned = trimmed.replace(/^["'`]|["'`]$/g, '').trim();
      if (cleaned.length > 500) throw Object.assign(new Error('That does not look like an API key.'), { status: 400 });
      saveKey(id, cleaned);
      return { ok: true, id, hasKey: !!cleaned, keyPreview: maskKey(cleaned) };
    },

    // Paged: a folder with thousands of generations must not turn into one
    // enormous response and one enormous DOM. The browser pulls the next page
    // as it scrolls.
    async 'GET /api/items'(req, res, url) {
      const folder = url.searchParams.get('folder') || '';
      const offset = Math.max(0, Number(url.searchParams.get('offset')) || 0);
      const limit = Math.min(Math.max(Number(url.searchParams.get('limit')) || 200, 1), 500);
      const all = listItems(root, folder);
      return { folder: resolveFolder(root, folder).rel, items: all.slice(offset, offset + limit), offset, total: all.length };
    },

    async 'GET /api/folders'() {
      return { folders: listFolders(root) };
    },

    async 'POST /api/folder'(req) {
      const { parent, name } = await readJsonBody(req);
      return { path: createFolder(root, parent, name), folders: listFolders(root) };
    },

    async 'POST /api/folder/rename'(req) {
      const { path: rel, name } = await readJsonBody(req);
      return { path: renameFolder(root, rel, name), folders: listFolders(root) };
    },

    async 'POST /api/folder/delete'(req) {
      const { path: rel } = await readJsonBody(req);
      const removed = deleteFolder(root, rel);
      return { removed, folders: listFolders(root) };
    },

    async 'POST /api/move'(req) {
      const { from, to, files } = await readJsonBody(req);
      return { ...moveItems(root, from, to, files), folders: listFolders(root) };
    },

    async 'POST /api/delete'(req) {
      const { folder, file } = await readJsonBody(req);
      return { ok: deleteItem(root, folder, file) };
    },

    // A reference image arrives either as a data URL from the browser or as a
    // path on disk: a script has files, and base64-ing a PNG through a shell is
    // nobody's idea of an API. Both dedupe to the same content-hash name, so
    // uploading the same picture twice costs nothing.
    async 'POST /api/context'(req) {
      const body = await readJsonBody(req, 64 * 1024 * 1024);
      if (body.dataUrl) return saveContext(root, body.dataUrl);
      const given = String(body.path || '').trim();
      if (!given) throw Object.assign(new Error('Send { dataUrl } or { path } — a local image file.'), { status: 400 });
      const full = path.resolve(given);
      if (kindOf(full) !== 'image') {
        throw Object.assign(new Error(`Not an image file: ${given} (png, jpg, webp or gif).`), { status: 400 });
      }
      let buf;
      try {
        buf = fs.readFileSync(full);
      } catch (e) {
        throw Object.assign(new Error(`Cannot read ${full}: ${e.code || e.message}`), { status: 400 });
      }
      if (buf.length > 32 * 1024 * 1024) throw Object.assign(new Error(`${given} is over 32 MB.`), { status: 400 });
      return { ...saveContext(root, `data:${mediaTypeOf(full)};base64,${buf.toString('base64')}`), path: full };
    },

    async 'POST /api/context/delete'(req) {
      const { file } = await readJsonBody(req);
      return { ok: deleteContext(root, file) };
    },

    async 'POST /api/thumb'(req) {
      const { folder, file, dataUrl } = await readJsonBody(req, 8 * 1024 * 1024);
      return saveThumb(root, folder, file, dataUrl);
    },

    // One id, a list of them, or everything still in flight — which is the
    // "stop the batch I just queued" escape hatch.
    async 'POST /api/cancel'(req) {
      const body = await readJsonBody(req);
      const cancelled = body.all
        ? jobs.cancelAll(body.kind === 'video' || body.kind === 'image' ? body.kind : '')
        : (Array.isArray(body.ids) ? body.ids : [body.id]).map((id) => String(id || '')).filter((id) => jobs.cancel(id));
      return { ok: cancelled.length > 0, cancelled };
    },

    // Poll instead of subscribe. `?ids=a,b,c` reports exactly those jobs and
    // `?wait=<seconds>` holds the request open until they have all finished, so
    // a script can submit and collect in two calls without parsing an event
    // stream. With no ids it reports the whole registry — finished jobs
    // included, for as long as they are retained — which is how you find a job
    // whose id you lost.
    async 'GET /api/jobs'(req, res, url) {
      const ids = (url.searchParams.get('ids') || '')
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean);
      const wait = waitSeconds(url.searchParams.get('wait'));
      const report = await collect(ids.length ? ids : jobs.pendingIds(), wait);
      if (ids.length) return report;
      const all = url.searchParams.get('active') === '1' ? jobs.active() : jobs.snapshot();
      return {
        ...report,
        results: all.sort((a, b) => (b.queuedAt || 0) - (a.queuedAt || 0)),
        active: jobs.pendingIds().length,
        limits: LIMITS,
        retainMs: JOB_RETAIN_MS
      };
    },

    // The catalogue on its own: every knob a generation may set, without the
    // folder tree, the cast and the reference library that `/api/state` carries.
    // `?kind=video`, `?api=openrouter`, `?q=veo`, `?detail=1`, `?limit=n`.
    'GET /api/models'(req, res, url) {
      const kind = url.searchParams.get('kind');
      const wantApi = url.searchParams.get('api');
      const q = (url.searchParams.get('q') || '').trim().toLowerCase();
      const detail = url.searchParams.get('detail') === '1';
      const limit = Math.min(Math.max(Number(url.searchParams.get('limit')) || 500, 1), 2000);
      const payload = modelsPayload();
      const row = (m, k, api, ready) => ({
        id: m.id,
        name: m.name || m.id,
        kind: k,
        api,
        ready,
        created: m.created || undefined,
        pricing: m.pricing || undefined,
        durations: m.durations || undefined,
        resolutions: m.resolutions || undefined,
        aspectRatios: m.aspectRatios || undefined,
        sizes: m.sizes || undefined,
        frames: m.frames || undefined,
        audio: m.audio || undefined,
        seed: m.seed || undefined,
        pricedAs: m.factsFrom || undefined,
        description: detail ? m.description || undefined : undefined,
        facts: detail ? m.facts : undefined
      });
      const rows = [];
      if (kind !== 'video') {
        for (const a of payload.apis) {
          if (wantApi && a.id !== wantApi) continue;
          for (const m of a.models) rows.push(row(m, 'image', a.id, a.hasKey));
        }
      }
      if (kind !== 'image' && (!wantApi || wantApi === VIDEO_KEY_API)) {
        const ready = !!keyOf(VIDEO_KEY_API);
        for (const m of payload.videoModels) rows.push(row(m, 'video', VIDEO_KEY_API, ready));
      }
      const matched = q ? rows.filter((m) => `${m.id} ${m.name}`.toLowerCase().includes(q)) : rows;
      return { models: matched.slice(0, limit), total: matched.length, defaultApi, videoApi: VIDEO_KEY_API };
    },

    // Rewrite the composer's prompt into something the picked model can work
    // with. Fast enough (a second or two) that the browser just waits, so this
    // is a plain request rather than a job.
    async 'POST /api/enhance'(req) {
      const body = await readJsonBody(req);
      const apiKey = keyOf(VIDEO_KEY_API);
      if (!apiKey) {
        throw Object.assign(
          new Error('Improving prompts needs an OpenRouter key. Run `bro imagine -p openrouter`, or add keys.openrouter to ~/.bro/config.json.'),
          { status: 400 }
        );
      }
      const kind = ['video', 'character'].includes(body.kind) ? body.kind : 'image';
      const model = String(body.model || '').trim();
      const spec = kind === 'video' ? videoModels.find((m) => m.id === model) : null;
      // Names only — the enhancer is told to keep them and not to re-describe
      // the face, which the reference images already carry.
      const cast = resolveCharacters(charactersRoot, body.characters).map((c) => ({ name: c.name }));
      const ctrl = new AbortController();
      const timer = setTimeout(() => ctrl.abort(), 45_000);
      try {
        return await enhancePrompt({
          apiKey,
          prompt: body.prompt,
          kind,
          model,
          spec,
          characters: cast,
          refs: Array.isArray(body.images) ? body.images.length : 0,
          enhanceModel,
          signal: ctrl.signal
        });
      } finally {
        clearTimeout(timer);
      }
    },

    // Queue one spec (`count` copies of it). `wait: <seconds>` turns it into a
    // blocking call that comes back with the finished items, which is all a
    // script wants for a single picture.
    async 'POST /api/generate'(req) {
      const body = await readJsonBody(req);
      const queued = queueGeneration(body);
      const wait = waitSeconds(body.wait);
      return wait ? { ...queued, ...(await collect(queued.jobs, wait)) } : queued;
    },

    // Many prompts, one request. The gallery's composer only ever sends one
    // spec, but a script writing a storyboard has fifty — and doing that as
    // fifty round trips means fifty chances to lose track of a job id.
    //
    //   { defaults: {…}, items: [{…}|"a prompt", …], wait: <seconds> }
    //
    // `defaults` is merged under every item, so the folder, model and knobs are
    // stated once. Images and video may be mixed freely; each item's `kind`
    // decides which queue it joins.
    async 'POST /api/batch'(req) {
      const body = await readJsonBody(req, 4 * 1024 * 1024);
      const list = Array.isArray(body) ? body : Array.isArray(body.items) ? body.items : null;
      if (!list?.length) throw Object.assign(new Error('Send { items: [ … ] } — one entry per generation.'), { status: 400 });
      if (list.length > BATCH_MAX_ITEMS) {
        throw Object.assign(new Error(`At most ${BATCH_MAX_ITEMS} items per batch (got ${list.length}).`), { status: 400 });
      }
      const defaults = (!Array.isArray(body) && body.defaults) || {};

      // Validate and count the whole batch before queueing any of it, so a typo
      // in item 40 doesn't leave 39 paid generations already running.
      const specs = list.map((raw, i) => {
        const spec = { ...defaults, ...(typeof raw === 'string' ? { prompt: raw } : raw || {}) };
        try {
          return { spec, plan: planGeneration(spec) };
        } catch (e) {
          throw Object.assign(new Error(`items[${i}]: ${e.message}`), { status: e.status || 400 });
        }
      });
      const total = specs.reduce((n, s) => n + s.plan.count, 0);
      if (total > BATCH_MAX_JOBS) {
        throw Object.assign(
          new Error(`That batch is ${total} generations; the limit is ${BATCH_MAX_JOBS}. Split it or lower "count".`),
          { status: 400 }
        );
      }

      const queued = specs.map(({ plan }) => queueFromPlan(plan));
      const ids = queued.flatMap((q) => q.jobs);
      const warnings = queued.map((q, i) => (q.warning ? `items[${i}]: ${q.warning}` : null)).filter(Boolean);
      const submitted = {
        jobs: ids,
        count: ids.length,
        images: queued.filter((q) => q.kind === 'image').reduce((n, q) => n + q.jobs.length, 0),
        videos: queued.filter((q) => q.kind === 'video').reduce((n, q) => n + q.jobs.length, 0),
        warnings: warnings.length ? warnings : undefined
      };
      const wait = waitSeconds(!Array.isArray(body) ? body.wait : 0);
      if (!wait) return submitted;
      return { ...submitted, ...(await collect(ids, wait)) };
    },

    // ---------- characters ----------

    async 'GET /api/characters'() {
      return { characters: listCharacters(charactersRoot) };
    },

    async 'POST /api/characters'(req) {
      const { name, description } = await readJsonBody(req);
      return { character: createCharacter(charactersRoot, { name, description }) };
    },

    async 'POST /api/characters/update'(req) {
      const { id, name, description, cover } = await readJsonBody(req);
      return { character: updateCharacter(charactersRoot, id, { name, description, cover }) };
    },

    async 'POST /api/characters/delete'(req) {
      const { id } = await readJsonBody(req);
      return { ok: deleteCharacter(charactersRoot, id) };
    },

    // A reference arrives either as an upload or as "make this generation one
    // of Nora's references", which is the loop that sharpens a character.
    async 'POST /api/characters/refs'(req) {
      const { id, dataUrl, folder, file, path: given } = await readJsonBody(req, 64 * 1024 * 1024);
      if (dataUrl) return addRefFromDataUrl(charactersRoot, id, dataUrl);
      // A photo on disk — the third way in, for the same reason /api/context
      // takes one: a script has files, not data URLs.
      if (given) {
        const full = path.resolve(String(given));
        if (kindOf(full) !== 'image') throw Object.assign(new Error(`Not an image file: ${given}`), { status: 400 });
        let buf;
        try {
          buf = fs.readFileSync(full);
        } catch (e) {
          throw Object.assign(new Error(`Cannot read ${full}: ${e.code || e.message}`), { status: 400 });
        }
        return addRefFromDataUrl(charactersRoot, id, `data:${mediaTypeOf(full)};base64,${buf.toString('base64')}`);
      }
      const src = resolveFile(root, folder, file);
      if (kindOf(src.name) !== 'image') throw Object.assign(new Error('Only images can be character references.'), { status: 400 });
      return addRefFromDataUrl(
        charactersRoot,
        id,
        `data:${mediaTypeOf(src.name)};base64,${fs.readFileSync(src.full).toString('base64')}`
      );
    },

    // Draw the character a set of reference shots with Nano Banana 2.
    async 'POST /api/characters/refs/generate'(req) {
      const { id, count } = await readJsonBody(req);
      const character = getCharacter(charactersRoot, id);
      if (!character) throw Object.assign(new Error(`No character "${id}"`), { status: 400 });
      const want = Math.min(Math.max(Number(count) || 5, 1), 5);
      const job = jobs.add({
        kind: 'image',
        folder: '',
        prompt: `Reference shots for ${character.name}`,
        model: CHARACTER_REF_MODEL,
        characterId: character.id,
        refs: character.refs.length,
        run: (j) => runCharacterRefsJob(j, { id: character.id, count: want })
      });
      return { job, model: CHARACTER_REF_MODEL, count: want };
    },

    // Promote chosen shots into real references; the rest are thrown away.
    async 'POST /api/characters/refs/keep'(req) {
      const { id, files } = await readJsonBody(req);
      return keepCandidates(charactersRoot, id, files);
    },

    async 'POST /api/characters/candidates/clear'(req) {
      const { id, files } = await readJsonBody(req);
      return { character: clearCandidates(charactersRoot, id, files) };
    },

    async 'POST /api/characters/refs/delete'(req) {
      const { id, file } = await readJsonBody(req);
      return { ok: deleteRef(charactersRoot, id, file), character: getCharacter(charactersRoot, id) };
    }
  };


  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://localhost');
    const method = req.method === 'HEAD' ? 'GET' : req.method;
    try {
      if (method === 'GET' && url.pathname === '/') {
        const html = fs.readFileSync(UI_HTML);
        res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store', 'content-length': html.length });
        res.end(req.method === 'HEAD' ? undefined : html);
        return;
      }

      // Live job feed. One long-lived response per tab; a comment line every
      // 25s keeps proxies and Windows' idle-socket reaper from closing it.
      if (method === 'GET' && url.pathname === '/api/events') {
        res.writeHead(200, {
          'content-type': 'text/event-stream; charset=utf-8',
          'cache-control': 'no-store',
          connection: 'keep-alive',
          'x-accel-buffering': 'no'
        });
        res.write(`retry: 2000\n\n`);
        res.write(`data: ${JSON.stringify({ type: 'snapshot', jobs: jobs.snapshot() })}\n\n`);
        const unsubscribe = jobs.subscribe(res);
        const ping = setInterval(() => {
          try {
            res.write(': ping\n\n');
          } catch {
            /* closing */
          }
        }, 25_000);
        ping.unref?.();
        const close = () => {
          clearInterval(ping);
          unsubscribe();
        };
        req.on('close', close);
        res.on('close', close);
        return;
      }

      if (method === 'GET' && (url.pathname.startsWith('/media/') || url.pathname.startsWith('/thumb/'))) {
        const isThumb = url.pathname.startsWith('/thumb/');
        const name = decodeURIComponent(url.pathname.slice('/media/'.length)); // '/thumb/' is the same length
        const { full, name: file, folder } = resolveFile(root, url.searchParams.get('f') || '', name);
        if (isThumb) {
          sendFile(req, res, thumbPath(root, folder, file), 'image/jpeg');
          return;
        }
        if (url.searchParams.get('dl')) res.setHeader('content-disposition', `attachment; filename="${file.replace(/[^\w.\-]/g, '_')}"`);
        sendFile(req, res, full, mediaTypeOf(file));
        return;
      }

      // A character's own reference images, served from the global library
      // rather than from any one gallery.
      if (method === 'GET' && url.pathname.startsWith('/charref/')) {
        // Kept shots keep their filename, so one route covers a reference and a
        // candidate still waiting to be judged.
        const name = path.basename(decodeURIComponent(url.pathname.slice('/charref/'.length)));
        const full = resolveCandidate(charactersRoot, url.searchParams.get('c') || '', name);
        if (!full) {
          res.writeHead(404);
          res.end('Not found');
          return;
        }
        sendFile(req, res, full, mediaTypeOf(name));
        return;
      }

      if (method === 'GET' && url.pathname.startsWith('/context/')) {
        const name = path.basename(decodeURIComponent(url.pathname.slice('/context/'.length)));
        sendFile(req, res, path.join(contextDir(root), name), mediaTypeOf(name));
        return;
      }

      const handler = routes[`${method} ${url.pathname}`];
      if (handler) {
        // Reads are harmless; a write from another site is not — it could spend
        // the user's credits or overwrite a saved key.
        if (method !== 'GET' && !sameOrigin(req)) {
          sendJson(res, 403, { error: 'Cross-site request refused.' });
          return;
        }
        sendJson(res, 200, (await handler(req, res, url)) ?? { ok: true });
        return;
      }

      res.writeHead(404);
      res.end('Not found');
    } catch (e) {
      if (res.headersSent) {
        res.end();
        return;
      }
      sendJson(res, e?.status || 500, { error: e?.message || String(e) });
    }
  });

  server.jobs = jobs;
  return server;
}

export function listenOnFreePort(server, start = 8790, tries = 20, host = '127.0.0.1') {
  return new Promise((resolve, reject) => {
    const attempt = (port, left) => {
      const onError = (err) => {
        server.removeListener('listening', onListening);
        if ((err.code === 'EADDRINUSE' || err.code === 'EACCES') && left > 0) attempt(port + 1, left - 1);
        else reject(err);
      };
      const onListening = () => {
        server.removeListener('error', onError);
        // Ask the socket rather than trusting `port` — with port 0 the OS picks
        // one, and only the socket knows which.
        resolve(server.address()?.port ?? port);
      };
      server.once('error', onError);
      server.once('listening', onListening);
      server.listen(port, host);
    };
    attempt(start, tries);
  });
}
