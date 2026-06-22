# bro

Run [Claude Code](https://claude.com/claude-code) against **any** model — Claude natively, or any OpenAI/Anthropic-compatible API through a proxy that installs itself.

Pick a provider, pick a model, go.

## Install

```sh
npm install -g bro-cli
# or: bun install -g bro-cli
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

Claude is first in the list and runs **natively** (your normal Claude login — no proxy). Other Anthropic-compatible providers (OpenRouter, Z.ai) just point Claude at their endpoint. OpenAI-format providers (Sakana, OpenAI, DeepSeek, Groq, …) are routed through [`claude-code-router`](https://github.com/musistudio/claude-code-router), which `bro` installs for you the first time you need it.

### Flags

```sh
bro -p sakana -m fugu     # skip the menus
bro --list                # list every provider + model
bro --dry-run             # show what would run, launch nothing
bro --safe                # don't pass --dangerously-skip-permissions
bro -- --resume           # everything after -- is passed to claude
```

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

Custom providers merge with the built-in list (same `id` adds models; new `id` adds a provider). The model list itself is served from `https://m.justgains.com/models.json` (override with `BRO_MODELS_URL`).

---

Made by [JustGains](https://justgains.com) · MIT
