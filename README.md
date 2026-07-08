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
2. Scroll to a **model** and press enter. Press **Tab** to flip the **Skip permissions** toggle right there — it opts this launch into the dangerous `--dangerously-skip-permissions` bypass.
3. First time on a paid provider it asks for an API key and saves it.

Your last provider + model are remembered and pre-selected next time (per provider).

### Permission mode

By default `bro` starts Claude Code in **auto mode** (`--permission-mode auto`) —
Claude's intelligent auto-mode approves safe actions and only prompts when it
needs to. Set `permissionMode` in `~/.bro/config.json` to change the default:

- `"auto"` — auto-mode (the default)
- `"manual"` — prompt for everything (same as `bro --safe`)
- `"bypass"` — skip every permission check (`--dangerously-skip-permissions`)

The **Skip permissions** toggle in the menu opts a single launch into `bypass`.
(The older `dangerouslySkipPermissions: true/false` config key still works when
`permissionMode` is unset.)

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

**Failover:** when the serving account's usage/rate limit runs out before any output has streamed, the pool transparently sidelines it and retries the turn on the next account — you just keep going. Set `CLAUDE_POOL_BACKEND=cli` to use the older subprocess backend. Requires Bun (`bro` finds it automatically; install from [bun.sh](https://bun.sh)). See [`pool/README.md`](./pool/README.md) for the pool's own docs, endpoints, and configuration.

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

## Providers

Claude is next in the list and runs **natively** (your normal Claude login — no proxy). Other Anthropic-compatible providers (OpenRouter, Z.ai) just point Claude at their endpoint. OpenAI-format providers (Sakana, OpenAI, DeepSeek, Groq, …) are routed through [`claude-code-router`](https://github.com/musistudio/claude-code-router), which `bro` installs for you the first time you need it.

### Flags

```sh
bro -p pool               # Multiple Claude Account Proxy (pool many plans)
bro -p sakana -m fugu     # skip the menus
bro --list                # list every provider + model
bro update                # refresh the model list from GitHub, cache it locally
bro --dry-run             # show what would run, launch nothing
bro --safe                # start in manual mode (prompt for everything)
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
