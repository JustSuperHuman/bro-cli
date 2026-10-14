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
On a fresh install it then lists the Quick Install tools and asks which to install (Enter: the recommended ones that
are missing). Set `BRO_TOOLS` to skip the question: e.g. `justtools git node claude`, `recommended`,
`all` or `none`.

Rerun it to update: with bro already installed it updates that copy in place, even while it's
running, and skips the Quick Install question.

bro also looks for a new release when it opens and every 12 hours, and tells you once when there is
one. Settings → updates → bro version installs it (or checks right away); `bro update` does the same
from a terminal and `bro update --check` only looks. Either way the download is checked against the
release's `SHA256SUMS.txt` and the new version runs after a restart. Turn "Check for updates" off in
settings to stop looking.

### Quick Install

```
bro install                        # list everything with what's installed; pick what to install
bro install justtools git node bun # install those (already installed ones are skipped)
bro install recommended            # JustTools, Git, Node.js, Bun, Claude Code, Codex
bro install agents                 # claude codex pi omp
bro install dotnet9 dotnet10 dotnet11
bro install codex --force          # reinstall / update one
```

Tools: `justtools`, `justwatch` (Windows), `justimagine`, `git`, `gh`, `node`, `bun`, `python`, `uv`, `dotnet9`, `dotnet10`, `dotnet11`,
`claude`, `codex`, `pi`, `omp`, `rg`, `ffmpeg`. Windows installs with WinGet, macOS with Homebrew
(installed first if missing), Linux with apt, dnf, pacman, zypper or apk. Bun, uv and .NET on
macOS / Linux (into `~/.dotnet`) use their official install scripts; JustTools, Claude Code, Codex,
Pi, omp and JustWatch use their own installers.

JustWatch keeps your local dev servers running from the Windows tray. Installing it also turns on
its `justwatch` skill, so new Claude and Codex sessions can check, restart and tail those services
through its API on `localhost:6767`. Switch the skill off or on in settings → integrations.

JustImagine is your own image, video, song and voice studio. `bro install justimagine` clones
[its `main` branch](https://github.com/JustGains/JustImagine) into `%LOCALAPPDATA%\Programs\JustImagine`
(`~/.local/share/justimagine` on macOS and Linux), runs `npm install` and registers it with its own
`service install`: a Task Scheduler task on Windows, a launchd agent on macOS, a systemd user unit on
Linux. It starts at login on port 8793. If an earlier install recorded a gallery folder and port, it
keeps them. It needs Git and Node.js, so install those first. JustImagine has no releases, so bro
compares the checkout with GitHub when it opens and every 12 hours, and says when `main` has new
commits. Enter on settings → integrations → JustImagine, or `bro install justimagine --force`, pulls
them, reinstalls dependencies and restarts the service.

## Env files

```
bro --export-env                     # your variables → ./bro.env, encrypted (asks for a passphrase)
bro --env bro.env                    # open bro with them; every session it starts inherits them
bro --env https://m.justgains.com    # a folder URL loads bro.env from there (then bro.env.enc)
bro --env bro.env --save             # keep them for every new program and service
bro env encrypt keys.env             # keys.env → keys.env.enc
bro env list bro.env                 # names and lengths, values hidden
bro env decrypt bro.env -o keys.env
```

An env file is plain `KEY=value` lines (`export`, quotes and `#` comments work), so one file can
carry `OPENROUTER_API_KEY`, `ELEVENLABS_API_KEY` and whatever else a new machine needs. Encrypted
files use Argon2id and XChaCha20-Poly1305 and ask for their passphrase when loaded, or read it from
`BRO_ENV_PASSWORD`. URLs must be https; plain http only works for localhost. bro ignores PATH, HOME
and the other variables the OS sets.

`--export-env` reads your user environment variables on Windows (`HKCU\Environment`) and the
current shell's environment on macOS and Linux. It leaves out OS, login and terminal variables
unless you pass `--all`. `--plain` skips the encryption, `-` as the file name prints it, and
`--force` replaces an existing file.

`--save` writes to `HKCU\Environment` on Windows. On macOS and Linux it writes
`~/.config/bro/env` (readable only by you), sourced from `~/.profile`, `~/.bashrc` and `~/.zshenv`,
plus fish's `conf.d`. It also runs `launchctl setenv` on macOS, and on Linux it writes systemd's
`environment.d` and imports the variables into the running user manager. New terminals and services
get the variables; ones already running keep what they had. If JustImagine is installed, its service
restarts to pick up the keys.

In bro, Quick Install is a checklist: settings → integrations → Quick install (or Enter on JustTools
when it isn't installed), the palette, or `prefix I`. Tick tools with space and press `i`: JustTools
installs in the background, the rest run in a new tab so you can follow them and answer prompts.
Picking an agent that isn't installed in the new-session picker installs it the same way.

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
