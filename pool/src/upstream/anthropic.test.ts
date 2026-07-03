import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import { loadConfig, type Config } from "../config.ts";
import { AccountManager } from "../accounts/manager.ts";
import { proxyAnthropicMessages } from "./anthropic.ts";

interface FetchCall {
  url: string;
  init: RequestInit;
  body: unknown;
  headers: Headers;
}

const realFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = realFetch;
});

function tempPool(accountNames: string[]): { poolDir: string; mgr: AccountManager; config: Config } {
  const poolDir = mkdtempSync(join(tmpdir(), "cmp-upstream-"));
  const accountsDir = join(poolDir, "accounts");
  for (const name of accountNames) {
    const dir = join(accountsDir, name);
    mkdirSync(dir, { recursive: true });
    writeFileSync(
      join(dir, ".credentials.json"),
      JSON.stringify(
        {
          claudeAiOauth: {
            accessToken: "tok-" + name,
            refreshToken: "refresh-" + name,
            expiresAt: Date.now() + 3_600_000,
            scopes: ["user:inference", "user:sessions:claude_code"],
            subscriptionType: "max",
            rateLimitTier: "default_claude_max_5x",
          },
        },
        null,
        2,
      ),
    );
  }
  const config = loadConfig({
    poolDir,
    accountsDir,
    usageFile: join(poolDir, "usage.json"),
    anthropicApiBaseUrl: "https://api.test",
    oauthTokenUrl: "https://oauth.test/token",
    requestTimeoutMs: 30_000,
  });
  return { poolDir, mgr: new AccountManager(config), config };
}

function mockFetch(handler: (url: string, init: RequestInit) => Response | Promise<Response>): FetchCall[] {
  const calls: FetchCall[] = [];
  globalThis.fetch = (async (input: Parameters<typeof fetch>[0], init?: RequestInit) => {
    const url = input instanceof Request ? input.url : input instanceof URL ? input.toString() : String(input);
    const requestInit = init ?? {};
    const headers = new Headers(requestInit.headers);
    const body = requestInit.body ? JSON.parse(String(requestInit.body)) : null;
    calls.push({ url, init: requestInit, headers, body });
    return handler(url, requestInit);
  }) as typeof fetch;
  return calls;
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function sseResponse(text: string, status = 200): Response {
  return new Response(text, {
    status,
    headers: { "content-type": "text/event-stream; charset=utf-8" },
  });
}

async function drain(response: Response): Promise<string> {
  return await response.text();
}

test("forwards Anthropic messages verbatim with the selected account OAuth token", async () => {
  const { poolDir, mgr, config } = tempPool(["a"]);
  try {
    const calls = mockFetch(() =>
      jsonResponse({
        id: "msg_1",
        type: "message",
        role: "assistant",
        content: [{ type: "text", text: "ok" }],
        usage: { input_tokens: 11, output_tokens: 3 },
      }),
    );

    const body = {
      model: "claude-sonnet-5",
      max_tokens: 32,
      system: "system prompt",
      tools: [{ name: "lookup", input_schema: { type: "object" } }],
      thinking: { type: "enabled", budget_tokens: 1024 },
      messages: [{ role: "user", content: "hello" }],
      metadata: { user_id: "session-1" },
    };

    const response = await proxyAnthropicMessages(
      body,
      new Headers({
        "anthropic-version": "2023-06-01",
        "anthropic-beta": "fine-grained-tool-streaming-2025-05-14,oauth-2025-04-20",
        "content-type": "application/json",
        "user-agent": "claude-code-test-harness",
        "x-app": "cli",
        "x-api-key": "local-proxy-key",
        host: "127.0.0.1:3456",
      }),
      mgr,
      config,
      new AbortController().signal,
    );

    expect(response.status).toBe(200);
    expect(response.headers.get("X-Pool-Account")).toBe("a");
    expect(await response.json()).toMatchObject({ id: "msg_1" });

    expect(calls).toHaveLength(1);
    expect(calls[0]!.url).toBe("https://api.test/v1/messages");
    expect(calls[0]!.body).toEqual(body);
    expect(calls[0]!.headers.get("authorization")).toBe("Bearer tok-a");
    expect(calls[0]!.headers.get("anthropic-version")).toBe("2023-06-01");
    expect(calls[0]!.headers.get("anthropic-beta")).toBe(
      "fine-grained-tool-streaming-2025-05-14,oauth-2025-04-20",
    );
    expect(calls[0]!.headers.get("content-type")).toBe("application/json");
    expect(calls[0]!.headers.get("user-agent")).toBe("claude-code-test-harness");
    expect(calls[0]!.headers.get("x-app")).toBe("cli");
    expect(calls[0]!.headers.has("x-api-key")).toBe(false);
    expect(calls[0]!.headers.has("host")).toBe(false);
    expect(mgr.getAccount("a").usage.totalRequests).toBe(1);
    expect(mgr.getAccount("a").usage.totalInputTokens).toBe(11);
    expect(mgr.getAccount("a").usage.totalOutputTokens).toBe(3);
  } finally {
    rmSync(poolDir, { recursive: true, force: true });
  }
});

test("refreshes an expired access token and persists rotated credentials", async () => {
  const { poolDir, mgr, config } = tempPool(["a"]);
  try {
    const credsPath = join(poolDir, "accounts", "a", ".credentials.json");
    const creds = JSON.parse(readFileSync(credsPath, "utf8")) as Record<string, any>;
    creds.claudeAiOauth.expiresAt = Date.now() - 1_000;
    writeFileSync(credsPath, JSON.stringify(creds, null, 2));

    const calls = mockFetch((url) => {
      if (url === "https://oauth.test/token") {
        return jsonResponse({
          access_token: "tok-rotated",
          refresh_token: "refresh-rotated",
          expires_in: 7200,
          scope: "user:inference user:sessions:claude_code",
        });
      }
      return jsonResponse({
        type: "message",
        usage: { input_tokens: 1, output_tokens: 1 },
        content: [],
      });
    });

    const response = await proxyAnthropicMessages(
      { model: "claude-sonnet-5", max_tokens: 1, messages: [{ role: "user", content: "hi" }] },
      new Headers(),
      mgr,
      config,
      new AbortController().signal,
    );

    expect(response.status).toBe(200);
    expect(calls).toHaveLength(2);
    expect(calls[0]!.url).toBe("https://oauth.test/token");
    expect(calls[0]!.body).toMatchObject({
      grant_type: "refresh_token",
      refresh_token: "refresh-a",
      client_id: config.oauthClientId,
    });
    expect(calls[1]!.headers.get("authorization")).toBe("Bearer tok-rotated");

    const updated = JSON.parse(readFileSync(credsPath, "utf8")) as Record<string, any>;
    expect(updated.claudeAiOauth.accessToken).toBe("tok-rotated");
    expect(updated.claudeAiOauth.refreshToken).toBe("refresh-rotated");
  } finally {
    rmSync(poolDir, { recursive: true, force: true });
  }
});

test("fails over to another account on a start-of-request rate limit", async () => {
  const { poolDir, mgr, config } = tempPool(["a", "b"]);
  try {
    const failovers: string[] = [];
    const calls = mockFetch((_, init) => {
      const token = new Headers(init.headers).get("authorization");
      if (token === "Bearer tok-a") {
        return jsonResponse(
          { type: "error", error: { type: "rate_limit_error", message: "usage limit reached" } },
          429,
        );
      }
      return jsonResponse({
        type: "message",
        content: [{ type: "text", text: "served by b" }],
        usage: { input_tokens: 2, output_tokens: 4 },
      });
    });

    const response = await proxyAnthropicMessages(
      { model: "claude-sonnet-5", max_tokens: 8, messages: [{ role: "user", content: "hi" }] },
      new Headers(),
      mgr,
      config,
      new AbortController().signal,
      { onFailover: (from, to) => failovers.push(`${from}->${to}`) },
    );

    expect(response.status).toBe(200);
    expect(response.headers.get("X-Pool-Account")).toBe("b");
    expect(calls.map((c) => c.headers.get("authorization"))).toEqual(["Bearer tok-a", "Bearer tok-b"]);
    expect(failovers).toEqual(["a->b"]);
    expect(mgr.getAccount("a").available).toBe(false);
    expect(mgr.getAccount("b").usage.totalRequests).toBe(1);
  } finally {
    rmSync(poolDir, { recursive: true, force: true });
  }
});

test("streams upstream SSE bytes through unchanged and records final usage", async () => {
  const { poolDir, mgr, config } = tempPool(["a"]);
  try {
    const upstreamSse =
      'event: message_start\ndata: {"type":"message_start","message":{"usage":{"input_tokens":7,"output_tokens":0}}}\n\n' +
      'event: content_block_delta\ndata: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}\n\n' +
      'event: message_delta\ndata: {"type":"message_delta","usage":{"output_tokens":5}}\n\n' +
      'event: message_stop\ndata: {"type":"message_stop"}\n\n';
    mockFetch(() => sseResponse(upstreamSse));

    const response = await proxyAnthropicMessages(
      {
        model: "claude-sonnet-5",
        max_tokens: 8,
        stream: true,
        messages: [{ role: "user", content: "hi" }],
      },
      new Headers(),
      mgr,
      config,
      new AbortController().signal,
    );

    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toContain("text/event-stream");
    expect(await drain(response)).toBe(upstreamSse);
    expect(mgr.getAccount("a").usage.totalRequests).toBe(1);
    expect(mgr.getAccount("a").usage.totalInputTokens).toBe(7);
    expect(mgr.getAccount("a").usage.totalOutputTokens).toBe(5);
  } finally {
    rmSync(poolDir, { recursive: true, force: true });
  }
});
