// Anthropic Messages API (what Claude Code, omp, Pi and DeepSeek Harness speak) served by ChatJimmy.
//
//   POST /v1/messages               streamed (SSE) or not; text + emulated tool_use blocks
//   POST /v1/messages/count_tokens  estimate of the request as sent
//   GET  /v1/models                 the model ids this endpoint answers to
//   HEAD /api/hello, GET /health    liveness
//
// Each request is rebuilt to fit ChatJimmy's window (prompt.js), the reply is scanned for a tool
// call (tools.js), and a call that cannot run — an unknown tool, a missing argument — is sent back
// to the model to fix before Claude Code ever sees it. An empty reply (ChatJimmy's context overflow)
// is retried with a smaller prompt.

import fs from 'node:fs';
import path from 'node:path';
import { randomBytes } from 'node:crypto';
import { jimmyChat, ContextOverflowError, JIMMY_MODEL } from './client.js';
import { buildPrompt, estimateTokens, DEFAULT_PROMPT_BUDGET } from './prompt.js';
import { parseToolCalls, repairToolInput, toolCallText } from './tools.js';
import { applyPolicy, latestRequest, answerProblem } from './policy.js';
import { stepsSummary, directiveFor } from './router.js';

export const JEV_MODEL_SUFFIX = '+jev';
export const MODEL_IDS = [JIMMY_MODEL, JIMMY_MODEL + JEV_MODEL_SUFFIX];

const id = (prefix) => prefix + randomBytes(12).toString('base64url');

const json = (body, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });

const apiError = (status, type, message) => json({ type: 'error', error: { type, message } }, status);

/**
 * @param {object} [o]
 * @param {Function} [o.chat]        jimmyChat-compatible (injectable for tests)
 * @param {string}   [o.token]       when set, requests must carry it (x-api-key or Bearer)
 * @param {number}   [o.budget]      prompt budget in tokens
 * @param {number}   [o.topK]        1 = greedy
 * @param {number}   [o.fixAttempts] how many times a broken tool call is sent back to the model
 * @param {string}   [o.logFile]     one line per request
 * @param {string}   [o.traceDir]    when set, every request/prompt/reply is written there (debugging)
 * @param {Function} [o.router]      Jev step router (router.js) — the llama3.1-8B+jev model
 * @param {number}   [o.routeThreshold] Jev's minimum confidence to overrule ChatJimmy's step
 */
export function createAnthropicHandler({
  chat = jimmyChat,
  token = null,
  budget = DEFAULT_PROMPT_BUDGET,
  topK = 1,
  fixAttempts = 2,
  logFile = null,
  traceDir = null,
  router = null,
  routeThreshold = 0.6
} = {}) {
  let seq = 0;
  // Trace files are prefixed per endpoint start, so successive launches (e.g. --continue) add to a
  // trace folder instead of overwriting each other.
  const runId = new Date().toISOString().replace(/[:.]/g, '-');
  const log = (line) => {
    if (!logFile) return;
    try { fs.appendFileSync(logFile, `${new Date().toISOString()} ${line}\n`); } catch {}
  };
  const trace = (n, name, data) => {
    if (!traceDir) return;
    try {
      fs.mkdirSync(traceDir, { recursive: true });
      fs.writeFileSync(path.join(traceDir, `${runId}-${String(n).padStart(4, '0')}-${name}.json`), JSON.stringify(data, null, 2));
    } catch {}
  };

  // One ChatJimmy call, shrinking the prompt when the reply would not fit.
  async function generate(body, extraTurns = []) {
    let b = budget;
    for (let attempt = 1; ; attempt++) {
      const p = buildPrompt(body, { budget: b });
      const messages = extraTurns.length ? [...p.messages, ...extraTurns] : p.messages;
      try {
        const r = await chat({ system: p.system, messages, topK });
        return { ...r, prompt: { ...p, messages } };
      } catch (err) {
        if (err instanceof ContextOverflowError && attempt < 3) { b = Math.floor(b * 0.6); continue; }
        throw err;
      }
    }
  }

  // Repair a call and run it past the guard rails. → { call, problem, answer, note }
  function vet(raw, { body, cwd, tools, text }) {
    const fixed = repairToolInput(raw.name, raw.input, tools, { cwd });
    if (fixed.problem) return { call: { name: fixed.name, input: fixed.input }, problem: fixed.problem };
    return applyPolicy({ name: fixed.name, input: fixed.input }, { messages: body.messages, cwd, tools, text });
  }

  // A reply may hold several calls. Later ones are kept only when they cannot depend on results the
  // model has not seen: reads/searches, and writing a file that does not exist yet.
  const independent = (c) => ['Read', 'Glob', 'Grep'].includes(c.name) || (c.name === 'Write' && typeof c.input.file_path === 'string' && !fs.existsSync(c.input.file_path));

  async function answer(body, n) {
    // Jev picks the next step in parallel with ChatJimmy's first draft, so agreeing costs nothing.
    const routing = router && body.tools?.length
      ? (async () => {
          const p = buildPrompt(body, { budget });
          if (!p.tools.length) return null;
          return router({ task: latestRequest(body.messages), cwd: p.env.cwd, steps: stepsSummary(body.messages), tools: p.tools.map((t) => t.name) });
        })()
      : null;
    const first = await generate(body);
    const tools = first.prompt.tools;
    const cwd = first.prompt.env.cwd;
    let r = first;
    let parsed = parseToolCalls(r.text, tools);
    let calls = [];
    let text = parsed.text;
    const fixes = [];
    const notes = [];
    const attempts = [r.text];

    const decision = routing ? await routing : null;
    const drafted = parsed.calls[0]?.name ?? (parsed.problem ? null : 'answer');
    // Overruling a finished answer is the risky direction (seen: a correct answer turned into more
    // searching that derailed), so it needs near-certainty.
    const needed = drafted === 'answer' && decision?.choice !== 'answer' ? Math.max(routeThreshold, 0.9) : routeThreshold;
    if (decision && decision.p >= needed) {
      const available = decision.choice === 'answer' || tools.some((t) => t.name === decision.choice);
      if (available && drafted !== decision.choice) {
        r = await generate(body, [{ role: 'assistant', content: r.text || '(empty)' }, { role: 'user', content: directiveFor(decision.choice) }]);
        attempts.push(r.text);
        parsed = parseToolCalls(r.text, tools);
        notes.push(`jev: ${drafted ?? 'invalid'}→${decision.choice} (${decision.p.toFixed(2)})`);
      } else {
        notes.push(`jev agreed: ${decision.choice} (${decision.p.toFixed(2)})`);
      }
    }

    for (let attempt = 0; attempt <= fixAttempts; attempt++) {
      let problem = parsed.problem;
      let shownCall = null;
      calls = [];
      text = parsed.text;
      if (parsed.calls.length) {
        const lead = vet(parsed.calls[0], { body, cwd, tools, text: parsed.text || '' });
        if (lead.note) notes.push(lead.note);
        if (lead.answer && !text) text = lead.answer;
        problem = lead.problem;
        shownCall = lead.call || parsed.calls[0];
        if (!problem && lead.call) {
          calls.push(lead.call);
          for (const more of parsed.calls.slice(1)) {
            const v = vet(more, { body, cwd, tools, text: '' });
            if (v.problem || !v.call || !independent(v.call) || v.call.name !== more.name) break;
            calls.push(v.call);
          }
          if (calls.length < parsed.calls.length) notes.push(`kept ${calls.length} of ${parsed.calls.length} calls`);
        }
      }
      // A final answer has to be grounded in the search it follows.
      if (!problem && !calls.length && text && attempt < fixAttempts) problem = answerProblem(body.messages, text);
      if (!problem && (calls.length || text)) break;
      if (!problem) problem = 'Your reply was empty. Call a tool or answer the user.';
      if (attempt === fixAttempts) break;
      fixes.push(problem);
      // Show the model its own reply and what is wrong with it, then ask again.
      const shown = shownCall ? toolCallText(shownCall.name, shownCall.input) : (r.text || '(empty)');
      r = await generate(body, [{ role: 'assistant', content: shown }, { role: 'user', content: `${problem}` }]);
      attempts.push(r.text);
      parsed = parseToolCalls(r.text, tools);
    }

    const content = [];
    if (text?.trim()) content.push({ type: 'text', text: text.trim() });
    for (const c of calls) content.push({ type: 'tool_use', id: id('toolu_'), name: c.name, input: c.input });
    if (!content.length) content.push({ type: 'text', text: '(ChatJimmy returned no answer.)' });
    const call = calls[0];

    const s = r.stats || {};
    const result = {
      content,
      stop_reason: calls.length ? 'tool_use' : 'end_turn',
      usage: { input_tokens: s.prefill_tokens ?? r.prompt.tokens, output_tokens: s.decode_tokens ?? estimateTokens(r.text) }
    };
    trace(n, 'reply', { prompt: first.prompt, attempts, fixes, notes, result });
    log(`#${n} ${body.model} tools=${tools.length} prompt≈${r.prompt.tokens}t dropped=${r.prompt.dropped} → ${calls.length ? `tool_use ${calls.map((c) => c.name).join("+")}` : "text"} ${fixes.length ? `(fixed ${fixes.length}×)` : ''}${notes.length ? ` [${notes.join('; ')}]` : ''} ${r.latencyMs}ms`);
    return result;
  }

  function sse(model, result) {
    const events = [];
    const emit = (type, data) => events.push(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`);
    emit('message_start', {
      message: { id: id('msg_'), type: 'message', role: 'assistant', model, content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: result.usage.input_tokens, output_tokens: 1 } }
    });
    result.content.forEach((block, index) => {
      if (block.type === 'text') {
        emit('content_block_start', { index, content_block: { type: 'text', text: '' } });
        emit('content_block_delta', { index, delta: { type: 'text_delta', text: block.text } });
      } else {
        emit('content_block_start', { index, content_block: { type: 'tool_use', id: block.id, name: block.name, input: {} } });
        emit('content_block_delta', { index, delta: { type: 'input_json_delta', partial_json: JSON.stringify(block.input) } });
      }
      emit('content_block_stop', { index });
    });
    emit('message_delta', { delta: { stop_reason: result.stop_reason, stop_sequence: null }, usage: { output_tokens: result.usage.output_tokens } });
    emit('message_stop', {});
    return new Response(events.join(''), { headers: { 'content-type': 'text/event-stream; charset=utf-8', 'cache-control': 'no-cache' } });
  }

  return async function handle(req) {
    const url = new URL(req.url);
    const p = url.pathname.replace(/\/+$/, '') || '/';
    if ((req.method === 'HEAD' || req.method === 'GET') && (p === '/' || p === '/api/hello' || p === '/health')) {
      return req.method === 'HEAD' ? new Response(null, { status: 200 }) : json({ status: 'ok', models: MODEL_IDS });
    }
    if (token) {
      const got = req.headers.get('x-api-key') || req.headers.get('authorization')?.replace(/^Bearer\s+/i, '');
      if (got !== token) return apiError(401, 'authentication_error', 'invalid x-api-key');
    }
    if (req.method === 'GET' && p === '/v1/models') {
      return json({ data: MODEL_IDS.map((m) => ({ type: 'model', id: m, display_name: `ChatJimmy ${m}`, created_at: '2026-01-01T00:00:00Z' })), has_more: false, first_id: MODEL_IDS[0], last_id: MODEL_IDS.at(-1) });
    }
    if (req.method !== 'POST' || !p.startsWith('/v1/messages')) return apiError(404, 'not_found_error', `No route for ${req.method} ${p}`);

    let body;
    try { body = await req.json(); } catch { return apiError(400, 'invalid_request_error', 'Request body must be JSON'); }
    if (p === '/v1/messages/count_tokens') {
      return json({ input_tokens: estimateTokens(JSON.stringify({ system: body.system, messages: body.messages, tools: body.tools })) });
    }
    if (p !== '/v1/messages') return apiError(404, 'not_found_error', `No route for POST ${p}`);
    if (!Array.isArray(body.messages) || !body.messages.length) return apiError(400, 'invalid_request_error', 'messages: at least one message is required');

    const n = ++seq;
    trace(n, 'request', body);
    let result;
    try {
      result = await answer(body, n);
    } catch (err) {
      log(`#${n} error: ${err.message}`);
      const status = err instanceof ContextOverflowError ? 400 : err.status && err.status < 500 ? 502 : 529;
      const type = err instanceof ContextOverflowError ? 'invalid_request_error' : status === 529 ? 'overloaded_error' : 'api_error';
      return apiError(status, type, `ChatJimmy: ${err.message}`);
    }
    const model = body.model || JIMMY_MODEL;
    if (body.stream) return sse(model, result);
    return json({ id: id('msg_'), type: 'message', role: 'assistant', model, ...result, stop_sequence: null });
  };
}
