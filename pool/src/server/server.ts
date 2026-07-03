/**
 * HTTP server (Bun.serve). Routes:
 *   GET  /                      -> status dashboard
 *   GET  /health                -> liveness + pool summary
 *   GET  /api/status            -> JSON account/usage status (dashboard polls this)
 *   GET  /v1/models             -> OpenAI-style model list
 *   POST /v1/chat/completions   -> OpenAI Chat Completions (stream + non-stream)
 *   POST /v1/messages           -> Anthropic Messages (stream + non-stream)
 */

import type { Config } from "../config.ts";
import { AccountManager } from "../accounts/manager.ts";
import { runClaude } from "../subprocess/claude.ts";
import type { Account } from "../accounts/types.ts";
import { runWithFailover } from "./failover.ts";
import { dashboardHtml } from "./dashboard.ts";
import { proxyAnthropicMessages } from "../upstream/anthropic.ts";
import {
  parseOpenAI,
  collectOpenAI,
  streamOpenAI,
  type OpenAIChatRequest,
} from "../adapters/openai.ts";
import {
  parseAnthropic,
  collectAnthropic,
  streamAnthropic,
  type AnthropicRequest,
} from "../adapters/anthropic.ts";

const APPEND_SYSTEM_PROMPT =
  "You are being used as an API model endpoint. Respond directly to the user's request. " +
  "Do not ask clarifying questions unless strictly necessary; produce the best answer you can from the information given.";

const MODELS = [
  "opus",
  "sonnet",
  "haiku",
  "claude-opus-4-8",
  "claude-sonnet-5",
  "claude-haiku-4-5",
];

export function startServer(config: Config): void {
  const mgr = new AccountManager(config);

  const server = Bun.serve({
    hostname: config.host,
    port: config.port,
    idleTimeout: 255, // allow long-running generations
    async fetch(req) {
      const url = new URL(req.url);
      const path = url.pathname;

      if (req.method === "GET" && path === "/") {
        return html(dashboardHtml());
      }
      if (req.method === "GET" && (path === "/health" || path === "/healthz")) {
        const accounts = mgr.listAccounts();
        return json({
          status: "ok",
          accounts: accounts.length,
          available: accounts.filter((a) => a.available).length,
        });
      }
      if (req.method === "GET" && path === "/api/status") {
        return json({
          accounts: mgr.listAccounts(),
          usageWindowMs: config.usageWindowMs,
          now: Date.now(),
        });
      }
      if (req.method === "GET" && (path === "/v1/models" || path === "/models")) {
        return json({
          object: "list",
          data: MODELS.map((m) => ({
            id: m,
            object: "model",
            created: 0,
            owned_by: "anthropic-claude-max-pool",
          })),
        });
      }

      // ---- Inference endpoints (require proxy auth if configured) ----
      if (path === "/v1/chat/completions" || path === "/v1/messages") {
        if (req.method !== "POST") return json({ error: "method not allowed" }, 405);
        const authErr = checkAuth(req, config);
        if (authErr) return authErr;

        let body: unknown;
        try {
          body = await req.json();
        } catch {
          return json({ error: { message: "Invalid JSON body" } }, 400);
        }

        return path === "/v1/chat/completions"
          ? handleOpenAI(body as OpenAIChatRequest, mgr, config, req.signal)
          : handleAnthropic(body, req.headers, mgr, config, req.signal);
      }

      return json({ error: "not found" }, 404);
    },
  });

  const accounts = mgr.listAccounts();
  const available = accounts.filter((a) => a.available).length;
  const origin = `http://${config.host}:${server.port}`;
  console.log(`\n  Claude Max Pool listening on ${origin}`);
  console.log(`  Dashboard:  ${origin}/`);
  console.log(`  Accounts:   ${accounts.length} total, ${available} available`);
  console.log(`  Backend:    ${config.backend === "oauth" ? "direct Anthropic OAuth" : "Claude CLI subprocess"}`);
  console.log(`  Proxy auth: ${config.proxyApiKey ? "enabled (bearer token required)" : "disabled (set PROXY_API_KEY to require a token)"}`);

  if (accounts.length === 0) {
    console.log(`
  ──────────────────────────────────────────────────────────────
  No accounts yet. Get started (each is a separate Claude login):

    1. Install the Claude CLI, if needed:
         npm install -g @anthropic-ai/claude-code

    2. Log in your first plan (do /login, then /exit in the CLI):
         bun run src/index.ts accounts login work

       …or import the login already on this machine:
         bun run src/index.ts accounts import primary

    3. Add more plans the same way, then send a request:
         curl ${origin}/v1/chat/completions \\
           -H "content-type: application/json" \\
           -d '{"model":"sonnet","messages":[{"role":"user","content":"Hello!"}]}'

  Open ${origin}/ for a live setup walkthrough + status.
  ──────────────────────────────────────────────────────────────
`);
  } else if (available === 0) {
    const anyAuthed = accounts.some((a) => a.authenticated);
    console.log(
      anyAuthed
        ? `\n  ⚠ No accounts are available right now (rate limited / token needs attention).\n    Check ${origin}/ — requests will 503 until one frees up.\n`
        : `\n  ⚠ No accounts are logged in. Authenticate one:\n      bun run src/index.ts accounts login <name>\n`,
    );
  } else {
    console.log("");
  }
}

// ---- request handlers ----------------------------------------------------

async function handleOpenAI(
  body: OpenAIChatRequest,
  mgr: AccountManager,
  config: Config,
  signal: AbortSignal,
): Promise<Response> {
  const parsed = parseOpenAI(body);
  const first = mgr.pick(parsed.sessionKey);
  if (!first) return json(noAccountError("openai", mgr), 503);

  const events = runWithFailover(
    mgr,
    parsed.sessionKey,
    first,
    makeEventFactory(config, parsed.prompt, parsed.model, signal),
    failoverHooks(config),
  );
  if (parsed.stream) {
    return sseResponse(streamOpenAI(events, parsed), first.name);
  }
  const { status, body: out } = await collectOpenAI(events, parsed);
  return json(out, status, { "X-Pool-Account": first.name });
}

async function handleAnthropic(
  body: unknown,
  headers: Headers,
  mgr: AccountManager,
  config: Config,
  signal: AbortSignal,
): Promise<Response> {
  if (config.backend === "oauth") {
    return proxyAnthropicMessages(body, headers, mgr, config, signal, failoverHooks(config));
  }

  const legacyBody = body as AnthropicRequest;
  const parsed = parseAnthropic(legacyBody);
  const first = mgr.pick(parsed.sessionKey);
  if (!first) return json(noAccountError("anthropic", mgr), 503);

  const events = runWithFailover(
    mgr,
    parsed.sessionKey,
    first,
    makeEventFactory(config, parsed.prompt, parsed.model, signal),
    failoverHooks(config),
  );
  if (parsed.stream) {
    return sseResponse(streamAnthropic(events, parsed), first.name);
  }
  const { status, body: out } = await collectAnthropic(events, parsed);
  return json(out, status, { "X-Pool-Account": first.name });
}

/** Builds the per-account event factory that spawns the Claude CLI. */
function makeEventFactory(
  config: Config,
  prompt: string,
  model: "opus" | "sonnet" | "haiku",
  signal: AbortSignal,
) {
  return (account: Account) =>
    runClaude(prompt, {
      claudeBin: config.claudeBin,
      configDir: account.configDir,
      model,
      appendSystemPrompt: APPEND_SYSTEM_PROMPT,
      timeoutMs: config.requestTimeoutMs,
      signal,
    });
}

function failoverHooks(config: Config) {
  return {
    onFailover: (from: string, to: string) => {
      if (config.logFailover) {
        console.log(`  ↻ failover: "${from}" exhausted → retrying on "${to}"`);
      }
    },
  };
}

// ---- helpers -------------------------------------------------------------

function checkAuth(req: Request, config: Config): Response | null {
  if (!config.proxyApiKey) return null;
  const header = req.headers.get("authorization") || "";
  const xkey = req.headers.get("x-api-key") || "";
  const bearer = header.startsWith("Bearer ") ? header.slice(7) : "";
  if (bearer === config.proxyApiKey || xkey === config.proxyApiKey) return null;
  return json({ error: { message: "Unauthorized: invalid or missing API key", type: "authentication_error" } }, 401);
}

function noAccountError(shape: "openai" | "anthropic", mgr: AccountManager): unknown {
  const total = mgr.listAccounts().length;
  const message =
    total === 0
      ? "No Claude accounts configured. Add one with: bun run src/index.ts accounts login <name>"
      : "All Claude accounts are currently unavailable (logged out or rate limited). Check the dashboard.";
  return shape === "openai"
    ? { error: { message, type: "service_unavailable" } }
    : { type: "error", error: { type: "overloaded_error", message } };
}

function json(body: unknown, status = 200, extraHeaders: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...extraHeaders },
  });
}

function html(body: string): Response {
  return new Response(body, { headers: { "content-type": "text/html; charset=utf-8" } });
}

function sseResponse(gen: AsyncGenerator<string>, accountName: string): Response {
  const stream = new ReadableStream<Uint8Array>({
    async pull(controller) {
      const encoder = new TextEncoder();
      try {
        const { value, done } = await gen.next();
        if (done) {
          controller.close();
          return;
        }
        controller.enqueue(encoder.encode(value));
      } catch (err) {
        controller.error(err);
      }
    },
  });
  return new Response(stream, {
    headers: {
      "content-type": "text/event-stream; charset=utf-8",
      "cache-control": "no-cache",
      connection: "keep-alive",
      "X-Pool-Account": accountName,
    },
  });
}
