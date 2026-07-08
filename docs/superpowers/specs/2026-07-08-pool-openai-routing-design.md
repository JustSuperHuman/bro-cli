# Pool: OpenAI (ChatGPT subscription) model routing — Design

**Date:** 2026-07-08
**Status:** Approved design, pending implementation plan

## Goal

Let a single Claude Code session, pointed at the pool (`ANTHROPIC_BASE_URL`),
use OpenAI models mid-session via Claude Code's `/model` custom-model entry —
e.g. plan on a Claude model, then switch to a GPT model for cheaper build work.
The pool routes those requests to OpenAI's Codex backend using **ChatGPT
subscription OAuth accounts** (the same auth the Codex CLI uses), and
load-balances/fails over across multiple such accounts exactly as it does for
Claude Max/Team accounts today.

**Out of scope (explicitly):** OpenAI platform API keys as an upstream. The
user only uses ChatGPT subscription auth. No API-key pool, no Chat Completions
upstream target.

## How it fits the existing architecture

Today `POST /v1/messages` unconditionally routes to `proxyAnthropicMessages`
(direct Anthropic OAuth upstream). The change: branch on `body.model` first.

```
POST /v1/messages
  │
  ├─ model in OpenAI model table? ──> OpenAI account pool
  │                                     upstream/openai-codex.ts
  │                                     (Anthropic Messages ⇄ Codex Responses API)
  └─ else ─────────────────────────> existing Claude account pool (unchanged)
```

Claude Code always speaks Anthropic Messages protocol to the pool; the new
upstream adapter translates request and (streamed) response between Anthropic
Messages and OpenAI's Responses API shape used by the Codex backend. This is
the same trick claude-code-router performs, done natively inside the pool so
one endpoint carries both providers with one dashboard and one failover story.

## Components

### 1. Account model: provider-tagged accounts

Each account directory under `~/.claude-max-pool/accounts/<name>/` gains a
`provider` marker (`"anthropic"` default, or `"openai"`). OpenAI accounts
store the Codex OAuth credential set (access token, refresh token, expiry,
account/plan info — the contents of Codex's `auth.json`) instead of Claude's
`.credentials.json`.

`AccountManager` becomes provider-aware:
- `pick(sessionKey, provider)` selects only from accounts of the requested
  provider.
- Token refresh dispatches per provider (Anthropic OAuth token endpoint vs.
  OpenAI's OAuth token endpoint, as used by the Codex CLI refresh flow).
- Usage counters, stickiness, cooldown, and persistence are shared logic.

### 2. Auth / account setup CLI

Same UX as today, extended with a provider flag:

```sh
bro accounts login work --provider openai   # ChatGPT OAuth browser flow
bro accounts list                            # one table, both providers
bro accounts remove work
```

The login flow implements the Codex OAuth (PKCE browser) flow directly —
the same flow the Codex CLI and third-party tools (Cline, OpenCode plugin)
use — and writes the credential set into the account dir. An `import` variant
can copy an existing `~/.codex/auth.json`.

**Note:** OpenAI positions subscription OAuth for individual interactive use;
pooling your own personal accounts is the same posture as the existing Claude
pool. Not for resale/multi-user service.

### 3. Upstream adapter: `pool/src/upstream/openai-codex.ts`

Parallel to `upstream/anthropic.ts`. Responsibilities:

- Ensure the account's Codex access token is fresh (refresh via OAuth refresh
  token, persist rotated tokens back to the account dir).
- Translate the incoming Anthropic Messages body → Codex Responses API
  request: system prompt, message history, tool definitions/results, streaming
  flag, model id (mapped through the model table).
- POST to the Codex backend endpoint with the account's bearer token.
- Translate the response back to Anthropic Messages shape:
  - Non-stream: full Responses object → Anthropic message JSON with usage.
  - Stream: Responses SSE events → Anthropic Messages SSE events
    (`message_start`, `content_block_delta`, tool_use blocks, `message_delta`
    with usage, `message_stop`).
- Translation fidelity requirements: text, tool use (Claude Code is
  tool-heavy — this must be lossless), stop reasons, token usage. Images/
  extended thinking are best-effort; unsupported features degrade gracefully
  rather than erroring.

### 4. Rate-limit snapshots and routing

Codex backend responses carry primary/secondary rate-limit window info
(used-percent + reset; a ~5-hour window and a weekly window — this is what
powers `codex /status`). This maps nearly 1:1 onto the existing
`RateLimitSnapshot` (5h / 7d unified windows), so:

- Parse those headers/fields on every Codex response into the same snapshot
  structure.
- Least-headroom routing and 429/limit-triggered cooldown + failover reuse the
  existing machinery (`failover.ts`, `markRateLimited`).
- Exact header/field names to be verified against a live Codex response during
  implementation (do not trust docs blindly).

### 5. Model table + `bro models update`

A model routing table replaces the hardcoded `MODELS` array as the source for
`/v1/models` and for the provider branch:

- `bro models update` queries each provider's upstream model list (the Codex
  backend's available models for OpenAI accounts; the static Claude list for
  Anthropic) and writes a cache (e.g. `~/.claude-max-pool/models.json`).
- Each entry: `{ id, provider, upstreamModel }` — `id` is what you type into
  Claude Code's `/model` custom entry (e.g. `gpt-5.6`), `upstreamModel` is the
  real model id sent upstream.
- The pool loads the cache at startup and on change; unknown model ids fall
  through to the Anthropic path (today's behavior).

### 6. Dashboard

`/api/status` and the dashboard list OpenAI accounts alongside Claude ones:
provider badge, auth state, plan, both rate-limit windows' utilization, rolling
usage. Failover log lines already name accounts; no change needed there.

## Error handling

- No available OpenAI account for an OpenAI-model request → 503 with an
  Anthropic-shaped `overloaded_error` (mirrors existing behavior).
- Codex token refresh failure → account marked unauthenticated, request fails
  over to another OpenAI account if any, else 503.
- Translation encountering an unsupported Anthropic feature → strip with a
  server log line, never a hard error mid-stream.
- Codex upstream 401/403 → mark account unauthenticated; 429/limit event →
  cooldown + failover (existing semantics).

## Testing

- Unit: request/response translation fixtures (Anthropic body ⇄ Codex
  Responses, stream event mapping, tool-use round-trips) — pure functions,
  same style as `upstream/anthropic.test.ts`.
- Unit: provider-aware `AccountManager.pick`, Codex token refresh persistence,
  rate-limit snapshot parsing.
- Integration (manual): live Codex account — verify a Claude Code session can
  `/model gpt-…`, stream a tool-using turn, and observe failover between two
  accounts by forcing a cooldown.

## Non-goals

- OpenAI platform API keys as an upstream.
- Other providers (Gemini, etc.) — though the provider-tagged account +
  upstream-adapter seam is the extension point if ever wanted.
- Replacing the ccr-based non-pool providers in bro's main menu.
