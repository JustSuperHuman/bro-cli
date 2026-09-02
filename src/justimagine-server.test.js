import { afterEach, beforeEach, expect, test } from 'bun:test';
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import {
  createJobs,
  createServer,
  explainRefFailure,
  frameSelection,
  listenOnFreePort,
  mapLimit,
  outputName
} from './justimagine-server.js';
import { appendHistory, listItems } from './justimagine-store.js';

const PNG = Buffer.from('89504e470d0a1a0a0000000d49484452', 'hex');
const HANG = 'NEVER_ANSWER_THIS';

let root;
let charRoot;
let upstream;
let upstreamBase;
let server;
let base;
let upstreamFail = false;
let upstreamBodies = [];
// Mutable so a test can hand the server a key mid-flight, the way saving one
// into the config does for a running service.
let keys = {};

beforeEach(async () => {
  root = fs.mkdtempSync(path.join(os.tmpdir(), 'ji-srv-'));
  charRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'ji-cast-'));
  upstreamFail = false;
  upstreamBodies = [];
  keys = { fake: 'test-key' };

  upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const c of req) chunks.push(c);
    upstreamBodies.push(Buffer.concat(chunks).toString('utf8'));
    // A distinctive marker makes the upstream never answer, so a test can cancel
    // a call that is genuinely in flight. It has to be a token no generated
    // instruction could contain — plain "hang" also matches "changes".
    if (Buffer.concat(chunks).toString().includes(HANG)) return;
    if (upstreamFail) {
      res.writeHead(402, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ error: { message: 'out of credit' } }));
      return;
    }
    res.writeHead(200, { 'content-type': 'application/json' });
    // A system message means the prompt enhancer is asking, not an image model.
    const asked = (() => { try { return JSON.parse(Buffer.concat(chunks).toString()); } catch { return {}; } })();
    if (asked.messages?.[0]?.role === 'system') {
      res.end(JSON.stringify({ choices: [{ message: { content: 'Here is the rewritten prompt: "a much better cat"' } }] }));
      return;
    }
    // A chat-routed image model returns the picture inside the message; the
    // images API returns a data array. Each chat reply gets distinct bytes,
    // because candidates are content-hashed and five identical shots would
    // collapse into one.
    if (asked.messages) {
      const unique = Buffer.concat([PNG, Buffer.from(`#${upstreamBodies.length}`)]).toString('base64');
      res.end(JSON.stringify({ choices: [{ message: { images: [{ image_url: { url: 'data:image/png;base64,' + unique } }] } }] }));
      return;
    }
    res.end(JSON.stringify({ data: [{ b64_json: PNG.toString('base64') }] }));
  });
  await new Promise((r) => upstream.listen(0, '127.0.0.1', r));
  upstreamBase = `http://127.0.0.1:${upstream.address().port}`;

  server = createServer({
    root,
    apis: [
      { id: 'fake', name: 'Fake', imagesUrl: `${upstreamBase}/v1/images/generations`, models: [{ id: 'gpt-image-1', name: 'Fake Image' }] },
      { id: 'openrouter', name: 'OpenRouter', chatUrl: `${upstreamBase}/v1/chat/completions`, video: true, models: [] }
    ],
    videoModels: [{ id: 'google/veo-3.1', name: 'Veo 3.1', durations: [4, 6, 8], audio: true, created: 1774224000, pricing: { perSecond: 0.2 } }],
    resolveKey: (id) => keys[id] || '',
    defaultApi: 'fake',
    charactersRoot: charRoot
  });
  // Port 0 lets the OS pick, so parallel test files can never collide.
  const port = await listenOnFreePort(server, 0);
  base = `http://127.0.0.1:${port}`;
});

// Drop the sockets first, then wait: close() alone waits for keep-alive
// connections the test client is still pooling, which never go idle in time.
const shutdown = async (s) => {
  const closed = new Promise((r) => s.close(r));
  s.closeAllConnections?.();
  await closed;
};

afterEach(async () => {
  server.jobs.closeAll();
  await shutdown(server);
  await shutdown(upstream);
  fs.rmSync(root, { recursive: true, force: true });
  fs.rmSync(charRoot, { recursive: true, force: true });
});

// What the fake upstream last received — JSON or multipart, both searchable.
const lastUpstreamBody = () => upstreamBodies[upstreamBodies.length - 1] || '';

const get = (p) => fetch(base + p);
const getJson = async (p) => {
  const r = await get(p);
  const j = await r.json();
  if (!r.ok) throw new Error(j.error);
  return j;
};
const post = async (p, body) => {
  const r = await fetch(base + p, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body ?? {}) });
  return { status: r.status, body: await r.json() };
};

// Drive the job feed the way the UI does, and resolve on the first terminal state.
function watchJobs(ids, { timeout = 8000 } = {}) {
  const want = new Set(ids);
  const done = new Map();
  return new Promise((resolve, reject) => {
    const ctrl = new AbortController();
    const timer = setTimeout(() => {
      ctrl.abort();
      reject(new Error(`timed out waiting for ${[...want].join(', ')}`));
    }, timeout);
    fetch(base + '/api/events', { signal: ctrl.signal })
      .then(async (res) => {
        const reader = res.body.getReader();
        let buf = '';
        for (;;) {
          const { value, done: eof } = await reader.read();
          if (eof) break;
          buf += Buffer.from(value).toString('utf8');
          let i;
          while ((i = buf.indexOf('\n\n')) >= 0) {
            const frame = buf.slice(0, i);
            buf = buf.slice(i + 2);
            const line = frame.split('\n').find((l) => l.startsWith('data: '));
            if (!line) continue;
            const msg = JSON.parse(line.slice(6));
            for (const job of msg.type === 'snapshot' ? msg.jobs : msg.job ? [msg.job] : []) {
              if (want.has(job.id) && ['done', 'error', 'cancelled'].includes(job.status)) done.set(job.id, job);
            }
            if (done.size === want.size) {
              clearTimeout(timer);
              ctrl.abort();
              return resolve([...done.values()]);
            }
          }
        }
      })
      .catch((e) => {
        if (e.name !== 'AbortError') {
          clearTimeout(timer);
          reject(e);
        }
      });
  });
}

// ---------- state ----------

test('state describes the gallery, both catalogues and the key situation', async () => {
  const s = await getJson('/api/state');
  expect(s.title).toBe('JustImagine');
  expect(s.root).toBe(root);
  expect(s.defaultApi).toBe('fake');
  expect(s.apis[0]).toMatchObject({ id: 'fake', hasKey: true, video: false });
  expect(s.apis[1]).toMatchObject({ id: 'openrouter', hasKey: false, video: true });
  expect(s.videoModels[0].id).toBe('google/veo-3.1');
  // every model row carries display-ready facts for the picker
  expect(s.videoModels[0].facts.cost).toMatchObject({ rating: 3, label: '20¢/s' });
  expect(s.videoModels[0].facts.age.label).toMatch(/^\d+(\.\d)?(mo|y|d)$|^new$/);
  expect(s.apis[0].models[0].facts).toEqual({ age: null, cost: null, speed: null, quality: null });
  expect(s.videoReady).toBe(false); // no openrouter key in this fixture
  expect(s.folders).toMatchObject({ path: '', total: 0 });
  expect(s.jobs).toEqual([]);
});

test('the UI is served at the root', async () => {
  const r = await get('/');
  expect(r.status).toBe(200);
  expect(r.headers.get('content-type')).toContain('text/html');
  expect(await r.text()).toContain('JustImagine');
});

// ---------- folders ----------

test('folders are created, renamed, listed and deleted through the API', async () => {
  expect((await post('/api/folder', { parent: '', name: 'Shoots' })).body.path).toBe('Shoots');
  expect((await post('/api/folder', { parent: 'Shoots', name: 'Night' })).body.path).toBe('Shoots/Night');

  const renamed = await post('/api/folder/rename', { path: 'Shoots/Night', name: 'Dusk' });
  expect(renamed.body.path).toBe('Shoots/Dusk');
  expect(renamed.body.folders.children[0].children[0].path).toBe('Shoots/Dusk');

  fs.writeFileSync(path.join(root, 'Shoots', 'Dusk', 'a.png'), PNG);
  const removed = await post('/api/folder/delete', { path: 'Shoots' });
  expect(removed.body.removed).toBe(1);
  expect(removed.body.folders.children).toEqual([]);
});

test('a folder path that tries to escape the root is refused', async () => {
  const r = await post('/api/folder/delete', { path: '../..' });
  expect(r.status).toBe(500);
  expect(r.body.error).toMatch(/Invalid folder path/);
  expect(fs.existsSync(root)).toBe(true);
});

test('items list per folder, and move between folders with their metadata', async () => {
  fs.mkdirSync(path.join(root, 'a'), { recursive: true });
  fs.writeFileSync(path.join(root, 'a', 'one.png'), PNG);
  appendHistory(path.join(root, 'a'), { file: 'one.png', kind: 'image', prompt: 'travels', model: 'm' });

  expect((await getJson('/api/items?folder=a')).items.map((i) => i.file)).toEqual(['one.png']);
  expect((await getJson('/api/items?folder=')).items).toEqual([]);

  const moved = await post('/api/move', { from: 'a', to: 'b', files: ['one.png'] });
  expect(moved.body.moved).toBe(1);
  const after = await getJson('/api/items?folder=b');
  expect(after.items[0].prompt).toBe('travels');
  expect(after.items[0].folder).toBe('b');
});

// ---------- media serving ----------

test('media is served with immutable caching and byte ranges', async () => {
  fs.writeFileSync(path.join(root, 'clip.mp4'), Buffer.from('0123456789'));
  const full = await get('/media/clip.mp4?f=');
  expect(full.status).toBe(200);
  expect(full.headers.get('content-type')).toBe('video/mp4');
  expect(full.headers.get('accept-ranges')).toBe('bytes');
  expect(full.headers.get('cache-control')).toContain('immutable');

  const part = await fetch(base + '/media/clip.mp4?f=', { headers: { range: 'bytes=2-5' } });
  expect(part.status).toBe(206);
  expect(part.headers.get('content-range')).toBe('bytes 2-5/10');
  expect(await part.text()).toBe('2345');

  const tail = await fetch(base + '/media/clip.mp4?f=', { headers: { range: 'bytes=-3' } });
  expect(await tail.text()).toBe('789');

  const open = await fetch(base + '/media/clip.mp4?f=', { headers: { range: 'bytes=8-' } });
  expect(await open.text()).toBe('89');

  const bad = await fetch(base + '/media/clip.mp4?f=', { headers: { range: 'bytes=99-200' } });
  expect(bad.status).toBe(416);
});

// Small files are answered in one write and large ones streamed; both have to
// return the exact bytes and leave the keep-alive socket usable for whatever
// the page asks for next. (Piping a small file used to lose that race on
// Windows and reset the connection.)
test('both the buffered and the streamed path return exact bytes and keep the socket alive', async () => {
  const big = crypto.randomBytes(700 * 1024); // past the buffered cutoff
  fs.writeFileSync(path.join(root, 'big.mp4'), big);
  fs.writeFileSync(path.join(root, 'small.png'), PNG);

  for (let i = 0; i < 12; i++) {
    const whole = Buffer.from(await (await get('/media/big.mp4?f=')).arrayBuffer());
    expect(whole.equals(big)).toBe(true);

    const mid = await fetch(base + '/media/big.mp4?f=', { headers: { range: 'bytes=500000-500099' } });
    expect(Buffer.from(await mid.arrayBuffer()).equals(big.subarray(500000, 500100))).toBe(true);

    const tiny = Buffer.from(await (await get('/media/small.png?f=')).arrayBuffer());
    expect(tiny.equals(PNG)).toBe(true);

    // the next request on that same connection must still work
    expect((await getJson('/api/folders')).folders.path).toBe('');
  }
});

test('an empty file is served as an empty body, not a hang', async () => {
  fs.writeFileSync(path.join(root, 'empty.png'), Buffer.alloc(0));
  const r = await get('/media/empty.png?f=');
  expect(r.status).toBe(200);
  expect(r.headers.get('content-length')).toBe('0');
  expect((await r.arrayBuffer()).byteLength).toBe(0);
});

test('a download link asks the browser to save the file', async () => {
  fs.writeFileSync(path.join(root, 'shot.png'), PNG);
  const r = await get('/media/shot.png?f=&dl=1');
  expect(r.headers.get('content-disposition')).toContain('shot.png');
});

test('media requests cannot read outside the gallery', async () => {
  fs.writeFileSync(path.join(path.dirname(root), 'secret.png'), 'nope');
  const r = await get('/media/' + encodeURIComponent('../secret.png') + '?f=');
  expect(r.status).toBe(404);
});

test('a big folder is served in pages so neither the response nor the DOM explodes', async () => {
  fs.mkdirSync(path.join(root, 'many'), { recursive: true });
  for (let i = 0; i < 250; i++) {
    const f = path.join(root, 'many', `shot-${String(i).padStart(3, '0')}.png`);
    fs.writeFileSync(f, PNG);
    fs.utimesSync(f, new Date(1_700_000_000 + i), new Date(1_700_000_000 + i));
  }

  const first = await getJson('/api/items?folder=many');
  expect(first.total).toBe(250);
  expect(first.offset).toBe(0);
  expect(first.items.length).toBe(200); // the default page
  expect(first.items[0].file).toBe('shot-249.png'); // newest first

  const second = await getJson('/api/items?folder=many&offset=200&limit=200');
  expect(second.items.length).toBe(50);
  expect(second.offset).toBe(200);
  expect(second.items[0].file).toBe('shot-049.png');
  // the two pages together are the whole folder, with nothing repeated
  const files = new Set([...first.items, ...second.items].map((i) => i.file));
  expect(files.size).toBe(250);

  // a page size beyond the cap is clamped rather than honoured
  expect((await getJson('/api/items?folder=many&limit=9999')).items.length).toBe(250);
  expect((await getJson('/api/items?folder=many&offset=999')).items).toEqual([]);
});

test('a missing thumbnail 404s so the UI can fall back to the original', async () => {
  fs.writeFileSync(path.join(root, 'shot.png'), PNG);
  expect((await get('/thumb/shot.png?f=')).status).toBe(404);
  const jpeg = 'data:image/jpeg;base64,' + Buffer.from('jpeg-bytes').toString('base64');
  expect((await post('/api/thumb', { folder: '', file: 'shot.png', dataUrl: jpeg })).body.ok).toBe(true);
  const t = await get('/thumb/shot.png?f=');
  expect(t.status).toBe(200);
  expect(t.headers.get('content-type')).toBe('image/jpeg');
});

// ---------- context ----------

test('reference images round-trip through the context library', async () => {
  const dataUrl = 'data:image/png;base64,' + PNG.toString('base64');
  const saved = await post('/api/context', { dataUrl });
  expect(saved.body.existed).toBe(false);
  expect((await post('/api/context', { dataUrl })).body.existed).toBe(true);

  const served = await get('/context/' + saved.body.file);
  expect(served.status).toBe(200);
  expect(Buffer.from(await served.arrayBuffer()).equals(PNG)).toBe(true);

  expect((await getJson('/api/state')).context.length).toBe(1);
  await post('/api/context/delete', { file: saved.body.file });
  expect((await getJson('/api/state')).context.length).toBe(0);
});

// ---------- generation ----------

test('a generation writes into the selected folder and reports through the job feed', async () => {
  await post('/api/folder', { parent: '', name: 'Set' });
  const { body } = await post('/api/generate', { kind: 'image', folder: 'Set', prompt: 'a red bicycle', model: 'gpt-image-1', count: 2 });
  expect(body.jobs.length).toBe(2);

  const finished = await watchJobs(body.jobs);
  expect(finished.every((j) => j.status === 'done')).toBe(true);

  const items = listItems(root, 'Set');
  expect(items.length).toBe(2);
  expect(items[0].prompt).toBe('a red bicycle');
  expect(items[0].kind).toBe('image');
  expect(items[0].api).toBe('fake');
  expect(items[0].folder).toBe('Set');
  expect(items[0].ms).toBeGreaterThanOrEqual(0);
  expect(fs.readFileSync(path.join(root, 'Set', items[0].file)).equals(PNG)).toBe(true);
  // and it is readable straight back out over http
  expect((await get('/media/' + encodeURIComponent(items[0].file) + '?f=Set')).status).toBe(200);
});

test('an upstream failure becomes an error job, not a crash', async () => {
  upstreamFail = true;
  const { body } = await post('/api/generate', { kind: 'image', folder: '', prompt: 'p', model: 'gpt-image-1' });
  const [job] = await watchJobs(body.jobs);
  expect(job.status).toBe('error');
  expect(job.error).toMatch(/out of credit/);
  expect(listItems(root, '')).toEqual([]);
});

test('generating without a prompt is refused up front', async () => {
  const r = await post('/api/generate', { kind: 'image', folder: '', prompt: '   ', model: 'gpt-image-1' });
  expect(r.status).toBe(400);
  expect(r.body.error).toMatch(/Prompt is required/);
});

test('video without an OpenRouter key fails with instructions, not a stack trace', async () => {
  const { body } = await post('/api/generate', { kind: 'video', folder: '', prompt: 'a wave', model: 'google/veo-3.1' });
  const [job] = await watchJobs(body.jobs);
  expect(job.status).toBe('error');
  expect(job.error).toMatch(/OpenRouter key/);
});

test('cancelling an in-flight generation aborts the upstream call and writes nothing', async () => {
  // The marker makes the fake upstream sit on the request, so the cancel lands
  // while the call is genuinely in flight.
  const { body } = await post('/api/generate', { kind: 'image', folder: '', prompt: HANG, model: 'gpt-image-1' });
  await new Promise((r) => setTimeout(r, 30));
  expect((await post('/api/cancel', { id: body.jobs[0] })).body.ok).toBe(true);
  const [job] = await watchJobs(body.jobs);
  expect(job.status).toBe('cancelled');
  expect(listItems(root, '')).toEqual([]);
});

test('a job still in the queue is cancelled without ever starting', async () => {
  const jobs = createJobs({ limits: { image: 1, video: 1 } });
  let started = 0;
  const first = jobs.add({ kind: 'image', folder: '', prompt: 'p', model: 'm', refs: 0, run: () => new Promise(() => {}) });
  const queued = jobs.add({
    kind: 'image', folder: '', prompt: 'p', model: 'm', refs: 0,
    run: () => { started++; return Promise.resolve({ file: 'x.png' }); }
  });
  expect(jobs.snapshot().find((j) => j.id === queued).status).toBe('queued');
  expect(jobs.cancel(queued)).toBe(true);
  expect(jobs.snapshot().find((j) => j.id === queued).status).toBe('cancelled');
  jobs.cancel(first);
  await new Promise((r) => setTimeout(r, 20));
  expect(started).toBe(0);
});

test('the event stream replays in-flight work to a page that reconnects', async () => {
  const jobs = createJobs({ limits: { image: 1, video: 1 } });
  let release;
  const gate = new Promise((r) => (release = r));
  const id = jobs.add({ kind: 'image', folder: '', prompt: 'slow', model: 'm', refs: 0, run: () => gate.then(() => ({ file: 'x.png' })) });
  expect(jobs.active().map((j) => j.id)).toEqual([id]);
  expect(jobs.snapshot()[0].status).toBe('running');
  release();
  await gate;
  await new Promise((r) => setTimeout(r, 10));
  expect(jobs.active()).toEqual([]);
});

test('the queue holds back work past the concurrency limit', async () => {
  const jobs = createJobs({ limits: { image: 1, video: 1 } });
  const gates = [];
  const ids = [0, 1].map(() => jobs.add({
    kind: 'image', folder: '', prompt: 'p', model: 'm', refs: 0,
    run: () => new Promise((r) => gates.push(r))
  }));
  await new Promise((r) => setTimeout(r, 10));
  const byId = new Map(jobs.snapshot().map((j) => [j.id, j.status]));
  expect(byId.get(ids[0])).toBe('running');
  expect(byId.get(ids[1])).toBe('queued');
  gates.forEach((g) => g({ file: 'x.png' }));
});

// ---------- prompt enhancement ----------

test('improving a prompt without an OpenRouter key explains what to do', async () => {
  const r = await post('/api/enhance', { prompt: 'a cat', kind: 'image', model: 'gpt-image-1' });
  expect(r.status).toBe(400);
  expect(r.body.error).toMatch(/OpenRouter key/);
});

test('the enhance route reports the model it would use', async () => {
  const s = await getJson('/api/state');
  expect(s.enhanceModel).toBe('google/gemini-3.7-flash');
  expect(s.enhanceReady).toBe(false); // no openrouter key in this fixture
});

// Give the running server an OpenRouter key and point the enhancer at the fake
// chat endpoint — the same thing saving a key into the config does live.
function armEnhancer() {
  keys.openrouter = 'or-key';
  process.env.JUSTIMAGINE_CHAT_URL = `${upstreamBase}/v1/chat/completions`;
  return () => delete process.env.JUSTIMAGINE_CHAT_URL;
}

test('a prompt is rewritten, with the model and the cast folded into the instruction', async () => {
  await post('/api/characters', { name: 'Nora', description: 'green field jacket' });
  const done = armEnhancer();
  try {
    expect((await getJson('/api/state')).enhanceReady).toBe(true);

    const r = await post('/api/enhance', {
      prompt: 'a cat',
      kind: 'video',
      model: 'google/veo-3.1',
      characters: ['nora'],
      images: ['x.png']
    });
    expect(r.status).toBe(200);
    expect(r.body.prompt).toBe('a much better cat'); // the fake reply, unwrapped
    expect(r.body.model).toBe('google/gemini-3.7-flash');

    const sent = JSON.parse(lastUpstreamBody());
    expect(sent.model).toBe('google/gemini-3.7-flash');
    const system = sent.messages[0].content;
    expect(system).toContain('text-to-video model "google/veo-3.1"');
    expect(system).toContain('4–8 seconds'); // pulled from that model's own capabilities
    expect(system).toContain('Nora is a saved character');
    expect(system).toContain('1 reference image will be attached');
    expect(sent.messages[1].content).toBe('a cat');
  } finally {
    done();
  }
});

test('an image prompt gets image guidance, not camera moves', async () => {
  const done = armEnhancer();
  try {
    await post('/api/enhance', { prompt: 'a cat', kind: 'image', model: 'google/gemini-3-pro-image' });
    const system = JSON.parse(lastUpstreamBody()).messages[0].content;
    expect(system).toContain('text-to-image model "google/gemini-3-pro-image"');
    expect(system).toContain('composition and framing');
    expect(system).not.toContain('camera');
  } finally {
    done();
  }
});

test('the character kind reaches the enhancer as its own job', async () => {
  const done = armEnhancer();
  try {
    const r = await post('/api/enhance', { kind: 'character', prompt: 'a woman with curly hair' });
    expect(r.status).toBe(200);
    const system = JSON.parse(lastUpstreamBody()).messages[0].content;
    expect(system).toContain('character description');
    expect(system).toContain('No pose, no location');
    expect(system).not.toContain('text-to-image');
  } finally {
    done();
  }
});

test('an unknown kind falls back to image rather than erroring', async () => {
  const done = armEnhancer();
  try {
    await post('/api/enhance', { kind: 'nonsense', prompt: 'a cat', model: 'm' });
    expect(JSON.parse(lastUpstreamBody()).messages[0].content).toContain('text-to-image');
  } finally {
    done();
  }
});

test('an empty prompt is refused before anything is called', async () => {
  const done = armEnhancer();
  try {
    const before = upstreamBodies.length;
    const r = await post('/api/enhance', { prompt: '   ', kind: 'image', model: 'x' });
    expect(r.status).toBe(500);
    expect(r.body.error).toMatch(/write an idea first/i);
    expect(upstreamBodies.length).toBe(before);
  } finally {
    done();
  }
});

// ---------- characters ----------

const charDataUrl = (seed) => 'data:image/png;base64,' + Buffer.from('ref-' + seed).toString('base64');

test('characters are created, listed, edited and deleted through the API', async () => {
  const made = await post('/api/characters', { name: 'Nora', description: 'freckles, green jacket' });
  expect(made.body.character).toMatchObject({ id: 'nora', name: 'Nora', refs: [] });

  expect((await getJson('/api/state')).characters.map((c) => c.id)).toEqual(['nora']);
  expect((await getJson('/api/characters')).characters.length).toBe(1);

  const renamed = await post('/api/characters/update', { id: 'nora', name: 'Nora Vale' });
  expect(renamed.body.character.id).toBe('nora-vale');

  expect((await post('/api/characters/delete', { id: 'nora-vale' })).body.ok).toBe(true);
  expect((await getJson('/api/characters')).characters).toEqual([]);
});

test('a reference can be uploaded, served back, and deleted', async () => {
  await post('/api/characters', { name: 'Nora' });
  const added = await post('/api/characters/refs', { id: 'nora', dataUrl: charDataUrl(1) });
  expect(added.body.existed).toBe(false);
  const file = added.body.file;

  const served = await get('/charref/' + file + '?c=nora');
  expect(served.status).toBe(200);
  expect(await served.text()).toBe('ref-1');

  expect((await post('/api/characters/refs/delete', { id: 'nora', file })).body.character.refs).toEqual([]);
});

test('a generated image can be promoted into a character reference', async () => {
  await post('/api/characters', { name: 'Nora' });
  fs.writeFileSync(path.join(root, 'portrait.png'), PNG);

  const promoted = await post('/api/characters/refs', { id: 'nora', folder: '', file: 'portrait.png' });
  expect(promoted.body.character.refs.length).toBe(1);
  const served = await get('/charref/' + promoted.body.file + '?c=nora');
  expect(Buffer.from(await served.arrayBuffer()).equals(PNG)).toBe(true);

  // a clip is not a usable likeness reference
  fs.writeFileSync(path.join(root, 'clip.mp4'), Buffer.from('x'));
  const bad = await post('/api/characters/refs', { id: 'nora', folder: '', file: 'clip.mp4' });
  expect(bad.status).toBe(400);
  expect(bad.body.error).toMatch(/Only images/);
});

test('a character reference request cannot escape the library', async () => {
  await post('/api/characters', { name: 'Nora' });
  expect((await get('/charref/x.png?c=' + encodeURIComponent('../..'))).status).toBe(500);
  expect((await get('/charref/' + encodeURIComponent('../character.json') + '?c=nora')).status).toBe(404);
});

test('picking a character attaches its references and names it in the prompt', async () => {
  await post('/api/characters', { name: 'Nora', description: 'freckles, green field jacket' });
  await post('/api/characters/refs', { id: 'nora', dataUrl: charDataUrl(1) });
  await post('/api/characters/refs', { id: 'nora', dataUrl: charDataUrl(2) });

  const { body } = await post('/api/generate', {
    kind: 'image',
    folder: '',
    prompt: '@Nora at a market stall',
    model: 'gpt-image-1',
    characters: ['nora']
  });
  const [job] = await watchJobs(body.jobs);
  expect(job.status).toBe('done');
  expect(job.characters).toEqual(['Nora']);

  // the upstream saw the composed prompt and both reference images
  const sent = lastUpstreamBody();
  expect(sent).toContain('Nora at a market stall');
  expect(sent).not.toContain('@Nora');
  expect(sent).toContain('freckles, green field jacket');

  // history records what the user typed, plus who was in it
  const item = listItems(root, '')[0];
  expect(item.prompt).toBe('@Nora at a market stall');
  expect(item.characters).toEqual(['Nora']);
  expect(item.refs).toBe(2);
});

test('an unknown character id is ignored rather than failing the generation', async () => {
  const { body } = await post('/api/generate', { kind: 'image', folder: '', prompt: 'a bench', model: 'gpt-image-1', characters: ['nobody', '../escape'] });
  const [job] = await watchJobs(body.jobs);
  expect(job.status).toBe('done');
  expect(job.characters).toEqual([]);
  expect(listItems(root, '')[0].characters).toBeUndefined();
});

// ---------- drawing a reference sheet ----------

test('mapLimit keeps order, caps concurrency, and survives a failure', async () => {
  let live = 0;
  let peak = 0;
  const out = await mapLimit([1, 2, 3, 4, 5, 6], 2, async (n) => {
    peak = Math.max(peak, ++live);
    await new Promise((r) => setTimeout(r, 5));
    live--;
    if (n === 3) throw new Error('shot 3 failed');
    return n * 10;
  });
  expect(peak).toBeLessThanOrEqual(2);
  expect(out.map((r) => (r.ok ? r.value : r.error))).toEqual([10, 20, 'shot 3 failed', 40, 50, 60]);
});

test('a reference sheet is one seed plus variations drawn from it', async () => {
  keys.openrouter = 'or-key';
  await post('/api/characters', { name: 'Nora', description: 'freckles, green field jacket' });

  const started = await post('/api/characters/refs/generate', { id: 'nora', count: 5 });
  expect(started.status).toBe(200);
  expect(started.body.model).toBe('google/gemini-3.1-flash-image'); // Nano Banana 2
  expect(started.body.count).toBe(5);

  const [job] = await watchJobs([started.body.job]);
  expect(job.status).toBe('done');
  expect(job.characterId).toBe('nora'); // so the gallery leaves it alone
  expect(job.item.kind).toBe('character-refs');
  expect(job.item.files.length).toBe(5);

  // they arrive as candidates, not references — nothing is kept without asking
  const c = (await getJson('/api/characters')).characters[0];
  expect(c.candidates.length).toBe(5);
  expect(c.refs).toEqual([]);

  // the seed went out with no reference; every variation carried exactly one
  const chats = upstreamBodies.map((b) => JSON.parse(b)).filter((b) => b.model === 'google/gemini-3.1-flash-image');
  expect(chats.length).toBe(5);
  const withRef = chats.filter((b) => Array.isArray(b.messages[0].content));
  expect(withRef.length).toBe(4);
  for (const b of withRef) expect(b.messages[0].content.filter((p) => p.type === 'image_url').length).toBe(1);
});

test('an already-seeded character draws more angles instead of a fresh face', async () => {
  keys.openrouter = 'or-key';
  await post('/api/characters', { name: 'Nora', description: 'freckles' });
  await post('/api/characters/refs', { id: 'nora', dataUrl: 'data:image/png;base64,' + PNG.toString('base64') });

  const started = await post('/api/characters/refs/generate', { id: 'nora', count: 5 });
  const [job] = await watchJobs([started.body.job]);
  expect(job.status).toBe('done');
  // no seed shot: all five are variations off the reference it already had
  const chats = upstreamBodies.map((b) => JSON.parse(b)).filter((b) => b.model === 'google/gemini-3.1-flash-image');
  expect(chats.every((b) => Array.isArray(b.messages[0].content))).toBe(true);
  expect(job.item.files.length).toBe(5);
});

test('the shots can be kept, and the rest thrown away', async () => {
  keys.openrouter = 'or-key';
  await post('/api/characters', { name: 'Nora', description: 'freckles' });
  const started = await post('/api/characters/refs/generate', { id: 'nora', count: 5 });
  await watchJobs([started.body.job]);
  const cands = (await getJson('/api/characters')).characters[0].candidates;

  const kept = await post('/api/characters/refs/keep', { id: 'nora', files: cands.slice(0, 2) });
  expect(kept.body.kept.length).toBe(2);
  expect(kept.body.character.refs.length).toBe(2);

  const cleared = await post('/api/characters/candidates/clear', { id: 'nora' });
  expect(cleared.body.character.candidates).toEqual([]);
  expect(cleared.body.character.refs.length).toBe(2); // keeping survived the clear
});

test('a kept shot is served from the same url it had as a candidate', async () => {
  keys.openrouter = 'or-key';
  await post('/api/characters', { name: 'Nora', description: 'freckles' });
  const started = await post('/api/characters/refs/generate', { id: 'nora', count: 1 });
  await watchJobs([started.body.job]);
  const file = (await getJson('/api/characters')).characters[0].candidates[0];

  expect((await get(`/charref/${file}?c=nora`)).status).toBe(200);
  await post('/api/characters/refs/keep', { id: 'nora', files: [file] });
  expect((await get(`/charref/${file}?c=nora`)).status).toBe(200);
});

test('drawing needs a key, a real character, and something to draw from', async () => {
  // no OpenRouter key
  await post('/api/characters', { name: 'Nora', description: 'freckles' });
  const noKey = await post('/api/characters/refs/generate', { id: 'nora', count: 5 });
  const [failed] = await watchJobs([noKey.body.job]);
  expect(failed.status).toBe('error');
  expect(failed.error).toMatch(/OpenRouter key/);

  // an unknown character never becomes a job at all
  const missing = await post('/api/characters/refs/generate', { id: 'nobody' });
  expect(missing.status).toBe(400);

  // nothing to draw from
  keys.openrouter = 'or-key';
  await post('/api/characters', { name: 'Blank' });
  const blank = await post('/api/characters/refs/generate', { id: 'blank', count: 5 });
  const [blankJob] = await watchJobs([blank.body.job]);
  expect(blankJob.status).toBe('error');
  expect(blankJob.error).toMatch(/description first/);
});

// ---------- undersized references ----------

// A 2x1 PNG: far below every upstream's floor.
const TINY_PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64');

test('a pixel-size rejection names the reference that caused it', () => {
  const refs = [{ file: 'crop.png', buf: TINY_PNG }];
  const upstream = '400 expected the width to be at least 300px, but received a 185x151px image instead';
  const out = explainRefFailure(upstream, refs);
  expect(out).toContain(upstream); // the original reason survives
  expect(out).toContain('crop.png (2×1)');
  expect(out).toContain('add it again');
});

test('an unrelated failure is passed through untouched', () => {
  const refs = [{ file: 'crop.png', buf: TINY_PNG }];
  expect(explainRefFailure('402 out of credit', refs)).toBe('402 out of credit');
  expect(explainRefFailure('429 slow down', refs)).toBe('429 slow down');
});

test('a size complaint with nothing small to blame is left alone', () => {
  const big = { file: 'ok.png', buf: Buffer.from('unmeasurable') };
  const msg = 'invalid resolution requested';
  expect(explainRefFailure(msg, [big])).toBe(msg);
  expect(explainRefFailure(msg, [])).toBe(msg);
});

// ---------- frame conditioning ----------

const A = { file: 'a.png' };
const B = { file: 'b.png' };
const both = { frames: ['first_frame', 'last_frame'] };

test('the first attachment opens the clip and the second can close it', () => {
  expect(frameSelection({ refs: [A, B], spec: both, firstFrame: true, lastFrame: true })).toEqual({
    firstFrame: A,
    lastFrame: B,
    refs: [],
    droppedCharacters: false
  });
});

test('a closing frame needs an opening frame, a second reference, and a model that takes one', () => {
  // no second reference
  expect(frameSelection({ refs: [A], spec: both, firstFrame: true, lastFrame: true }).lastFrame).toBe(null);
  // frame conditioning turned off entirely
  expect(frameSelection({ refs: [A, B], spec: both, firstFrame: false, lastFrame: true }).lastFrame).toBe(null);
  // the model only accepts an opening frame
  expect(frameSelection({ refs: [A, B], spec: { frames: ['first_frame'] }, firstFrame: true, lastFrame: true })).toMatchObject({
    firstFrame: A,
    lastFrame: null
  });
});

test('references a model cannot use as frames become style guidance instead', () => {
  const none = { firstFrame: null, lastFrame: null, droppedCharacters: false };
  // a model with no frame support at all
  expect(frameSelection({ refs: [A, B], spec: { frames: null }, firstFrame: true })).toEqual({ ...none, refs: [A, B] });
  // frame conditioning switched off by the user
  expect(frameSelection({ refs: [A, B], spec: both, firstFrame: false })).toEqual({ ...none, refs: [A, B] });
  // frames and references are mutually exclusive, because upstream ignores the
  // references the moment a frame is present
  expect(frameSelection({ refs: [A, B], spec: both, firstFrame: true }).refs).toEqual([]);
});

test('no attachments means no frames and nothing to guide with', () => {
  const none = { firstFrame: null, lastFrame: null, refs: [], droppedCharacters: false };
  expect(frameSelection({ refs: [], spec: both, firstFrame: true, lastFrame: true })).toEqual(none);
  expect(frameSelection({})).toEqual(none);
});

// A character portrait must never be pinned as frame one — that would force
// every clip to open on that exact photo instead of using it as "who this is".
const NORA = { file: 'nora-1.png' };

test('character references guide the shot rather than becoming its frames', () => {
  expect(frameSelection({ refs: [], characterRefs: [NORA], spec: both, firstFrame: true })).toEqual({
    firstFrame: null,
    lastFrame: null,
    refs: [NORA],
    droppedCharacters: false
  });
});

test('a hand-attached frame wins over the cast, and says so', () => {
  const r = frameSelection({ refs: [A], characterRefs: [NORA], spec: both, firstFrame: true });
  expect(r.firstFrame).toBe(A);
  expect(r.refs).toEqual([]);
  // upstream would silently ignore the references, so the caller is told
  expect(r.droppedCharacters).toBe(true);
});

test('with no frame taken, attachments and the cast guide the shot together', () => {
  const r = frameSelection({ refs: [A], characterRefs: [NORA], spec: { frames: null }, firstFrame: true });
  expect(r.refs).toEqual([A, NORA]);
  expect(r.droppedCharacters).toBe(false);
});

// ---------- naming ----------

test('output names are timestamped, slugged and collision-resistant', () => {
  const a = outputName('A Very Long Prompt About Bicycles!! ' + 'x'.repeat(80), 'png');
  expect(a).toMatch(/^\d{8}-\d{6}-a-very-long-prompt-about-bicycles-x+-[0-9a-f]{4}\.png$/);
  expect(a.length).toBeLessThan(90);
  expect(outputName('***', 'mp4')).toMatch(/-gen-[0-9a-f]{4}\.mp4$/);
  expect(outputName('same', 'png')).not.toBe(outputName('same', 'png'));
});

test('an unknown route is a plain 404', async () => {
  expect((await get('/nope')).status).toBe(404);
});

// ---------- settings / config ----------

// A server of its own, so the key-writing spy and the fixture's shared state
// never interfere. Returns the base URL and a log of what was written.
async function withConfigServer(run, { saveKey = true } = {}) {
  const written = [];
  const s = createServer({
    root,
    apis: [
      {
        id: 'fake',
        name: 'Fake',
        imagesUrl: 'http://127.0.0.1:1/x',
        keyEnv: 'JI_TEST_FAKE_KEY',
        keyUrl: 'https://example.com/keys',
        models: [{ id: 'gpt-image-1' }]
      },
      { id: 'openrouter', name: 'OpenRouter', chatUrl: 'http://127.0.0.1:1/x', video: true, models: [{ id: 'a' }, { id: 'b' }] }
    ],
    videoModels: [],
    // Mirrors the real keyResolver: the config first, then the provider's
    // environment variable — which is what makes `source: 'env'` reachable.
    resolveKey: (id) => keys[id] || (id === 'fake' ? process.env.JI_TEST_FAKE_KEY || '' : ''),
    defaultApi: 'fake',
    charactersRoot: charRoot,
    configPath: '/tmp/ji-test-config.json',
    saveKey: saveKey
      ? (id, key) => {
          written.push([id, key]);
          keys[id] = key;
        }
      : null
  });
  const port = await listenOnFreePort(s, 0);
  try {
    await run({ url: `http://127.0.0.1:${port}`, written });
  } finally {
    s.jobs.closeAll();
    await shutdown(s);
  }
}

test('the settings panel is told which providers are ready, and never the keys', async () => {
  keys = { fake: 'sk-fake-abcdefghijklmnop' };
  await withConfigServer(async ({ url }) => {
    const cfg = await (await fetch(url + '/api/config')).json();
    expect(cfg.configPath).toBe('/tmp/ji-test-config.json');
    expect(cfg.writable).toBe(true);

    const fake = cfg.apis.find((a) => a.id === 'fake');
    expect(fake.hasKey).toBe(true);
    expect(fake.source).toBe('config');
    expect(fake.keyUrl).toBe('https://example.com/keys');
    expect(fake.models).toBe(1);
    // Masked: enough to recognise which key is saved, never enough to use it.
    expect(fake.keyPreview).toBe('sk-f…mnop');
    expect(JSON.stringify(cfg)).not.toContain('abcdefghijklmnop');

    const or = cfg.apis.find((a) => a.id === 'openrouter');
    expect(or.hasKey).toBe(false);
    expect(or.keyPreview).toBe('');
    expect(or.video).toBe(true);
  });
});

test('a key set in the environment is reported as the shell to change, not ours', async () => {
  process.env.JI_TEST_FAKE_KEY = 'sk-from-the-environment';
  keys = {};
  try {
    await withConfigServer(async ({ url }) => {
      const cfg = await (await fetch(url + '/api/config')).json();
      const fake = cfg.apis.find((a) => a.id === 'fake');
      expect(fake.hasKey).toBe(true);
      expect(fake.source).toBe('env');
      expect(fake.keyEnv).toBe('JI_TEST_FAKE_KEY');
    });
  } finally {
    delete process.env.JI_TEST_FAKE_KEY;
  }
});

test('saving a key writes it, and an empty one clears it', async () => {
  keys = {};
  await withConfigServer(async ({ url, written }) => {
    const save = async (body) =>
      (
        await fetch(url + '/api/config/key', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify(body)
        })
      ).json();

    const ok = await save({ id: 'fake', key: 'sk-brand-new-key-1234' });
    expect(ok.hasKey).toBe(true);
    expect(ok.keyPreview).toBe('sk-b…1234');
    expect(written[0]).toEqual(['fake', 'sk-brand-new-key-1234']);

    // A key pasted with quotes or whitespace around it still lands clean.
    await save({ id: 'fake', key: '  "sk-quoted-key-98765"  ' });
    expect(written[1]).toEqual(['fake', 'sk-quoted-key-98765']);

    // Empty means remove.
    const cleared = await save({ id: 'fake', key: '' });
    expect(cleared.hasKey).toBe(false);
    expect(written[2]).toEqual(['fake', '']);

    // An unknown provider and an absurd key are both refused.
    const bad = await fetch(url + '/api/config/key', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ id: 'nope', key: 'x' })
    });
    expect(bad.status).toBe(400);
    const huge = await fetch(url + '/api/config/key', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ id: 'fake', key: 'x'.repeat(600) })
    });
    expect(huge.status).toBe(400);
    expect(written.length).toBe(3);
  });
});

test('a gallery that cannot write the config says so instead of failing on save', async () => {
  keys = {};
  await withConfigServer(
    async ({ url }) => {
      const cfg = await (await fetch(url + '/api/config')).json();
      expect(cfg.writable).toBe(false);
      const r = await fetch(url + '/api/config/key', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ id: 'fake', key: 'sk-nope' })
      });
      expect(r.status).toBe(400);
    },
    { saveKey: false }
  );
});

// The server listens on loopback, but any page in any tab can still POST to
// 127.0.0.1. Without this guard a visited website could spend the user's
// credits or overwrite a saved key.
test('a write from another website is refused; reads and our own writes are not', async () => {
  keys = { fake: 'sk-fake-abcdefghijklmnop' };
  await withConfigServer(async ({ url, written }) => {
    const attempt = (headers) =>
      fetch(url + '/api/config/key', {
        method: 'POST',
        headers: { 'content-type': 'application/json', ...headers },
        body: JSON.stringify({ id: 'fake', key: 'sk-attacker-key-0000' })
      });

    expect((await attempt({ origin: 'https://evil.example.com' })).status).toBe(403);
    expect((await attempt({ 'sec-fetch-site': 'cross-site' })).status).toBe(403);
    expect((await attempt({ 'sec-fetch-site': 'same-site' })).status).toBe(403);
    expect(written.length).toBe(0);

    // Reading is harmless and stays allowed.
    expect((await fetch(url + '/api/config', { headers: { origin: 'https://evil.example.com' } })).status).toBe(200);

    // The gallery's own fetch carries same-origin and goes through.
    expect((await attempt({ 'sec-fetch-site': 'same-origin' })).status).toBe(200);
    expect(written.length).toBe(1);
  });
});

test('a provider model borrows the catalogue facts and blurb for the same model', async () => {
  const s = createServer({
    root,
    apis: [
      {
        id: 'openrouter',
        name: 'OpenRouter',
        video: true,
        models: [
          {
            id: 'google/gemini-3.1-flash-image',
            name: 'Google: Gemini 3.1 Flash Image',
            via: 'chat',
            created: 1770000000,
            pricing: { perImage: 0.067, imageOutput: 120 },
            quality: { winRate: 65.1, rank: 2, arena: 'image' },
            description: 'Nano Banana 2 is fast. It edits too.'
          }
        ]
      },
      // The same model under an aggregator's shorter id, with nothing known.
      { id: 'yunwu', name: 'Yunwu', models: [{ id: 'gemini-3.1-flash-image', name: 'Gemini 3.1 Flash Image' }] }
    ],
    videoModels: [
      {
        id: 'google/veo-3.1',
        name: 'Veo 3.1',
        durations: [4, 8],
        audio: true,
        created: 1774224000,
        pricing: { perSecond: 0.2 },
        description: 'Veo 3.1 makes clips with sound.'
      }
    ],
    resolveKey: () => 'k',
    defaultApi: 'openrouter',
    charactersRoot: charRoot
  });
  const port = await listenOnFreePort(s, 0);
  try {
    const state = await (await fetch(`http://127.0.0.1:${port}/api/state`)).json();

    const yunwu = state.apis.find((a) => a.id === 'yunwu').models[0];
    expect(yunwu.facts.cost.label).toBe('≈6.7¢');
    expect(yunwu.facts.quality.label).toBe('#2');
    expect(yunwu.detail.blurb).toBe('Nano Banana 2 is fast. It edits too.');
    expect(yunwu.detail.borrowedFrom).toBe('google/gemini-3.1-flash-image');
    // The borrowed price is attributed, not passed off as this vendor's own.
    expect(yunwu.facts.cost.title).toContain('may charge differently');
    // Routing is the provider's own — `via` must not have come across.
    expect(yunwu.via).toBeUndefined();

    // Video models carry the publisher's blurb and capability chips.
    const veo = state.videoModels[0];
    expect(veo.detail.blurb).toBe('Veo 3.1 makes clips with sound.');
    expect(veo.detail.chips.map((c) => c.label)).toEqual(['to 8s', 'audio']);
    expect(veo.facts.cost.label).toBe('20¢/s');
  } finally {
    s.jobs.closeAll();
    await shutdown(s);
  }
});

// ---------- batch generation, polling, waiting ----------
//
// The routes a script drives: submit many specs at once, then block until they
// have landed. Everything here runs against the fake upstream, so a whole batch
// is a handful of milliseconds.

test('one batch queues many specs, with defaults merged under each', async () => {
  const r = await post('/api/batch', {
    defaults: { kind: 'image', folder: 'Set', api: 'fake', model: 'gpt-image-1' },
    items: [
      'a cat on a wall',
      { prompt: 'a dog in a field', count: 2 },
      { prompt: 'a slow pan over rooftops', kind: 'video', model: 'google/veo-3.1' }
    ]
  });
  expect(r.status).toBe(200);
  expect(r.body).toMatchObject({ count: 4, images: 3, videos: 1 });
  expect(r.body.jobs).toHaveLength(4);

  const listed = await getJson(`/api/jobs?ids=${r.body.jobs.join(',')}`);
  expect(listed.results.map((j) => j.prompt)).toEqual([
    'a cat on a wall',
    'a dog in a field',
    'a dog in a field',
    'a slow pan over rooftops'
  ]);
  // The default folder reached every item; the video item kept its own kind.
  expect(new Set(listed.results.map((j) => j.folder))).toEqual(new Set(['Set']));
  expect(listed.results[3].kind).toBe('video');
});

test('a batch with `wait` comes back with the finished items', async () => {
  const r = await post('/api/batch', {
    defaults: { folder: 'Wait', api: 'fake', model: 'gpt-image-1' },
    items: ['first picture', 'second picture'],
    wait: 20
  });
  expect(r.body.settled).toBe(true);
  expect(r.body.items).toHaveLength(2);
  expect(r.body.failed).toEqual([]);
  expect(r.body.pending).toEqual([]);
  expect(r.body.items.every((i) => i.folder === 'Wait' && i.file.endsWith('.png'))).toBe(true);
  expect(listItems(root, 'Wait')).toHaveLength(2);
});

test('a failing generation is reported per item rather than failing the batch', async () => {
  upstreamFail = true;
  const r = await post('/api/batch', {
    defaults: { api: 'fake', model: 'gpt-image-1' },
    items: ['doomed one', 'doomed two'],
    wait: 20
  });
  expect(r.body.items).toEqual([]);
  expect(r.body.failed).toHaveLength(2);
  expect(r.body.failed[0].error).toMatch(/out of credit/);
  expect(r.body.failed[0].prompt).toBe('doomed one');
});

test('a batch is validated whole, before anything is queued', async () => {
  const empty = await post('/api/batch', { items: [] });
  expect(empty.status).toBe(400);
  expect(empty.body.error).toMatch(/one entry per generation/);

  const bad = await post('/api/batch', { items: [{ prompt: 'fine' }, { prompt: '  ' }] });
  expect(bad.status).toBe(400);
  expect(bad.body.error).toBe('items[1]: Prompt is required.');
  // Nothing from that batch ran — the first item was never queued.
  expect((await getJson('/api/jobs')).results).toEqual([]);

  const tooMany = await post('/api/batch', { items: Array.from({ length: 51 }, () => 'x') });
  expect(tooMany.status).toBe(400);
  expect(tooMany.body.error).toMatch(/At most 50 items/);

  const tooBig = await post('/api/batch', { items: Array.from({ length: 10 }, () => ({ prompt: 'x', count: 12 })) });
  expect(tooBig.status).toBe(400);
  expect(tooBig.body.error).toMatch(/120 generations; the limit is 100/);
});

test('a bare array of prompts is a batch too', async () => {
  expect((await post('/api/batch', ['one', 'two'])).body.count).toBe(2);
});

test('generate can block on its own jobs and hand back the item', async () => {
  const r = await post('/api/generate', {
    folder: 'Solo',
    api: 'fake',
    model: 'gpt-image-1',
    prompt: 'one good picture',
    wait: 20
  });
  expect(r.body.jobs).toHaveLength(1);
  expect(r.body.settled).toBe(true);
  expect(r.body.items[0]).toMatchObject({ folder: 'Solo', kind: 'image', prompt: 'one good picture' });
});

test('polling reports finished jobs long after the event stream has moved on', async () => {
  const r = await post('/api/generate', { api: 'fake', model: 'gpt-image-1', prompt: 'kept around', wait: 20 });
  const id = r.body.jobs[0];

  const one = await getJson(`/api/jobs?ids=${id}`);
  expect(one.results[0]).toMatchObject({ id, status: 'done' });
  expect(one.results[0].finishedAt).toBeGreaterThan(0);
  expect(one.missing).toBeUndefined();

  // With no ids the whole retained registry comes back, plus the queue limits.
  const all = await getJson('/api/jobs');
  expect(all.results.map((j) => j.id)).toContain(id);
  expect(all.active).toBe(0);
  expect(all.limits).toEqual({ image: 8, video: 3 });
  expect(all.retainMs).toBeGreaterThan(60_000);

  // An id the registry no longer holds is named rather than silently dropped.
  const gone = await getJson('/api/jobs?ids=nope-nope');
  expect(gone.missing).toEqual(['nope-nope']);
  expect(gone.results).toEqual([]);
  expect(gone.settled).toBe(true);
});

test('a wait that runs out reports what is still pending instead of hanging', async () => {
  // The upstream never answers this prompt, so the job cannot finish.
  const r = await post('/api/generate', { api: 'fake', model: 'gpt-image-1', prompt: HANG, wait: 1 });
  expect(r.body.settled).toBe(false);
  expect(r.body.pending).toEqual(r.body.jobs);
  expect(r.body.items).toEqual([]);
  await post('/api/cancel', { all: true });
});

test('cancel takes one id, a list, or everything in flight', async () => {
  const queued = await post('/api/batch', {
    defaults: { api: 'fake', model: 'gpt-image-1' },
    items: [{ prompt: HANG }, { prompt: `${HANG} two` }, { prompt: `${HANG} three` }]
  });
  const [a, b, c] = queued.body.jobs;

  expect((await post('/api/cancel', { id: a })).body).toMatchObject({ ok: true, cancelled: [a] });
  expect((await post('/api/cancel', { ids: [b, 'not-a-job'] })).body.cancelled).toEqual([b]);

  expect((await post('/api/cancel', { all: true })).body.cancelled).toEqual([c]);
  expect((await getJson('/api/jobs?active=1')).results).toEqual([]);
  // Cancelling nothing is not an error; it just cancels nothing.
  expect((await post('/api/cancel', { all: true })).body).toMatchObject({ ok: false, cancelled: [] });
});

test('the model catalogue is served on its own, filterable', async () => {
  const all = await getJson('/api/models');
  expect(all.videoApi).toBe('openrouter');
  expect(all.defaultApi).toBe('fake');
  expect(all.models.map((m) => m.id)).toEqual(['gpt-image-1', 'google/veo-3.1']);
  expect(all.models[0]).toMatchObject({ kind: 'image', api: 'fake', ready: true });
  // No openrouter key in this fixture, so video is listed but not ready.
  expect(all.models[1]).toMatchObject({ kind: 'video', api: 'openrouter', ready: false, audio: true });
  expect(all.models[1].durations).toEqual([4, 6, 8]);
  // The heavy fields stay out until they are asked for.
  expect(all.models[1].facts).toBeUndefined();

  expect((await getJson('/api/models?kind=video')).models.map((m) => m.id)).toEqual(['google/veo-3.1']);
  expect((await getJson('/api/models?kind=image&api=fake')).models).toHaveLength(1);
  expect((await getJson('/api/models?q=veo')).models.map((m) => m.id)).toEqual(['google/veo-3.1']);
  expect((await getJson('/api/models?q=nothing-matches')).total).toBe(0);
  expect((await getJson('/api/models?kind=video&detail=1')).models[0].facts.cost.label).toBe('20¢/s');
});

test('a reference image can be attached from a path on disk', async () => {
  const src = path.join(root, 'from-disk.png');
  fs.writeFileSync(src, Buffer.concat([PNG, Buffer.from('#disk')]));

  const saved = await post('/api/context', { path: src });
  expect(saved.status).toBe(200);
  expect(saved.body.file).toMatch(/^[0-9a-f]{16}\.png$/);
  expect(saved.body.existed).toBe(false);
  expect(saved.body.path).toBe(src);
  expect(fs.existsSync(path.join(root, '.context', saved.body.file))).toBe(true);

  // The same bytes are stored once, whichever way they arrive.
  expect((await post('/api/context', { path: src })).body).toMatchObject({ file: saved.body.file, existed: true });

  // And it is usable as a reference straight away.
  const used = await post('/api/generate', {
    api: 'fake',
    model: 'gpt-image-1',
    prompt: 'in this style',
    images: [saved.body.file],
    wait: 20
  });
  expect(used.body.items[0].images).toEqual([saved.body.file]);
});

test('a path that is not a readable image is refused, and says which path', async () => {
  const missing = await post('/api/context', { path: path.join(root, 'nope.png') });
  expect(missing.status).toBe(400);
  expect(missing.body.error).toMatch(/Cannot read .*nope\.png/);

  fs.writeFileSync(path.join(root, 'notes.txt'), 'hello');
  const wrong = await post('/api/context', { path: path.join(root, 'notes.txt') });
  expect(wrong.status).toBe(400);
  expect(wrong.body.error).toMatch(/Not an image file/);

  const neither = await post('/api/context', {});
  expect(neither.status).toBe(400);
  expect(neither.body.error).toMatch(/dataUrl.*path/);
});

test('a generation can name reference images by path, registering them on the way', async () => {
  const a = path.join(root, 'ref-a.png');
  fs.writeFileSync(a, Buffer.concat([PNG, Buffer.from('#a')]));

  const r = await post('/api/generate', {
    api: 'fake',
    model: 'gpt-image-1',
    prompt: 'like this one',
    images: [a],
    wait: 20
  });
  // The path became a content-hash name in the gallery's own reference library.
  const saved = r.body.items[0].images[0];
  expect(saved).toMatch(/^[0-9a-f]{16}\.png$/);
  expect(fs.existsSync(path.join(root, '.context', saved))).toBe(true);

  // A plain name still means a reference that is already registered, and both
  // forms can be mixed in one call.
  const mixed = await post('/api/generate', {
    api: 'fake',
    model: 'gpt-image-1',
    prompt: 'and both of these',
    images: [saved, a],
    wait: 20
  });
  // The same bytes twice are the same reference, so the pair collapses to one.
  expect(mixed.body.items[0].images).toEqual([saved, saved]);

  const missing = await post('/api/generate', { api: 'fake', model: 'gpt-image-1', prompt: 'x', images: ['/nope/gone.png'] });
  expect(missing.status).toBe(400);
  expect(missing.body.error).toMatch(/Cannot read reference/);
});

test('a character reference can come from a path on disk too', async () => {
  const photo = path.join(root, 'nora.png');
  fs.writeFileSync(photo, Buffer.concat([PNG, Buffer.from('#nora')]));
  await post('/api/characters', { name: 'Nora', description: 'freckles' });

  const added = await post('/api/characters/refs', { id: 'nora', path: photo });
  expect(added.status).toBe(200);
  expect(added.body.character.refs).toHaveLength(1);

  const wrong = await post('/api/characters/refs', { id: 'nora', path: path.join(root, 'no-such.png') });
  expect(wrong.status).toBe(400);
  expect(wrong.body.error).toMatch(/Cannot read/);
});

test('the leaderboard ranks both media on one scale, and says when it cannot', async () => {
  const s = createServer({
    root,
    apis: [
      {
        id: 'yunwu',
        name: 'Yunwu',
        models: [{ id: 'dall-e-3', name: 'DALL·E 3' }, { id: 'nothing-ranks-this', name: 'Obscure' }]
      }
    ],
    videoModels: [
      { id: 'google/veo-3.1', name: 'Veo 3.1', created: 1774224000, pricing: { perSecond: 0.2 } },
      { id: 'runway/gen-4.5', name: 'Gen-4.5', created: 1774224000, pricing: { perSecond: 0.12 } }
    ],
    // The board as loadDesignArena returns it: best-first, per category.
    designArena: {
      image: [
        { id: 'gpt-image-2', category: 'image', rank: 1, winRate: 72.1, elo: 1381, battles: 55968 },
        { id: 'dalle-3', category: 'image', rank: 2, winRate: 38.3, elo: 1086, battles: 134905 }
      ],
      video: [{ id: 'veo-3.1', category: 'video', rank: 1, winRate: 59.7, elo: 1186, battles: 18082 }]
    },
    resolveKey: () => 'k',
    defaultApi: 'yunwu',
    charactersRoot: charRoot
  });
  const port = await listenOnFreePort(s, 0);
  try {
    const state = await (await fetch(`http://127.0.0.1:${port}/api/state`)).json();

    // An image model OpenRouter does not list at all is ranked from the board.
    const dalle = state.apis[0].models.find((m) => m.id === 'dall-e-3');
    expect(dalle.facts.quality).toMatchObject({ label: '#2', rating: 1 });
    expect(dalle.facts.quality.title).toContain('rank 2 of 2');
    expect(dalle.facts.quality.title).toContain('134,905 head-to-head votes');

    // Video used to have no quality at all; now it is on the same scale.
    const veo = state.videoModels.find((m) => m.id === 'google/veo-3.1');
    expect(veo.facts.quality).toMatchObject({ label: '#1', rating: 4 });
    expect(veo.facts.quality.title).toContain('video leaderboard');

    // And a model nobody has voted on says so rather than showing a bare dash.
    const gen45 = state.videoModels.find((m) => m.id === 'runway/gen-4.5');
    expect(gen45.facts.quality).toBeNull();
    expect(gen45.detail.qualityNote).toContain('has not ranked');
    expect(state.apis[0].models.find((m) => m.id === 'nothing-ranks-this').facts.quality).toBeNull();
  } finally {
    s.jobs.closeAll();
    await shutdown(s);
  }
});

test('with no leaderboard the gallery still serves models, just unranked', async () => {
  const s = createServer({
    root,
    apis: [{ id: 'yunwu', name: 'Yunwu', models: [{ id: 'dall-e-3' }] }],
    videoModels: [{ id: 'google/veo-3.1', pricing: { perSecond: 0.2 } }],
    designArena: null,
    resolveKey: () => 'k',
    defaultApi: 'yunwu',
    charactersRoot: charRoot
  });
  const port = await listenOnFreePort(s, 0);
  try {
    const state = await (await fetch(`http://127.0.0.1:${port}/api/state`)).json();
    expect(state.apis[0].models[0].facts.quality).toBeNull();
    expect(state.videoModels[0].facts.quality).toBeNull();
    // The rest of the row is unaffected — the ranking is additive.
    expect(state.videoModels[0].facts.cost.label).toBe('20¢/s');
  } finally {
    s.jobs.closeAll();
    await shutdown(s);
  }
});
