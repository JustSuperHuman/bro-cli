// Client for chatjimmy.ai — Taalas' demo of Llama 3.1 8B hard-wired into silicon (~15-20k tok/s).
//
// The web app streams plain text from /api/chat and ends it with a stats sentinel:
//   <|stats|>{"prefill_tokens":…,"decode_tokens":…,"done_reason":"stop",…}<|/stats|>
// Measured limits (Sep 2026): the context is ~6.1k tokens *including* the reply, and a request
// that would overflow it comes back as HTTP 200 with an empty body — no error, no stats. That case
// is surfaced here as a ContextOverflowError so callers can shrink the prompt and retry.

export const JIMMY_BASE = 'https://chatjimmy.ai';
export const JIMMY_MODEL = 'llama3.1-8B';
export const JIMMY_CONTEXT_TOKENS = 6144;

// Headers the page itself sends from Edge; the endpoint needs no cookie or key.
export const BROWSER_HEADERS = {
  accept: '*/*',
  'accept-language': 'en-US,en;q=0.9',
  'content-type': 'application/json',
  origin: JIMMY_BASE,
  referer: `${JIMMY_BASE}/`,
  'sec-ch-ua': '"Microsoft Edge";v="140", "Chromium";v="140", "Not=A?Brand";v="24"',
  'sec-ch-ua-mobile': '?0',
  'sec-ch-ua-platform': '"Windows"',
  'sec-fetch-dest': 'empty',
  'sec-fetch-mode': 'cors',
  'sec-fetch-site': 'same-origin',
  'user-agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0'
};

export class ContextOverflowError extends Error {
  constructor(message = 'ChatJimmy returned an empty reply — the prompt plus the answer exceeded its ~6k-token context') {
    super(message);
    this.name = 'ContextOverflowError';
    this.status = 413;
  }
}

const STATS_OPEN = '<|stats|>';
const STATS_RE = /<\|stats\|>([\s\S]*?)<\|\/stats\|>/;

/**
 * One chat turn.
 * @param {object} o
 * @param {{role:'user'|'assistant', content:string}[]} o.messages
 * @param {string} [o.system]
 * @param {number} [o.topK]   1 = greedy (deterministic); the site default is 8
 * @param {(delta:string)=>void} [o.onText]  visible text as it arrives (the sentinel never leaks)
 * @returns {Promise<{text:string, stats:object|null, latencyMs:number}>}
 */
export async function jimmyChat({ messages, system = '', topK = 1, model = JIMMY_MODEL, onText, signal, fetchImpl = fetch }) {
  const t0 = Date.now();
  const res = await fetchImpl(`${JIMMY_BASE}/api/chat`, {
    method: 'POST',
    headers: BROWSER_HEADERS,
    body: JSON.stringify({ messages, chatOptions: { selectedModel: model, systemPrompt: system, topK }, attachment: null }),
    signal: signal ?? AbortSignal.timeout(120_000)
  });
  if (!res.ok) {
    const err = new Error(`ChatJimmy HTTP ${res.status}: ${(await res.text()).slice(0, 200)}`);
    err.status = res.status;
    throw err;
  }

  const decoder = new TextDecoder();
  let raw = '';
  let emitted = 0;
  let sentinelAt = -1;
  // Emit everything except a tail that might still turn out to be the start of the sentinel.
  const flush = (final) => {
    if (sentinelAt === -1) sentinelAt = raw.indexOf(STATS_OPEN);
    let safeEnd = sentinelAt !== -1 ? sentinelAt : raw.length;
    if (sentinelAt === -1 && !final) {
      for (let k = Math.min(STATS_OPEN.length - 1, raw.length); k > 0; k--) {
        if (STATS_OPEN.startsWith(raw.slice(raw.length - k))) { safeEnd = raw.length - k; break; }
      }
    }
    if (safeEnd > emitted) {
      const delta = raw.slice(emitted, safeEnd);
      emitted = safeEnd;
      onText?.(delta);
    }
  };
  for await (const chunk of res.body) {
    raw += decoder.decode(chunk, { stream: true });
    flush(false);
  }
  raw += decoder.decode();
  flush(true);

  let stats = null;
  const m = raw.match(STATS_RE);
  if (m) { try { stats = JSON.parse(m[1]); } catch {} }
  const text = sentinelAt === -1 ? raw : raw.slice(0, sentinelAt);
  if (!text && !stats) throw new ContextOverflowError();
  return { text, stats, latencyMs: Date.now() - t0 };
}
