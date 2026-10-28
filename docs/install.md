# Installing and updating bro

## Install

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/JustSuperHuman/bro-cli/main/install.ps1 | iex
```

Or download `bro-<version>-x86_64-pc-windows-msvc-setup.exe` from the
[latest release](https://github.com/JustSuperHuman/bro-cli/releases/latest) and run it. It installs bro for
your user only (no admin prompt), adds **bro** to the Start menu (it opens the desktop app), can put bro on your
`PATH` and a shortcut on your desktop, can start bro in the background when you sign in (a startup app for your
user), and is listed in Settings → Apps for uninstalling (which stops the session server and removes bro from your
startup apps). Run a newer one to
update, even while bro is open.

**macOS**: download the `.dmg` for your Mac from the
[latest release](https://github.com/JustSuperHuman/bro-cli/releases/latest):
`aarch64-apple-darwin` for Apple Silicon, or `x86_64-apple-darwin` for Intel.
Open it, drag **Bro.app** to **Applications**, then open Bro from Applications.
The app is ad-hoc signed but is not Apple-notarized. If macOS blocks it, open
System Settings > Privacy & Security and choose **Open Anyway** after trying to open Bro.
Install a newer DMG by replacing Bro.app in Applications.

**macOS / Linux, from a terminal**:

```sh
curl -fsSL https://raw.githubusercontent.com/JustSuperHuman/bro-cli/main/install.sh | sh
```

The scripts download the latest release for your platform, check its SHA-256 and install `bro` and `bro-cli` to
`%LOCALAPPDATA%\Programs\bro` (added to your user `PATH`) or `~/.local/bin`. Set `BRO_VERSION` (e.g. `v2.1.0`)
to pin a release or `BRO_INSTALL_DIR` to change the location. On a fresh install they then list the Quick Install
tools and ask which to install (Enter: the recommended ones that are missing). Set `BRO_TOOLS` to skip the
question: e.g. `justtools git node claude`, `recommended`, `all` or `none`. They also ask whether bro should start in the
background when you sign in, for your user only: a startup app on Windows, a LaunchAgent on macOS, a systemd user
unit on Linux (Enter: no; bro still keeps your sessions running while its window is closed). Set `BRO_STARTUP=yes` or `no` to skip that question; Settings → Session server → Start at
sign-in and `bro host install` / `bro host uninstall` change it later.

What gets installed:

| | |
|---|---|
| `bro` | opens the desktop app (over SSH, in tmux or with no display, the terminal UI; a bro already running in tmux is attached to, so `ssh box -t bro` picks up your sessions); every subcommand (`bro install`, `bro update`, `bro ctl`, `bro open`…) |
| `bro-cli` | the terminal UI (`bro --tui`) |
| `bro-app` (Windows) | the Start menu entry: the desktop app without a console window |

## Update

bro looks for a new release when it opens and every 12 hours, and tells you once when there is one.
Settings → Updates → bro version installs it (or checks right away); `bro update` does the same from a terminal
and `bro update --check` only looks. The download is checked against the release's `SHA256SUMS.txt`, it works
while bro is running, and the new version runs after a restart. Turn "Check for updates" off in settings to stop
looking.

Running the install script or the setup again also updates, in place, and skips the Quick Install question.

## By hand

1. Download the archive for your system from the [latest release](https://github.com/JustSuperHuman/bro-cli/releases/latest).
2. Unpack it and put `bro` and `bro-cli` (on Windows also `bro-app.exe`) somewhere on your `PATH`.
3. Run `bro`.

## Build from source

Needs a recent stable [Rust](https://rustup.rs). On Linux also install `libasound2-dev` and `pkg-config`.

```
git clone https://github.com/JustSuperHuman/bro-cli.git
cd bro-cli
cargo install --path crates/bro-tui
```

`cargo install` puts `bro` on your PATH; `bro --tui` is the terminal UI (the first run of `bro` writes the
`bro-cli` launcher next to it when it can).

## Coming from v1

bro v2 reads the same data as v1 (`~/.bro`, `~/.claude-max-pool`, `~/.bro/codex-profiles`), so your profiles and
providers carry over.

## The code

| Crate | What it is |
|---|---|
| `crates/bro-tui` | the `bro` binary: panes, sidebar, launcher, views, and the desktop app (`bro gui`) |
| `crates/bro-gfx` | the desktop app's GPU drawing: shapes, text, terminal cells, images |
| `crates/bro-media` | picture and video decoding for the desktop app's media panel |
| `crates/bro-core` | profiles, providers, sessions, usage |
| `crates/bro-proxy` | Anthropic ↔ OpenAI translating proxy |
| `crates/bro-bridge` | bridge host for the mobile and web clients |
| `crates/bro-voice` | push-to-talk voice input |

See [PLAN.md](../PLAN.md) for the full product plan.
