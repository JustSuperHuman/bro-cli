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

**Failover:** when the serving account's usage/rate limit runs out before any output has streamed, the pool transparently sidelines it and retries the turn on the next account — you just keep going. Set `CLAUDE_POOL_BACKEND=cli` to use the older subprocess backend. Requires Bun (`bro` finds it automatically; install from [bun.sh](https://bun.sh)). See [`pool/README.md`](./pool/README.md) for the pool's own docs, endpoints, and configuration.

Claude is next in the list and runs **natively** (your normal Claude login — no proxy). Other Anthropic-compatible providers (OpenRouter, Z.ai) just point Claude at their endpoint. OpenAI-format providers (Sakana, OpenAI, DeepSeek, Groq, …) are routed through [`claude-code-router`](https://github.com/musistudio/claude-code-router), which `bro` installs for you the first time you need it.

### Flags

```sh
bro -p pool               # Multiple Claude Account Proxy (pool many plans)
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
