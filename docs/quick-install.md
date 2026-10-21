# Quick Install

```
bro install                        # list everything with what's installed; pick what to install
bro install justtools git node bun # install those (already installed ones are skipped)
bro install recommended            # JustTools, Git, Node.js, Bun, Claude Code, Codex
bro install agents                 # claude codex pi omp
bro install dotnet9 dotnet10 dotnet11
bro install codex --force          # reinstall / update one
```

Tools: `justtools`, `justpaste` (Windows), `justwatch` (Windows), `justimagine`, `git`, `gh`, `node`, `bun`, `python`, `uv`, `dotnet9`, `dotnet10`, `dotnet11`,
`claude`, `codex`, `pi`, `omp`, `rg`, `ffmpeg`, `yt-dlp`, `tailscale`, `powertoys` (Windows), `sharex` (Windows, JustSuperHuman's fork). Windows installs with WinGet, macOS with Homebrew
(installed first if missing), Linux with apt, dnf, pacman, zypper or apk. Bun, uv and .NET on
macOS / Linux (into `~/.dotnet`) use their official install scripts; JustTools, Claude Code, Codex,
Pi, omp and JustWatch use their own installers.

JustPaste comes with JustTools. `bro install justpaste` (or the Smart Paste switch in settings →
integrations) turns on its Smart Paste for File Explorer, now and at sign-in: Ctrl+V in a folder
saves a copied image as a PNG, downloads copied links and clones copied repository links there.
Copied files and folders still paste the normal Windows way; JustPaste only copies them itself if
you turn on its copy filters (skip `node_modules`, `.gitignore`d files…) in its tray settings.

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

In bro, Quick Install is a checklist: settings → integrations → Quick install (or Enter on JustTools
when it isn't installed), the palette, or `prefix I`. Tick tools with space and press `i`: JustTools
installs in the background, the rest run in a new tab so you can follow them and answer prompts.
Picking an agent that isn't installed in the new-session picker installs it the same way.
