# bro

One terminal workspace for all your coding agents — Claude Code, Codex, Pi and omp — side by side.

> v2 is a full rewrite in Rust. The old Node version lives on at the [`v0.8.0`](https://github.com/JustSuperHuman/bro-cli/tree/v0.8.0) tag.

## What it does

- **Runs your agents in panes** — split, zoom, and switch between live sessions, grouped by project.
- **Shows who needs you** — each agent is marked working, blocked, done or idle.
- **Switches accounts** — multiple Claude and Codex profiles, with usage meters for each.
- **Resumes anything** — pick up a past session, or fork it into another profile.
- **Mixes models** — a built-in proxy lets Claude Code run on OpenAI-format models, and Codex / Pi / omp run on Claude.
- **Reaches your phone** — a built-in bridge lets the Just Terminal mobile and web clients connect.

## Install

1. Download the archive for your system from the [latest release](https://github.com/JustSuperHuman/bro-cli/releases/latest).
2. Unpack it and put `bro` somewhere on your `PATH`.
3. Run it:

```
bro
```

Press **F1** inside bro to see every key binding.

On Windows the archive also contains `bro-app.exe`, which opens bro in its own window — pin it to the Start menu or taskbar.

You need at least one agent installed (Claude Code, Codex, Pi or omp); bro launches the ones it finds.

## Build from source

Needs a recent stable [Rust](https://rustup.rs). On Linux also install `libasound2-dev` and `pkg-config`.

```
git clone https://github.com/JustSuperHuman/bro-cli.git
cd bro-cli
cargo install --path crates/bro-tui
```

## Coming from v1

bro v2 reads the same data as v1 (`~/.bro`, `~/.claude-max-pool`, `~/.bro/codex-profiles`), so your profiles and providers carry over.

## Layout

| Crate | What it is |
|---|---|
| `crates/bro-tui` | the `bro` binary: panes, sidebar, launcher, views |
| `crates/bro-core` | profiles, providers, sessions, usage |
| `crates/bro-proxy` | Anthropic ↔ OpenAI translating proxy |
| `crates/bro-bridge` | bridge host for the mobile and web clients |
| `crates/bro-voice` | push-to-talk voice input |

See [PLAN.md](PLAN.md) for the full product plan.

## License

MIT
