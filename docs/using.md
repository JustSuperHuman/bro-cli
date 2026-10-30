# Using bro

Press **F1** inside bro to see every key binding. You need at least one agent installed (Claude Code, Codex, Pi or
omp); bro launches the ones it finds, and `bro install agents` installs them.

## Quips

Open **Settings → Quips → Manage Quips** to browse, edit, or create shortcuts. The library opens even with no agent session running.

In the desktop app, type `:` in an agent session to find a Quip. Pick one with **↑/↓**, then **Tab** or **Enter**. This works in conversation view and the regular terminal view. Sending is a separate step. Try `Explain this bill :plain-english :keep-it-short`.

In conversation view, recognized shortcuts light up in your message. In terminal view, selected Quips stay in small removable tags below the editor; Bro adds their instructions when you press **Enter** to send. **Esc** clears the tags. If a colon starts ordinary text, keep typing or press **Esc** to put it in the terminal as written. Shell sessions keep their own keys.

Quips are Bro's quick, local alternative to loading a skill. Bro expands each shortcut when you send, so your AI gets the instructions directly without choosing or opening a skill. Unused Quips add nothing to context. The instructions you send still use prompt tokens. Your conversation keeps the short form.

**Full-Send** tells the AI to make reasonable assumptions, finish and check the task without questions, and carry a pull request through to merge. **Full-Stop** does the work and checks it, then leaves the final action to you, including merging a pull request. These two are inspired by [TStack](https://github.com/t3dotgg/tstack); Bro keeps their instructions short and lets you edit them. Use one per message.

The other starters are **Plain English**, **Keep It Short**, **Step by Step**, **Make a Plan**, **Second Look**, and **Check the Facts**. In Settings, choose **Edit** to see or change a Quip's instructions, or **New** to write your own. Custom Quips stay on this computer in `~/.bro/quips.json`.

## Profiles

Choose **Manage profiles** in the new-session picker, or press **Alt+O**, to add
Claude or Codex profiles. Choose **+ Claude** or **+ Codex**, enter a name, then
**Create + log in**. Bro opens that profile's login tab; in Claude, run `/login`.
Select a profile and choose **Remove** to delete its local login, settings and
saved sessions after confirmation. Bro protects the machine's default logins
(`claude:local` and `codex:local`) from removal.

## Proxy mode

With several Claude or ChatGPT subscriptions you don't have to pick one per session. Turn on **Proxy mode**
(Settings › Sessions, `s` in the proxy view, or "proxy mode" in the palette) and you only pick the agent. Every
signed-in login of its kind shares the work:

- **Each conversation stays on one login.** Its prompt cache lives there, so moving it would cost you a full-price
  rebuild. Subagents stay with the session that started them.
- **New conversations go to the login with the most left.** bro reads that from the rate-limit headers on each
  login's last response, and from its usage meters until a login has served one.
- **A login at its limit hands off.** When a login hits its limit, bro rests it until it resets and sends the same
  request to the next login, before your agent has seen a byte. Only when every login is out does the agent see a
  limit: its own message, with the reset of whichever login comes back first.

Claude Code and Codex look and work exactly as they do on their own. They stay signed in as one of your logins (the
machine's own when it's signed in, otherwise the first other one; a session you resume keeps the login it already
lives in). `/model`, the model lists, plans, `/status`, effort levels, web search and compaction are all theirs.
Only the address their requests go to changes: bro's proxy takes each request and sends it as the login with the
most room. Claude Code is told the proxy is Anthropic's own API (`_CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL`).
Codex keeps its built-in OpenAI provider with `openai_base_url` set to the proxy.

Two differences remain, both from the harnesses themselves:

- Codex's `/model` menu shows a red line noting that its OpenAI base URL is changed. Picking models works as
  usual: the list is the one your own login gets.
- Claude Code's Remote Control isn't available while `ANTHROPIC_BASE_URL` points elsewhere. bro's phone bridge
  still reaches the session.

Pi and omp keep their own login. In the launcher they can also run on all your Claude or all your ChatGPT logins.
The same rows let Claude Code run on your ChatGPT logins and Codex on your Claude ones. The proxy view lists each
shared login: ready or back at its reset, how much of its 5-hour and weekly windows it has used, and how many
conversations it carries. Add or sign out a login in Profiles and it joins or leaves within seconds, with no
restart. Turning proxy mode off changes new sessions only.

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

## Tasks

Choose **Tasks · outcomes** in the sidebar, press **Ctrl+Space V** (lowercase `v`), or use **Alt+Shift+D**. Tasks opens beside your workspace in both the desktop app and terminal UI. Your other sessions keep running.

The list tracks outcomes separately from agent activity. **Working** means an assigned worker is active. **Waiting** shows a blocker and its wake condition. **Needs review** means a candidate still needs acceptance evidence. **Accepted** means the current contract has passed its required checks. A worker going idle does not accept its task.

Choose **New** (`n`) to enter an outcome, an observable review check, and the project folder. Use **Tab** to move between fields and **Ctrl+Enter** to save. **Ctrl+Q** changes the quality mode. Quick uses focused checks, Standard verifies the changed behavior, and Critical requires an independent review. The project's own instructions still apply. Saved work wakes the coordinator and stays in the task ledger across restarts.

Select a task with **Up/Down**. **Steer** (`s`) revises its goal and first acceptance criterion while retaining its other requirements. **Cancel** (`x`) requests that it stop; a running worker retains ownership until its stop or isolation is confirmed. **Retry** (`r`) respects remaining budgets and unresolved ownership. **Review** (`v`) asks the coordinator to inspect a candidate, collect evidence, stop its task-owned worker and verify the final artifact before acceptance. **Retire worker** (`z`) requests that safe stop. These actions preserve unrelated sessions. Accepted delegated work returns as a separate **Accepted outcome** in the original orchestrator conversation; an initial reply can acknowledge work before it is finished.

**Context** (`c`) shows the task's handoff, prerequisites and recovery notices, followed by recorded outcomes and available cost reports. **Finished** (`a`) includes accepted and cancelled work. **Evidence** (`e`) opens the latest recorded verification log when one is available. **Details** (`d`) adds technical identities and evidence digests. **F5** refreshes; **Page Up/Down** scrolls the detail. The bridge must be on for live task state; a connection error keeps the last snapshot visible.

Phone, web and voice requests have separate request identities. Ordinary requests wait for a ready orchestrator prompt and run one at a time when the adapter has no separate turn channel. A response can answer only its matching request. Clearing conversation context preserves pending work. After a restart, an uncertain sent request waits for its original answer or explicit cancellation; Bro does not blindly repeat its side effects.

See [Coordination](coordination.md) for the task tools, evidence protocol, recovery rules and reproducible checks.

## Everything it does

- **Runs your agents in panes** — split, zoom, and switch between live sessions, grouped by project.
- **Keeps them running when bro closes** — sessions run in bro's session server, a background process of its own, so closing bro's window, a crash or an update doesn't end them: the next bro (terminal or desktop app) picks them up with their screens, names and history, and proxied sessions get their route back. Quitting says how many are still running; with none left the server stops too. While no window is open, a background bro carries on with them: the phone bridge, the translating proxy, auto-resume and watchdogs keep working (Keep working while bro is closed, in Settings › Session server); the window you open next takes over from it. If the server itself stops without being asked to (a crash, or the computer shutting down), bro brings its sessions back: at once into the panes of a bro that's open, or, after a restart, when you say yes as bro opens. Claude and Codex resume their conversations; shells start fresh. What it keeps on disk for that (`~/.bro/v2-host-sessions.json`, readable by you alone) holds no keys: their environment stays in memory.
- **More than one window** — `bro --view` opens another window on the bro that's running, showing its sessions live: type into one and it's yours (its size follows the window you typed in; the other shows ⇄ watching). Over SSH beside the desktop app, `bro` opens this way by itself. What you launch from it runs in the bro it watches, with that bro's proxy and phone bridge. Settings › Session server shows it and restarts it (its sessions end; Claude and Codex sessions come back in their panes, resuming their conversations, and shells start fresh). Turn off Keep sessions running there to have new sessions run inside bro and end with it, as before. Start at sign-in (also `bro host install` / `bro host uninstall`) starts the background bro (and the server) when you sign in, so your phone reaches bro even before you open it, and keeps them running instead of stopping 10 minutes after bro closes with nothing left: a startup app on Windows (Task Manager › Startup apps lists it), a LaunchAgent on macOS, a systemd user unit on Linux, for your user only and without admin. It isn't a system service on purpose: one would run outside your desktop, where sessions couldn't open a browser to log in, show windows or reach your credential store, and a reboot ends every session anyway. `bro host status` lists what it runs, who drives it and whether it starts at sign-in, and `bro host stop` ends it (and the background bro with it). The server answers what programs ask their terminal (device attributes, colours, the kitty keyboard flags) the way the desktop app does, once, whichever windows watch. A second bro started with `--new` keeps its own sessions.
- **Shows who needs you** — each agent is marked working, blocked, done or idle.
- **Switches accounts** — multiple Claude and Codex profiles, with usage meters for each. Or turn on [proxy mode](#proxy-mode) and stop choosing: every signed-in login shares the work, and a login at its limit hands off to the next.
- **Resumes anything** — pick up a past session, or fork it into another profile.
- **Mixes models** — a built-in proxy lets Claude Code run on OpenAI-format models, and Codex / Pi / omp run on Claude.
- **Reaches your phone** — a built-in bridge lets the Just Terminal mobile and web clients connect. The phone opens the pictures, videos and sounds that belong to your work, even outside the session's folder: the ones in your projects, the ones your agents made, sent, looked at or named, and the ones you attached. Its gallery lists them for a chat or for all of them. Code and text never leave the session's folders. To let the phone open any media on this computer, turn off **Protect phone media** in Settings › Session server.
- **Sets how Claude Code runs** — pick a preset (Quiet, Private, Lockdown, Incognito, Presenter, Marathon) in Settings (the ⚙ in the sidebar, `alt+,` or prefix `A`), next to theme, icons, auto-resume, recaps and compression, to control telemetry, updates, retries and more for every launch. See [docs/claude-env.md](claude-env.md).
- **Keeps projects apart** — Claude sessions only find and message sessions in their own project and the orchestrator. Claude Code on its own lets any session reach any other. Change it under Settings › Claude Code › Messages between sessions. See [docs/claude-env.md](claude-env.md#messages-between-sessions).
- **Orchestrators** — each project can have its own coordinator on Claude Code, Codex, Pi or omp. Open New session's gold ★ orchestrator tab (or `alt+O`), choose its project folder, and launch it. Starting another in the same project takes you to the running one. The Orchestrators project tab shows every coordinator; each project tab shows that project's coordinator with its workers. Choose **Orchestrator** (`alt+g` in the launcher) for the root coordinator across projects. It runs in `~/orchestrator`, opens or reuses project orchestrators, and starts new ones with its own model and launch settings. Push-to-talk, phone and web panel messages, watchdog reviews and scheduled reminders go to the root Orchestrator. Each running orchestrator comes back with its own project and settings when you reopen bro. `bro ctl <tool>` gives any shell the same tools, including `open_orchestrator`.
- **Agent teams** — on for every Claude session: ask Claude for teammates and watch them work. Each lead lists its teammates in the sidebar (working or idle, what each is doing, tasks done), and `ctrl+space M` opens the team board with the shared task list. Switch them off or pick the teammates' model under Settings → Claude Code → agent teams. See [docs/claude-env.md](claude-env.md#agent-teams).
- **Watchdogs** — keep a session working while you're away. Press `ctrl+space w` on a session and list its milestones. Each time the agent stops and sits idle, bro hands the review to the orchestrator, starting one if none is running. It reads the chat and the code, checks the work against the milestones and sends the agent its next task. It can clear the agent's context before a new milestone, or open a second-opinion session (an Opus pass over UI work Codex did, for example). Edit what every reviewer is told under Settings → watchdog. See [docs/watchdog.md](watchdog.md).
- **The desktop app** — `bro gui` opens the same workspace in its own window: terminals drawn on the GPU, every row, tab and button clickable, and the pictures and videos your agents make, open or send you shown in a panel beside your sessions instead of another app. See [docs/gui.md](docs/gui.md).
- **The bro mod for Claude Code** — off by default, in Settings › bro mod: `/bro` for your sessions and limits, a chime when Claude asks you something and an optional timer on its questions. See [docs/claude-mod.md](claude-mod.md).
- **Caveman mode** — terse replies that keep every technical fact ([Caveman](https://github.com/JuliusBrussee/caveman) rules: caveman, ultracave or megacave) for new Claude, Codex and Pi sessions. Turn it on in Settings or the palette ("caveman"); ⛏ in the sidebar and on each session's header shows it. Nothing to install. See [docs/caveman.md](caveman.md).
- **Skills** — The Skills view (`skills` in the palette, or Settings › Skills › Manage skills) shows every skill your agents have, grouped by where each came from (bro's own, a repo bro watches, Skill Issue, Codex's built-ins, yours) with the agents that get it: Claude, Codex, `~/.agents`, Pi, omp. Each sidebar project has a page of its own: its `.claude/skills`, `.codex/skills` and `.agents/skills`, then the skills every project gets, so you see exactly what an agent working there has. `enter` opens a skill's SKILL.md in bro's preview, `o` shows its folder, `/` searches, `tab` goes to the next page, and `m` (or a right-click, or a row's ⋯) opens a skill's menu with everything that can be done with it. **Add repo** (`a`) watches a git repo: paste `https://github.com/t3dotgg/tstack`, `owner/repo` or any git URL (or run `bro skills add t3dotgg/tstack`) and its page lists every folder with a `SKILL.md` at the repo's root, one folder down or under its `skills/` (or, in a repo with none there, under `.claude/skills/`, `.agents/skills/` or `plugins/*/skills/`). Watching installs nothing: tick the skills you want (`space`, or their boxes) and install them together everywhere (`i`: every Claude and Codex home, so every project) or into one project (`p`: its `.claude/skills` and `.agents/skills`), or use a row's Install. bro pulls the repos when it opens and every 6 hours (Settings › Skills › Keep them in sync) and keeps what you installed in step: a skill that leaves its repo leaves too, and a skill of yours with the same name is never replaced. A repo's page pulls it now (`r`) or stops watching it (`w`: keep what you installed from it, or disable that too). **Removing a skill disables it** (`d`, or Disable in its menu): every copy of it moves out of your agents' reach into `~/.bro/disabled-skills/<name>`, with a note of where each was, and the **Disabled** page enables it again (`e`: each copy goes back where it was) or deletes it for good (`x`, asked once more). bro's own skills are switched off instead, and Codex keeps its own. **Discover** features Theo's [tstack](https://github.com/t3dotgg/tstack) and browses [Skill Issue](https://skill-issue.in), 300+ agent skills ranked by what actually gets used: its domain tabs (`←`/`→` or `1`–`9`), its ranking or most clicked (`s`), a search (`/`), and `enter` installs one, asking where (`i` installs it everywhere straight away, `p` into a project). bro watches just that skill's folder in its repo (`owner/repo#skill`), so it stays in step, and `d` disables it. bro's own skills (`bro-open`, JustTools', JustWatch's, JustImagine's) have their switches on the bro page and in Settings › Skills. From a shell: `bro skills installed [--project <dir>]`, `bro skills install t3dotgg/tstack council full-send [--project <dir>]`, `bro skills disable <skill>`, `bro skills enable <skill>`, `bro skills disabled`, `bro skills search react --domain frontend`, then `bro skills install <id>`.
- **Sounds** — PeonPing voice lines when a session starts, finishes, fails or needs you, in Claude Code, Codex, Pi and omp. Settings › Integrations › Sounds (or "sounds" in the palette): pick a pack, preview and install from the registry, choose which moments speak. Shares peon-ping's config and packs when it's installed. See [docs/sounds.md](sounds.md).
