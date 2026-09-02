import { afterAll, beforeAll, expect, test } from 'bun:test';
import http from 'node:http';
import {
  buildVideoBody,
  cleanEnhanced,
  enhancePrompt,
  enhanceSystemPrompt,
  generateImage,
  lastCompleteSentence,
  generateVideo,
  mergeImageApis,
  pollDelay,
  usesChatApi
} from './justimagine-gen.js';

// A stand-in upstream: it speaks the OpenAI images/chat shapes and OpenRouter's
// asynchronous video shape, and records what it was asked for.
let server;
let base;
const seen = [];
let videoPolls = 0;
let videoStatus = 'completed';

const PNG = Buffer.from('89504e470d0a1a0a', 'hex');

beforeAll(async () => {
  server = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const raw = Buffer.concat(chunks);
    const url = new URL(req.url, base);
    const json = (code, obj) => {
      res.writeHead(code, { 'content-type': 'application/json' });
      res.end(JSON.stringify(obj));
    };
    seen.push({ path: url.pathname, method: req.method, auth: req.headers.authorization, type: req.headers['content-type'], raw });

    if (url.pathname === '/v1/images/generations') return json(200, { data: [{ b64_json: PNG.toString('base64'), revised_prompt: 'tidied' }] });
    if (url.pathname === '/v1/images/edits') return json(200, { data: [{ url: `${base}/file.png` }] });
    if (url.pathname === '/v1/chat/completions') {
      const body = JSON.parse(raw.toString());
      if (body.model === 'markdown-model') {
        return json(200, { choices: [{ message: { content: `here you go ![img](${base}/file.png)` } }] });
      }
      if (body.model === 'talky-model') return json(200, { choices: [{ message: { content: 'I cannot do that' } }] });
      // the prompt enhancer asks the same endpoint for plain text back
      if (body.messages?.[0]?.role === 'system') return json(200, { choices: [{ message: { content: '  "a much better cat"  ' } }] });
      return json(200, { choices: [{ message: { images: [{ image_url: { url: 'data:image/png;base64,' + PNG.toString('base64') } }] } }] });
    }
    if (url.pathname === '/file.png') {
      res.writeHead(200, { 'content-type': 'image/png' });
      return res.end(PNG);
    }
    if (url.pathname === '/v1/videos' && req.method === 'POST') {
      videoPolls = 0;
      return json(200, { id: 'job-1', status: 'pending', polling_url: `${base}/v1/videos/job-1` });
    }
    if (url.pathname === '/v1/videos/job-1') {
      videoPolls++;
      if (videoPolls < 2) return json(200, { id: 'job-1', status: 'in_progress', polling_url: `${base}/v1/videos/job-1` });
      if (videoStatus !== 'completed') return json(200, { id: 'job-1', status: videoStatus, error: 'model said no' });
      return json(200, {
        id: 'job-1',
        status: 'completed',
        generation_id: 'gen-9',
        unsigned_urls: [`${base}/out.mp4`],
        usage: { cost: 0.42 }
      });
    }
    if (url.pathname === '/out.mp4') {
      res.writeHead(200, { 'content-type': 'video/mp4' });
      return res.end(Buffer.from('mp4-bytes'));
    }
    if (url.pathname === '/v1/images/broken') return json(429, { error: { message: 'slow down' } });
    res.writeHead(404);
    res.end('nope');
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  base = `http://127.0.0.1:${server.address().port}`;
});
afterAll(() => server.close());

const imagesApi = () => ({ id: 'fake', name: 'Fake', imagesUrl: `${base}/v1/images/generations`, models: [{ id: 'gpt-image-1' }] });
const chatApi = () => ({ id: 'fakechat', name: 'FakeChat', chatUrl: `${base}/v1/chat/completions`, chatBody: { modalities: ['image', 'text'] }, models: [] });

// ---------- api catalogue ----------

test('mergeImageApis extends an existing api and adds new ones', () => {
  const merged = mergeImageApis([
    { id: 'openai', name: 'My OpenAI', models: [{ id: 'extra' }] },
    { id: 'custom', name: 'Custom', imagesUrl: 'https://x/v1/images/generations', models: [{ id: 'c1' }] }
  ]);
  const openai = merged.find((a) => a.id === 'openai');
  expect(openai.name).toBe('My OpenAI');
  expect(openai.models.map((m) => m.id)).toContain('extra');
  expect(openai.models.map((m) => m.id)).toContain('gpt-image-1');
  expect(merged.find((a) => a.id === 'custom').models[0].id).toBe('c1');
  // the built-in list is never mutated by a merge
  expect(mergeImageApis().find((a) => a.id === 'openai').name).toBe('OpenAI');
});

test('openrouter is the api that carries video', () => {
  const or = mergeImageApis().find((a) => a.id === 'openrouter');
  expect(or.video).toBe(true);
  expect(mergeImageApis().find((a) => a.id === 'openai').video).toBeUndefined();
});

test('chat routing is by declaration, then by name, and always for chat-only apis', () => {
  const api = { id: 'x', imagesUrl: 'https://x/v1/images/generations', models: [{ id: 'a', via: 'chat' }, { id: 'b' }] };
  expect(usesChatApi(api, 'a')).toBe(true);
  expect(usesChatApi(api, 'b')).toBe(false);
  expect(usesChatApi(api, 'gemini-3.1-flash-image')).toBe(true);
  expect(usesChatApi(api, 'dall-e-3')).toBe(false);
  expect(usesChatApi({ id: 'y', chatUrl: 'https://y' }, 'anything')).toBe(true);
});

// ---------- images ----------

test('an images API returns base64 and the revised prompt', async () => {
  const r = await generateImage({ api: imagesApi(), apiKey: 'k', prompt: 'a cat', model: 'gpt-image-1', size: '1024x1024', quality: 'high' });
  expect(r.ext).toBe('png');
  expect(r.buf.equals(PNG)).toBe(true);
  expect(r.revisedPrompt).toBe('tidied');
  const call = seen.findLast((s) => s.path === '/v1/images/generations');
  expect(JSON.parse(call.raw.toString())).toEqual({ model: 'gpt-image-1', prompt: 'a cat', n: 1, size: '1024x1024', quality: 'high' });
  expect(call.auth).toBe('Bearer k');
});

test("'auto' size and quality are left off the request entirely", async () => {
  await generateImage({ api: imagesApi(), apiKey: 'k', prompt: 'p', model: 'gpt-image-1', size: 'auto', quality: 'auto' });
  const body = JSON.parse(seen.findLast((s) => s.path === '/v1/images/generations').raw.toString());
  expect(body.size).toBeUndefined();
  expect(body.quality).toBeUndefined();
});

test('reference images route an images API through /images/edits as multipart', async () => {
  const refs = [
    { file: 'a.png', buf: PNG, type: 'image/png', ext: 'png', dataUrl: 'data:image/png;base64,' + PNG.toString('base64') },
    { file: 'b.png', buf: PNG, type: 'image/png', ext: 'png', dataUrl: 'data:image/png;base64,' + PNG.toString('base64') }
  ];
  const r = await generateImage({ api: imagesApi(), apiKey: 'k', prompt: 'edit it', model: 'gpt-image-1', refs });
  expect(r.buf.equals(PNG)).toBe(true);
  const call = seen.findLast((s) => s.path === '/v1/images/edits');
  expect(call.type).toContain('multipart/form-data');
  // more than one reference uses the array field name
  expect(call.raw.toString('latin1')).toContain('name="image[]"');
});

test('a chat-routed model returns the image embedded in the reply', async () => {
  const r = await generateImage({ api: chatApi(), apiKey: 'k', prompt: 'a dog', model: 'gemini-x' });
  expect(r.buf.equals(PNG)).toBe(true);
  const body = JSON.parse(seen.findLast((s) => s.path === '/v1/chat/completions').raw.toString());
  expect(body.modalities).toEqual(['image', 'text']);
  expect(body.messages[0].content).toBe('a dog');
});

test('chat references become image_url content parts', async () => {
  const refs = [{ file: 'a.png', buf: PNG, type: 'image/png', ext: 'png', dataUrl: 'data:image/png;base64,zz' }];
  await generateImage({ api: chatApi(), apiKey: 'k', prompt: 'with ref', model: 'gemini-x', refs });
  const body = JSON.parse(seen.findLast((s) => s.path === '/v1/chat/completions').raw.toString());
  expect(body.messages[0].content[0]).toEqual({ type: 'text', text: 'with ref' });
  expect(body.messages[0].content[1].image_url.url).toBe('data:image/png;base64,zz');
});

test('a markdown image link in the reply is downloaded', async () => {
  const r = await generateImage({ api: chatApi(), apiKey: 'k', prompt: 'p', model: 'markdown-model' });
  expect(r.buf.equals(PNG)).toBe(true);
});

test('a model that replies with only words fails with what it said', async () => {
  await expect(generateImage({ api: chatApi(), apiKey: 'k', prompt: 'p', model: 'talky-model' })).rejects.toThrow(/I cannot do that/);
});

test('an upstream error surfaces its own message', async () => {
  const api = { id: 'z', imagesUrl: `${base}/v1/images/broken`, models: [] };
  await expect(generateImage({ api, apiKey: 'k', prompt: 'p', model: 'dall-e-3' })).rejects.toThrow(/429 slow down/);
});

// ---------- prompt enhancement ----------

test('the enhancer is told which model and kind it is writing for', () => {
  const img = enhanceSystemPrompt({ kind: 'image', model: 'google/gemini-3-pro-image' });
  expect(img).toContain('text-to-image model "google/gemini-3-pro-image"');
  expect(img).toContain('composition and framing');
  expect(img).toContain('Return ONLY the rewritten prompt');
  expect(img).not.toContain('camera');

  const vid = enhanceSystemPrompt({ kind: 'video', model: 'google/veo-3.1', spec: { durations: [4, 6, 8], audio: true } });
  expect(vid).toContain('text-to-video model "google/veo-3.1"');
  expect(vid).toContain('Name the camera move');
  expect(vid).toContain('4–8 seconds');
});

test('a silent model is told not to describe sound', () => {
  expect(enhanceSystemPrompt({ kind: 'video', model: 'm', spec: { audio: false } })).toContain('no audio');
  expect(enhanceSystemPrompt({ kind: 'video', model: 'm', spec: { audio: true } })).not.toContain('no audio');
});

test('picked characters are protected from being re-described', () => {
  const one = enhanceSystemPrompt({ kind: 'image', model: 'm', characters: [{ name: 'Nora' }] });
  expect(one).toContain('Nora is a saved character');
  expect(one).toContain('do NOT invent or restate');

  const two = enhanceSystemPrompt({ kind: 'image', model: 'm', characters: [{ name: 'Nora' }, { name: 'Kai' }] });
  expect(two).toContain('Nora, Kai are saved characters');

  expect(enhanceSystemPrompt({ kind: 'image', model: 'm' })).not.toContain('saved character');
});

test('attached references are mentioned so the rewrite does not restate them', () => {
  expect(enhanceSystemPrompt({ kind: 'image', model: 'm', refs: 1 })).toContain('1 reference image will be attached');
  expect(enhanceSystemPrompt({ kind: 'image', model: 'm', refs: 3 })).toContain('3 reference images will be attached');
  expect(enhanceSystemPrompt({ kind: 'image', model: 'm', refs: 0 })).not.toContain('reference image');
});

// A character description is reused in every prompt they appear in, so it is a
// different job from writing one scene.
test('a character description is asked for the permanent look only', () => {
  const c = enhanceSystemPrompt({ kind: 'character' });
  expect(c).toContain('character description');
  expect(c).toContain('reused in every prompt');
  expect(c).toContain('apparent age, build, face, hair, skin');
  expect(c).toContain('No pose, no location, no lighting');
  expect(c).toContain('Do not give them a name');
  expect(c).toContain('Return ONLY the rewritten prompt');
  // none of the scene-prompt guidance leaks in ("no camera" is an exclusion here,
  // not the video instruction to describe one)
  expect(c).not.toContain('text-to-image');
  expect(c).not.toContain('Name the camera move');
  expect(c).not.toContain('composition and framing');
});

test('a wrapped or prefixed answer is unwrapped to the bare prompt', () => {
  expect(cleanEnhanced('  a quiet street  ')).toBe('a quiet street');
  expect(cleanEnhanced('```\na quiet street\n```')).toBe('a quiet street');
  expect(cleanEnhanced('```text\na quiet street\n```')).toBe('a quiet street');
  expect(cleanEnhanced('Here is the rewritten prompt: a quiet street')).toBe('a quiet street');
  expect(cleanEnhanced('Prompt: a quiet street')).toBe('a quiet street');
  expect(cleanEnhanced('"a quiet street"')).toBe('a quiet street');
  expect(cleanEnhanced('“a quiet street”')).toBe('a quiet street');
  // a model that offered alternatives despite being told not to
  expect(cleanEnhanced('1. a quiet street at dawn\n2. a busy street at noon')).toBe('a quiet street at dawn');
  // a legitimate quote inside the prompt is left alone
  expect(cleanEnhanced('a sign reading "OPEN" above a door')).toBe('a sign reading "OPEN" above a door');
  expect(cleanEnhanced('')).toBe('');
  expect(cleanEnhanced(null)).toBe('');
});

test('enhancePrompt sends the composed instruction and returns the cleaned text', async () => {
  const prev = process.env.JUSTIMAGINE_CHAT_URL;
  process.env.JUSTIMAGINE_CHAT_URL = `${base}/v1/chat/completions`;
  try {
    const mod = await import(`./justimagine-gen.js?enhance=${Math.random()}`);
    const r = await mod.enhancePrompt({
      apiKey: 'ek',
      prompt: 'a cat',
      kind: 'image',
      model: 'enhancer-echo',
      characters: [{ name: 'Nora' }]
    });
    expect(r.prompt).toBe('a much better cat');
    expect(r.model).toBe(mod.ENHANCE_MODEL);
    expect(mod.ENHANCE_MODEL).toBe('google/gemini-3.7-flash');

    const sent = JSON.parse(seen.findLast((s) => s.path === '/v1/chat/completions').raw.toString());
    expect(sent.model).toBe('google/gemini-3.7-flash');
    expect(sent.messages[0].role).toBe('system');
    expect(sent.messages[0].content).toContain('Nora is a saved character');
    expect(sent.messages[1]).toEqual({ role: 'user', content: 'a cat' });
    // Room for a reasoning model to think and still finish the sentence.
    expect(sent.max_tokens).toBe(1200);
  } finally {
    if (prev === undefined) delete process.env.JUSTIMAGINE_CHAT_URL;
    else process.env.JUSTIMAGINE_CHAT_URL = prev;
  }
});

// A reasoning model spends most of its budget thinking, so an answer can stop
// mid-clause. Handing that to an image model wastes a generation.
test('a truncated answer is trimmed back to the last complete sentence', () => {
  expect(lastCompleteSentence('A cat on a sill. Sunlight falls across the wo')).toBe('A cat on a sill.');
  expect(lastCompleteSentence('One. Two! Three? And then it stops mid')).toBe('One. Two! Three?');
  // nothing complete enough to salvage
  expect(lastCompleteSentence('A slow macro push in focuses tightly on a windowpane')).toBe('');
  expect(lastCompleteSentence('Hi. and')).toBe('');
  expect(lastCompleteSentence('')).toBe('');
});

test('an empty prompt is refused before any call is made', async () => {
  const calls = seen.length;
  await expect(enhancePrompt({ apiKey: 'k', prompt: '   ' })).rejects.toThrow(/write an idea first/i);
  expect(seen.length).toBe(calls);
});

// ---------- video request shaping ----------

test('buildVideoBody sends only the knobs that were set', () => {
  expect(buildVideoBody({ model: 'google/veo-3.1', prompt: 'a wave' })).toEqual({ model: 'google/veo-3.1', prompt: 'a wave' });
  expect(buildVideoBody({ model: 'm', prompt: 'p', duration: 8, resolution: '1080p', aspectRatio: '16:9', generateAudio: true, seed: 42 })).toEqual({
    model: 'm', prompt: 'p', duration: 8, resolution: '1080p', aspect_ratio: '16:9', generate_audio: true, seed: 42
  });
  expect(buildVideoBody({ model: 'm', prompt: 'p', resolution: 'auto', aspectRatio: 'auto', size: 'auto', seed: '' })).toEqual({ model: 'm', prompt: 'p' });
});

test('an explicit size replaces resolution and aspect ratio rather than contradicting them', () => {
  const body = buildVideoBody({ model: 'm', prompt: 'p', size: '1920x1080', resolution: '720p', aspectRatio: '9:16' });
  expect(body.size).toBe('1920x1080');
  expect(body.resolution).toBeUndefined();
  expect(body.aspect_ratio).toBeUndefined();
});

test('frame images are tagged, and win over plain references', () => {
  const first = { dataUrl: 'data:image/png;base64,AA' };
  const last = { url: 'https://example.com/last.png' };
  const refs = [{ dataUrl: 'data:image/png;base64,BB' }];
  const body = buildVideoBody({ model: 'm', prompt: 'p', firstFrame: first, lastFrame: last, refs });
  expect(body.frame_images).toEqual([
    { type: 'image_url', image_url: { url: 'data:image/png;base64,AA' }, frame_type: 'first_frame' },
    { type: 'image_url', image_url: { url: 'https://example.com/last.png' }, frame_type: 'last_frame' }
  ]);
  expect(body.input_references).toBeUndefined();

  const refsOnly = buildVideoBody({ model: 'm', prompt: 'p', refs });
  expect(refsOnly.input_references).toEqual([{ type: 'image_url', image_url: { url: 'data:image/png;base64,BB' } }]);
  expect(refsOnly.frame_images).toBeUndefined();
});

test('polling ramps up rather than sitting on one long interval', () => {
  expect(pollDelay(0)).toBe(2000);
  expect(pollDelay(5)).toBe(4000);
  expect(pollDelay(10)).toBe(8000);
  expect(pollDelay(50)).toBe(15000);
  for (let i = 0; i < 60; i++) expect(pollDelay(i)).toBeGreaterThanOrEqual(pollDelay(Math.max(0, i - 1)));
});

// ---------- video end to end ----------

// The endpoint is read from the module's VIDEO_BASE, so the fake upstream is
// injected by importing the module fresh with the env var pointed at it.
async function withFakeVideoApi(fn) {
  const prev = process.env.JUSTIMAGINE_VIDEO_URL;
  process.env.JUSTIMAGINE_VIDEO_URL = `${base}/v1/videos`;
  try {
    const mod = await import(`./justimagine-gen.js?video=${Math.random()}`);
    return await fn(mod);
  } finally {
    if (prev === undefined) delete process.env.JUSTIMAGINE_VIDEO_URL;
    else process.env.JUSTIMAGINE_VIDEO_URL = prev;
  }
}

test('a video job is submitted, polled until complete, then downloaded', async () => {
  videoStatus = 'completed';
  await withFakeVideoApi(async (mod) => {
    const phases = [];
    const r = await mod.generateVideo({
      apiKey: 'vk',
      params: { model: 'google/veo-3.1', prompt: 'a wave', duration: 8 },
      sleep: () => Promise.resolve(),
      onProgress: (p) => phases.push(p.phase)
    });
    expect(r.buf.toString()).toBe('mp4-bytes');
    expect(r.ext).toBe('mp4');
    expect(r.cost).toBe(0.42);
    expect(r.generationId).toBe('gen-9');
    expect(phases).toEqual(['submitting', 'queued', 'generating', 'generating', 'downloading']);
    // the key travels with both the poll and the same-origin download
    expect(seen.findLast((s) => s.path === '/v1/videos/job-1').auth).toBe('Bearer vk');
    expect(seen.findLast((s) => s.path === '/out.mp4').auth).toBe('Bearer vk');
  });
});

test('a terminal failure reports the upstream reason', async () => {
  videoStatus = 'failed';
  await withFakeVideoApi(async (mod) => {
    await expect(
      mod.generateVideo({ apiKey: 'vk', params: { model: 'm', prompt: 'p' }, sleep: () => Promise.resolve() })
    ).rejects.toThrow(/model said no/);
  });
  videoStatus = 'completed';
});

test('a job that never finishes gives up instead of polling forever', async () => {
  videoStatus = 'in_progress';
  await withFakeVideoApi(async (mod) => {
    await expect(
      mod.generateVideo({ apiKey: 'vk', params: { model: 'm', prompt: 'p' }, sleep: () => Promise.resolve(), maxMs: -1 })
    ).rejects.toThrow(/timed out/);
  });
  videoStatus = 'completed';
});

test('generateVideo is exported from the module surface', () => {
  expect(typeof generateVideo).toBe('function');
});

test('models the catalogue cannot describe still get a one-line note', () => {
  const apis = mergeImageApis([
    { id: 'mine', name: 'Mine', models: [{ id: 'dall-e-3' }, { id: 'house-model', description: 'Our own thing.' }, { id: 'unknown-model' }] }
  ]);
  const byId = (apiId, modelId) => apis.find((a) => a.id === apiId).models.find((m) => m.id === modelId);

  // The same note reaches every provider serving that model, keyed by
  // normalised id rather than by which shop it came from.
  expect(byId('yunwu', 'dall-e-3').description).toContain('DALL·E 3');
  expect(byId('openai', 'dall-e-3').description).toBe(byId('yunwu', 'dall-e-3').description);
  expect(byId('mine', 'dall-e-3').description).toBe(byId('openai', 'dall-e-3').description);
  expect(byId('yunwu', 'gpt-image-1').description).toContain('Images API');

  // A model that says something for itself keeps its own words.
  expect(byId('mine', 'house-model').description).toBe('Our own thing.');
  // And one nobody has anything for stays silent rather than being invented for.
  expect(byId('mine', 'unknown-model').description).toBeUndefined();

  // Models the live catalogue describes are left alone, so the richer blurb
  // from OpenRouter wins rather than being pre-empted by a note.
  expect(byId('yunwu', 'gemini-3.1-flash-image').description).toBeUndefined();
});
