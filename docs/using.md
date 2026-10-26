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

## Session menu and reload

Right-click a session's title bar, or press **Ctrl+Space**, to open the
session menu in the desktop app or terminal UI. Click an action, press its key,
or use **Up/Down** and **Enter**. **Esc** closes the menu. The menu follows your
custom prefix and key bindings; **Ctrl+Space** twice still sends it to the agent.

In the desktop app, right-click terminal content for **Copy** and **Paste**.
Programs that handle mouse input keep their right-clicks; hold **Shift** while
right-clicking to open Bro's clipboard menu instead.

Choose **Reload session with a profile**, or press **Ctrl+Space**, then **Shift+R**.
Pick the current profile to load its latest settings, or choose another profile
to continue the conversation there. Reload keeps the pane's position and your
session name. If Bro cannot find the transcript or build the new command, the
current session stays open.

**Alt+R** still opens earlier sessions; its prefix shortcut is now **Ctrl+Space C**.
Use **Ctrl+Space k/j** to focus the pane above/below; **Up/Down** select menu actions.

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

## Themes

Settings → Appearance → **Theme**, or `theme` in the palette, shows each theme live as you move through the list.
Next to bro's own (graphite, paper, nebula, and the skinned outrun, vaporwave, cyber, blueprint, phosphor and
digital-rain), bro carries the most-installed VS Code themes, taken from their published theme files: the editor
background, the side bar, the cursor and all sixteen terminal colours. A few accents are nudged where text on them
would be hard to read.

- **Dark:** GitHub Dark and Dark Dimmed, One Dark Pro, Dracula, Monokai Pro, Night Owl, VS Code's Dark Modern, Ayu
  Dark and Mirage, Palenight, Tokyo Night and Storm, Catppuccin Mocha, Macchiato and Frappé, Rosé Pine, Kanagawa,
  Gruvbox, Nord, Everforest, Solarized Dark, Poimandres, Vitesse Dark, Noctis, Aura and Andromeda (with stars).
- **Loud:** SynthWave '84 (its neon glows in the desktop app), Shades of Purple, Cobalt2 and Panda.
- **Light:** GitHub Light, Atom One Light, Night Owl Light, Ayu Light, Solarized Light, Catppuccin Latte and Rosé
  Pine Dawn.

Make your own in `~/.bro/themes/<name>.toml`: start from any of them (`base = "dracula"`) and list only what you
change. bro picks up each save as you make it.

## Everything it does

- **Runs your agents in panes** — split, zoom, and switch between live sessions, grouped by project.
- **Shows who needs you** — each agent is marked working, blocked, done or idle.
- **Switches accounts** — multiple Claude and Codex profiles, with usage meters for each.
- **Resumes anything** — pick up a past session, or fork it into another profile.
- **Mixes models** — a built-in proxy lets Claude Code run on OpenAI-format models, and Codex / Pi / omp run on Claude.
- **Reaches your phone** — a built-in bridge lets the Just Terminal mobile and web clients connect.
- **Sets how Claude Code runs** — pick a preset (Quiet, Private, Lockdown, Incognito, Presenter, Marathon) in Settings (the ⚙ in the sidebar, `alt+,` or prefix `A`), next to theme, icons, auto-resume, recaps and compression, to control telemetry, updates, retries and more for every launch. See [docs/claude-env.md](claude-env.md).
- **Keeps projects apart** — Claude sessions only find and message sessions in their own project and the orchestrator. Claude Code on its own lets any session reach any other. Change it under Settings › Claude Code › Messages between sessions. See [docs/claude-env.md](claude-env.md#messages-between-sessions).
- **The orchestrator** — one session that runs the others. Start it from New session (the gold ★ orchestrator tab, or `alt+O`) on Claude Code, Codex, Pi or omp. It opens sessions, hands them tasks, waits on them and reports back. Push-to-talk, the phone and web orchestrator panels, watchdog reviews and the reminders it schedules for itself (once, every so often or daily, for this session or across restarts) all go to it. It sits in gold at the top of the sidebar, runs in `~/orchestrator`, and comes back with the same settings when you reopen bro. `bro ctl <tool>` gives any shell the same tools.
- **Agent teams** — on for every Claude session: ask Claude for teammates and watch them work. Each lead lists its teammates in the sidebar (working or idle, what each is doing, tasks done), and `ctrl+space M` opens the team board with the shared task list. Switch them off or pick the teammates' model under Settings → Claude Code → agent teams. See [docs/claude-env.md](claude-env.md#agent-teams).
- **Watchdogs** — keep a session working while you're away. Press `ctrl+space w` on a session and list its milestones. Each time the agent stops and sits idle, bro hands the review to the orchestrator, starting one if none is running. It reads the chat and the code, checks the work against the milestones and sends the agent its next task. It can clear the agent's context before a new milestone, or open a second-opinion session (an Opus pass over UI work Codex did, for example). Edit what every reviewer is told under Settings → watchdog. See [docs/watchdog.md](watchdog.md).
- **The desktop app** — `bro gui` opens the same workspace in its own window: terminals drawn on the GPU, every row, tab and button clickable, and the pictures and videos your agents make, open or send you shown in a panel beside your sessions instead of another app. See [docs/gui.md](docs/gui.md).
- **The bro mod for Claude Code** — off by default, in Settings › bro mod: `/bro` for your sessions and limits, a chime when Claude asks you something and an optional timer on its questions. See [docs/claude-mod.md](claude-mod.md).
- **Caveman mode** — terse replies that keep every technical fact ([Caveman](https://github.com/JuliusBrussee/caveman) rules: caveman, ultracave or megacave) for new Claude, Codex and Pi sessions. Turn it on in Settings or the palette ("caveman"); ⛏ in the sidebar and on each session's header shows it. Nothing to install. See [docs/caveman.md](caveman.md).
- **Sounds** — PeonPing voice lines when a session starts, finishes, fails or needs you, in Claude Code, Codex, Pi and omp. Settings › Integrations › Sounds (or "sounds" in the palette): pick a pack, preview and install from the registry, choose which moments speak. Shares peon-ping's config and packs when it's installed. See [docs/sounds.md](sounds.md).
