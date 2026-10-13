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
- **Sets how Claude Code runs** — pick a preset (Quiet, Private, Lockdown, Incognito, Presenter, Marathon) in Settings (the ⚙ in the sidebar, `alt+,` or prefix `A`), next to theme, icons, auto-resume, recaps and compression, to control telemetry, updates, retries and more for every launch. See [docs/claude-env.md](docs/claude-env.md).
- **Caveman mode** — terse replies that keep every technical fact ([Caveman](https://github.com/JuliusBrussee/caveman) rules: caveman, ultracave or megacave) for new Claude, Codex and Pi sessions. Turn it on in Settings or the palette ("caveman"); ⛏ in the sidebar and on each session's header shows it. Nothing to install. See [docs/caveman.md](docs/caveman.md).

## Install

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/JustSuperHuman/bro-cli/main/install.ps1 | iex
```

**macOS / Linux**:

```sh
curl -fsSL https://raw.githubusercontent.com/JustSuperHuman/bro-cli/main/install.sh | sh
```

The installer downloads the latest release for your platform, verifies its SHA-256 checksum and
installs bro to `%LOCALAPPDATA%\Programs\bro` (added to your user `PATH`) or `~/.local/bin`.
Set `BRO_VERSION` (e.g. `v2.0.11`) to pin a release or `BRO_INSTALL_DIR` to change the location.
Rerun it to update.

Or install by hand:

1. Download the archive for your system from the [latest release](https://github.com/JustSuperHuman/bro-cli/releases/latest).
2. Unpack it and put `bro` somewhere on your `PATH`.
3. Run it:

```
bro
```

Press **F1** inside bro to see every key binding.

Choose **Manage profiles** in the new-session picker, or press **Alt+O**, to add
Claude or Codex profiles. Choose **+ Claude** or **+ Codex**, enter a name, then
**Create + log in**. Bro opens that profile's login tab; in Claude, run `/login`.
Select a profile and choose **Remove** to delete its local login, settings and
saved sessions after confirmation. Bro protects the machine's default logins
(`claude:local` and `codex:local`) from removal.

Alt+V pastes a clipboard image into the focused agent as a file path. On Windows,
Bro uses the original clipboard PNG when available; this avoids blank images
from a browser's bitmap copy. Clipboard contents stay unchanged.

On Windows the archive also contains `bro-app.exe`, which opens bro in its own window — pin it to the Start menu or taskbar.
In that window (with the Just Terminal fork) Settings also has a **Font** choice that switches live: bro bundles
JetBrains Mono, Fira Code, IBM Plex Mono, Maple Mono, Victor Mono, Monaspace Krypton and Radon, Space Mono,
Share Tech Mono, Comic Mono, Departure Mono and VT323, video-game-era Kode Mono, Doto, Monocraft (Minecraft),
IBM VGA 8x16 (DOS), Workbench (Amiga), Sixtyfour (C64) and Press Start 2P, plus the Nerd Font symbols for its icons —
all open source, licences in [crates/bro-tui/assets/fonts/licenses](crates/bro-tui/assets/fonts/licenses).

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
