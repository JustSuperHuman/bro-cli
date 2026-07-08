# Make the account pool the backend for *all* Claude Code sessions (incl. agents)

Date: 2026-07-08
Status: approved (Approach A)

## Problem

`bro -p pool` starts the local pool server and launches **one** foreground `claude`
process with `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` set in that process's
environment. Two consequences:

1. Only that one process (and its direct children) is pointed at the pool.
   Sessions started from Claude Code's **agents view** / background-agents feature
   are not children of that process, so they fall back to the user's normal Claude
   login instead of routing through the pool.
2. The pool server is killed the moment the foreground `claude` exits
   (`stopProxy()` in `runPool`'s `finally`), so nothing survives for a later agent
   to talk to anyway.

**Goal:** every Claude Code session on the machine — foreground, new windows, and
background agents — routes through the account pool, until the user explicitly
turns it off.

## Confirmed Claude Code behavior (from docs)

- Background agents honor the `env` block in `settings.json`
  (`~/.claude/settings.json` user scope), same as foreground sessions.
- Precedence: shell env var > CLI flag > `/model` > `settings.json` `env` >
  `settings.json` config.
- **No fallback:** if `ANTHROPIC_BASE_URL` is unreachable, sessions fail — they do
  *not* silently fall back to Anthropic. So a stale pool URL with a dead server
  bricks `claude`. Lifecycle + self-heal are mandatory, not optional.
- A non-first-party base URL (`127.0.0.1`) disables MCP tool search and Remote
  Control for those sessions. Accepted tradeoff.

## Approach A (chosen)

Make the pool a **persistent, global backend**:

1. Run the pool server **detached** so it survives the launching terminal and the
   foreground `claude` exiting.
2. Write `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN` into the user-global
   `~/.claude/settings.json` `env` block, so every session (agents included) reads
   it.
3. Explicit teardown (`bro pool down`) stops the server and restores settings.
4. Self-heal: on any `bro` run, if the pool env is active in settings but the
   server is not healthy, restore settings so `claude` is never bricked.

## Components

### `src/settings.js` (new)

Surgical, reversible edits to `~/.claude/settings.json`. Pure/testable (path
overridable for tests).

- `applyPoolEnv({ baseUrl, token })` — snapshot the current `env.ANTHROPIC_BASE_URL`
  and `env.ANTHROPIC_AUTH_TOKEN` (present-or-absent) into a bro state file
  (`~/.bro/pool-settings.json`), then set them to the pool values. Preserves all
  other settings keys and 2-space formatting.
- `clearPoolEnv()` — restore the snapshotted prior values (delete the keys if they
  were absent before), remove the `env` object if it ends up empty, clear the state
  file. Idempotent.
- `isPoolEnvActive()` — cheap check (state file exists / env keys point at the pool
  base URL).

Restore is snapshot-based (not "delete our keys") so a user who already had their
own `ANTHROPIC_BASE_URL` gets it back exactly.

### `src/pool.js` (changes)

- `startProxy` → add `detached: true` (keep `unref()` + log file) so the server
  outlives bro; write `child.pid` to `~/.bro/pool-proxy.pid`.
- `poolUp()` — `ensureAccount`, start server if not already healthy, wait healthy,
  `applyPoolEnv`, print dashboard URL + "pool is now the backend for all Claude
  sessions; run `bro pool down` to stop". Does not launch claude.
- `poolDown()` — `clearPoolEnv`, kill server via pidfile (fallback: `lsof -ti:PORT`),
  remove pidfile, confirm.
- `poolStatus()` — health + whether pool env is active + account panel.
- `runPool` (menu / `bro -p pool` path) — becomes: self-heal check → `poolUp`
  behavior (ensure server + apply settings env) → flash status → launch foreground
  `claude`. On exit, **leave the server + settings up** and print a reminder that
  the pool is still the active backend and how to stop it. (No teardown in
  `finally` anymore.)
- `selfHealPoolEnv()` — if `isPoolEnvActive()` and not `healthy(port)`, `clearPoolEnv`
  and warn.

### `src/cli.js` (changes)

- Route `bro pool <up|down|status>` as a top-level command (mirrors
  `bro accounts …`), before provider resolution.
- Call `selfHealPoolEnv()` once early in `main()` (only does work if pool env is
  active — cheap otherwise).
- Update HELP.

### Docs

- README: document the new persistent/global behavior, `bro pool up|down|status`,
  the "all sessions route through the pool" semantics, and the dead-pool/self-heal
  safety note + the MCP-tool-search/Remote-Control caveat.

## Testing

- `src/settings.js`: unit tests against a temp settings file — apply then clear
  round-trips to the original bytes; apply over a pre-existing user
  `ANTHROPIC_BASE_URL` restores it exactly; clear is idempotent; other keys
  untouched.
- Manual/dry-run: `bro pool status` with server down; `bro -p pool --dry-run` still
  shows the intended launch; self-heal strips a stale env when server is absent.

## Out of scope (possible follow-ups)

- launchd/service-managed pool (Approach C) for an always-up global default.
- `bro -p pool --once` ephemeral mode (old self-contained behavior).
