import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { setKey, CONFIG_PATH } from './config.js';
import { select, promptHidden, isInteractive } from './ui.js';
import { rememberSelection, lastModelFor } from './state.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const UI_HTML = path.join(__dirname, 'imagegen.html');

// The provider-menu entry that routes into this flow (mode 'image' is handled
// in cli.js before any claude launching logic).
export const IMAGE_PROVIDER = { id: 'image-gen', name: '🎨 Image Gen', mode: 'image', models: [] };

// Image APIs. All are OpenAI-compatible `/images/generations` endpoints; keys are
// shared with the chat providers of the same id (so the saved yunwu key just works).
// Users can add/extend via `imageApis` in ~/.bro/config.json (merged by id).
export const IMAGE_APIS = [
  {
    id: 'yunwu',
    name: 'Yunwu (云雾)',
    imagesUrl: 'https://yunwu.ai/v1/images/generations',
    keyEnv: 'YUNWU_API_KEY',
    keyUrl: 'https://yunwu.ai',
    models: [
      { id: 'gpt-image-2', name: 'GPT Image 2' },
      { id: 'gpt-image-1', name: 'GPT Image 1' },
      { id: 'gemini-3.1-flash-image', name: 'Gemini 3.1 Flash Image', via: 'chat' },
      { id: 'dall-e-3', name: 'DALL·E 3' }
    ]
  },
  {
    id: 'openai',
    name: 'OpenAI',
    imagesUrl: 'https://api.openai.com/v1/images/generations',
    keyEnv: 'OPENAI_API_KEY',
    keyUrl: 'https://platform.openai.com/api-keys',
    models: [
      { id: 'gpt-image-1', name: 'GPT Image 1' },
      { id: 'dall-e-3', name: 'DALL·E 3' }
    ]
  }
];

function mergeImageApis(configApis = []) {
  const apis = IMAGE_APIS.map((a) => ({ ...a, models: [...a.models] }));
  const byId = new Map(apis.map((a) => [a.id, a]));
  for (const c of configApis) {
    if (!c || !c.id) continue;
    const existing = byId.get(c.id);
    if (existing) {
      for (const f of ['imagesUrl', 'keyEnv', 'keyUrl', 'name']) if (c[f] != null) existing[f] = c[f];
      for (const m of c.models || []) existing.models.push(m);
    } else {
      const np = { ...c, models: [...(c.models || [])] };
      apis.push(np);
      byId.set(np.id, np);
    }
  }
  return apis;
}

// ---------- help ----------

// Built from the live API list so it never drifts from what actually runs
// (including any extra APIs added via `imageApis` in ~/.bro/config.json).
export function imageHelp(config = {}) {
  const apis = mergeImageApis(config.imageApis);
  const width = Math.max(...apis.map((a) => a.id.length), 8);
  const apiLines = apis
    .map((a) => {
      const models = (a.models || []).map((m) => m.id).join(', ') || '(no models listed)';
      return `  ${a.id.padEnd(width)}  ${a.name || a.id}\n  ${' '.repeat(width)}  \x1b[2m${models}\x1b[0m`;
    })
    .join('\n');

  return `bro image — generate images from a local web UI.

Usage:
  bro image              Pick an image API, then open the web UI
  bro image -p <api>     Skip the API menu (e.g. bro image -p yunwu)
  bro image help         Show this help

How it works:
  Pick an API (or pass -p). bro starts a small local server and opens
  it in your browser. Type a prompt, optionally drop in reference
  images, and generated images stream into the gallery. Everything
  runs on your machine — nothing is uploaded except to the image API.

Image APIs:
${apiLines}

  Keys are shared with the chat provider of the same id, so a saved
  yunwu key just works. Add more APIs via the "imageApis" array in
  your config file.

Files (relative to the current directory):
  Output:   ./.bro/image-gen   generated images + history.jsonl
  Context:  ./.bro/context     reference images you upload

Ctrl-C stops the server.

Config:  ${CONFIG_PATH}`;
}

// ---------- generation ----------

const EXT_BY_TYPE = { 'image/png': 'png', 'image/jpeg': 'jpg', 'image/webp': 'webp', 'image/gif': 'gif' };
const TYPE_BY_EXT = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', webp: 'image/webp', gif: 'image/gif' };

function slug(text) {
  return (
    String(text)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 48) || 'image'
  );
}

function stamp() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}

// Some image models (Gemini and friends) aren't on /images/generations — the
// aggregator serves them through /chat/completions and the image comes back
// embedded in the assistant message (a data: URL, an images[] array, or a
// markdown link). Models tagged { via: 'chat' } take that path; unknown/custom
// model ids fall back to a name heuristic.
function chatUrlOf(api) {
  return api.chatUrl || api.imagesUrl.replace(/\/images\/generations\/?$/, '/chat/completions');
}

// Reference images go to the edits endpoint (multipart) instead of generations.
function editsUrlOf(api) {
  return api.editsUrl || api.imagesUrl.replace(/\/generations\/?$/, '/edits');
}

function usesChatApi(api, model) {
  const known = (api.models || []).find((m) => m.id === model);
  if (known) return known.via === 'chat';
  return /gemini|flash-image|banana/i.test(model);
}

async function readApiResponse(res) {
  const text = await res.text();
  if (!res.ok) {
    let msg = text.slice(0, 500);
    try {
      msg = JSON.parse(text).error?.message || msg;
    } catch {
      /* keep raw */
    }
    throw new Error(`${res.status} ${msg}`);
  }
  return JSON.parse(text);
}

async function postJson(url, apiKey, body, signal) {
  const res = await fetch(url, {
    method: 'POST',
    signal,
    headers: { 'content-type': 'application/json', authorization: `Bearer ${apiKey}` },
    body: JSON.stringify(body)
  });
  return readApiResponse(res);
}

// multipart POST — fetch sets the boundary header from the FormData itself.
async function postForm(url, apiKey, form, signal) {
  const res = await fetch(url, {
    method: 'POST',
    signal,
    headers: { authorization: `Bearer ${apiKey}` },
    body: form
  });
  return readApiResponse(res);
}

async function download(url, signal) {
  const img = await fetch(url, { signal });
  if (!img.ok) throw new Error(`Image download failed: HTTP ${img.status}`);
  const ext = EXT_BY_TYPE[(img.headers.get('content-type') || '').split(';')[0]] || 'png';
  return { buf: Buffer.from(await img.arrayBuffer()), ext };
}

function decodeDataUrl(url) {
  const m = url.match(/^data:(image\/[a-z+]+);base64,(.+)$/is);
  if (!m) return null;
  return { buf: Buffer.from(m[2], 'base64'), ext: EXT_BY_TYPE[m[1].toLowerCase()] || 'png' };
}

async function imageFromData(data, signal) {
  if (!data) throw new Error('Empty response (no data[0])');
  if (data.b64_json) {
    return { buf: Buffer.from(data.b64_json, 'base64'), ext: 'png', revisedPrompt: data.revised_prompt };
  }
  if (data.url) {
    return { ...(await download(data.url, signal)), revisedPrompt: data.revised_prompt };
  }
  throw new Error('Response had neither b64_json nor url');
}

// Call the upstream images API for a single image. Concurrency comes from the
// browser firing many of these requests at once, so n is always 1 here.
// `images` are decoded reference images ({ buf, ext, type, dataUrl }) — with any
// present, images-API models go through /images/edits and chat models get the
// references embedded as image_url content parts.
async function generateOne({ api, apiKey, prompt, model, size, quality, images = [] }) {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), 300000);
  try {
    if (usesChatApi(api, model)) {
      // size/quality knobs don't exist on the chat path — steer with the prompt instead.
      const userContent = images.length
        ? [{ type: 'text', text: prompt }, ...images.map((im) => ({ type: 'image_url', image_url: { url: im.dataUrl } }))]
        : prompt;
      const json = await postJson(
        chatUrlOf(api),
        apiKey,
        { model, messages: [{ role: 'user', content: userContent }] },
        ctrl.signal
      );
      const msg = json.choices?.[0]?.message || {};
      const fromImages = msg.images?.[0]?.image_url?.url || msg.images?.[0]?.url;
      const content = typeof msg.content === 'string' ? msg.content : '';
      const dataUrl = fromImages || content.match(/data:image\/[a-z+]+;base64,[A-Za-z0-9+/=]+/i)?.[0];
      if (dataUrl) {
        const decoded = decodeDataUrl(dataUrl);
        if (decoded) return decoded;
        return await download(dataUrl, ctrl.signal);
      }
      const httpUrl = content.match(/!\[[^\]]*\]\((https?:\/\/[^)\s]+)\)/)?.[1];
      if (httpUrl) return await download(httpUrl, ctrl.signal);
      throw new Error('Model replied without an image: ' + (content.slice(0, 200) || JSON.stringify(json).slice(0, 200)));
    }

    if (images.length) {
      const form = new FormData();
      form.append('model', model);
      form.append('prompt', prompt);
      if (size && size !== 'auto') form.append('size', size);
      if (quality && quality !== 'auto') form.append('quality', quality);
      // gpt-image models take multiple references via image[]; single stays `image`
      // for compatibility with stricter backends.
      const field = images.length > 1 ? 'image[]' : 'image';
      images.forEach((im, i) => form.append(field, new Blob([im.buf], { type: im.type }), `ref-${i}.${im.ext}`));
      const json = await postForm(editsUrlOf(api), apiKey, form, ctrl.signal);
      return await imageFromData(json.data?.[0], ctrl.signal);
    }

    const body = { model, prompt, n: 1 };
    if (size && size !== 'auto') body.size = size;
    if (quality && quality !== 'auto') body.quality = quality;
    const json = await postJson(api.imagesUrl, apiKey, body, ctrl.signal);
    return await imageFromData(json.data?.[0], ctrl.signal);
  } finally {
    clearTimeout(timer);
  }
}

// ---------- history (jsonl in the output dir, so it travels with the images) ----------

function historyPath(outDir) {
  return path.join(outDir, 'history.jsonl');
}

function appendHistory(outDir, entry) {
  try {
    fs.appendFileSync(historyPath(outDir), JSON.stringify(entry) + '\n');
  } catch {
    /* best-effort */
  }
}

function readHistory(outDir, limit = 300) {
  try {
    const lines = fs.readFileSync(historyPath(outDir), 'utf8').trim().split('\n');
    const entries = [];
    for (const line of lines.slice(-limit)) {
      try {
        const e = JSON.parse(line);
        if (e.file && fs.existsSync(path.join(outDir, e.file))) entries.push(e);
      } catch {
        /* skip bad line */
      }
    }
    return entries;
  } catch {
    return [];
  }
}

// ---------- context library (.bro/context — reference images, deduped by hash) ----------

function readContext(ctxDir, limit = 500) {
  try {
    return fs
      .readdirSync(ctxDir)
      .filter((f) => TYPE_BY_EXT[path.extname(f).slice(1).toLowerCase()])
      .map((f) => ({ file: f, ts: fs.statSync(path.join(ctxDir, f)).mtimeMs }))
      .sort((a, b) => b.ts - a.ts)
      .slice(0, limit);
  } catch {
    return [];
  }
}

// Resolve context file names into buffers + data URLs for the upstream call.
function resolveRefs(ctxDir, images) {
  const refs = [];
  for (const f of (Array.isArray(images) ? images : []).slice(0, 8)) {
    const name = path.basename(String(f || ''));
    const full = path.join(ctxDir, name);
    if (!name || !fs.existsSync(full)) continue;
    const buf = fs.readFileSync(full);
    const type = TYPE_BY_EXT[path.extname(name).slice(1).toLowerCase()] || 'image/png';
    refs.push({ file: name, buf, type, ext: EXT_BY_TYPE[type] || 'png', dataUrl: `data:${type};base64,${buf.toString('base64')}` });
  }
  return refs;
}

// ---------- server ----------

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

function sendJson(res, code, obj) {
  const body = JSON.stringify(obj);
  res.writeHead(code, { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store' });
  res.end(body);
}

function createServer({ api, apiKey, outDir, ctxDir }) {
  return http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://localhost');
    try {
      if (req.method === 'GET' && url.pathname === '/') {
        res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
        res.end(fs.readFileSync(UI_HTML, 'utf8'));
        return;
      }

      if (req.method === 'GET' && url.pathname === '/api/state') {
        sendJson(res, 200, {
          api: { id: api.id, name: api.name },
          models: api.models,
          defaultModel: api.models[0]?.id || '',
          outDir,
          history: readHistory(outDir),
          context: readContext(ctxDir)
        });
        return;
      }

      // Save a reference image into the context library. The file name is the
      // sha256 of the bytes, so re-uploading the same image is a no-op.
      if (req.method === 'POST' && url.pathname === '/api/context') {
        const { dataUrl } = JSON.parse(await readBody(req, 64 * 1024 * 1024));
        const m = String(dataUrl || '').match(/^data:(image\/[a-z0-9+.-]+);base64,(.+)$/is);
        if (!m) return sendJson(res, 400, { error: 'Expected a base64 image data URL.' });
        const type = m[1].toLowerCase();
        const buf = Buffer.from(m[2], 'base64');
        const hash = crypto.createHash('sha256').update(buf).digest('hex').slice(0, 16);
        const file = `${hash}.${EXT_BY_TYPE[type] || 'png'}`;
        const full = path.join(ctxDir, file);
        const existed = fs.existsSync(full);
        if (!existed) {
          fs.mkdirSync(ctxDir, { recursive: true });
          fs.writeFileSync(full, buf);
        }
        sendJson(res, 200, { file, existed });
        return;
      }

      if (req.method === 'POST' && url.pathname === '/api/context/delete') {
        const { file } = JSON.parse(await readBody(req));
        const name = path.basename(String(file || ''));
        const full = path.join(ctxDir, name);
        if (name && fs.existsSync(full)) fs.unlinkSync(full);
        sendJson(res, 200, { ok: true });
        return;
      }

      if (req.method === 'POST' && url.pathname === '/api/generate') {
        // `images` is a list of context-library file names (uploaded via /api/context).
        // `batch` groups the entries of one Generate click so the UI can tab them.
        const { prompt, model, size, quality, images, batch } = JSON.parse(await readBody(req));
        if (!prompt || !String(prompt).trim()) return sendJson(res, 400, { error: 'Prompt is required.' });
        const useModel = (model || api.models[0]?.id || '').trim();
        if (!useModel) return sendJson(res, 400, { error: 'Model is required.' });

        const refs = resolveRefs(ctxDir, images);
        const started = Date.now();
        const { buf, ext, revisedPrompt } = await generateOne({ api, apiKey, prompt, model: useModel, size, quality, images: refs });
        const file = `${stamp()}-${slug(prompt)}-${Math.random().toString(36).slice(2, 6)}.${ext}`;
        fs.mkdirSync(outDir, { recursive: true });
        fs.writeFileSync(path.join(outDir, file), buf);

        const entry = {
          file,
          batch: batch ? String(batch).slice(0, 64) : undefined,
          prompt: String(prompt),
          revisedPrompt: revisedPrompt || undefined,
          model: useModel,
          size: size || 'auto',
          quality: quality || 'auto',
          images: refs.length ? refs.map((r) => r.file) : undefined,
          refs: refs.length || undefined,
          ms: Date.now() - started,
          ts: Date.now()
        };
        appendHistory(outDir, entry);
        sendJson(res, 200, entry);
        return;
      }

      // Wipe every generated image + the history file. Context refs are untouched.
      if (req.method === 'POST' && url.pathname === '/api/delete-all') {
        let deleted = 0;
        for (const f of fs.readdirSync(outDir)) {
          if (!TYPE_BY_EXT[path.extname(f).slice(1).toLowerCase()]) continue;
          try {
            fs.unlinkSync(path.join(outDir, f));
            deleted++;
          } catch {
            /* keep going */
          }
        }
        try {
          fs.unlinkSync(historyPath(outDir));
        } catch {
          /* may not exist */
        }
        sendJson(res, 200, { ok: true, deleted });
        return;
      }

      if (req.method === 'POST' && url.pathname === '/api/delete') {
        const { file } = JSON.parse(await readBody(req));
        const name = path.basename(String(file || ''));
        const full = path.join(outDir, name);
        if (name && fs.existsSync(full)) fs.unlinkSync(full);
        sendJson(res, 200, { ok: true });
        return;
      }

      if (req.method === 'GET' && (url.pathname.startsWith('/images/') || url.pathname.startsWith('/context/'))) {
        const dir = url.pathname.startsWith('/context/') ? ctxDir : outDir;
        const name = path.basename(decodeURIComponent(url.pathname.split('/').slice(2).join('/')));
        const full = path.join(dir, name);
        if (!fs.existsSync(full)) {
          res.writeHead(404);
          res.end('Not found');
          return;
        }
        const type = TYPE_BY_EXT[path.extname(name).slice(1).toLowerCase()] || 'application/octet-stream';
        res.writeHead(200, { 'content-type': type, 'cache-control': 'public, max-age=31536000, immutable' });
        fs.createReadStream(full).pipe(res);
        return;
      }

      res.writeHead(404);
      res.end('Not found');
    } catch (e) {
      sendJson(res, 500, { error: e?.message || String(e) });
    }
  });
}

function listenOnFreePort(server, start = 8790, tries = 20) {
  return new Promise((resolve, reject) => {
    const attempt = (port, left) => {
      const onError = (err) => {
        server.removeListener('listening', onListening);
        if (err.code === 'EADDRINUSE' && left > 0) attempt(port + 1, left - 1);
        else reject(err);
      };
      const onListening = () => {
        server.removeListener('error', onError);
        resolve(port);
      };
      server.once('error', onError);
      server.once('listening', onListening);
      server.listen(port, '127.0.0.1');
    };
    attempt(start, tries);
  });
}

function openBrowser(url) {
  try {
    if (process.platform === 'win32') {
      spawn(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', `start "" "${url}"`], {
        windowsVerbatimArguments: true,
        detached: true,
        stdio: 'ignore'
      }).unref();
    } else if (process.platform === 'darwin') {
      spawn('open', [url], { detached: true, stdio: 'ignore' }).unref();
    } else {
      spawn('xdg-open', [url], { detached: true, stdio: 'ignore' }).unref();
    }
  } catch {
    /* the printed URL is enough */
  }
}

// ---------- entry point ----------

// Pick an image API, make sure we have its key, then serve the web UI and stay up
// until Ctrl-C. Images land in <cwd>/.bro/image-gen.
export async function runImageGen({ config, apiId, dryRun = false }) {
  const apis = mergeImageApis(config.imageApis);

  let api;
  if (apiId) {
    api = apis.find((a) => a.id === apiId || (a.name || '').toLowerCase() === apiId.toLowerCase());
    if (!api) {
      console.error(`Unknown image API: ${apiId}  (available: ${apis.map((a) => a.id).join(', ')})`);
      return 1;
    }
  } else if (apis.length === 1 || !isInteractive) {
    api = apis[0];
  } else {
    const lastApi = lastModelFor(IMAGE_PROVIDER.id);
    const width = Math.max(...apis.map((a) => (a.name || a.id).length));
    const choice = await select({
      message: 'Choose an image API:',
      startIndex: Math.max(0, apis.findIndex((a) => a.id === lastApi)),
      choices: apis.map((a) => ({
        label: `${(a.name || a.id).padEnd(width)}  \x1b[2m${a.models[0]?.id || ''}\x1b[0m`,
        value: a
      }))
    }).catch(() => null);
    if (!choice) {
      console.log('Cancelled.');
      return 0;
    }
    api = choice.value;
  }

  const outDir = path.join(process.cwd(), '.bro', 'image-gen');
  const ctxDir = path.join(process.cwd(), '.bro', 'context');

  if (dryRun) {
    console.log(JSON.stringify({ via: 'image-gen web ui', api: api.id, imagesUrl: api.imagesUrl, outDir, ctxDir }, null, 2));
    return 0;
  }

  // Key: shared with the chat provider of the same id, so a saved yunwu key is reused.
  let apiKey = (config.keys && config.keys[api.id]) || (api.keyEnv && process.env[api.keyEnv]) || '';
  if (!apiKey) {
    const hint = api.keyUrl ? `  \x1b[2m(get one: ${api.keyUrl})\x1b[0m` : '';
    apiKey = await promptHidden(`Enter API key for ${api.name || api.id}${hint}\n> `).catch(() => '');
    if (!apiKey) {
      console.error('No key entered.');
      return 1;
    }
    setKey(api.id, apiKey);
    console.log(`Saved to ${CONFIG_PATH}`);
  }

  rememberSelection(IMAGE_PROVIDER.id, api.id);
  fs.mkdirSync(outDir, { recursive: true });
  fs.mkdirSync(ctxDir, { recursive: true });

  const server = createServer({ api, apiKey, outDir, ctxDir });
  const port = await listenOnFreePort(server);
  const url = `http://127.0.0.1:${port}`;

  console.log(`\n\x1b[1m🎨 bro image gen\x1b[0m — ${api.name || api.id}`);
  console.log(`   UI:      \x1b[36m${url}\x1b[0m`);
  console.log(`   Output:  ${outDir}`);
  console.log(`   Context: ${ctxDir}`);
  console.log(`   \x1b[2mCtrl-C to stop\x1b[0m\n`);
  openBrowser(url);

  return new Promise((resolve) => {
    const stop = () => {
      console.log('\nStopping image gen server…');
      server.close(() => resolve(0));
      // Don't let lingering keep-alive sockets hold the process open.
      setTimeout(() => resolve(0), 1500).unref();
    };
    process.on('SIGINT', stop);
    process.on('SIGTERM', stop);
  });
}
