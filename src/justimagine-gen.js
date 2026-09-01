import { EXT_BY_TYPE } from './justimagine-store.js';

// Upstream calls for both media kinds.
//
//   image  — OpenAI-shaped /images/generations and /images/edits, plus the
//            "chat-routed" models (Gemini and friends) that aggregators serve
//            through /chat/completions with the picture embedded in the reply.
//   video  — OpenRouter's asynchronous /api/v1/videos: submit, poll, download.
//
// Nothing here touches the filesystem; callers hand in decoded reference images
// and get a buffer back.

// ---------- image APIs ----------

// Keys are shared with the chat provider of the same id, so a saved yunwu key
// just works. Users extend this via `imageApis` in ~/.bro/config.json.
export const IMAGE_APIS = [
  {
    id: 'openrouter',
    name: 'OpenRouter',
    // OpenRouter has no /images/generations — image models run through chat
    // completions with `modalities` set. The list is refreshed live from their
    // catalogue on selection; this is just the offline fallback.
    chatUrl: 'https://openrouter.ai/api/v1/chat/completions',
    chatBody: { modalities: ['image', 'text'] },
    keyEnv: 'OPENROUTER_API_KEY',
    keyUrl: 'https://openrouter.ai/keys',
    video: true,
    models: [
      { id: 'google/gemini-3.1-flash-image', name: 'Nano Banana 2 (Gemini 3.1 Flash Image)', via: 'chat' },
      { id: 'google/gemini-3-pro-image', name: 'Nano Banana Pro (Gemini 3 Pro Image)', via: 'chat' },
      { id: 'openai/gpt-5.4-image-2', name: 'GPT-5.4 Image 2', via: 'chat' }
    ]
  },
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

export function mergeImageApis(configApis = []) {
  const apis = IMAGE_APIS.map((a) => ({ ...a, models: [...a.models] }));
  const byId = new Map(apis.map((a) => [a.id, a]));
  for (const c of configApis) {
    if (!c || !c.id) continue;
    const existing = byId.get(c.id);
    if (existing) {
      for (const f of ['imagesUrl', 'chatUrl', 'chatBody', 'editsUrl', 'keyEnv', 'keyUrl', 'name', 'video']) {
        if (c[f] != null) existing[f] = c[f];
      }
      for (const m of c.models || []) existing.models.push(m);
    } else {
      const np = { ...c, models: [...(c.models || [])] };
      apis.push(np);
      byId.set(np.id, np);
    }
  }
  return apis;
}

// ---------- shared http helpers ----------

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
  try {
    return JSON.parse(text);
  } catch {
    throw new Error(`Upstream returned non-JSON: ${text.slice(0, 200)}`);
  }
}

async function postJson(url, apiKey, body, signal) {
  return readApiResponse(
    await fetch(url, {
      method: 'POST',
      signal,
      headers: { 'content-type': 'application/json', authorization: `Bearer ${apiKey}` },
      body: JSON.stringify(body)
    })
  );
}

// multipart POST — fetch sets the boundary header from the FormData itself.
async function postForm(url, apiKey, form, signal) {
  return readApiResponse(await fetch(url, { method: 'POST', signal, headers: { authorization: `Bearer ${apiKey}` }, body: form }));
}

async function download(url, signal, headers = {}) {
  const res = await fetch(url, { signal, headers, redirect: 'follow' });
  if (!res.ok) throw new Error(`Download failed: HTTP ${res.status}`);
  const type = (res.headers.get('content-type') || '').split(';')[0].toLowerCase();
  const fromUrl = (url.split('?')[0].match(/\.([a-z0-9]{2,4})$/i) || [])[1];
  const ext = EXT_BY_TYPE[type] || (fromUrl ? fromUrl.toLowerCase() : '') || 'bin';
  return { buf: Buffer.from(await res.arrayBuffer()), ext };
}

function decodeDataUrl(url) {
  const m = String(url).match(/^data:(image\/[a-z0-9+.-]+);base64,(.+)$/is);
  if (!m) return null;
  return { buf: Buffer.from(m[2], 'base64'), ext: EXT_BY_TYPE[m[1].toLowerCase()] || 'png' };
}

// ---------- images ----------

function chatUrlOf(api) {
  return api.chatUrl || api.imagesUrl.replace(/\/images\/generations\/?$/, '/chat/completions');
}

// Reference images go to the edits endpoint (multipart) instead of generations.
function editsUrlOf(api) {
  return api.editsUrl || api.imagesUrl.replace(/\/generations\/?$/, '/edits');
}

export function usesChatApi(api, model) {
  if (!api.imagesUrl) return true; // chat-only API (e.g. OpenRouter)
  const known = (api.models || []).find((m) => m.id === model);
  if (known) return known.via === 'chat';
  return /gemini|flash-image|banana/i.test(model);
}

async function imageFromData(data, signal) {
  if (!data) throw new Error('Empty response (no data[0])');
  if (data.b64_json) return { buf: Buffer.from(data.b64_json, 'base64'), ext: 'png', revisedPrompt: data.revised_prompt };
  if (data.url) return { ...(await download(data.url, signal)), revisedPrompt: data.revised_prompt };
  throw new Error('Response had neither b64_json nor url');
}

// One image. Concurrency comes from the browser firing several of these at
// once, so n is always 1 here.
export async function generateImage({ api, apiKey, prompt, model, size, quality, refs = [], signal }) {
  if (usesChatApi(api, model)) {
    // size/quality knobs don't exist on the chat path — steer with the prompt.
    const userContent = refs.length
      ? [{ type: 'text', text: prompt }, ...refs.map((im) => ({ type: 'image_url', image_url: { url: im.dataUrl } }))]
      : prompt;
    const json = await postJson(
      chatUrlOf(api),
      apiKey,
      { model, messages: [{ role: 'user', content: userContent }], ...(api.chatBody || {}) },
      signal
    );
    const msg = json.choices?.[0]?.message || {};
    const fromImages = msg.images?.[0]?.image_url?.url || msg.images?.[0]?.url;
    const content = typeof msg.content === 'string' ? msg.content : '';
    const dataUrl = fromImages || content.match(/data:image\/[a-z+]+;base64,[A-Za-z0-9+/=]+/i)?.[0];
    if (dataUrl) return decodeDataUrl(dataUrl) || (await download(dataUrl, signal));
    const httpUrl = content.match(/!\[[^\]]*\]\((https?:\/\/[^)\s]+)\)/)?.[1];
    if (httpUrl) return await download(httpUrl, signal);
    throw new Error('Model replied without an image: ' + (content.slice(0, 200) || JSON.stringify(json).slice(0, 200)));
  }

  if (refs.length) {
    const form = new FormData();
    form.append('model', model);
    form.append('prompt', prompt);
    if (size && size !== 'auto') form.append('size', size);
    if (quality && quality !== 'auto') form.append('quality', quality);
    // gpt-image models take multiple references via image[]; a single one stays
    // `image` for compatibility with stricter backends.
    const field = refs.length > 1 ? 'image[]' : 'image';
    refs.forEach((im, i) => form.append(field, new Blob([im.buf], { type: im.type }), `ref-${i}.${im.ext}`));
    const json = await postForm(editsUrlOf(api), apiKey, form, signal);
    return await imageFromData(json.data?.[0], signal);
  }

  const body = { model, prompt, n: 1 };
  if (size && size !== 'auto') body.size = size;
  if (quality && quality !== 'auto') body.quality = quality;
  const json = await postJson(api.imagesUrl, apiKey, body, signal);
  return await imageFromData(json.data?.[0], signal);
}

// ---------- prompt enhancement ----------

// A one-line idea is rarely what a generation model wants. This rewrites it
// into something specific — subject, framing, light, and for video what
// actually moves — using a fast text model on the same OpenRouter key the rest
// of JustImagine uses.
export const ENHANCE_MODEL = 'google/gemini-3.7-flash';
// Resolved per call rather than at import, so pointing this at a proxy takes
// effect without restarting a long-running service.
export const enhanceUrl = () => process.env.JUSTIMAGINE_CHAT_URL || 'https://openrouter.ai/api/v1/chat/completions';

const RULES = [
  'Return ONLY the rewritten prompt. No preamble, no quotes, no markdown, no explanation, no options.',
  'Keep the user\'s subject and intent exactly. Add specificity; never substitute a different scene.',
  'Write flowing descriptive prose, not a comma-separated keyword dump, and never pad with "8k, masterpiece, trending on artstation" filler.',
  'Do not mention aspect ratio, resolution, duration or file format — those are separate controls.'
];

export function enhanceSystemPrompt({ kind = 'image', model = '', spec = null, characters = [], refs = 0 } = {}) {
  // A character description is not a scene: it is the fixed part of a person
  // that has to read the same in every shot they appear in.
  if (kind === 'character') {
    return [
      'You rewrite a character description for an image and video generation tool.',
      'The description is reused in every prompt this character appears in, so it must cover only what stays the same about them.',
      '',
      'Rules:',
      ...RULES.slice(0, 3).map((r) => `- ${r}`),
      '- Cover apparent age, build, face, hair, skin, and signature clothing — in that order, as one flowing phrase.',
      '- Describe only the permanent look. No pose, no location, no lighting, no action, no camera, no mood: those belong to the individual prompt.',
      '- Do not give them a name; the tool supplies that separately.',
      '- Keep it under about 45 words, lower case, no trailing full stop, so it reads naturally inside a longer sentence.'
    ].join('\n');
  }

  const lines = [
    `You rewrite prompts for the ${kind === 'video' ? 'text-to-video' : 'text-to-image'} model "${model || 'unknown'}".`,
    '',
    'Rules:',
    ...RULES.map((r) => `- ${r}`)
  ];

  if (kind === 'video') {
    lines.push(
      '- Describe one continuous shot: what the camera does, what the subject does, and how the scene changes from start to end.',
      '- Name the camera move explicitly (locked off, slow push in, handheld follow, orbit, crane down).',
      '- Keep it under about 110 words. One paragraph.'
    );
    if (spec?.audio === false) lines.push('- This model produces no audio, so do not describe sound.');
    if (spec && Array.isArray(spec.durations) && spec.durations.length) {
      lines.push(`- The clip is short (${spec.durations[0]}–${spec.durations[spec.durations.length - 1]} seconds), so describe one beat, not a sequence of events.`);
    }
  } else {
    lines.push(
      '- Cover subject, composition and framing, lighting, colour and mood, and material or surface detail.',
      '- Keep it under about 80 words. One paragraph.'
    );
  }

  if (characters.length) {
    const names = characters.map((c) => c.name || c).join(', ');
    lines.push(
      `- ${names} ${characters.length === 1 ? 'is a saved character whose' : 'are saved characters whose'} appearance comes from attached reference images. Keep ${characters.length === 1 ? 'the name' : 'the names'} exactly as written and do NOT invent or restate ${characters.length === 1 ? 'their' : 'their'} face, hair, age or clothing — describe only what they are doing and where.`
    );
  }
  if (refs) {
    lines.push(`- ${refs} reference image${refs === 1 ? '' : 's'} will be attached, so describe the scene rather than restating what the reference already shows.`);
  }
  return lines.join('\n');
}

// Models sometimes wrap the answer despite being told not to; strip the usual
// wrappers rather than handing the user a quoted, prefixed blob.
export function cleanEnhanced(text) {
  let out = String(text || '').trim();
  out = out.replace(/^```[a-z]*\s*\n?([\s\S]*?)\n?```$/i, '$1').trim();
  out = out.replace(/^(?:here(?:'s| is)[^\n:]*:|rewritten prompt:|prompt:|enhanced prompt:)\s*/i, '').trim();
  if (out.length > 1 && /^["'“”](.|\n)*["'“”]$/.test(out)) out = out.slice(1, -1).trim();
  // A model that ignored "one paragraph" and gave alternatives — take the first.
  const numbered = out.match(/^\s*1[.)]\s+([\s\S]*?)(?=\n\s*2[.)]\s)/);
  if (numbered) out = numbered[1].trim();
  return out;
}

// Cut back to the last sentence that actually ends, so a truncated answer
// becomes a shorter usable prompt rather than a dangling clause.
export function lastCompleteSentence(text) {
  const end = Math.max(text.lastIndexOf('.'), text.lastIndexOf('!'), text.lastIndexOf('?'));
  if (end < 0) return '';
  const cut = text.slice(0, end + 1).trim();
  // A couple of surviving words is not a prompt; better to say it failed.
  return cut.length >= 12 ? cut : '';
}

export async function enhancePrompt({ apiKey, prompt, kind = 'image', model, spec, characters = [], refs = 0, enhanceModel = ENHANCE_MODEL, signal }) {
  const text = String(prompt || '').trim();
  if (!text) throw new Error('Nothing to improve yet — write an idea first.');
  const json = await postJson(
    enhanceUrl(),
    apiKey,
    {
      model: enhanceModel,
      messages: [
        { role: 'system', content: enhanceSystemPrompt({ kind, model, spec, characters, refs }) },
        { role: 'user', content: text }
      ],
      // Generous, because a reasoning model spends most of this budget thinking
      // before it writes a word — Gemini 3.7 Flash burns ~350 tokens on a
      // rewrite this small, and a tighter cap cuts the answer off mid-sentence.
      max_tokens: 1200,
      temperature: 0.8
    },
    signal
  );
  const choice = json.choices?.[0];
  const out = cleanEnhanced(choice?.message?.content);
  if (!out) throw new Error('The prompt model returned nothing usable.');
  // A model that ran out of room mid-sentence must not hand back a fragment.
  if (choice?.finish_reason === 'length') {
    const whole = lastCompleteSentence(out);
    if (!whole) throw new Error('The prompt model ran out of room before finishing. Try again, or shorten what you wrote.');
    return { prompt: whole, model: enhanceModel, cost: json.usage?.cost, truncated: true };
  }
  return { prompt: out, model: enhanceModel, cost: json.usage?.cost };
}

// ---------- video (OpenRouter) ----------

export const VIDEO_BASE = process.env.JUSTIMAGINE_VIDEO_URL || 'https://openrouter.ai/api/v1/videos';
export const VIDEO_KEY_API = 'openrouter';
const origin = (url) => new URL(url).origin;

// Frame images name a first/last frame for image-to-video; input_references
// steer style/subject without pinning a frame. OpenRouter documents these as
// directly downloadable URLs, so an https reference passes straight through and
// a local one is inlined as a data URL (which most upstreams accept).
function frameEntry(ref, frameType) {
  const url = ref.url || ref.dataUrl;
  return { type: 'image_url', image_url: { url }, ...(frameType ? { frame_type: frameType } : {}) };
}

export function buildVideoBody({ model, prompt, duration, resolution, aspectRatio, size, generateAudio, seed, firstFrame, lastFrame, refs = [] }) {
  const body = { model, prompt };
  if (duration) body.duration = Number(duration);
  // `size` fully determines resolution + aspect ratio, so sending all three
  // risks a contradiction the upstream would reject.
  if (size && size !== 'auto') body.size = size;
  else {
    if (resolution && resolution !== 'auto') body.resolution = resolution;
    if (aspectRatio && aspectRatio !== 'auto') body.aspect_ratio = aspectRatio;
  }
  if (typeof generateAudio === 'boolean') body.generate_audio = generateAudio;
  if (Number.isFinite(Number(seed)) && String(seed).trim() !== '') body.seed = Number(seed);
  const frames = [];
  if (firstFrame) frames.push(frameEntry(firstFrame, 'first_frame'));
  if (lastFrame) frames.push(frameEntry(lastFrame, 'last_frame'));
  if (frames.length) body.frame_images = frames;
  // frame_images wins upstream when both are present, so only send references
  // when no frame was pinned — otherwise they are silently ignored.
  else if (refs.length) body.input_references = refs.map((r) => frameEntry(r));
  return body;
}

const TERMINAL_FAIL = new Set(['failed', 'cancelled', 'expired']);

// Poll gently but not slowly: video takes tens of seconds to minutes, and a
// fixed 30s tick would add half a minute of dead time to every short clip.
export function pollDelay(attempt) {
  if (attempt < 3) return 2000;
  if (attempt < 8) return 4000;
  if (attempt < 20) return 8000;
  return 15000;
}

export async function generateVideo({ apiKey, params, signal, onProgress = () => {}, sleep = (ms) => new Promise((r) => setTimeout(r, ms)), maxMs = 20 * 60 * 1000 }) {
  const body = buildVideoBody(params);
  const auth = { authorization: `Bearer ${apiKey}` };
  onProgress({ phase: 'submitting' });
  let job = await postJson(VIDEO_BASE, apiKey, body, signal);
  if (!job?.id && !job?.polling_url) throw new Error('Video API did not return a job id.');
  onProgress({ phase: 'queued', jobId: job.id, status: job.status || 'pending' });

  const started = Date.now();
  for (let attempt = 0; !TERMINAL_FAIL.has(job.status) && job.status !== 'completed'; attempt++) {
    if (Date.now() - started > maxMs) throw new Error('Video generation timed out.');
    await sleep(pollDelay(attempt));
    if (signal?.aborted) throw new Error('Cancelled.');
    const pollUrl = new URL(job.polling_url || `${VIDEO_BASE}/${job.id}`, origin(VIDEO_BASE));
    const res = await fetch(pollUrl, { signal, headers: auth });
    job = await readApiResponse(res);
    onProgress({ phase: 'generating', jobId: job.id, status: job.status || 'pending', attempt, elapsed: Date.now() - started });
  }
  if (job.status !== 'completed') throw new Error(job.error || `Video generation ${job.status}.`);

  const url = job.unsigned_urls?.[0] || job.url || job.output?.[0];
  if (!url) throw new Error('Completed job had no video URL.');
  onProgress({ phase: 'downloading', jobId: job.id });
  // Unsigned URLs are served from OpenRouter and need the key; a signed URL
  // elsewhere must not receive it.
  const sameHost = String(url).startsWith(origin(VIDEO_BASE));
  const { buf, ext } = await download(String(url), signal, sameHost ? auth : {});
  return {
    buf,
    ext: ext === 'bin' ? 'mp4' : ext,
    cost: job.usage?.cost ?? undefined,
    generationId: job.generation_id || job.id
  };
}
