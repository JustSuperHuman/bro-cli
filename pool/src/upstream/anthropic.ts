/**
 * Direct Anthropic Messages proxy.
 *
 * This backend avoids spawning `claude --print`: it selects a pooled account,
 * refreshes that account's Claude Code OAuth token when needed, forwards the
 * caller's /v1/messages JSON to Anthropic, and streams Anthropic's SSE bytes
 * back unchanged.
 */

import type { Config } from "../config.ts";
import { AccountManager } from "../accounts/manager.ts";
import type { Account, ClaudeOauthCreds } from "../accounts/types.ts";
import type { CliUsage } from "../subprocess/types.ts";

interface ProxyHooks {
  onFailover?: (from: string, to: string) => void;
}

interface RetryReason {
  status: number;
  type: string;
  message: string;
  rateLimited: boolean;
  resetAt?: number;
}

type AttemptResult =
  | { kind: "response"; response: Response }
  | { kind: "retry"; reason: RetryReason }
  | { kind: "terminal"; response: Response };

interface FetchResult {
  response: Response;
  cleanup: () => void;
}

type ByteReadResult = { done: true; value?: undefined } | { done: false; value: Uint8Array };

interface ByteReader {
  read(): Promise<ByteReadResult>;
  cancel(reason?: unknown): Promise<void>;
}

const refreshLocks = new Map<string, Promise<ClaudeOauthCreds>>();

export interface UpstreamModel {
  id: string;
  display_name: string;
}

// The upstream API only knows real model ids; the short aliases that the
// Claude CLI accepts ("opus") 404 there. Resolved against the live model list
// when possible, with a static fallback.
const MODEL_ALIASES: Record<string, string> = {
  opus: "claude-opus-4-8",
  sonnet: "claude-sonnet-5",
  haiku: "claude-haiku-4-5-20251001",
};

const MODELS_CACHE_MS = 5 * 60 * 1000;
let modelsCache: { at: number; models: UpstreamModel[] } | null = null;

/**
 * Fetch the live model list from Anthropic using a pooled account's OAuth
 * token. Returns null (so callers can fall back) when no account is available,
 * the backend is not oauth, or the upstream call fails.
 */
export async function fetchUpstreamModels(
  mgr: AccountManager,
  config: Config,
): Promise<UpstreamModel[] | null> {
  if (config.backend !== "oauth") return null;
  if (modelsCache && Date.now() - modelsCache.at < MODELS_CACHE_MS) return modelsCache.models;

  const account = mgr.pick();
  if (!account) return null;
  try {
    const token = await accessTokenFor(account, mgr, config, false);
    const response = await fetch(modelsListUrl(config.anthropicApiBaseUrl), {
      headers: {
        authorization: `Bearer ${token}`,
        "anthropic-version": "2023-06-01",
        "anthropic-beta": "oauth-2025-04-20",
      },
    });
    if (!response.ok) return null;
    const json = parseJson(await response.text());
    const data = json?.data;
    if (!Array.isArray(data)) return null;
    const models: UpstreamModel[] = [];
    for (const entry of data) {
      const obj = asObject(entry);
      const id = stringProp(obj, "id");
      if (!id) continue;
      models.push({ id, display_name: stringProp(obj, "display_name") ?? id });
    }
    if (models.length === 0) return null;
    modelsCache = { at: Date.now(), models };
    return models;
  } catch {
    return null;
  }
}

/** Map a CLI-style alias ("opus") to a real model id; real ids pass through. */
async function resolveModel(model: string, mgr: AccountManager, config: Config): Promise<string> {
  const fallback = MODEL_ALIASES[model];
  if (!fallback) return model;
  const models = await fetchUpstreamModels(mgr, config);
  // Upstream lists newest first, so the first family match is the latest.
  const match = models?.find((m) => m.id.includes(model));
  return match?.id ?? fallback;
}

function modelsListUrl(baseUrl: string): string {
  const clean = baseUrl.replace(/\/+$/, "").replace(/\/v1(\/messages)?$/, "");
  return `${clean}/v1/models?limit=1000`;
}

export async function proxyAnthropicMessages(
  body: unknown,
  incomingHeaders: Headers,
  mgr: AccountManager,
  config: Config,
  signal: AbortSignal,
  hooks: ProxyHooks = {},
): Promise<Response> {
  const sessionKey = extractSessionKey(body);
  const first = mgr.pick(sessionKey);
  if (!first) return anthropicError(503, "overloaded_error", noAccountMessage(mgr));

  const requestObj = asObject(body);
  const requestedModel = stringProp(requestObj, "model");
  if (requestedModel && MODEL_ALIASES[requestedModel]) {
    body = { ...requestObj, model: await resolveModel(requestedModel, mgr, config) };
  }

  const bodyText = JSON.stringify(body ?? {});
  const tried = new Set<string>();
  let account: Account | null = first;
  let lastRetry: RetryReason | null = null;

  while (account) {
    tried.add(account.name);
    const attempt = await tryAccount(account, bodyText, incomingHeaders, mgr, config, signal);

    if (attempt.kind === "response") {
      if (sessionKey) mgr.setAffinity(sessionKey, account.name);
      return attempt.response;
    }

    if (attempt.kind === "terminal") return attempt.response;

    lastRetry = attempt.reason;
    const next = mgr.pick(sessionKey, tried);
    if (!next) break;
    hooks.onFailover?.(account.name, next.name);
    account = next;
  }

  return anthropicError(
    lastRetry?.status ?? 503,
    lastRetry?.type ?? "overloaded_error",
    lastRetry?.message ?? noAccountMessage(mgr),
  );
}

async function tryAccount(
  account: Account,
  bodyText: string,
  incomingHeaders: Headers,
  mgr: AccountManager,
  config: Config,
  signal: AbortSignal,
): Promise<AttemptResult> {
  let upstream: FetchResult;
  try {
    upstream = await fetchWithAccount(account, bodyText, incomingHeaders, mgr, config, signal, false);
  } catch (err) {
    const message = (err as Error).message;
    mgr.recordError(account.name, message);
    return { kind: "retry", reason: authOrNetworkReason(message) };
  }

  if (upstream.response.status === 401) {
    upstream.cleanup();
    try {
      upstream = await fetchWithAccount(account, bodyText, incomingHeaders, mgr, config, signal, true);
    } catch (err) {
      const message = (err as Error).message;
      mgr.recordError(account.name, message);
      return { kind: "retry", reason: authOrNetworkReason(message) };
    }
  }

  const streamRequested = bodyRequestsStream(bodyText);
  const contentType = upstream.response.headers.get("content-type") ?? "";
  const isSse = streamRequested || contentType.toLowerCase().includes("text/event-stream");

  if (!upstream.response.ok) {
    const text = await upstream.response.text().catch(() => "");
    upstream.cleanup();
    const reason = classifyHttpError(upstream.response.status, upstream.response.headers, text);
    if (reason.rateLimited) {
      mgr.markRateLimited(account.name, reason.resetAt);
      return { kind: "retry", reason };
    }
    mgr.recordError(account.name, reason.message);
    return {
      kind: "terminal",
      response: responseFromUpstreamText(text, upstream.response, account.name),
    };
  }

  if (isSse && upstream.response.body) {
    return prepareStreamingResponse(upstream, account, mgr);
  }

  const text = await upstream.response.text();
  upstream.cleanup();
  recordJsonUsage(text, mgr, account.name);
  return {
    kind: "response",
    response: responseFromUpstreamText(text, upstream.response, account.name),
  };
}

async function fetchWithAccount(
  account: Account,
  bodyText: string,
  incomingHeaders: Headers,
  mgr: AccountManager,
  config: Config,
  signal: AbortSignal,
  forceRefresh: boolean,
): Promise<FetchResult> {
  const token = await accessTokenFor(account, mgr, config, forceRefresh);
  const abort = makeAbort(config, signal);
  try {
    const response = await fetch(messagesUrl(config.anthropicApiBaseUrl), {
      method: "POST",
      headers: upstreamHeaders(incomingHeaders, token),
      body: bodyText,
      signal: abort.signal,
    });
    return { response, cleanup: abort.cleanup };
  } catch (err) {
    abort.cleanup();
    if (signal.aborted) throw new Error("Request aborted by client");
    throw err;
  }
}

async function accessTokenFor(
  account: Account,
  mgr: AccountManager,
  config: Config,
  forceRefresh: boolean,
): Promise<string> {
  const oauth = mgr.getOAuthCreds(account.name);
  if (!oauth?.accessToken) throw new Error(`Account "${account.name}" has no OAuth access token`);
  if (!forceRefresh && tokenFresh(oauth, config)) return oauth.accessToken;
  if (!oauth.refreshToken) {
    if (!forceRefresh) return oauth.accessToken;
    throw new Error(`Account "${account.name}" cannot refresh OAuth token; re-run accounts login`);
  }

  const existing = refreshLocks.get(account.name);
  if (existing) {
    const refreshed = await existing;
    if (!refreshed.accessToken) throw new Error(`Account "${account.name}" refresh returned no access token`);
    return refreshed.accessToken;
  }

  const refresh = refreshOAuth(account.name, mgr, config, forceRefresh).finally(() => {
    refreshLocks.delete(account.name);
  });
  refreshLocks.set(account.name, refresh);
  const refreshed = await refresh;
  if (!refreshed.accessToken) throw new Error(`Account "${account.name}" refresh returned no access token`);
  return refreshed.accessToken;
}

async function refreshOAuth(
  accountName: string,
  mgr: AccountManager,
  config: Config,
  forceRefresh: boolean,
): Promise<ClaudeOauthCreds> {
  const current = mgr.getOAuthCreds(accountName);
  if (!current?.refreshToken) throw new Error(`Account "${accountName}" has no OAuth refresh token`);
  if (!forceRefresh && current.accessToken && tokenFresh(current, config)) return current;

  const response = await fetch(config.oauthTokenUrl, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      grant_type: "refresh_token",
      refresh_token: current.refreshToken,
      client_id: config.oauthClientId,
    }),
  });
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`OAuth refresh failed for "${accountName}" (${response.status}): ${safeErrorText(text)}`);
  }

  const json = parseJson(text);
  const accessToken = stringProp(json, "access_token") ?? stringProp(json, "accessToken");
  if (!accessToken) throw new Error(`OAuth refresh failed for "${accountName}": no access token in response`);

  const refreshToken =
    stringProp(json, "refresh_token") ?? stringProp(json, "refreshToken") ?? current.refreshToken;
  const expiresIn = numberProp(json, "expires_in") ?? numberProp(json, "expiresIn") ?? 3600;
  const scope = stringProp(json, "scope");
  const next: ClaudeOauthCreds = {
    ...current,
    accessToken,
    refreshToken,
    expiresAt: Date.now() + Math.max(1, expiresIn) * 1000,
    scopes: scope ? scope.split(/\s+/).filter(Boolean) : current.scopes,
  };
  mgr.updateOAuthCreds(accountName, next);
  return next;
}

function tokenFresh(oauth: ClaudeOauthCreds, config: Config): boolean {
  if (!oauth.expiresAt) return true;
  return oauth.expiresAt - Date.now() > config.tokenRefreshSkewMs;
}

async function prepareStreamingResponse(
  upstream: FetchResult,
  account: Account,
  mgr: AccountManager,
): Promise<AttemptResult> {
  const body = upstream.response.body;
  if (!body) {
    upstream.cleanup();
    mgr.recordError(account.name, "Anthropic returned an empty streaming body");
    return {
      kind: "terminal",
      response: anthropicError(502, "api_error", "Anthropic returned an empty streaming body"),
    };
  }

  const reader = body.getReader() as ByteReader;
  const prefix: Uint8Array[] = [];
  let prefixBytes = 0;
  const tap = new StreamUsageTap(mgr, account.name);

  try {
    while (!tap.committed && !tap.initialRateLimit && prefixBytes < 64 * 1024) {
      const { value, done } = await reader.read();
      if (done) {
        tap.finish();
        upstream.cleanup();
        return {
          kind: "response",
          response: new Response(bytesStream(prefix), {
            status: upstream.response.status,
            statusText: upstream.response.statusText,
            headers: responseHeaders(upstream.response.headers, account.name),
          }),
        };
      }
      if (!value) continue;
      prefix.push(value);
      prefixBytes += value.byteLength;
      tap.push(value);
    }

    if (tap.initialRateLimit) {
      await reader.cancel().catch(() => {});
      upstream.cleanup();
      mgr.markRateLimited(account.name, tap.initialRateLimit.resetAt);
      return { kind: "retry", reason: tap.initialRateLimit };
    }

    const stream = streamWithTap(reader, prefix, tap, upstream.cleanup);
    return {
      kind: "response",
      response: new Response(stream, {
        status: upstream.response.status,
        statusText: upstream.response.statusText,
        headers: responseHeaders(upstream.response.headers, account.name),
      }),
    };
  } catch (err) {
    upstream.cleanup();
    mgr.recordError(account.name, (err as Error).message);
    return {
      kind: "terminal",
      response: anthropicError(502, "api_error", `Streaming proxy error: ${(err as Error).message}`),
    };
  }
}

function streamWithTap(
  reader: ByteReader,
  prefix: Uint8Array[],
  tap: StreamUsageTap,
  cleanup: () => void,
): ReadableStream<Uint8Array> {
  let prefixIndex = 0;
  let finished = false;

  const finish = () => {
    if (finished) return;
    finished = true;
    tap.finish();
    cleanup();
  };

  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      if (prefixIndex < prefix.length) {
        controller.enqueue(prefix[prefixIndex++]!);
        return;
      }
      try {
        const { value, done } = await reader.read();
        if (done) {
          finish();
          controller.close();
          return;
        }
        if (value) {
          tap.push(value);
          controller.enqueue(value);
        }
      } catch (err) {
        tap.error((err as Error).message);
        cleanup();
        controller.error(err);
      }
    },
    async cancel(reason) {
      tap.cancel();
      cleanup();
      await reader.cancel(reason).catch(() => {});
    },
  });
}

class StreamUsageTap {
  committed = false;
  initialRateLimit: RetryReason | null = null;

  private parser = new SseParser((event) => this.onEvent(event));
  private usage: CliUsage = { input_tokens: 0, output_tokens: 0 };
  private sawError = false;
  private finalError: RetryReason | null = null;
  private done = false;

  constructor(
    private mgr: AccountManager,
    private accountName: string,
  ) {}

  push(chunk: Uint8Array): void {
    this.parser.push(chunk);
  }

  finish(): void {
    if (this.done) return;
    this.done = true;
    this.parser.end();
    if (this.sawError && this.finalError) {
      if (this.finalError.rateLimited) this.mgr.markRateLimited(this.accountName, this.finalError.resetAt);
      else this.mgr.recordError(this.accountName, this.finalError.message);
      return;
    }
    this.mgr.recordSuccess(this.accountName, this.usage, 0);
  }

  error(message: string): void {
    if (this.done) return;
    this.done = true;
    this.mgr.recordError(this.accountName, message);
  }

  cancel(): void {
    this.done = true;
  }

  private onEvent(event: SseEvent): void {
    const data = parseJson(event.data);
    const type = stringProp(data, "type") ?? event.event;

    if (type === "ping") return;

    if (type === "message_start") {
      const message = objectProp(data, "message");
      this.mergeUsage(objectProp(message, "usage"));
    } else if (type === "message_delta") {
      this.mergeUsage(objectProp(data, "usage"));
    } else if (type === "error") {
      const reason = classifySseError(data);
      if (!this.committed && reason.rateLimited) {
        this.initialRateLimit = reason;
        return;
      }
      this.sawError = true;
      this.finalError = reason;
    }

    if (!this.committed) this.committed = true;
  }

  private mergeUsage(usage: Record<string, unknown> | null): void {
    if (!usage) return;
    this.usage.input_tokens = numberProp(usage, "input_tokens") ?? this.usage.input_tokens;
    this.usage.output_tokens = numberProp(usage, "output_tokens") ?? this.usage.output_tokens;
    const cacheCreation = numberProp(usage, "cache_creation_input_tokens");
    const cacheRead = numberProp(usage, "cache_read_input_tokens");
    if (cacheCreation != null) this.usage.cache_creation_input_tokens = cacheCreation;
    if (cacheRead != null) this.usage.cache_read_input_tokens = cacheRead;
  }
}

interface SseEvent {
  event: string;
  data: string;
}

class SseParser {
  private decoder = new TextDecoder();
  private buffer = "";
  private eventName = "";
  private data: string[] = [];

  constructor(private onEvent: (event: SseEvent) => void) {}

  push(chunk: Uint8Array): void {
    this.pushText(this.decoder.decode(chunk, { stream: true }));
  }

  end(): void {
    const rest = this.decoder.decode();
    if (rest) this.pushText(rest);
  }

  private pushText(text: string): void {
    this.buffer += text;
    while (true) {
      const i = this.buffer.indexOf("\n");
      if (i < 0) return;
      const raw = this.buffer.slice(0, i);
      this.buffer = this.buffer.slice(i + 1);
      this.line(raw.endsWith("\r") ? raw.slice(0, -1) : raw);
    }
  }

  private line(line: string): void {
    if (line === "") {
      this.dispatch();
      return;
    }
    if (line.startsWith(":")) return;

    const colon = line.indexOf(":");
    const field = colon < 0 ? line : line.slice(0, colon);
    let value = colon < 0 ? "" : line.slice(colon + 1);
    if (value.startsWith(" ")) value = value.slice(1);

    if (field === "event") this.eventName = value;
    else if (field === "data") this.data.push(value);
  }

  private dispatch(): void {
    if (!this.eventName && this.data.length === 0) return;
    this.onEvent({ event: this.eventName || "message", data: this.data.join("\n") });
    this.eventName = "";
    this.data = [];
  }
}

function recordJsonUsage(text: string, mgr: AccountManager, accountName: string): void {
  const json = parseJson(text);
  const usage = objectProp(json, "usage");
  mgr.recordSuccess(
    accountName,
    {
      input_tokens: numberProp(usage, "input_tokens") ?? 0,
      output_tokens: numberProp(usage, "output_tokens") ?? 0,
      cache_creation_input_tokens: numberProp(usage, "cache_creation_input_tokens") ?? 0,
      cache_read_input_tokens: numberProp(usage, "cache_read_input_tokens") ?? 0,
    },
    0,
  );
}

function classifyHttpError(status: number, headers: Headers, text: string): RetryReason {
  const json = parseJson(text);
  const error = objectProp(json, "error");
  const type = stringProp(error, "type") ?? (status === 429 ? "rate_limit_error" : "api_error");
  const message = stringProp(error, "message") ?? (text.slice(0, 500) || `Anthropic API returned HTTP ${status}`);
  const rateLimited = status === 429 || isRateLimit(type, message);
  return { status, type, message, rateLimited, resetAt: resetAtFromHeaders(headers) };
}

function classifySseError(data: Record<string, unknown> | null): RetryReason {
  const error = objectProp(data, "error");
  const type = stringProp(error, "type") ?? "api_error";
  const message = stringProp(error, "message") ?? "Anthropic streaming error";
  const rateLimited = isRateLimit(type, message);
  return { status: rateLimited ? 429 : 502, type, message, rateLimited };
}

function isRateLimit(type: string, message: string): boolean {
  const text = `${type}\n${message}`.toLowerCase();
  return (
    text.includes("rate_limit") ||
    text.includes("rate limit") ||
    text.includes("usage limit") ||
    text.includes("limit reached") ||
    text.includes("too many requests")
  );
}

function resetAtFromHeaders(headers: Headers): number | undefined {
  const retryAfter = headers.get("retry-after");
  if (retryAfter) {
    const seconds = Number.parseInt(retryAfter, 10);
    if (Number.isFinite(seconds)) return Date.now() + seconds * 1000;
    const parsed = Date.parse(retryAfter);
    if (Number.isFinite(parsed)) return parsed;
  }

  for (const name of [
    "anthropic-ratelimit-requests-reset",
    "anthropic-ratelimit-tokens-reset",
    "anthropic-ratelimit-input-tokens-reset",
    "anthropic-ratelimit-output-tokens-reset",
  ]) {
    const value = headers.get(name);
    if (!value) continue;
    const parsed = Date.parse(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return undefined;
}

function authOrNetworkReason(message: string): RetryReason {
  return { status: 401, type: "authentication_error", message, rateLimited: false };
}

function upstreamHeaders(incoming: Headers, token: string): Headers {
  const headers = new Headers(incoming);

  for (const name of HOP_BY_HOP_REQUEST_HEADERS) {
    headers.delete(name);
  }

  // The caller's proxy credential is local to this pool. Upstream auth must be
  // the selected Claude account's OAuth bearer token.
  headers.set("authorization", `Bearer ${token}`);
  headers.delete("x-api-key");
  return headers;
}

const HOP_BY_HOP_REQUEST_HEADERS = [
  "connection",
  "content-length",
  "host",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
];

function responseFromUpstreamText(text: string, upstream: Response, accountName: string): Response {
  return new Response(text, {
    status: upstream.status,
    statusText: upstream.statusText,
    headers: responseHeaders(upstream.headers, accountName),
  });
}

function responseHeaders(source: Headers, accountName: string): Headers {
  const headers = new Headers(source);
  for (const name of [
    "connection",
    "content-encoding",
    "content-length",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
  ]) {
    headers.delete(name);
  }
  headers.set("X-Pool-Account", accountName);
  return headers;
}

function anthropicError(status: number, type: string, message: string): Response {
  return new Response(JSON.stringify({ type: "error", error: { type, message } }), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function bytesStream(chunks: Uint8Array[]): ReadableStream<Uint8Array> {
  return new ReadableStream<Uint8Array>({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(chunk);
      controller.close();
    },
  });
}

function makeAbort(config: Config, signal: AbortSignal): { signal: AbortSignal; cleanup: () => void } {
  const controller = new AbortController();
  const onAbort = () => controller.abort();
  signal.addEventListener("abort", onAbort, { once: true });
  const timeout = setTimeout(() => controller.abort(), config.requestTimeoutMs);
  return {
    signal: controller.signal,
    cleanup: () => {
      clearTimeout(timeout);
      signal.removeEventListener("abort", onAbort);
    },
  };
}

function extractSessionKey(body: unknown): string | undefined {
  const metadata = objectProp(asObject(body), "metadata");
  const userId = stringProp(metadata, "user_id");
  return userId || undefined;
}

function bodyRequestsStream(bodyText: string): boolean {
  const body = parseJson(bodyText);
  return Boolean(body?.stream);
}

function noAccountMessage(mgr: AccountManager): string {
  const total = mgr.listAccounts().length;
  return total === 0
    ? "No Claude accounts configured. Add one with: bun run src/index.ts accounts login <name>"
    : "All Claude accounts are currently unavailable (logged out or rate limited). Check the dashboard.";
}

function messagesUrl(baseUrl: string): string {
  const clean = baseUrl.replace(/\/+$/, "");
  if (clean.endsWith("/v1/messages")) return clean;
  return clean.endsWith("/v1") ? `${clean}/messages` : `${clean}/v1/messages`;
}

function safeErrorText(text: string): string {
  const json = parseJson(text);
  const error = objectProp(json, "error");
  return stringProp(error, "message") ?? (text.slice(0, 500) || "unknown OAuth error");
}

function parseJson(text: string): Record<string, unknown> | null {
  try {
    const parsed = JSON.parse(text) as unknown;
    return asObject(parsed);
  } catch {
    return null;
  }
}

function asObject(value: unknown): Record<string, unknown> | null {
  return value != null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function objectProp(value: Record<string, unknown> | null, key: string): Record<string, unknown> | null {
  return asObject(value?.[key]);
}

function stringProp(value: Record<string, unknown> | null, key: string): string | undefined {
  const raw = value?.[key];
  return typeof raw === "string" ? raw : undefined;
}

function numberProp(value: Record<string, unknown> | null, key: string): number | undefined {
  const raw = value?.[key];
  return typeof raw === "number" && Number.isFinite(raw) ? raw : undefined;
}
