# Architecture

## The core idea

The Claude Code CLI stores its OAuth credentials in a config directory chosen by the `CLAUDE_CONFIG_DIR` environment variable (default `~/.claude`). Point it at a different directory and it uses a different login. That single fact is what makes multi-account pooling possible: we give every Claude plan its own config directory, read its OAuth token, and route each request with the selected account's bearer token.

```
~/.claude-max-pool/
  accounts/
    work/       .credentials.json   ← CLAUDE_CONFIG_DIR for "work"
    personal/   .credentials.json   ← CLAUDE_CONFIG_DIR for "personal"
    team2/      .credentials.json   ← CLAUDE_CONFIG_DIR for "team2"
  usage.json                        ← persisted rolling usage counters
```

Each `.credentials.json` holds a `claudeAiOauth` block with `accessToken`, `refreshToken`, `expiresAt`, `scopes`, `subscriptionType` (e.g. `team`, `max`), and `rateLimitTier` (e.g. `default_claude_max_5x`). The direct backend reads that block for status, refreshes expiring access tokens with `refreshToken`, and persists rotated tokens back to the same file.

## Request lifecycle

```
HTTP request
   │
   │ 1. Server (server/server.ts) matches the route and checks PROXY_API_KEY.
   ▼
Route parse
   │ 2. For /v1/messages, keep the request body verbatim and read only
   │    metadata.user_id for stickiness.
   │    For /v1/chat/completions or CLAUDE_POOL_BACKEND=cli, use the legacy
   │    adapters that flatten messages into a single CLI prompt.
   ▼
AccountManager.pick(sessionKey)  (accounts/manager.ts)
   │ 3. Sticky by session if still available, else least-loaded authenticated,
   │    non-rate-limited account. Returns null if none available (→ 503).
   ▼
Direct upstream (upstream/anthropic.ts)
   │ 4. Ensure the account access token is fresh. If needed:
   │      POST https://platform.claude.com/v1/oauth/token
   │      grant_type=refresh_token
   │ 5. POST the original JSON body to:
   │      https://api.anthropic.com/v1/messages
   │    with the caller/harness request headers preserved. The only changes:
   │      Authorization is replaced with Bearer <account accessToken>
   │      hop-by-hop headers and local x-api-key proxy auth are stripped
   ▼
Instrument response
   │ 6. Non-stream: parse upstream JSON usage, then return the upstream body.
   │    Stream: parse SSE only as a tap for initial rate-limit failover and
   │    usage; forward upstream SSE bytes unchanged.
   ▼
Response
   │ 7. Return upstream status/body/stream with X-Pool-Account set.
   ▼
HTTP response  (X-Pool-Account header names the chosen account)
```

The direct backend intentionally does not synthesize Anthropic protocol headers
such as `anthropic-version` or `anthropic-beta`. Those come from the harness
request. Claude Code already sends the OAuth beta header it needs.

## Legacy CLI backend

Set `CLAUDE_POOL_BACKEND=cli` to use the previous `/v1/messages` path. That path calls `runClaude(prompt, { configDir, model, ... })`, spawns the CLI with `--print --output-format stream-json --verbose --include-partial-messages`, parses newline-delimited CLI JSON into normalized `TurnEvent`s, and re-serializes those events through the Anthropic adapter.

The OpenAI compatibility endpoint also still uses the legacy adapter path because it has to translate OpenAI chat messages into Anthropic/Claude text responses.

## Normalized `TurnEvent`

```ts
type TurnEvent =
  | { kind: "text"; text: string }              // assistant text delta
  | { kind: "text_block_boundary" }             // separator between text blocks
  | { kind: "tool_use"; id: string; name: string }
  | { kind: "done"; usage; stopReason; costUsd } // final result line
  | { kind: "error"; message; rateLimited; resetAt? };
```

This remains the seam for the CLI fallback and OpenAI compatibility path.

## Routing details

`AccountManager` keeps in-memory rolling usage per account (persisted to `usage.json`) and a `sessionKey → account` affinity map:

- **Availability** = authenticated **and** not in an active rate-limit cooldown. Expired access tokens still count as available because the direct backend refreshes them on use.
- **Selection** = the available account with the fewest requests in the current window; ties are broken round-robin so load spreads evenly.
- **Stickiness** = if a request carries a session key (OpenAI `user`, Anthropic `metadata.user_id`) and its prior account is still available, it stays there; otherwise it's rerouted and re-pinned.
- **Rate-limit handling** = the direct backend handles HTTP 429 and initial streaming `rate_limit_error` events before any SSE bytes are returned. The CLI fallback scans CLI errors/stderr for limit hints. `markRateLimited` sidelines the account and drops its affinities so sessions move away.

## Failure modes

- **No accounts / none available** → `503` with an OpenAI- or Anthropic-shaped error body.
- **Client disconnects** → the request's `AbortSignal` aborts the upstream fetch or subprocess.
- **Timeout** → `REQUEST_TIMEOUT_MS` aborts the direct upstream request/stream or subprocess.
- **CLI missing** → only affects `CLAUDE_POOL_BACKEND=cli` or OpenAI compatibility requests; a spawn error is surfaced as an `error` TurnEvent (→ `502`).
