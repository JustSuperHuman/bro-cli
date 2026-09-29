# bro v2 — agentic workspace

One Rust binary (`bro`) that merges:

| Source | What we take |
|---|---|
| **z4-oriel** (Rust, ratatui + vt100 + portable-pty) | The core: event loop (deadline sleep + 12 ms burst coalescing), `Pane` trait / `Action` pattern, split-tree `layout.rs`, `term.rs` PTY pane with full xterm key/mouse encoding, selection→clipboard, themes, alerts/toasts, agent screen-scrape status (working / blocked / done / idle). |
| **bro-cli** (Node) | Profiles (Claude `CLAUDE_CONFIG_DIR` accounts, Codex `CODEX_HOME` profiles), providers + `models.json`, harness launchers (Claude Code / Codex / Pi / omp), session discovery + cross-profile resume/fork, usage meters (OAuth usage endpoint, `codex app-server` rate limits, measured 5h↔week ratio, headroom). |
| **terminal** rust-bridge (axum) | The bridge connector: `/ws` + `/api/*` with the same wire protocol, token auth and multi-interface URLs, so the existing Just Terminal mobile app + web client connect unchanged. |

Dropped (not agentic): music, system monitor, storage, calendar, notes, files app, JustImagine, ChatJimmy, DSH.

## Crates (cargo workspace)

```
crates/
  bro-core    config/state (~/.bro compatible), profiles, providers, sessions, usage
  bro-proxy   Anthropic Messages <-> OpenAI (Chat Completions + Responses) translating proxy
  bro-bridge  axum host: /ws, /api/sessions…, token auth, projects, prompt detection, notify
  bro-tui     the app: event loop, panes, sidebar, launcher, views
```

## Proxy (new, native — replaces ccr + codex-bridge.js)

- `POST /v1/messages` → OpenAI Chat Completions or Responses upstream (incl. ChatGPT Codex backend via Codex OAuth). Lets **Claude Code** run on any OpenAI-format model.
- `POST /v1/chat/completions`, `POST /v1/responses` → Anthropic Messages upstream (API key or Claude OAuth profile). Lets **Codex / Pi / omp** run on Claude.
- Full fidelity: system, text, images, tools/tool_choice, tool results, thinking/reasoning, streaming SSE state machines, usage, stop reasons, errors, count_tokens, /v1/models.
- Golden-fixture tests for each direction.

## UI

```
┌ bro ─────────────────────┬──────────────────────────────────────────────┐
│ ▾ bro-cli-v2     F:\…    │ claude · work · opus               ◐ working │
│   ◐ claude  work  opus   │                                              │
│   ● codex   personal     │   (live agent terminal — splits, zoom)       │
│   ↺ 3 past sessions      │                                              │
│ ▸ justgains      2 live  │                                              │
│ ▸ terminal               │                                              │
│ ─ usage ───────────────  │                                              │
│ work   5h ▰▰▰▱▱ 58% wk 31%│                                              │
│ codex  5h ▰▱▱▱▱ 12%      │                                              │
│ ─ bridge ● :10001  3 dev │                                              │
└──────────────────────────┴──────────────────────────────────────────────┘
```

- Sidebar grouped by project (git root, else cwd), live agent tabs with status dots, collapsible past sessions to resume (any profile → fork into another).
- Launcher palette: harness × profile/provider × model × project; recent combos first.
- Views: Usage, Profiles, Proxy (routes + live request log), Bridge (URLs, token, QR pairing, connected clients).
- 100 % keyboard navigable; every action in the command palette; F1 help overlay lists live bindings.

## Phases

1. Workspace scaffold, port oriel core (term/layout/loop/theme/ui/clip/alerts), strip non-agent apps.
2. bro-core: config/state/profiles/providers/sessions/usage (reads existing ~/.bro, ~/.claude-max-pool, ~/.codex data).
3. Sidebar project grouping, launcher, resume/fork, harness launching.
4. bro-proxy + fixtures.
5. bro-bridge port wired to our own PTYs (+ fixes: refresh-host alias, BEL/OSC 9/777 notifications, QR pairing).
6. Polish: themes, help, toasts, headless snapshot tests, installer.
