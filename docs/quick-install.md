# Quick Install

```
bro install                        # list everything with what's installed; pick what to install
bro install justtools git node bun # install those (already installed ones are skipped)
bro install recommended            # JustTools, Git, Node.js, Bun, Claude Code, Codex
bro install agents                 # claude codex pi omp
bro install dotnet9 dotnet10 dotnet11
bro install codex --force          # reinstall / update one
```

Tools: `justtools`, `justpaste` (Windows), `justwatch` (Windows), `justimagine`, `justbrowse`, `git`, `gh`, `node`, `bun`, `python`, `uv`, `dotnet9`, `dotnet10`, `dotnet11`,
`claude`, `codex`, `pi`, `omp`, `agent-desktop`, `rg`, `ffmpeg`, `yt-dlp`, `tailscale`, `powertoys` (Windows), `sharex` (Windows, JustSuperHuman's fork). Windows installs with WinGet, macOS with Homebrew
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
through its API on `localhost:6767`. Switch the skill off or on in settings → skills.

JustImagine is your own image, video, song and voice studio. `bro install justimagine` clones
[its `main` branch](https://github.com/JustGains/JustImagine) into `%LOCALAPPDATA%\Programs\JustImagine`
(`~/.local/share/justimagine` on macOS and Linux), runs `npm install` and registers it with its own
`service install`: a Task Scheduler task on Windows, a launchd agent on macOS, a systemd user unit on
Linux. It starts at login on port 8793. If an earlier install recorded a gallery folder and port, it
keeps them. It needs Git and Node.js, so install those first. JustImagine has no releases, so bro
compares the checkout with GitHub when it opens and every 12 hours, and says when `main` has new
commits. Enter on settings → integrations → JustImagine, or `bro install justimagine --force`, pulls
them, reinstalls dependencies and restarts the service.

## Browser and desktop tools

`bro install justbrowse` downloads the newest [JustBrowseMCP release](https://github.com/JustSuperHuman/mcp-chrome-but-good/releases),
requires a matching SHA-256 in `SHA256SUMS`, extracts the extension and registers its native bridge.
It needs Node.js 20 or newer. Files go in `%LOCALAPPDATA%\JustBrowseMCP` on Windows,
`~/Library/Application Support/JustBrowseMCP` on macOS, or `~/.local/share/justbrowse-mcp` on Linux
(override with `JUSTBROWSE_DIR`; choose a release with `JUSTBROWSE_VERSION`).

Browsers require one manual step:

1. Open `chrome://extensions` or `edge://extensions` and turn on **Developer mode**.
2. Choose **Load unpacked**, then select the installed `extension` folder.
3. Open JustBrowseMCP's popup and click **Connect**. After updating, click **Reload** on the extensions page first.

Select JustBrowseMCP in Quick install to see those instructions and the exact folder again.
**Open extension folder** opens it; **Check connection** checks whether a browser is connected,
not merely whether the bridge is running. New Claude and Codex sessions, including pooled logins,
get the local `justbrowse` MCP server automatically. Bro does not rewrite global agent configuration.
Claude's explicit `--strict-mcp-config` still excludes Bro's automatic browser configuration.

`bro install agent-desktop` installs [Vercel's native desktop CLI](https://github.com/vercel-labs/agent-desktop)
through npm. It needs Node.js and Git. Bro installs the main `agent-desktop` skill and the guide for
this machine's OS, including their reference files—not the other platforms, FFI integration or optional
model-provider helper. Run `agent-desktop status` to check readiness. On macOS, use
`agent-desktop permissions --guide` to grant Accessibility access. On Linux, run from the graphical
session with `at-spi2-core` available; individual features may need X11, GNOME or `wl-clipboard`.
Bro does not grant OS permissions or enable physical input automatically.

Both tools appear under **Skills → Bro**. Bro keeps a complete cache in `~/.bro/tool-skills`,
refreshes its managed copies on startup and every six hours, and supplies cached skills to new profiles
without network access during launch. Desktop skill downloads follow the Skills auto-sync setting;
the browser skill comes from its installed release (older installations can fetch it from the repository).
Personal skills and links of the same name stay untouched. Update either tool with
`bro install <tool> --force`. The launcher no longer has Browser or Opens as switches; permissions remain
per session, and explicit split entry points still work.

In bro, Quick Install is a checklist: settings → integrations → Quick install (or Enter on JustTools
when it isn't installed), the palette, or `prefix I`. Tick tools with space and press `i`: JustTools
installs in the background, the rest run in a new tab so you can follow them and answer prompts.
Picking an agent that isn't installed in the new-session picker installs it the same way.
