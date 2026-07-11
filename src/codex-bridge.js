// Anthropic ⇄ Codex-backend bridge — lets Claude Code (or anything speaking
// the Anthropic Messages API) run on a ChatGPT subscription.
//
// A tiny local HTTP server exposes:
//   POST /v1/messages               (streaming + non-streaming)
//   POST /v1/messages/count_tokens  (rough estimate)
//   GET  /v1/models                 (the subscription's live Codex model list)
//   GET  /health
//
// Each request is translated to the OpenAI Responses API and sent to
// https://chatgpt.com/backend-api/codex/responses with the ChatGPT OAuth token
// from codex-auth.js, presenting as a Codex client. Verified against the live
// backend: custom `instructions` are accepted as-is, function tools and
// multi-turn tool round-trips work, reasoning items don't need to be echoed
// back, and `max_output_tokens` must be omitted.
//
// Pure Node (node:http + fetch) — no proxy packages, no codex CLI, no Bun.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import crypto from 'node:crypto';
import { BRO_DIR } from './config.js';
import { freshCodexAuth } from './codex-auth.js';

const BACKEND_URL = 'https://chatgpt.com/backend-api/codex/responses';
const MODELS_URL = 'https://chatgpt.com/backend-api/codex/models';
const CLIENT_VERSION = '0.144.1'; // codex client version we present as
export const DEFAULT_PORT = Number.parseInt(process.env.BRO_CODEX_PORT || '', 10) || 3458;

const MODELS_CACHE = path.join(BRO_DIR, 'codex-models.json');
const CODEX_HOME = process.env.CODEX_HOME || path.join(os.homedir(), '.codex');

// Used only when the live fetch and every cache are unavailable.
const FALLBACK_MODELS = [
  { id: 'gpt-5.6-sol', name: 'GPT-5.6-Sol', defaultEffort: 'medium', efforts: ['low', 'medium', 'high', 'xhigh'] },
  { id: 'gpt-5.6-terra', name: 'GPT-5.6-Terra', defaultEffort: 'medium', efforts: ['low', 'medium', 'high'] },
  { id: 'gpt-5.5', name: 'GPT-5.5', defaultEffort: 'medium', efforts: ['low', 'medium', 'high'] },
  { id: 'gpt-5.4-mini', name: 'GPT-5.4-Mini', defaultEffort: 'medium', efforts: ['low', 'medium', 'high'] }
];

function readJson(p) {
  try {
    return JSON.parse(fs.readFileSync(p, 'utf8'));
  } catch {
    return null;
  }
}

function backendHeaders(auth, sessionId) {
  return {
    authorization: `Bearer ${auth.accessToken}`,
    ...(auth.accountId ? { 'chatgpt-account-id': auth.accountId } : {}),
    'openai-beta': 'responses=experimental',
    originator: 'codex_cli_rs',
    'user-agent': `codex_cli_rs/${CLIENT_VERSION}`,
    session_id: sessionId,
    accept: 'text/event-stream',
    'content-type': 'application/json'
  };
}

function mapModelList(list) {
  const models = (list || [])
    .filter((m) => m && m.slug && m.visibility !== 'hide')
    .map((m) => ({
      id: m.slug,
      name: m.display_name || m.slug,
      description: m.description || '',
      defaultEffort: m.default_reasoning_level || 'medium',
      efforts: (m.supported_reasoning_levels || []).map((l) => l.effort).filter(Boolean)
    }));
  return models.length ? models : null;
}

// Live model list for the logged-in subscription. Falls back to bro's cache,
// then the Codex CLI's cache (if that happens to exist), then a static list.
export async function fetchCodexModels() {
  try {
    const auth = await freshCodexAuth();
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), 10000);
    try {
      const res = await fetch(`${MODELS_URL}?client_version=${CLIENT_VERSION}`, {
        signal: ctrl.signal,
        headers: backendHeaders(auth, crypto.randomUUID())
      });
      if (res.ok) {
        const models = mapModelList((await res.json()).models);
        if (models) {
          try {
            fs.mkdirSync(BRO_DIR, { recursive: true });
            fs.writeFileSync(MODELS_CACHE, JSON.stringify(models, null, 2));
          } catch {
            /* cache is best-effort */
          }
          return models;
        }
      }
    } finally {
      clearTimeout(timer);
    }
  } catch {
    /* fall through to caches */
  }
  return (
    readJson(MODELS_CACHE) ||
    mapModelList(readJson(path.join(CODEX_HOME, 'models_cache.json'))?.models) ||
    FALLBACK_MODELS
  );
}

// --- Anthropic request -> Responses API request ------------------------------

function textOf(content) {
  if (typeof content === 'string') return content;
  return (content || [])
    .filter((b) => b && b.type === 'text' && b.text)
    .map((b) => b.text)
    .join('\n');
}

function toInputContent(block) {
  if (block.type === 'text') return { type: 'input_text', text: block.text };
  if (block.type === 'image' && block.source?.type === 'base64') {
    return { type: 'input_image', image_url: `data:${block.source.media_type};base64,${block.source.data}` };
  }
  if (block.type === 'image' && block.source?.type === 'url') {
    return { type: 'input_image', image_url: block.source.url };
  }
  return null;
}

// Flatten Anthropic messages into Responses input items. Thinking blocks are
// dropped (verified: the backend accepts histories without reasoning items),
// tool_use becomes function_call, tool_result becomes function_call_output.
function toResponsesInput(messages) {
  const items = [];
  let pending = null; // accumulating message item

  const flush = () => {
    if (pending && pending.content.length) items.push(pending);
    pending = null;
  };
  const pushContent = (role, part) => {
    if (!pending || pending.role !== role) {
      flush();
      pending = { type: 'message', role, content: [] };
    }
    pending.content.push(part);
  };

  for (const msg of messages || []) {
    const blocks = typeof msg.content === 'string' ? [{ type: 'text', text: msg.content }] : msg.content || [];
    for (const block of blocks) {
      if (msg.role === 'assistant') {
        if (block.type === 'text' && block.text) {
          pushContent('assistant', { type: 'output_text', text: block.text });
        } else if (block.type === 'tool_use') {
          flush();
          items.push({
            type: 'function_call',
            call_id: block.id,
            name: block.name,
            arguments: JSON.stringify(block.input ?? {})
          });
        }
        // thinking / redacted_thinking: dropped
      } else {
        if (block.type === 'tool_result') {
          flush();
          const out = typeof block.content === 'string' ? block.content : textOf(block.content);
          items.push({
            type: 'function_call_output',
            call_id: block.tool_use_id,
            output: (block.is_error ? '[tool error] ' : '') + (out || '(no output)')
          });
        } else {
          const part = toInputContent(block);
          if (part) pushContent('user', part);
        }
      }
    }
    flush();
  }
  flush();
  return items;
}

function toResponsesTools(tools) {
  return (tools || [])
    .filter((t) => t && t.name && (t.input_schema || !t.type || t.type === 'custom'))
    .map((t) => ({
      type: 'function',
      name: t.name,
      description: t.description || '',
      strict: false,
      parameters: t.input_schema || { type: 'object', properties: {} }
    }));
}

function toToolChoice(choice) {
  if (!choice) return 'auto';
  if (choice.type === 'any') return 'required';
  if (choice.type === 'tool') return { type: 'function', name: choice.name };
  return 'auto';
}

const EFFORT_ORDER = ['low', 'medium', 'high', 'xhigh', 'max', 'ultra'];

function effortFor(body, modelInfo, override) {
  let effort = override || modelInfo?.defaultEffort || 'medium';
  const budget = body.thinking?.type === 'enabled' ? body.thinking.budget_tokens : null;
  if (budget != null) {
    effort = budget < 4096 ? 'low' : budget < 16384 ? 'medium' : budget < 32768 ? 'high' : 'xhigh';
  }
  const supported = modelInfo?.efforts?.length ? modelInfo.efforts : EFFORT_ORDER.slice(0, 4);
  if (supported.includes(effort)) return effort;
  // Clamp to the nearest supported level.
  const want = EFFORT_ORDER.indexOf(effort);
  return supported.reduce((best, e) =>
    Math.abs(EFFORT_ORDER.indexOf(e) - want) < Math.abs(EFFORT_ORDER.indexOf(best) - want) ? e : best
  );
}

// --- Responses SSE -> Anthropic events ---------------------------------------

// Parse an upstream SSE stream and invoke onEvent(json) per data payload.
async function readSse(res, onEvent) {
  const reader = res.body.getReader();
  const dec = new TextDecoder();
  let buf = '';
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    let i;
    while ((i = buf.indexOf('\n\n')) >= 0) {
      const chunk = buf.slice(0, i);
      buf = buf.slice(i + 2);
      const data = chunk
        .split('\n')
        .filter((l) => l.startsWith('data:'))
        .map((l) => l.slice(5).trim())
        .join('');
      if (!data || data === '[DONE]') continue;
      try {
        onEvent(JSON.parse(data));
      } catch {
        /* skip malformed frames */
      }
    }
  }
}

// Translate upstream Responses events into Anthropic streaming events via
// emit(type, payload). Returns { stopReason, usage } when the response ends.
function makeTranslator(emit, reqModel) {
  let started = false;
  let index = -1;
  let open = null; // 'text' | 'thinking' | 'tool_use'
  let sawToolUse = false;

  const start = (id) => {
    if (started) return;
    started = true;
    emit('message_start', {
      type: 'message_start',
      message: {
        id: id || `msg_${crypto.randomUUID()}`,
        type: 'message',
        role: 'assistant',
        model: reqModel,
        content: [],
        stop_reason: null,
        stop_sequence: null,
        usage: { input_tokens: 0, output_tokens: 0 }
      }
    });
  };
  const closeBlock = () => {
    if (open == null) return;
    if (open === 'thinking') {
      emit('content_block_delta', { type: 'content_block_delta', index, delta: { type: 'signature_delta', signature: '' } });
    }
    emit('content_block_stop', { type: 'content_block_stop', index });
    open = null;
  };
  const openBlock = (type, block) => {
    if (open === type && type !== 'tool_use') return;
    closeBlock();
    index++;
    open = type;
    emit('content_block_start', { type: 'content_block_start', index, content_block: block });
  };

  return {
    handle(ev) {
      switch (ev.type) {
        case 'response.created':
          start(ev.response?.id);
          break;
        case 'response.output_item.added':
          start();
          if (ev.item?.type === 'function_call') {
            sawToolUse = true;
            openBlock('tool_use', { type: 'tool_use', id: ev.item.call_id, name: ev.item.name, input: {} });
          }
          break;
        case 'response.output_text.delta':
          start();
          if (open !== 'text') openBlock('text', { type: 'text', text: '' });
          emit('content_block_delta', { type: 'content_block_delta', index, delta: { type: 'text_delta', text: ev.delta || '' } });
          break;
        case 'response.reasoning_summary_text.delta':
          start();
          if (open !== 'thinking') openBlock('thinking', { type: 'thinking', thinking: '' });
          emit('content_block_delta', { type: 'content_block_delta', index, delta: { type: 'thinking_delta', thinking: ev.delta || '' } });
          break;
        case 'response.reasoning_summary_part.added':
          if (open === 'thinking') {
            emit('content_block_delta', { type: 'content_block_delta', index, delta: { type: 'thinking_delta', thinking: '\n\n' } });
          }
          break;
        case 'response.function_call_arguments.delta':
          if (open === 'tool_use') {
            emit('content_block_delta', { type: 'content_block_delta', index, delta: { type: 'input_json_delta', partial_json: ev.delta || '' } });
          }
          break;
        case 'response.output_item.done':
          closeBlock();
          break;
        case 'response.completed': {
          start();
          closeBlock();
          const u = ev.response?.usage || {};
          const usage = {
            input_tokens: u.input_tokens || 0,
            output_tokens: u.output_tokens || 0,
            cache_read_input_tokens: u.input_tokens_details?.cached_tokens || 0
          };
          const stopReason = sawToolUse ? 'tool_use' : 'end_turn';
          emit('message_delta', { type: 'message_delta', delta: { stop_reason: stopReason, stop_sequence: null }, usage });
          emit('message_stop', { type: 'message_stop' });
          return { done: true };
        }
        case 'response.failed':
        case 'error': {
          const message = ev.response?.error?.message || ev.error?.message || ev.message || 'upstream response failed';
          if (started) {
            closeBlock();
            emit('error', { type: 'error', error: { type: 'api_error', message } });
          }
          return { done: true, error: message };
        }
      }
      return { done: false };
    },
    get started() {
      return started;
    }
  };
}

// --- request handling ---------------------------------------------------------

function errorBody(type, message) {
  return JSON.stringify({ type: 'error', error: { type, message } });
}

function errorTypeFor(status) {
  if (status === 401 || status === 403) return 'authentication_error';
  if (status === 429) return 'rate_limit_error';
  if (status >= 500) return 'api_error';
  return 'invalid_request_error';
}

async function readBody(req) {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  return Buffer.concat(chunks).toString('utf8');
}

export function startCodexBridge({ port = DEFAULT_PORT, defaultModel = '', models = [], effort = '', quiet = true } = {}) {
  const sessionId = crypto.randomUUID();
  const byId = new Map(models.map((m) => [m.id, m]));
  const smallModel =
    models.find((m) => /mini|spark/i.test(m.id))?.id || defaultModel || models[0]?.id || 'gpt-5.4-mini';

  // Resolve the Anthropic-side model name (may be a claude-* id from Claude
  // Code's background calls) to a codex slug, with an optional ":effort" suffix.
  const resolveModel = (name) => {
    let [slug, effortSuffix] = String(name || '').split(':');
    let forcedEffort = EFFORT_ORDER.includes(effortSuffix) ? effortSuffix : effort || '';
    if (!byId.has(slug)) {
      slug = /haiku|mini|small/i.test(slug) ? smallModel : defaultModel || models[0]?.id || slug;
    }
    return { slug, forcedEffort, info: byId.get(slug) };
  };

  const handleMessages = async (req, res, body) => {
    const wantStream = Boolean(body.stream);
    const { slug, forcedEffort, info } = resolveModel(body.model);
    const upstreamBody = {
      model: slug,
      instructions: textOf(body.system) || 'You are a helpful coding agent.',
      input: toResponsesInput(body.messages),
      tools: toResponsesTools(body.tools),
      tool_choice: toToolChoice(body.tool_choice),
      parallel_tool_calls: true,
      reasoning: { effort: effortFor(body, info, forcedEffort), summary: 'auto' },
      include: ['reasoning.encrypted_content'],
      prompt_cache_key: sessionId,
      store: false,
      stream: true
    };

    const ctrl = new AbortController();
    req.on('close', () => ctrl.abort());

    // One retry after a forced token refresh on 401.
    let upstream;
    for (let attempt = 0; ; attempt++) {
      const auth = await freshCodexAuth({ force: attempt > 0 });
      upstream = await fetch(BACKEND_URL, {
        method: 'POST',
        headers: backendHeaders(auth, sessionId),
        body: JSON.stringify(upstreamBody),
        signal: ctrl.signal
      });
      if (upstream.status !== 401 || attempt > 0) break;
    }

    if (!upstream.ok) {
      const detail = (await upstream.text()).slice(0, 500);
      res.writeHead(upstream.status, { 'content-type': 'application/json' });
      res.end(errorBody(errorTypeFor(upstream.status), `codex backend: ${detail || upstream.statusText}`));
      return;
    }

    if (wantStream) {
      res.writeHead(200, {
        'content-type': 'text/event-stream',
        'cache-control': 'no-cache',
        connection: 'keep-alive'
      });
      const emit = (type, payload) => res.write(`event: ${type}\ndata: ${JSON.stringify(payload)}\n\n`);
      const translator = makeTranslator(emit, body.model);
      let finished = false;
      await readSse(upstream, (ev) => {
        if (finished) return;
        finished = translator.handle(ev).done;
      });
      if (!finished && !translator.started) {
        emit('error', { type: 'error', error: { type: 'api_error', message: 'upstream stream ended unexpectedly' } });
      }
      res.end();
      return;
    }

    // Non-streaming: accumulate the translated stream into one message.
    const content = [];
    let stopReason = 'end_turn';
    let usage = { input_tokens: 0, output_tokens: 0 };
    let msgId = `msg_${crypto.randomUUID()}`;
    let errorMsg = null;
    const emit = (type, payload) => {
      if (type === 'message_start') msgId = payload.message.id;
      else if (type === 'content_block_start') content.push(structuredClone(payload.content_block));
      else if (type === 'content_block_delta') {
        const block = content[content.length - 1];
        const d = payload.delta;
        if (d.type === 'text_delta') block.text += d.text;
        else if (d.type === 'thinking_delta') block.thinking += d.thinking;
        else if (d.type === 'input_json_delta') block._json = (block._json || '') + d.partial_json;
      } else if (type === 'message_delta') {
        stopReason = payload.delta.stop_reason;
        usage = payload.usage;
      } else if (type === 'error') errorMsg = payload.error.message;
    };
    const translator = makeTranslator(emit, body.model);
    let finished = false;
    await readSse(upstream, (ev) => {
      if (finished) return;
      finished = translator.handle(ev).done;
    });
    if (errorMsg || !finished) {
      res.writeHead(502, { 'content-type': 'application/json' });
      res.end(errorBody('api_error', errorMsg || 'upstream stream ended unexpectedly'));
      return;
    }
    for (const block of content) {
      if (block.type === 'tool_use') {
        try {
          block.input = block._json ? JSON.parse(block._json) : {};
        } catch {
          block.input = {};
        }
      }
      delete block._json;
    }
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(
      JSON.stringify({
        id: msgId,
        type: 'message',
        role: 'assistant',
        model: body.model,
        content,
        stop_reason: stopReason,
        stop_sequence: null,
        usage
      })
    );
  };

  const server = http.createServer(async (req, res) => {
    try {
      const url = new URL(req.url, 'http://localhost');
      if (req.method === 'GET' && (url.pathname === '/health' || url.pathname === '/')) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ ok: true, via: 'bro codex bridge', defaultModel }));
        return;
      }
      if (req.method === 'GET' && url.pathname === '/v1/models') {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(
          JSON.stringify({
            data: models.map((m) => ({ type: 'model', id: m.id, display_name: m.name })),
            has_more: false
          })
        );
        return;
      }
      if (req.method === 'POST' && url.pathname === '/v1/messages/count_tokens') {
        const raw = await readBody(req);
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ input_tokens: Math.max(1, Math.ceil(raw.length / 4)) }));
        return;
      }
      if (req.method === 'POST' && url.pathname === '/v1/messages') {
        let body;
        try {
          body = JSON.parse(await readBody(req));
        } catch {
          res.writeHead(400, { 'content-type': 'application/json' });
          res.end(errorBody('invalid_request_error', 'request body was not valid JSON'));
          return;
        }
        await handleMessages(req, res, body);
        return;
      }
      res.writeHead(404, { 'content-type': 'application/json' });
      res.end(errorBody('not_found_error', `no route for ${req.method} ${url.pathname}`));
    } catch (e) {
      if (!quiet) console.error(`bridge error: ${e.message}`);
      if (!res.headersSent) {
        res.writeHead(500, { 'content-type': 'application/json' });
        res.end(errorBody('api_error', e.message));
      } else {
        res.end();
      }
    }
  });

  // Try up to 20 ports so concurrent `bro codex` sessions don't collide.
  return new Promise((resolve, reject) => {
    let attempt = 0;
    const tryListen = (p) => {
      server.once('error', (e) => {
        if (e.code === 'EADDRINUSE' && attempt++ < 20) {
          tryListen(p + 1);
        } else {
          reject(e);
        }
      });
      server.listen(p, '127.0.0.1', () => {
        resolve({
          server,
          port: p,
          baseUrl: `http://127.0.0.1:${p}`,
          close: () =>
            new Promise((r) => {
              server.close(() => r());
              server.closeAllConnections?.();
            })
        });
      });
    };
    tryListen(port);
  });
}
