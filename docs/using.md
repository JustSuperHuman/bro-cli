# Using bro

Press **F1** inside bro to see every key binding. You need at least one agent installed (Claude Code, Codex, Pi or
omp); bro launches the ones it finds, and `bro install agents` installs them.

## Profiles

Choose **Manage profiles** in the new-session picker, or press **Alt+O**, to add
Claude or Codex profiles. Choose **+ Claude** or **+ Codex**, enter a name, then
**Create + log in**. Bro opens that profile's login tab; in Claude, run `/login`.
Select a profile and choose **Remove** to delete its local login, settings and
saved sessions after confirmation. Bro protects the machine's default logins
(`claude:local` and `codex:local`) from removal.

## Pasting images

Alt+V pastes a clipboard image into the focused agent as a file path. On Windows,
Bro uses the original clipboard PNG when available; this avoids blank images
from a browser's bitmap copy. Clipboard contents stay unchanged.

## Fonts

Settings → Appearance → **Font** switches the terminal font live: bro bundles
JetBrains Mono, Fira Code, IBM Plex Mono, Maple Mono, Victor Mono, Monaspace Krypton and Radon, Space Mono,
Share Tech Mono, Comic Mono, Departure Mono and VT323, video-game-era Kode Mono, Doto, Monocraft (Minecraft),
IBM VGA 8x16 (DOS), Workbench (Amiga), Sixtyfour (C64) and Press Start 2P, plus the Nerd Font symbols for its icons —
all open source, licences in [crates/bro-tui/assets/fonts/licenses](../crates/bro-tui/assets/fonts/licenses).

## Everything it does

- **Runs your agents in panes** — split, zoom, and switch between live sessions, grouped by project.
- **Shows who needs you** — each agent is marked working, blocked, done or idle.
- **Switches accounts** — multiple Claude and Codex profiles, with usage meters for each.
- **Resumes anything** — pick up a past session, or fork it into another profile.
- **Mixes models** — a built-in proxy lets Claude Code run on OpenAI-format models, and Codex / Pi / omp run on Claude.
- **Reaches your phone** — a built-in bridge lets the Just Terminal mobile and web clients connect.
- **Sets how Claude Code runs** — pick a preset (Quiet, Private, Lockdown, Incognito, Presenter, Marathon) in Settings (the ⚙ in the sidebar, `alt+,` or prefix `A`), next to theme, icons, auto-resume, recaps and compression, to control telemetry, updates, retries and more for every launch. See [docs/claude-env.md](claude-env.md).
- **The orchestrator** — one session that runs the others. Start it from New session (the gold ★ orchestrator tab, or `alt+O`) on Claude Code, Codex, Pi or omp. It opens sessions, hands them tasks, waits on them and reports back. Push-to-talk, the phone and web orchestrator panels, watchdog reviews and the reminders it schedules for itself (once, every so often or daily, for this session or across restarts) all go to it. It sits in gold at the top of the sidebar, runs in `~/orchestrator`, and comes back with the same settings when you reopen bro. `bro ctl <tool>` gives any shell the same tools.
- **Watchdogs** — keep a session working while you're away. Press `ctrl+space w` on a session and list its milestones. Each time the agent stops and sits idle, bro hands the review to the orchestrator, starting one if none is running. It reads the chat and the code, checks the work against the milestones and sends the agent its next task. It can clear the agent's context before a new milestone, or open a second-opinion session (an Opus pass over UI work Codex did, for example). Edit what every reviewer is told under Settings → watchdog. See [docs/watchdog.md](watchdog.md).
- **The desktop app** — `bro gui` opens the same workspace in its own window: terminals drawn on the GPU, every row, tab and button clickable, and the pictures and videos your agents make, open or send you shown in a panel beside your sessions instead of another app. See [docs/gui.md](docs/gui.md).
- **Caveman mode** — terse replies that keep every technical fact ([Caveman](https://github.com/JuliusBrussee/caveman) rules: caveman, ultracave or megacave) for new Claude, Codex and Pi sessions. Turn it on in Settings or the palette ("caveman"); ⛏ in the sidebar and on each session's header shows it. Nothing to install. See [docs/caveman.md](caveman.md).
