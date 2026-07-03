# Claude Max Pool

**Pool any number of Claude Max / Team plans behind one endpoint that speaks both the OpenAI and Anthropic APIs — with a live dashboard showing which accounts are authenticated and how much of each plan's usage window is left.**

Built on [Bun](https://bun.sh). The default Anthropic `/v1/messages` path forwards directly to `https://api.anthropic.com/v1/messages` with the selected account's Claude Code OAuth bearer token, so Claude Code requests are not nested through another `claude --print` subprocess. The legacy CLI subprocess backend is still available with `CLAUDE_POOL_BACKEND=cli`.

```
  ┌──────────────────────────────────────────────────────────────┐
  │  Your apps (Continue.dev, Cursor, OpenAI/Anthropic SDKs, …)    │
  └───────────────┬──────────────────────────┬───────────────────┘
       OpenAI  /v1/chat/completions   Anthropic  /v1/messages
                  └───────────┬──────────────┘
                     Claude Max Pool  (Bun HTTP server)
                              │  pick least-loaded, authenticated,
                              │  non-rate-limited account
             ┌────────────────┼────────────────┐
   CLAUDE_CONFIG_DIR   CLAUDE_CONFIG_DIR   CLAUDE_CONFIG_DIR
      accounts/work      accounts/personal   accounts/team2
             │                │                │
        OAuth token      OAuth token      OAuth token      (each its own Max/Team plan)
             │                │                │
             └──────── direct Anthropic Messages API ──────┘
```

## Why

A single Claude Max plan has a rolling usage limit (~every 5 hours). If you have more than one plan — a personal Max, a Team seat, a second subscription — there's no built-in way to use them together from your tools. This proxy gives every plan its own isolated login directory, exposes them all as one endpoint, and routes each request to the least-loaded plan that isn't rate-limited. When one plan's window fills up, traffic flows to the others.

| Approach | Cost | Limit |
|---|---|---|
| Claude API keys | pay per token | none, but expensive |
| One Claude Max plan | flat monthly | one usage window |
| **This proxy (N plans)** | flat monthly × N | **N usage windows, pooled** |

## Prerequisites

- [Bun](https://bun.sh) ≥ 1.1
- [Claude Code CLI](https://docs.claude.com/en/docs/claude-code): `npm install -g @anthropic-ai/claude-code`
- One or more Claude Max / Team subscriptions to log into

## Install

```bash
git clone <this-repo> claude-max-pool && cd claude-max-pool
bun install
```

## Add accounts

Each account is an isolated Claude login stored under `~/.claude-max-pool/accounts/<name>/` (used as that account's `CLAUDE_CONFIG_DIR`).

```bash
# Interactive login for a new account (runs the Claude CLI; do /login, then /exit)
bro accounts login work
bro accounts login personal

# Or copy the machine's existing `claude` login into the pool as one account
bro accounts import primary

# See status of every account
bro accounts list

# Remove one
bro accounts remove work
```

If you are running the pool standalone from this directory, the equivalent
direct form is `bun run src/index.ts accounts <command> [name]`.

`accounts list` shows plan type, rate-limit tier, token validity, and rolling usage per account.

## Run the server

```bash
bun start                      # http://127.0.0.1:3456
# or: bun run src/index.ts serve --port 8080
```

Open **http://127.0.0.1:3456/** for the live dashboard — one card per account with auth state, plan, rate tier, token expiry, rolling-window usage bars, and rate-limit cooldowns. It refreshes every few seconds.

## Use it

### OpenAI-compatible

```bash
curl http://127.0.0.1:3456/v1/chat/completions \
  -H "content-type: application/json" \
  -d '{"model":"sonnet","messages":[{"role":"user","content":"Hello!"}]}'
```

Point any OpenAI client at `http://127.0.0.1:3456/v1` with any (or no) API key — unless you set `PROXY_API_KEY`, in which case send it as the bearer token.

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:3456/v1", api_key="unused")
client.chat.completions.create(model="sonnet", messages=[{"role":"user","content":"hi"}])
```

### Anthropic-compatible

```bash
curl http://127.0.0.1:3456/v1/messages \
  -H "content-type: application/json" \
  -d '{"model":"claude-sonnet-5","max_tokens":1024,"messages":[{"role":"user","content":"Hello!"}]}'
```

Both endpoints support streaming (`"stream": true`). The default `/v1/messages` backend forwards the Anthropic request body verbatim, including `system`, `tools`, `thinking`, beta features, and `stream`. It also preserves the caller/harness Anthropic headers instead of inventing defaults; the only upstream header substitution is `Authorization: Bearer <selected account token>`, with hop-by-hop headers and local `x-api-key` proxy auth stripped. Use `CLAUDE_POOL_BACKEND=cli` if you need the old CLI alias-mapping behavior.

### Endpoints

| Method | Path | Purpose |
|---|---|---|
| GET | `/` | Live status dashboard |
| GET | `/health` | Liveness + pool summary |
| GET | `/api/status` | JSON status of all accounts (the dashboard polls this) |
| GET | `/v1/models` | OpenAI-style model list |
| POST | `/v1/chat/completions` | OpenAI Chat Completions (stream + non-stream) |
| POST | `/v1/messages` | Anthropic Messages (stream + non-stream) |

## Routing & usage

- **Selection:** each request goes to the authenticated, non-rate-limited account with the fewest requests in the current window (round-robin on ties). Pass an OpenAI `user` field or Anthropic `metadata.user_id` to keep a conversation pinned to one account.
- **Graceful failover:** if the account serving a request has its usage/rate limit run out **before any output has streamed** (e.g. a Claude plan hits its cap), the proxy transparently sidelines that account and retries the same request on the next available one. For direct Anthropic streaming, the proxy buffers only the first SSE event long enough to detect an initial rate-limit error; once real SSE output starts, bytes are passed through unchanged and the proxy is committed to that account. Failovers are logged (disable with `LOG_FAILOVER=0`).
- **Rate limits:** a sidelined account stays out for a cooldown (`RATE_LIMIT_COOLDOWN_MS`, default 1h); the proxy tries to recover a reset time from the error and reroutes traffic in the meantime.
- **Usage tracking:** requests, tokens, and cost are counted per account over a rolling window (`USAGE_WINDOW_MS`, default 5h) and persisted to `<pool>/usage.json`. Anthropic doesn't publish exact quota numbers over the API, so these are *observed* usage plus the plan's `rateLimitTier` label — enough to see which plan to lean on next.
- **OAuth refresh:** the direct backend refreshes expired account access tokens with each account's stored `refreshToken` and writes rotated tokens back to `.credentials.json`.

## Configuration

All optional — see [`.env.example`](./.env.example). Key vars: `CLAUDE_POOL_DIR`, `CLAUDE_POOL_BACKEND` (`oauth` default, `cli` fallback), `ANTHROPIC_API_BASE_URL`, `CLAUDE_BIN`, `HOST`, `PORT`, `PROXY_API_KEY`, `REQUEST_TIMEOUT_MS`, `USAGE_WINDOW_MS`, `RATE_LIMIT_COOLDOWN_MS`.

## Notes & limitations

- The default `/v1/messages` backend is a direct reverse proxy. It preserves Anthropic request fields and harness headers, swaps only upstream authorization to the selected account token, and streams upstream SSE bytes back unchanged after the initial failover check.
- The OpenAI compatibility endpoint and `CLAUDE_POOL_BACKEND=cli` fallback use the older adapter path, which flattens chat history into one CLI prompt and supports text responses only.
- This uses Claude Code OAuth credentials from your subscription login. Review Anthropic's terms for your plan before pooling multiple accounts for shared/automated use.

## Layout

```
src/
  index.ts            entry — dispatches `serve` vs `accounts`
  config.ts           env-resolved configuration
  cli.ts              account-management commands
  accounts/           AccountManager: discovery, auth status, usage, routing
  upstream/           direct Anthropic OAuth reverse proxy
  subprocess/         spawns the Claude CLI, normalizes its JSON stream
  adapters/           OpenAI + Anthropic request/response translation
  server/             Bun.serve HTTP routing + status dashboard
```

See [ARCHITECTURE.md](./ARCHITECTURE.md) for the request lifecycle.

## License

MIT
