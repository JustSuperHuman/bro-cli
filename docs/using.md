# Using bro

Press **F1** inside bro to see every key binding. You need at least one agent installed (Claude Code, Codex, Pi or
omp); bro launches the ones it finds, and `bro install agents` installs them.

## Quips

Open **Settings → Quips → Manage Quips** to browse, edit, or create shortcuts. The library opens even with no agent session running.

In the desktop app, type `:` in an agent session to find a Quip. Pick one with **↑/↓**, then **Tab** or **Enter**. This works in conversation view and the regular terminal view. Sending is a separate step. Try `Explain this bill :plain-english :keep-it-short`.

The last entry in that list is **New…**. It opens the Quip editor with whatever you typed after the colon as the shortcut, and saving puts the new Quip straight into your message. Esc takes you back to the message as you left it.

In conversation view, recognized shortcuts light up in your message. In terminal view, selected Quips stay in small removable tags below the editor; Bro adds their instructions when you press **Enter** to send. **Esc** clears the tags. If a colon starts ordinary text, keep typing or press **Esc** to put it in the terminal as written. Shell sessions keep their own keys.

Quips are Bro's quick, local alternative to loading a skill. Bro expands each shortcut when you send, so your AI gets the instructions directly without choosing or opening a skill. Unused Quips add nothing to context. The instructions you send still use prompt tokens. Your conversation keeps the short form.

**Full-Send** tells the AI to make reasonable assumptions, finish and check the task without questions, and carry a pull request through to merge. **Full-Stop** does the work and checks it, then leaves the final action to you, including merging a pull request. These two are inspired by [TStack](https://github.com/t3dotgg/tstack); Bro keeps their instructions short and lets you edit them. Use one per message.

The other starters are **Plain English**, **Keep It Short**, **Step by Step**, **Make a Plan**, **Second Look**, and **Check the Facts**. In Settings, choose **Edit** to see or change a Quip's instructions, or **New** to write your own. Custom Quips stay on this computer in `~/.bro/quips.json`.

**Remotion Update** (`:remotion-update`) asks the agent for a short narrated Remotion video about the work it just did: the goal, what changed and why, and what you need to know, including decisions, risks, anything untested and next steps. It voices the video with ElevenLabs when the agent can use it, otherwise with JustImagine, and falls back to captions alone when neither is there. Starters added in an update show up once at the end of a library you've already saved. If you delete one, it stays deleted.

## New session

**New session** (`alt+n`) asks for two things. **Harness** is the agent: Claude Code, Codex, Pi or omp. **Providers**
is what it runs on, in one list:

- Your subscriptions come first: your Claude and ChatGPT logins with what each has left, the Just Superhuman
  account, Gemini. With proxy mode on, Claude and ChatGPT are one row each.
- **Connected** lists the API providers you have a key for (OpenRouter, Z.ai, a local Ollama).
- **Other providers** holds the ones with no key yet. Click it, or press Enter on it, to open it.

A login of the harness's own kind launches straight away. Anything else asks for a model beside the list. The list
opens on what that harness ran last, with its model already picked. Typing picks the project.

## Profiles

Choose **Manage profiles** in the new-session picker, or press **Alt+O**, to add
Claude or Codex profiles. Choose **+ Claude** or **+ Codex**, enter a name, then
**Create + log in**. Bro opens that profile's login tab; in Claude, run `/login`.
Select a profile and choose **Remove** to delete its local login, settings and
saved sessions after confirmation. Bro protects the machine's default logins
(`claude:local` and `codex:local`) from removal.

## Just Superhuman

Your Just Superhuman account sits at the bottom of Settings, under the sections, marked with the knot. **Sign in**
opens your browser to approve "bro" on this computer (Settings shows the code to check, with **Open browser** and
**Cancel**, and waits), or run `bro jsh login`. Signed in, it shows your name and credits (hover for the email and
organization) with **Sign out** (`bro jsh logout`), and the launcher gets a **Just Superhuman** row straight
after your logins, with the organization, its credits and how much of the month's allowance is left. Claude Code started on it runs through Just Superhuman's Claude and spends the organization's credits; no
Claude login is involved. Codex and Pi can use it through bro-proxy. The token lives in `~/.bro/jsh/token.json`,
apart from `config.json`. `BRO_JSH_ORIGIN` points bro at another accounts server (`http://auth.localhost:8790`
locally).

The credits show where your logins' usage does. The usage view's first card is the account: the credits left and
their dollar value, this month's allowance as a meter with when it resets, and what's been spent, granted (one-time
credits that never expire) and held for replies still being written, plus anything stopping Claude (out of credits,
AI paused). The sidebar's **Usage left** has a **Just Superhuman** line with the credits left. Clicking either opens
the launcher on the account to pick a model. Credits refresh every few minutes, and again when you press refresh in
the usage view; if the device is revoked on the website, bro signs out and says so.

Phone (bridge, same token auth as the other `/api/*` routes; times are ms since the epoch, the token is never sent):

- `GET /api/jsh` returns `{ signedIn, account?: { user: { name, email }, organization: { name, slug }, credits:
  { available, granted, monthlyAllowance, resetsAt }, claude: { available, reason }, host?: { url, status } },
  pending?: { userCode, verificationUri, verificationUriComplete, expiresAt }, error? }`.
- `POST /api/jsh/login` starts the device flow on this computer and polls it in the background; it returns the
  same shape with `pending`. A second call while one waits returns that flow. If Just Superhuman can't be reached:
  502 with the same shape, `error` and `message`.
- `POST /api/jsh/logout` revokes and returns `{ signedIn: false }` (plus `warning` if the revoke couldn't reach
  the server; the token is gone from this computer either way).
- While signed in, the profile list has `{ id: "jsh:claude", label: "Claude Code · Just Superhuman", group:
  "agent", agent: "claude", description: "1,840 credits · Org" }`. Creating a session with that `profileId`
  launches Claude Code on the Just Superhuman provider.

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

In the launcher, proxy mode shows **Claude** and **ChatGPT** as one row each (how many logins share the work and
what they have left together) in place of a row per login. Pi and omp keep their own login and can also run on
either row. The same rows let Claude Code run on your ChatGPT logins and Codex on your Claude ones. The proxy view lists each
shared login: ready or back at its reset, how much of its 5-hour and weekly windows it has used, and how many
conversations it carries. Add or sign out a login in Profiles and it joins or leaves within seconds, with no
restart. Turning proxy mode off changes new sessions only.

## Session menu and reload

In the desktop app, right-click a session's title bar or tab to fork, rename,
reload, or close that session. Press **Ctrl+Space** for the full list of prefix
commands. In the terminal UI, right-clicking the title bar opens that full list.
Click an action or use **Up/Down** and **Enter**. **Esc** closes the menu. The
prefix menu follows your custom bindings. **Ctrl+Space** twice sends it to the agent.

In the desktop app, right-click terminal or conversation content for **Paste**,
**Copy** when text is selected, and **Fork conversation** in Claude Code and
Codex sessions. Hold **Shift** while right-clicking if the terminal program
handles mouse input.

Choose **Reload session…** in the desktop context menu, or press **Ctrl+Space**,
then **Shift+R**.
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
JetBrains Mono, Fira Code, IBM Plex Mono, Maple Mono, JustGains Mono (ours: Poppins on a grid, where `:mal:`,
`:xp:` and friends draw as icons), Victor Mono, Monaspace Krypton and Radon, Space Mono,
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

In the desktop app, Settings → Appearance → **Create a custom theme** opens a floating editor over your
workspace. Background and Accent derive the full palette using T3 Code's color engine. Every valid change
previews across the app; nothing is saved until **Save theme**. Cancel restores the theme you started with.
Drag the panel by its heading, resize its lower-right corner or minimize it to see more of the app.

**Advanced** has T3's 22 color families. Each updates its related foregrounds, borders and hover colors together.
**Inspect app colors** selects the family used by a visible app area; its click does not run the underlying
button or link. Registered targets include the sidebar, headers, controls, message bubbles, code blocks and
terminal backgrounds. The editor outlines visible uses of the selected family. Light and Dark retain separate
palettes in the same file. Returning to the two-color controls regenerates the active palette and replaces its
advanced adjustments. Escape in a color field reverts that color edit, including linked colors; Escape again
closes the editor. Tab moves between fields, Alt+Up/Down selects color families, and Ctrl+S saves a copy.

**Aurora studio demo** starts a paired light/dark theme with violet and sea-glass colors, ambient gradients,
rounded message and code surfaces, and embedded CSS for T3 Code. The bundled stylesheet has a matching native
treatment in Bro; arbitrary browser CSS still does not run in Bro. The editor's Custom CSS field edits the
exported browser stylesheet. Changes to that field take effect in T3 Code after import.

You can also make your own in `~/.bro/themes/<name>.toml`: start from any built-in (`base = "dracula"`) and list
only what you change. Bro picks up each save as you make it.

Appearance and the theme picker both have **Import** and **Export**. Import a T3 Code v1 theme or a self-contained VS Code
color-theme file (`.json` or `.jsonc`). You can also put the file directly in `~/.bro/themes/`. Existing themes
are kept; importing another theme with the same name creates a separate copy. Files must be 2 MB or smaller.
An invalid edit keeps the last working palette and shows the problem.

T3 imports keep all 61 color roles, collection metadata and both appearances. A file with a dark variant has a
second picker entry marked **dark**; choosing it is independent of the system appearance. Literal CSS colors,
including OKLCH, Display P3 and alpha, are supported. The desktop app uses the palette's UI and terminal colors
separately; the terminal UI composites transparent colors onto their corresponding surfaces.

VS Code files support comments and trailing commas. Bro maps their workbench colors using T3 Code's importer
and keeps their sixteen terminal colors too. Import the actual color-theme file, not an extension's
`package.json` or a VSIX archive. Local `include` files are not loaded, and syntax-token rules do not change
Bro's interface.

**Export** saves a new T3 Code JSON file under `~/.bro/themes/exports/`, ready to import into T3 Code. A native
T3 theme keeps its variants and embedded CSS when exported. Browser CSS is preserved as data but is not
executed by Bro's native renderer; CSS selectors, web fonts and layout effects do not carry over. Colors set
only through that CSS need to be moved into the file's `colors` map to affect Bro.

Compatibility tests use published Catppuccin Mocha, Dracula, Nord, One Dark Pro, Vitesse Dark and Solarized
Light files, with pinned sources and licenses in
[`crates/bro-tui/tests/fixtures/themes/`](../crates/bro-tui/tests/fixtures/themes/README.md).

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
- **Orchestrators** — each project can have its own coordinator on Claude Code, Codex, Pi or omp. Open New session's gold ★ orchestrator tab (or `alt+O`), choose its project folder, and launch it. Starting another in the same project takes you to the running one. **Orchestrators** in the sidebar lists every coordinator in the session tabs; each project lists its own coordinator beside its workers. Choose **Orchestrator** (`alt+g` in the launcher) for the root coordinator across projects. It runs in `~/orchestrator`, opens or reuses project orchestrators, and starts new ones with its own model and launch settings. Push-to-talk, phone and web panel messages, watchdog reviews and scheduled reminders go to the root Orchestrator. Each running orchestrator comes back with its own project and settings when you reopen bro. `bro ctl <tool>` gives any shell the same tools, including `open_orchestrator`.
- **Agent teams** — on for every Claude session: ask Claude for teammates and watch them work. Each lead lists its teammates in the sidebar (working or idle, what each is doing, tasks done), and `ctrl+space M` opens the team board with the shared task list. Switch them off or pick the teammates' model under Settings → Claude Code → agent teams. See [docs/claude-env.md](claude-env.md#agent-teams).
- **Create Slop** — start a session for making video and media. Open New session's Create Slop tab (or `alt+shift+s`), cast characters from character.slop.cc or Claudia, and start. The session gets each character's MCP server and the media tools this computer has (JustImagine, ElevenLabs, Higgsfield, SLOPCANNON, ffmpeg, Remotion). A project remembers its cast. See [docs/create-slop.md](create-slop.md).
- **Watchdogs** — keep a session working while you're away. Press `ctrl+space w` on a session and list its milestones. Each time the agent stops and sits idle, bro hands the review to the orchestrator, starting one if none is running. It reads the chat and the code, checks the work against the milestones and sends the agent its next task. It can clear the agent's context before a new milestone, or open a second-opinion session (an Opus pass over UI work Codex did, for example). Edit what every reviewer is told under Settings → watchdog. See [docs/watchdog.md](watchdog.md).
- **The desktop app** — `bro gui` opens the same workspace in its own window: terminals drawn on the GPU, every row, tab and button clickable, and the pictures and videos your agents make, open or send you shown in a panel beside your sessions instead of another app. See [docs/gui.md](docs/gui.md).
- **The bro mod for Claude Code** — off by default, in Settings › bro mod: `/bro` for your sessions and limits, a chime when Claude asks you something and an optional timer on its questions. See [docs/claude-mod.md](claude-mod.md).
- **Slop Collector (beta)** — when a render or test your agent runs gets cut short, the programs it started can keep running out of sight (usually a headless browser from Remotion, Playwright or Puppeteer, holding gigabytes until the session closes), and its throwaway temp folder stays on disk. Turn on Settings › Slop Collector and the session server checks your sessions every 30 seconds, closes a leftover once nothing has used it for 10 minutes (you can change that, or have it wait for you), clears the temp folders those runs leave, and tells you what came back. It never closes anything with a window, a server you're using, or anything your sessions are still running. `bro slop` lists what it finds; `bro slop collect` cleans it up now. See [docs/slop-collector.md](slop-collector.md).
- **Caveman mode** — terse replies that keep every technical fact ([Caveman](https://github.com/JuliusBrussee/caveman) rules: caveman, ultracave or megacave) for new Claude, Codex and Pi sessions. Turn it on in Settings or the palette ("caveman"); ⛏ in the sidebar and on each session's header shows it. Nothing to install. See [docs/caveman.md](caveman.md).
- **Skills** — The Skills view (`skills` in the palette, or Settings › Skills › Manage skills) shows every skill your agents have, grouped by where each came from (bro's own, a repo bro watches, Skill Issue, Codex's built-ins, yours) with the agents that get it: Claude, Codex, `~/.agents`, Pi, omp. Each sidebar project has a page of its own: its `.claude/skills`, `.codex/skills` and `.agents/skills`, then the skills every project gets, so you see exactly what an agent working there has. `enter` opens a skill's SKILL.md in bro's preview, `o` shows its folder, `/` searches, `tab` goes to the next page, and `m` (or a right-click, or a row's ⋯) opens a skill's menu with everything that can be done with it. **Add repo** (`a`) watches a git repo: paste `https://github.com/t3dotgg/tstack`, `owner/repo` or any git URL (or run `bro skills add t3dotgg/tstack`) and its page lists every folder with a `SKILL.md` at the repo's root, one folder down or anywhere under its `skills/`, including families of skills kept in folders of their own (`skills/dcc/blender/<name>`; in a repo with none there, under `.claude/skills/`, `.agents/skills/` or `plugins/*/skills/`, else wherever they are). A SKILL.md inside another skill's folder is part of that skill, and when two folders share a name the shallowest is the one listed. Paste a link to a folder (`https://github.com/scenario-labs/skills/tree/main/skills/dcc/blender`) to watch just the skills in it; `owner/repo#dcc` or `owner/repo#blender` does the same by the folder's name. A repo that keeps its skills in families lists each family under its own heading, with how many of it are installed and **Tick all**: a family's skills often lean on each other (Blender's specialists run the lead skill's scripts), so install them together. Watching installs nothing: tick the skills you want (`space`, or their boxes) and install them together everywhere (`i`: every Claude and Codex home, so every project) or into one project (`p`: its `.claude/skills` and `.agents/skills`), or use a row's Install. bro pulls the repos when it opens and every 6 hours (Settings › Skills › Keep them in sync) and keeps what you installed in step: a skill that leaves its repo leaves too, and a skill of yours with the same name is never replaced. A repo's page pulls it now (`r`) or stops watching it (`w`: keep what you installed from it, or disable that too). **Removing a skill disables it** (`d`, or Disable in its menu): every copy of it moves out of your agents' reach into `~/.bro/disabled-skills/<name>`, with a note of where each was, and the **Disabled** page enables it again (`e`: each copy goes back where it was) or deletes it for good (`x`, asked once more). bro's own skills are switched off instead, and Codex keeps its own. **Discover** features Theo's [tstack](https://github.com/t3dotgg/tstack) and browses [Skill Issue](https://skill-issue.in), 300+ agent skills ranked by what actually gets used: its domain tabs (`←`/`→` or `1`–`9`), its ranking or most clicked (`s`), a search (`/`), and `enter` installs one, asking where (`i` installs it everywhere straight away, `p` into a project). bro watches just that skill's folder in its repo (`owner/repo#skill`), so it stays in step, and `d` disables it. bro's own skills (`bro-open`, JustTools', JustWatch's, JustImagine's) have their switches on the bro page and in Settings › Skills. From a shell: `bro skills installed [--project <dir>]`, `bro skills install t3dotgg/tstack council full-send [--project <dir>]`, `bro skills disable <skill>`, `bro skills enable <skill>`, `bro skills disabled`, `bro skills search react --domain frontend`, then `bro skills install <id>`.
- **Sounds** — PeonPing voice lines when a session starts, finishes, fails or needs you, in Claude Code, Codex, Pi and omp. Settings › Integrations › Sounds (or "sounds" in the palette): pick a pack, preview and install from the registry, choose which moments speak. Shares peon-ping's config and packs when it's installed. See [docs/sounds.md](sounds.md).
