# bro

Run [Claude Code](https://claude.com/claude-code) against **any** model — Claude natively, or any OpenAI/Anthropic-compatible API through a proxy that installs itself.

Pick a provider, pick a model, go.

## Install

```sh
npm install -g bro-claude
# or: bun install -g bro-claude
```

You also need the `claude` CLI installed (that's the thing `bro` launches).

## Use

```sh
bro
```

1. Scroll to a **provider** and press enter.
2. Scroll to a **model** and press enter. Press **Tab** to flip the **Skip permissions** toggle (`--dangerously-skip-permissions`) on/off right there.
3. First time on a paid provider it asks for an API key and saves it.

Your last provider + model are remembered and pre-selected next time (per provider).

## Multiple Claude Account Proxy

The **top** option in the menu (`bro -p pool`) pools any number of Claude Max / Team logins behind one local endpoint and launches Claude Code across all of them — so a single session draws from several plans and **fails over automatically** the moment one runs out of usage.

Pick it and `bro` handles everything:

1. **Setup** — if you have no pooled accounts yet, it offers to log in a new one (opens Claude to sign in) or import the login already on this machine. Add as many as you like; each is stored in its own isolated config dir under `~/.claude-max-pool/`.
2. **Start the proxy** — launches the pool server (in `pool/`, runs on [Bun](https://bun.sh)) in the background and waits for it to go healthy. A live dashboard shows each account's auth state, plan, rate tier, and rolling usage at `http://127.0.0.1:3456/`.
3. **Launch Claude** — starts Claude Code pointed at the pool (`ANTHROPIC_BASE_URL`). The pool forwards Claude's Anthropic `/v1/messages` calls directly to Anthropic with the least-loaded account's OAuth token by default, without nesting another `claude --print` subprocess. When Claude exits, the proxy is stopped.

Manage pool accounts directly through `bro`:

```sh
bro accounts login work       # add/log in a new pooled Claude account
bro accounts import primary   # copy this machine's current Claude login
bro accounts list             # show account status and usage
bro accounts remove work      # delete a pooled account
```

Use one logged-in account directly without the pool:

```sh
bro account                   # pick a Claude account profile from a menu
bro account work              # launch Claude using the "work" profile
bro --account personal        # same shortcut, explicit flag form
```

Profiles are the same standard Claude Code logins stored under
`~/.claude-max-pool/accounts/<name>/`; `bro` switches by setting
`CLAUDE_CONFIG_DIR` for that Claude launch and does not overwrite `~/.claude`.
The interactive profile menu shows each account's current five-hour, weekly,
and Fable usage before you choose one.

**Failover:** when the serving account's usage/rate limit runs out before any output has streamed, the pool transparently sidelines it and retries the turn on the next account — you just keep going. Set `CLAUDE_POOL_BACKEND=cli` to use the older subprocess backend. Requires Bun (`bro` finds it automatically; install from [bun.sh](https://bun.sh)). See [`pool/README.md`](./pool/README.md) for the pool's own docs, endpoints, and configuration.

## Codex (ChatGPT subscription)

`bro -p codex` (pinned in the menu) runs **Claude Code itself on your ChatGPT subscription** — the GPT‑5.x Codex models, driving Claude Code's normal harness (tools, agentic loop, streaming). No `codex` CLI, no extra packages, no API key: just your ChatGPT login.

How it works:

1. **Login** — a built-in ChatGPT OAuth sign-in (the same flow the Codex CLI uses) opens in your browser and stores credentials at `~/.bro/codex-auth.json`. If you already have the Codex CLI logged in, that login is reused automatically. Tokens are refreshed on their own as they expire.
2. **Models** — the list is fetched live from your subscription, so it always matches what you can actually run (GPT‑5.6‑Sol, GPT‑5.5, Codex‑Spark, …). Falls back to a cache, then a small built-in list, when offline.
3. **Bridge** — `bro` starts a tiny local Anthropic-compatible server that translates Claude Code's `/v1/messages` calls into OpenAI Responses-API calls against the ChatGPT Codex backend, and streams the answers back (tool calls, thinking, and usage all mapped through). It's pure Node — nothing to install.
4. **Launch** — Claude Code runs pointed at the bridge (`ANTHROPIC_BASE_URL`). Pick a model in the usual menu (Tab toggles skip-permissions); `-m <model>` skips it. When Claude exits, the bridge is torn down.

```sh
bro -p codex              # pick a GPT-5.x model, launch Claude Code on it
bro -p codex -m gpt-5.5   # skip the menu
bro -p codex --omp        # use the omp harness instead of Claude Code
bro codex login           # log in / switch ChatGPT account
bro codex status          # show login + plan
bro codex logout          # remove stored credentials
```

Add `:effort` to a model to set reasoning depth, e.g. `bro -p codex -m gpt-5.6-sol:high`. This impersonates a Codex client to a subscription backend, which is outside OpenAI's normal API terms — use it on your own account at your own discretion.

## 🎨 Image Gen

`bro image` (also the second option in the menu) doesn't launch Claude at all — it asks which image API to use (Yunwu with `gpt-image-2` first, plus OpenAI), then serves a local web UI and opens it in your browser.

- **Prompt fast** — type, press Enter, keep typing. Every generation is a card that shimmers while it works and fades the image in when it lands.
- **Concurrent by design** — the batch stepper fires N generations at once, and you can keep firing more while others are still running.
- **Switch models in the UI** — pick from the API's list (including chat-routed models like `gemini-3.1-flash-image`) or type any custom model id. Size and quality knobs included where the API supports them.
- **Reference images** — paste, drag-drop, or attach images to the prompt as context. They're saved to `./.bro/context/` named by content hash (the same image is never stored twice) and appear in a library strip for one-click reuse. Image-API models route through `/images/edits`; chat-routed models get them as vision input.
- **Files land in `./.bro/image-gen/`** of the directory you launched from, with a `history.jsonl` so the gallery survives reloads.

```sh
bro image             # pick an image API, then the web UI opens
bro image -p yunwu    # skip the API menu
```

Keys are shared with the chat provider of the same id, so a saved Yunwu key just works. Add your own APIs via `imageApis` in `~/.bro/config.json` (merged by `id`, same as providers).

### Image Gen HTTP API

The image-gen web UI is backed by local JSON routes, and scripts can call the same routes directly while `bro image` is running. Start the server, copy the printed `http://127.0.0.1:<port>` URL, then call `/api/generate`:

```sh
curl -s http://127.0.0.1:8790/api/generate \
  -H "content-type: application/json" \
  -d '{"prompt":"a clean product photo of a steel water bottle","model":"gpt-image-2","size":"1024x1024","quality":"high"}'
```

The response includes the saved file name and metadata; download the image from `/images/<file>`. Reference images use the same flow as the UI: upload a base64 data URL to `/api/context`, then pass the returned context file names as `images` in `/api/generate`. See [docs/image-api.md](./docs/image-api.md) for the complete route list and examples.

## Providers

Claude is next in the list and runs **natively** (your normal Claude login — no proxy). Other Anthropic-compatible providers (OpenRouter, Z.ai) just point Claude at their endpoint. OpenAI-format providers (Sakana, OpenAI, DeepSeek, Groq, …) are routed through [`claude-code-router`](https://github.com/musistudio/claude-code-router), which `bro` installs for you the first time you need it.

### Flags

```sh
bro -p pool               # Multiple Claude Account Proxy (pool many plans)
bro account work          # launch Claude using one logged-in account profile
bro -p codex              # Codex on your ChatGPT subscription (live model list)
bro -p sakana -m fugu     # skip the menus
bro --list                # list every provider + model
bro update                # refresh the model list from GitHub, cache it locally
bro --dry-run             # show what would run, launch nothing
bro --safe                # don't pass --dangerously-skip-permissions
bro --resume <session-id> # pick provider/model, then resume Claude there
bro -p pool --resume <id> # resume through the Multiple Claude Account Proxy
bro -- --help             # force a bro flag name through to claude
```

Put `bro`'s own flags first. The first unrecognized argument, and everything
after it, is passed verbatim to the Claude session after provider/model
selection.

## Config

Keys and your own providers/models live in `~/.bro/config.json`:

```jsonc
{
  "keys": {
    "sakana": "fish_...",
    "#openai": "sk-...   ← any key starting with # is ignored (notes / test data)"
  },
  "providers": [
    {
      "id": "mylocal",
      "name": "My Local LLM",
      "mode": "openai",
      "baseUrl": "http://localhost:1234/v1/chat/completions",
      "noKey": true,
      "models": [{ "id": "my-model", "name": "My Model" }]
    }
  ]
}
```

Custom providers merge with the built-in list (same `id` adds models; new `id` adds a provider). The built-in model list is pulled from [`models.json`](https://github.com/JustSuperHuman/bro-cli/blob/main/models.json) on GitHub and cached at `~/.bro/models.cache.json` — run `bro update` to refresh it (override the source with `BRO_MODELS_URL`).

---

Made by [JustGains](https://justgains.com) · MIT
