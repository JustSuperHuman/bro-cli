I read every assigned file in full, plus the code that wires them up (`app/files.rs`, `app/mouse.rs`, `app/commands.rs`, the parts of `panes/term.rs` that use these helpers, `services/*`, and `bro-core` claude_env/config/quick_install). I edited nothing and ran no builds. Paths below are relative to `F:\bro-cli-v2\crates\bro-tui\src\` unless they start with `F:\`.

# 0. Read first

1. **bro-tui only builds programs, not a library.** `F:\bro-cli-v2\crates\bro-tui\Cargo.toml` has `[[bin]] bro` and `bro-app`, and no `[lib]`. Every module in `main.rs:7-53` is a private `mod`. A separate GUI crate cannot import anything here today. To reuse code, either give bro-tui a `[lib]` target or move the logic into bro-core or a new crate.
2. **Every view is a ratatui `Pane`** (`pane.rs:190-246`). It draws into `render(&mut Frame, Rect, &mut Cx)` and takes crossterm key and mouse events. The model state (selection, modes, pending jobs) sits in private fields of the view struct. Labels, values and help text are built straight into `ratatui::Span`. No view has a separate model type.
3. **Shared live data is `services::State`** (`services\mod.rs:111-133`), read through `Services`, a cheap clone wrapping a `RwLock<State>`. `Services` sends on a `Sender<pane::Event>`, and `Event` carries crossterm events and `file_view::Loaded` (which holds ratatui `Style`). A GUI needs this decoupled, for example behind a waker or callback trait.
4. **The file preview shows text only.** On load, these are handed straight to the OS default app (ShellExecuteW "open" on a `file://` URI on Windows, `open` on macOS, `xdg-open` on Linux) and the overlay closes:
   - images: png jpg jpeg gif webp bmp ico avif svg
   - video: mp4 m4v mov webm mkv ogv
   - audio: mp3 wav ogg oga opus m4a aac flac
   - PDFs, and any binary, >2 MB or non-UTF-8 file

   Evidence: `file_view.rs:172-176` and `186-190`, `app\files.rs:102-107`, the test `app\tests.rs:2797-2806` ("should open without a second keypress"), and `F:\bro-cli-v2\docs\file-links.md:169`. The media card drawn at `file_view.rs:614-626` is effectively never seen. bro never draws a picture of a file. Only the phone and web clients do: images inline as base64 (≤2 MB), video and audio streamed from `/raw` with HTTP Range.
5. The only image drawing in bro is for harness and provider logos (`icons.rs`, sixel/kitty/iTerm, `icons` setting). It is not used for previews.

---

# 1. Views

## 1.0 Common to all views
- **Registry:** `views\mod.rs:15-27` (`usage`, `profiles`, `proxy`, `bridge`, `stats`, `settings`, `quick-install`, `setup`).
- **Opening** (`app\commands.rs:127-134`, then `open_view` at `:455-477`):
  - Views are singletons.
  - If the view is already focused and there is more than one tab, the key goes back to the previous tab.
  - If it is open elsewhere, it gets focus. Otherwise it opens in a new tab.
  - Opening profiles calls `svc.refresh_data()`. Opening usage calls `svc.refresh_usage()`.
- **`Action::Open` of a view pane** closes any existing pane of the same view first (`app\mod.rs:773-781`).
- **Default keys** (`keymap.rs:327-336`, `379-400`; rebindable via `[keys]` in v2.toml):
  - Direct: `alt+u` usage, `alt+U` sidebar usage details, `alt+o` profiles, `alt+y` proxy, `alt+g` bridge, `alt+,` settings.
  - After the prefix (default `ctrl+space`): `u` usage, `U` usage details, `S` stats, `A` settings, `o` profiles, `y` proxy, `g` bridge, `I` quick install, `e` setup, `t` themes, `i` icons, `r` refresh usage, `f` last opened file, `F` agent files.
  - The palette lists every action.
- **Sidebar clicks:** usage, stats (the ⓘ on "usage left"), proxy, bridge and settings blocks (`app\mouse.rs:214-220`).
- **Setup auto-open:** setup opens once when profiles load, if there are no tabs, it is not the demo, `setup_done=false` and nothing is signed in (`app\welcome.rs:12-29`).
- **Hint line:** every view draws hints on its bottom row. They are **not clickable** (`ui.rs:482-490`).
- **Unused keys:** `?` opens help and `Esc` focuses the sidebar (`app\input.rs:98-106`).
- **Pane icons:** settings "settings", stats "stats", usage "gauge", profiles "account", proxy "proxy", bridge "bridge", quick-install "settings", setup "rocket".
- **Side effects in `render`:** first-render checks run there. A GUI must trigger them when a view opens.
- **Selection marker:** most lists mark the selected row with a left `▌` bar on a `t.frame` background. Your global CLAUDE.md forbids accent bars in designs, so the GUI should use background or border instead.

## 1.1 Settings — `views\settings.rs`
Saved to `~/.bro/v2.toml` through `svc.save_settings` (`services\mod.rs:353-364`: updates State, then writes atomically on a thread; a no-op in the demo).

**Layout** (`render` `:999-1107`):
- The hint line reads `j/k move · tab next section · enter/→ change · ← back`, plus `r reset to preset` (Preset/Knob rows) or `r bro's default` (Watchdog rows).
- At width ≥76, a 28-column section list sits on the left:
  - A " SETTINGS" title.
  - One line per section: icon, label, and a summary on the right (`summary` `:937-977`).
  - The current section gets a ▌ and a lit background.
  - " saved to ~/.bro/v2.toml" at the bottom if there is room, and a │ divider.
- Narrower than 76: no list; the section title says "N of 7".
- The body is at most 80 columns. Each section shows its title, a blurb (`:320-330`), a ─ rule, then items. Each item has:
  - its name, and a tag ("changed", "+ N changes");
  - its value on the right;
  - wrapped help;
  - notes (wrapped text, or a progress bar).
- ▲/▼ appear at the right edge when there is more above or below.
- Scrolling: the first row of a section scrolls to the top. The last row scrolls to the bottom, which reveals the Claude launch preview. Otherwise the selection is kept on screen.

**Value styles:**
- Switch: `● on` (good, bold) or `○ off` (muted). An "on" that does nothing yet is muted.
- Choice: `‹ value ›`. Enter-only rows show `value ›`.

**Section summaries** in the left list:

| Section | Summary |
|---|---|
| Appearance | theme name |
| Sessions | "N of 4 on" |
| Caveman | level, or off |
| Watchdog | "N idle" |
| Claude Code | preset label, "+N" when overridden |
| Integrations | spinner, "updating", or "update" |
| Updates | spinner, "→ x.y.z", or the current version |

**Keys** (`:1114-1131`):
- `j`/`↓` and `k`/`↑` move through rows across all sections.
- `g`/Home and `G`/End go to the first and last row.
- Tab goes to the next section and shift+Tab/BackTab to the previous one, wrapping (`jump` `:479-486`).
- Enter, space, `l` and → step the value by +1. `h` and ← step by −1 (`step` `:555-612`).
- `r` resets (`:614-634`).

**Mouse** (`:1132-1152`):
- The wheel moves the selection, not the viewport.
- Clicking a section in the left list selects its first row.
- Clicking any line of an item selects it. Clicking the already-selected item steps it by +1.

**Polling:** `tick_every` is 100 ms while a JustTools job, a Smart Paste switch, or a bro update or check is running. It is 3 s while watching a JustImagine update. Otherwise there is no tick (`:1108-1113`).

**First render:** calls `check_justtools()` and `check_justimagine(true)` (`:1001-1005`). When the view opens it also checks for the upstream Caveman plugin (`:470-472`).

**Rows, in order** (`rows()` `:255-268`, `Section::of` `:285-295`). Without the font row the list has 45–47 rows: 46 on Windows, where Smart Paste and the JustWatch skill appear.

| # | Section | Row (label) | Setting key / effect | Values and how they change |
|---|---|---|---|---|
| 1 | Appearance | Theme | `theme` | Enter, ← or → opens the theme palette (`Act::Themes` → palette "theme ", live preview) |
| 2 | Appearance | Font (only when `fonts::available()`: Just Terminal fork with `--app-window`, or `BRO_TERMINAL_FONTS=1`, `fonts.rs:75-80`) | `font` | "terminal's own" plus 19 bundled fonts (`fonts.rs:35-56`). ←/→ wraps and applies at once: unpacks to `~/.bro/fonts`, then sends OSC 50 with a Nerd symbols fallback. Re-applied at startup (`app\mod.rs:795`). Help shows the font's blurb. |
| 3 | Appearance | Digital rain | `rain` | off, everywhere, or working sessions (wraps) |
| 4 | Appearance | Summary screensaver | `summary_screensaver_secs` | off, 1, 2, 5, 10, 15 or 30 min (0/60/120/300/600/900/1800 s; wraps; an off-list value snaps up) |
| 5 | Appearance | Shared tile borders | `shared_borders` | on/off toggle |
| 6 | Appearance | Project tabs | `project_tabs` | on/off toggle |
| 7 | Appearance | Nerd Font icons | `nerd_font` and the global `ui::NERD` | `Act::ToggleIcons`, with a toast |
| 8 | Sessions | Auto-resume | `auto_resume.enabled` (default on) | `Act::ToggleAutoResume`, with a toast |
| 9 | Sessions | Auto-approve prompts | `auto_approve` (default off) | toggle |
| 10 | Sessions | Recaps | `insights.generate_recaps` (default on) | toggle |
| 11 | Sessions | Request compression | `proxy.trim` | `Act::ToggleTrim` → `svc.set_trim`: applies to the running proxy and toasts |
| 12 | Caveman | Caveman mode | `caveman.on` and `caveman.level` | off, caveman, ultracave, megacave (wraps). Help per level. A note appears if the upstream Caveman plugin is in `~/.claude`. |
| 13 | Caveman | Claude subagents too | `caveman.subagents` (default true) | toggle; shows a muted "on" while caveman is off |
| 14 | Watchdog | Watchdog prompt | `watchdog.prompt` (None means bro's default) | shows "bro's default ›" or "customized ›". Enter opens the prompt editor overlay (`Act::WatchdogPrompt`). `r` sets None. |
| 15 | Watchdog | Wake after | `watchdog.idle_secs` | 20, 60, 120, 300, 600, 1800 s; clamps, no wrap (`watchdog\mod.rs:22,36`); `r` resets to 60 |
| 16 | Watchdog | Reviewer | `watchdog.reviewer` | "auto", then every authenticated Claude/Codex profile id, plus the current value (wraps); `r` resets to auto |
| 17 | Watchdog | Reviewer model | `watchdog.model` | opus, fable, sonnet, haiku (wraps; an unknown value jumps to opus); `r` resets to opus |
| 18 | Watchdog | Wake-up budget | `watchdog.max_wakes` | 10, 25, 50, 100, 250 (clamps); `r` resets to 25 |
| 19 | Claude Code | Preset | `claude_env.preset` | Standard, Quiet, Private, Lockdown, Incognito, Presenter, Marathon (wraps). Switching clears overrides. Tag "+ N changes". `r` clears all overrides. Definitions: `F:\bro-cli-v2\crates\bro-core\src\claude_env.rs:273-317`. |
| 20–39 | Claude Code | the 20 knobs, under sub-headings PRIVACY, UPDATES, LONG RUNS, FEATURES | `claude_env.overrides[knob_id]` | Each cycles its options (wraps). Picking the value the preset already gives drops the override (`set_choice`). Value style: override = bold shine + "changed" tag; preset = inline; default = muted. `r` removes the override. |
| 40 | Integrations | JustTools | state only | See the Integrations section below |
| 41 | Integrations | Quick install | — | "open ›", opens the Quick Install view |
| 42 | Integrations | JustTools skill | `justtools.skill` (default false) | On downloads the skill (shows a spinner "adding"). Off removes bro's copies everywhere and toasts. Refuses if JustTools is not installed. |
| 43 | Integrations | Auto-update JustTools | `justtools.auto_update` (default true) | toggle |
| 44 | Integrations | JustWatch skill (Windows) | `justwatch.skill` | On needs JustWatch installed and fetches the skill. Off removes it and toasts. |
| 45 | Integrations | Smart Paste in Explorer (Windows) | JustPaste's own state, not in v2.toml | See below |
| 46 | Integrations | JustImagine | state only | See below |
| 47 | Updates | bro version | self-update | See below |
| 48 | Updates | Check for updates | `updates.check` (default true) | toggle; turning it on runs a quiet check. Checks happen at startup and every 12 h; each release is announced once. |

**The 20 knobs** (`claude_env.rs:78-257`; options[0] is the default):

- **privacy**
  - `error_reporting`: default / off
  - `feedback`: default / off
  - `fingerprint`: default / off
  - `telemetry`: default / off
  - `network` ("Nonessential traffic"): default / off
  - `connectors` ("claude.ai connectors"): default / off
  - `secrets` ("Secrets in tool shells"): default / scrubbed
  - `mcp_env` ("MCP server env"): default / minimal
  - `history` ("Transcripts & history"): default / off
  - `memory` ("Auto memory"): default / off
  - `screen_share` ("Screen-share safe"): default / on
- **updates**
  - `updates` ("Claude updates"): default / manual / frozen
- **long runs**
  - `retry` ("Overload errors"): default / ride out
  - `shell_timeout` ("Shell command time limit"): default / long
  - `cache_ttl` ("Prompt cache lifetime"): default / 1 hour / 5 minutes
  - `compact` ("Auto-compact"): default / early (70%) / off
- **features**
  - `tool_search` ("Tool search via proxy"): auto / Claude default. It only applies to launches proxied to an Anthropic upstream.
  - `agent_teams` ("Agent teams"): default / on
  - `focus` ("Focus view"): on / Claude default (sets `--settings viewMode`)
  - `spinner_tips` ("Spinner tips"): off / Claude default (sets `--settings spinnerTipsEnabled`)

**Claude launch preview** (under the knobs):
- A "NEXT CLAUDE LAUNCH SETS" sub-heading.
- One line per `K=V` from `env.vars(false)`: the always-on `CLAUDE_CODE_NO_FLICKER=1` first; tool_search is excluded.
- Then `--settings {json}`.
- Then a muted "more variables: [claude_env.extra] in ~/.bro/v2.toml" (`env_parts` `:651-659`, `blocks` `:768-801`).

**Integrations details** (`integrations` `:836-913`, `tool` `:490-524`, `imagine` `:527-552`):
- **JustTools row**
  - Value states: spinner "installing"/"updating"; "checking…"; "not installed"; `v → new` (an update is available); offline; up to date.
  - Notes: a step progress bar (`━━━━` per finished step, then "N of 5 · step label · Ns"), `✗ error · enter tries again`, `✓ done`, "in <bin> · restart bro so new sessions find it on PATH", or the GitHub/SHA-256 note.
  - Enter: if not installed, opens Quick Install. In the demo it toasts. With an update, it installs. Otherwise it checks again. It is ignored while a job runs.
- **Smart Paste** (`:916-934`)
  - States: switching spinner; checking; on/off; "needs JustTools"; "update JustTools"; "not in this JustTools".
  - Notes: "paused from its tray icon…", or "won't start at sign-in · enter twice turns that back on".
  - Enter:
    - while switching: nothing;
    - no JustTools: opens Quick Install;
    - JustTools without JustPaste but an update exists: updates JustTools;
    - JustTools without JustPaste: toast;
    - otherwise: `set_smart_paste(!running)`.
- **JustImagine**
  - Value states: "updating in a tab"; checking; not installed; `+N new commits`; unmanaged copy; GitHub error; up to date with a URL; up to date.
  - Notes: "open at <url> · starts at login through <mechanism>", or (danger) "its service isn't answering · `bro install justimagine --force`…".
  - Enter:
    - not installed: opens Quick Install;
    - an update: opens a tab running `bro install --wait justimagine --force`, then refreshes every 3 s for up to 10 min until the commit moves;
    - running: installs its skills on a thread, then opens the signed-in URL in the browser;
    - otherwise: checks again.
- **bro version** (`blocks` `:803-831`)
  - Value states: "⠋ updating" with a progress note over 3 steps (download, SHA-256 check, replacing bro); "x installed · restart bro"; "checking…"; no prebuilt for this platform; `cur → new`; offline; newest.
  - Enter: installs if a release is available, otherwise checks (not quiet).

**Not in this view** (v2.toml only, or other UIs): `prefix`, `shell`, `usage_expanded` (alt+U), `sidebar_width` (drag), `icons` (image protocol), all of `bridge.*` (`enabled` is the bridge view's `t`; port, bind, automatic_port, web_interface, remote_min_*), `proxy.enabled`/`port`/`trim_route_native`/`trim_level`, all of `voice.*`, `auto_resume.prompt`/`max_attempts`/`after_limit_reset`, `permission` (set by the launcher), `claude_env.extra`, `keys`, `setup_done` (setup view). Struct and defaults: `F:\bro-cli-v2\crates\bro-core\src\config.rs:135-186`, `321-377`.

## 1.2 Stats — `views\stats.rs`
Token totals per login, in two tabs: "tokens" and "profiles".

**Scan** (`start` `:87-113`):
- Runs on a thread over the profiles from `state.profiles`, one per login directory (deduplicated by `(is_claude, path_key(dir))`).
- Uses `bro_core::usage::tokens::scan` (cached per file) and `claude_counter` (Claude's own counter).
- Logins with no counter and no log files are dropped.
- Starts on the first render once profiles are ready.

**First load, no report yet** (`:116-135`):
- A centred gauge "tallying every token", a byte progress bar, "X of Y" or "finding logs…", and "only new logs next time" on the right.
- Before profiles are ready: "waiting for your logins…".
- A rescan keeps the previous report on screen.

**Top row:**
- Tab chips (the active one on an accent background) are click targets.
- The right side shows status: spinner ◐◓◑◒ with "reading logs X / Y", or "logs read just now / N ago in X.Xs".

**Tokens tab** (`tokens_lines` `:279-355`):
- Headline: lifetime total (≥ when it is only a floor counted from logs), last-30-days total, and each provider's % share.
- Header columns: login, lifetime, last 30d, share bar with %, and a 30-day sparkline axis "Mon d ╌ Mon d".
- Rows are grouped per harness (glyph and label in the brand colour), with a subtotal when a provider has more than one login, a rule, a total row, and a "≥ at least — counted from the logs still on disk" footnote.
- The Claude counter is topped up from transcripts for days after its `through` date (`token_figures` `:254-271`).
- The share bar shows only if there is room (6–28 cells). The sparkline needs ≥58 spare columns (`cols` `:231-239`).

**Profiles tab** (`:358-435`):
- Headline: 24h, 7d and 30d totals.
- Per-login columns 24h/7d/30d (`·` for zero) and a 30-day mix bar: input in shine, cache write #A78BFA, cache read #60A5FA, output in good (`kinds` `:189-191`).
- A total row, a legend, and "X% of the last 30 days was re-read from cache · Y% was written by the model".
- With no logins: "no Claude or Codex logins yet — alt+o to add one".

**Number format:** `compact` ("9.62B", "38.1K").

**Keys** (`:523-537`): Tab, BackTab, ←, →, `h`, `l` toggle tabs; `1`/`t` tokens; `2`/`p` profiles; `r` rescans (ignored while running); `j`/`k`/arrows scroll.

**Mouse:** click a tab chip; the wheel scrolls.

**Tick:** 250 ms while scanning.

## 1.3 Usage — `views\usage.rs`
- Header: " USAGE " then "just refreshed", "refreshed Ns ago · every 60s", or "not refreshed yet" (from `state.usage_at`). While profiles load it shows `st.profiles.why()`.
- One 5-row block per profile (`block` `:42-106`):
  - **Row 1:** harness glyph, id (bold brand colour), plan (from usage, else the profile, else "—"), "· email". On the right: " + session " if authenticated, the " large task " pick (good, reversed), the " small task " pick (shine, reversed), and "↻" while fetching.
  - **Rows 2–3:** 5h and week bars (`ui::bar` eighth-blocks, width clamped 10–48), % coloured by `pct_color` (≥85 danger, ≥60 inline, else good), "resets in <countdown>". Missing data shows "—".
  - **Row 4:** headroom "now ≈X%" (≈ when capped), "week Y%", "capped by the weekly ratio", and up to 2 scoped windows ("name N%").
  - **Row 5:** "last refresh failed: …" in danger colour.
  - With no usage: "fetching…", the error, "not logged in — alt+o to log in", or "no reading yet".
- With no profiles: "no profiles yet — alt+o to add a Claude account or Codex profile".
- **Click** anywhere on an authenticated profile's block: `Action::LaunchUsage(harness, Some(id))` → `launch_usage` (`app\commands.rs:411-430`), a quick-launch on that account, or the launcher if a quick spec is unavailable.
- **Keys:** `r` → `refresh_usage` plus the toast "refreshing usage…"; `j`/`k` scroll one profile at a time (clamped).
- **Mouse wheel** scrolls but does not clamp. Wheeling past the end leaves the view blank until you wheel back (minor bug: `:173`).
- **Data:** `state.profiles`, `state.usage[id]` (UsageEntry{usage, headroom, error, fetching}), `state.picks`, `state.usage_at`.

## 1.4 Profiles — `views\profiles.rs`
Modes: List, Add(kind, name), Confirm(profile); pending: Create or Remove (`:18-29`).

**Layout** (`render` `:159-227`):
- Smaller than 8 rows or 30 columns: "Enlarge this pane to manage profiles".
- Title " Profiles · Claude and Codex logins".
- Clickable buttons (List mode only): " + Claude (a) ", " + Codex (c) ", " Log in (l) ", and " Remove (d) " (hidden for machine logins).
- Rows: `▌ id · email|Logged in|Not logged in · machine login`. The selected row is bold, in the brand colour, on the frame background.
- Footer in List mode: the selected profile's short dir path, plus "Machine login: can be used here; cannot be removed by Bro." or "Remove deletes this profile's login, settings and saved sessions."

**Add mode:**
- Prompt " Claude|Codex profile name: name▏".
- Detail line: an error, or "<CLI> isn't installed: Create installs it first, then log in with l.", or the rule "1–40 letters, digits, . _ - (example: work). 'local' is reserved."
- Buttons: " Create + log in (↵) ", and " Cancel (esc) " when width ≥37.
- Validation: `bro_core::profiles::valid_name`. Invalid Enter shows the error. Typing is capped at 40 bytes and ignores ctrl chords.
- Paste is supported: trimmed, control chars dropped, capped at 40 (`:301-309`).

**Confirm mode:**
- "Remove X? Deletes login, settings and saved sessions." in danger colour, the full directory, and buttons " Remove (y) " / " Cancel (esc) ".
- `y`/`Y` removes. Any other key toasts "kept it".

**Pending:** shows " Saving profile…"; all keys are swallowed; ticks every 50 ms.

**Create succeeds:**
- Selects the new profile and toasts "Created X. Complete sign-in in the login tab." (unless the CLI is missing).
- Then `open_login` (`:110-143`):
  - If the CLI is missing (not in the demo): toast and open a tab `bro install --wait claude|codex`.
  - Otherwise: a `Term` tab labelled `"login · <id>"` (`LOGIN_LABEL` `:106`) running `svc.login_command(p)`, plus `svc.watch_login`. That polls `profiles::get` every 2 s for up to 20 min, toasts "<id> signed in as <email>" and reloads profiles.
  - Closing a login tab calls `login_closed`, which reloads profiles (`app\mod.rs:643-647`).

**Remove:** toast "Removed X" or the error. Machine logins refuse with "the local logins (~/.claude, ~/.codex) aren't removed by bro".

**Keys:** `j`/`k`/arrows, Home/End, PgUp/PgDn, `a` (add Claude), `c` (add Codex), `r` (`refresh_data` plus "reloading profiles…"), `l`/Enter (log in), `d`/Delete (remove).

**Mouse:** the wheel acts as ↑/↓; a click on a button sends its key; a click on a row selects it.

**Other entry:** `ProfilesView::adding(kind)` opens straight on the name prompt; setup uses it.

## 1.5 Proxy — `views\proxy.rs`
- **Header:** " PROXY ● :port base   N routes · M requests seen", or "○ <why>".
- **Compression line** (`trim_line` `:69-86`):
  - Off: "off   c turns on llmtrim (lossless; small savings…)".
  - On: "on · <trim_level> · ~X tokens saved in N requests (estimate) · native logins launched from now on go through the proxy" or "proxied launches only".
- **"routes" rule:** up to 6 routes, each with id (shine), label, upstream (`upstream_label` `:47-58`), and default model or "client's model". Empty: hint "none yet — launching a harness on a foreign provider…".
- **"claude pool" rule** (only if there is a pool): up to 6 accounts, each with ● ready / ◌ cooling <countdown> / ○ unavailable, name, "5h N req X in Y out $Z", and the last error.
- **"live requests"**, newest first:
  - time HH:MM:SS local (`hms`);
  - status code (2xx good, 429 inline bold, other 4xx inline, else danger bold);
  - "inbound → upstream", model (shine);
  - in, out and cache tokens; saved (`saved`: "—" when compression did not run);
  - latency in seconds (<2 s good, <8 s inline, else danger);
  - note (error in danger, "acct X", or "stream").
  - Empty: "no requests yet".
- **Keys:** `c` → `svc.set_trim(!trim)` with no toast (unlike `Act::ToggleTrim`); `j`/`k` scroll the log toward older entries; `g`/Home jumps to the newest.
- **Mouse:** the wheel scrolls 3 rows.
- **Data:** `state.proxy` (status Avail<ProxyInfo>, routes, events ring capped at 400, pool) and `settings.proxy`.

## 1.6 Bridge — `views\bridge.rs`
- **Header:**
  - Running: " BRIDGE ● running on :port   N client(s) connected".
  - Stopped: "○ stopped <error>".
  - Not running: "○ off — t turns it on", or the reason.
- Then: "Pair the Just Terminal app or open a URL in a browser: your agent sessions show up there live."
- **"urls":** each URL is labelled "this machine" (index 0), "tailscale" (contains `://100.`), or "lan". The selected one gets ▌. Tokens are masked (`mask`: 8 bullets plus the last 4; `show_url` `:25-36`).
- **"token":** shown masked or revealed, with "r reveals it" / "r hides it". The status error appears below.
- **QR:** at the right, "scan to pair", drawn as white-on-black half-block text from `state.bridge.qr`. It encodes `Bridge::pairing_url()`, the first URL containing `?token=` (`F:\bro-cli-v2\crates\bro-bridge\src\lib.rs:720-740`, `net.rs:78`).
- **Keys:**
  - `r` reveals or hides the token.
  - `t` → `set_bridge_enabled(!enabled)` plus a toast ("starting the bridge…" / "bridge off").
  - `j`/`k` pick a URL.
  - `c`/Enter copies the URL with `clip::copy` and toasts "copied the pairing URL (it includes the token)".
- **No mouse handling.**
- **Data:** `state.bridge` (status Avail<BridgeStatus{running, port, urls, token, clients, error}>, qr, enabled). A status poller updates it every 2 s (`services\workers.rs:415-483`).

## 1.7 Quick Install — `views\quick_install.rs`
- **Look:** a background check (`check` `:115-138`) runs `bro_core::quick_install::detect()`, `Os::detect()` and `how(tool, os)` for every tool. The first look ticks the recommended missing tools: JustTools, Git, Node, Bun, Claude Code, Codex (`pick_defaults` `:141-146`).
- **Header:** " QUICK INSTALL " plus the OS description, or "looking at this machine…".
- **Groups** ("──── label"): Just apps, essentials, runtimes & SDKs, coding agents, utilities.
- **Tools** on this OS (`F:\bro-cli-v2\crates\bro-core\src\quick_install.rs:72-96`): JustTools, JustPaste\*, JustWatch\*, JustImagine, Git, GitHub CLI, Node.js LTS, Bun, Python 3, uv, .NET 9/10/11, Claude Code, Codex CLI, Pi, omp, ripgrep, FFmpeg, yt-dlp, Tailscale, PowerToys\*, ShareX\*. (\* Windows only.)
- **Each row:**
  - ▌ when selected;
  - a box: `[⠋]` (JustTools job or Smart Paste switching), `[✓]`, `[x]` ticked, or `[ ]`;
  - name in 16 columns (JustTools in bold shine) and ★ if recommended;
  - detail: version or "installed", step label, "switching on", justpaste status, "checking…", or "not installed";
  - blurb.
  - Below JustTools: `✗ error` or `✓ done`.
- **Install row:** "▶ Install N · names (6 shown, then "N more")", or "nothing ticked to install". Below it:
  - "each installer runs in a new tab so you can follow it and answer its prompts";
  - while watching: "open a new terminal or restart bro afterwards so new sessions find the new tools on PATH";
  - for the selected tool: "<Tool> · how: <how text>".
- **Install** (`:184-203`):
  - JustTools installs in the background (`install_justtools`).
  - JustPaste is switched on once JustTools is installed and idle (`poll` `:246-252`).
  - The rest run in a new tab `bro install --wait <ids…>` (`install_term` `:36-41`, `bro_term` `:44-64`).
  - It then watches for 15 min, re-checking every 3 s. When JustWatch lands, its skill is turned on. When JustImagine lands, `check_justimagine(true)` runs.
  - Toggling an installed tool toasts "X is already installed · settings update it / settings turn it off / `bro install --force` reinstalls it". The demo installs nothing.
- **Keys:** `j`/`k`, `g`/`G`/Home/End, space/`x`/Enter activate (tick, or install on the Install row), `i` install, `a` tick all missing, `n` tick none, `r` look again (plus `check_justtools`).
- **Mouse:** the wheel moves the selection; a click selects **and** activates (it toggles at once).
- **Tick:** 100 ms while a JustTools job is busy; 3 s while watching.

## 1.8 Setup — `views\setup.rs`
- **Header:** " SET UP BRO " plus a status: "✓ ready: launch an agent below", "start here: install an agent", or "next: sign in".
- **1 · install an agent:** rows for Claude Code and Codex, each with a mark (…, ✓ with the short exe path, … "installing in another tab…", or ✗ "not installed · enter installs it") and a blurb. Then a note pointing to Quick Install.
- **2 · sign in:** three lines of explanation, then up to 6 profiles (● signed in with email; ○ "needs <CLI> first · enter installs it"; ○ "not signed in · enter opens its login"; "machine login"). Then "N more in profiles", "+ Add a Claude account", "+ Add a Codex profile", and a note.
- **3 · go:** "▶ Launch an agent" and "✓ Done · don't open this at startup again".
- **Actions** (`activate` `:92-117`):
  - Agent row → install tab.
  - Login row → `open_login`, or install if the CLI is missing; a missing profile toasts "is gone".
  - More → `Act::Profiles`.
  - Add → `ProfilesView::adding(kind)` in a tab.
  - Launch → `Act::NewSession`.
  - Done → `setup_done=true`, toast, close the pane.
- **Agent detection:** `agent_install::installed` on a thread, re-checked every 3 s for 15 min after an install starts. It toasts "<Agent> is installed · next: sign in" and stops watching once both agents are installed (`poll` `:152-173`).
- **Keys:** `j`/`k`, `g`/`G`, space/Enter, `r` (look again plus `refresh_data`).
- **Mouse:** the wheel moves; a click selects and activates.

---

# 2. File preview and related code

## 2.1 Entry points (`app\files.rs`)
- **Clicking a terminal link** whose URI is `file:` → `open_file_link` (`:47-53`) → overlay. A non-file link, or no session roots, goes to `Event::OpenLink` and the OS.
- **Clicking a file link on a turned-over card** (`app\mouse.rs:351-363`) works the same way.
- **`bro open`:** the bridge sends `BridgeCommand::OpenFile` → `agent_opened_file` (`:156-169`). It sets `last_opened` (one per app, not per pane). The preview opens **only** if that pane is focused and visible, the sidebar is not focused, no rename is in progress, and there is no overlay or only a file overlay. Otherwise it toasts "<session> opened <path:line> · <prefix f> to view".
- **prefix `f`:** `show_last_opened` (`:206-216`).
- **prefix `F`:** the agent files overlay (`:172-189`). Enter or a click opens the preview at the item's line.
- **Remote "Open on desktop":** `BridgeCommand::OpenInEditor` → `open_in_editor`, which always runs, focused or not (`app\sessions.rs:259-262`).

## 2.2 Loading and resolution (`file_view.rs:331-357`, worker thread per request)
- **Resolution:** `panes::file_cache::resolve_local` (`file_cache.rs:30-48`) is the bridge's `resolve_path` (`F:\bro-cli-v2\crates\bro-bridge\src\file_preview.rs:183-231`), tried in this order:
  - `file://` URLs, `~/`, and MSYS `/c/` are normalised;
  - absolute paths must canonicalise inside the session cwd or project root (the git root if the project is not set);
  - relative paths are tried against cwd, then the project, then with `a/` / `b/` stripped;
  - a unique suffix match in the project file index.
  - If none match, the **TUI only** (the bridge does not do this) also accepts any absolute or `~/` path straight from disk, except UNC/`//` paths and control chars.
  - A folder fails with "This is a folder, not a file." If the click had a fallback URI, that URI goes to the OS, so the file manager opens (`apply` `:179-185`).
- **Media check by extension** (`image_mime` / `media_mime`, `file_preview.rs:290-320`) → `Payload::Media{path, relative, mention, mime, bytes}`.
- **Text:** `read_preview` (`file_preview.rs:324-411`):
  - limit 2 MB; a NUL byte means "binary"; non-UTF-8 is rejected;
  - CR is trimmed and the final newline dropped;
  - the language comes from `language_for`;
  - window: the line minus 21 (`CONTEXT_ABOVE` 20), then `DEFAULT_LIMIT` 300 lines; markdown loads from the top up to `MAX_LIMIT` 5000;
  - content is capped at max(limit×400, 120 000) chars.
  - Any read error whose path exists becomes `Err((msg, Some(path)))`.
- **Mention:** `@rel/path` relative to the session cwd, else absolute; `@"…"` when it contains whitespace (`mention_for` `:360-367`).
- **Stale replies** (an older request id) are dropped (`apply` `:162`).

## 2.3 Rendering text
- **Highlighting:** syntect with two-face's extended grammars (bat's set). The syntax is chosen by extension, then file name, then language token (shell→sh, powershell→ps1, csharp→cs, cpp), then first line. "Plain Text" counts as none (`highlight` `:469-520`).
- **Theme mapping:** a synthetic syntect theme whose colours encode slot numbers (r=slot, g=0x5a, b=0xa5) maps scopes to 7 theme slots: fg, muted, good, inline, accent, shine, danger, plus bold/italic/underline (`Palette` `:397-466`; full scope table `:440-461`). This keeps code in the bro theme, including indexed colours.
- **Budget:** highlighting stops after 1.5 s and the rest stays plain (`HIGHLIGHT_BUDGET` `:26`).
- **Cleaning:** tabs become 4-column stops; control chars become `·` (`clean` `:370-391`).
- **The theme is captured when the preview opens.** A later theme change does not re-highlight.
- **Popup** (`draw` `:563-687`):
  - width = area width − 4 (max 200), height = area height − 2;
  - title: file icon plus "relative:line";
  - header left: "lang · N lines · size   <full path>" (path in the frame colour);
  - header right: "top–last of total", with "→N ·" when scrolled sideways and "loading… ·" while paging;
  - body: a line-number gutter (width max(3, digits)). The target line gets a bold accent number, a ▌ bar and a background tint `project_tint(accent)`. Unloaded lines show "…".
  - `center_on_target`: top = line − rows/3; if column > 60, scroll left = column − 20 (`:254-262`).

## 2.4 Paging (`scroll` / `page` `:231-252`)
- When the view comes within `PREFETCH` (20) lines of the loaded edge, it requests ±300 more lines.
- The window is capped at 5000 lines and the far side is dropped.
- A failed page keeps what is shown.
- One `std::thread` is spawned per request.

## 2.5 Keys, mouse and hints
- **Keys** (`key` `:264-292`):
  - Esc/`q` close.
  - `j`/`k`/↑/↓ move one line.
  - ctrl+d / ctrl+u move half a page.
  - PgDn, space and ctrl+f move a page down; PgUp and `b` a page up.
  - `g`/Home top; `G`/End bottom.
  - `h`/← and `l`/→ scroll sideways 8 columns; `0` resets.
  - `e`: editor for text; system app for media.
  - Enter: editor for text; system app otherwise.
  - `o`: system app.
  - `c`/`y`: copy path.
  - `@`/`i`: mention.
- **Mouse** (`:294-312`): the wheel scrolls ±3 lines; the horizontal wheel ±8 columns; a click on a hint fires its action; a click outside the popup closes it.
- **Hints** are clickable and underlined on hover (this needs mouse-move tracking):
  - text: `e editor · o system app · c copy path · @ mention · ↑↓ pgup pgdn scroll(inert) · esc close`
  - media: `⏎ open · c copy path · @ mention · esc close`
  - failed with a path: `⏎ system app · c copy path · esc close`

## 2.6 Outcomes (`app\files.rs:66-109`)
- **Copy:** `clip::copy(path)` and the toast "copied <path>".
- **Mention:** closes the preview, focuses the source session, leaves the sidebar, and pastes `"@path "` with bracketed paste. If the session is gone: "that session has closed".
- **Editor:** `open_in_editor` (§2.8).
- **System:** `Event::OpenLink(file_uri)`, which calls `links::open` on a thread (`app\mod.rs:1034-1043`). Errors are toasted.
- **Fallback(uri):** sends the original URI to the OS.

## 2.7 Images, video, audio and PDF today
- **TUI:**
  - Media arrives as `Payload::Media`; `apply` returns `Outcome::System` at once, the overlay closes, and the OS default viewer or player opens. There is no confirmation.
  - PDFs and other binaries go to `Body::Failed{path: Some}` (NUL byte or >2 MB), then `Outcome::System`, then the OS (PDF viewer, Office, and so on).
  - The same happens for `bro open chart.png` from an agent: the desktop OS viewer pops up (only if that session is being watched).
  - **Risk:** a clicked link to a `.exe`/`.msi`/`.scr` is binary, so it is likely ShellExecute'd with the "open" verb, which runs it. Text scripts (.bat, .ps1) preview safely. Worth a guard in the GUI.
- **Phone and web, via the bridge:**
  - `GET /api/sessions/{id}/file`: text windows; images as base64 (≤2 MB); video/audio as kind + mime + bytes + `url=/api/sessions/{id}/raw?path=…` (`F:\bro-cli-v2\crates\bro-bridge\src\rest_sessions.rs:446-483`).
  - `/raw` streams with Range, has no size cap, and serves PDF as `application/pdf` and SVG under a CSP sandbox (`raw_file.rs:1-43`).
  - `open` publishes a WS `openFile` event to remote clients and returns the client count (`rest_sessions.rs:612-642`).
  - The bridge is confined to session roots. The TUI is not.

## 2.8 Agent files overlay — `agent_files.rs`
- **Items** (`items` `:82-102`) come from `Snapshot.facts`:
  - "sent": Claude's `SendUserFile`, with a caption.
  - "touched": edit, write, create, delete, move, read, with line numbers.
  - Each section is newest first.
  - `shown_path` (`:62-68`) is relative inside cwd or project, else absolute.
- **Popup:** width = area width − 6, clamped 40–110; height = items + 9, at least 10. Title "agent files · <session>". Hints "⏎ preview · ↑↓ move · esc close".
- **Section headers:** "sent to you N" and "touched N".
- **Rows:**
  - ▌ when selected;
  - a glyph: ➤ sent, ✎ edit/write, + create, − delete (name struck through), → move, · read;
  - `path:line` (bold accent when selected), then the caption in muted text;
  - on the right: "kind  ago".
- **Empty states:** "nothing sent or touched yet"; "waiting for the agent's transcript (it appears once the agent has answered)"; "<Harness> sessions don't report their files"; "a shell has no agent files"; "agent files: focus an agent session first" (toast).
- **Keys:** Esc/`q`, `j`/`k`/arrows, PgUp/PgDn, `g`/`G`/Home/End, Enter opens.
- **Mouse:** the wheel moves ±3; a click opens the item at once.

## 2.9 Editor — `editor.rs`
- **Choice:** `$BRO_EDITOR`, then `$VISUAL`, then `$EDITOR` (whitespace-split, quotes honoured), then `code` if it is on PATH, else the OS default app (`plan` `:76-143`).
- **Per-editor argument syntax:**
  - VS Code family (`code`, `cursor`, `windsurf`, …): `-g p:l:c`, with `--wait`/`-w` removed.
  - JetBrains: `--line`, `--column`.
  - subl, zed, lapce, hx, micro: `p:l:c`.
  - notepad++: `-nL`.
  - kate, mate: `-l`.
  - nano: `+l,c`.
  - emacs, kak: `+l:c`.
  - vi family: `+l`.
- **Terminal editors** (vim, nvim, nano, hx, … and `emacs -nw`) come back as `Event::EditorPane`. They open in a new bro pane split beside the source session (Split, or Tab when there are no tabs), named "<editor> <file>", with cwd = the session cwd or the file's folder (`app\files.rs:142-152`).
- **GUI editors** spawn detached with `CREATE_NO_WINDOW` and toast "opened app.rs:42 in code".

## 2.10 `bro open` / `bro ctl` (command line, not UI)
- **`open_cmd.rs`:**
  - Usage: `bro open <path>[:line[:col]] [--editor|-e] [--session id]`. It accepts `:L`, `:L:C`, `#L12`, `#L12C3` and `(L,C)`.
  - It needs `BRO_SESSION_ID`, plus the bridge from `BRO_BRIDGE_URL` or the discovery file in `~/.bro/bridge`.
  - An existing relative path is made absolute against the cwd.
  - It POSTs `/api/sessions/{id}/open` or `/open-in-editor` and prints "opened X in bro and N connected clients".
  - It fails when the bridge is off.
- **Pane environment** (`panes\term.rs:355-371`): `BRO=1`, `BRO_SESSION_ID`, `BRO_BIN`, `BRO_BRIDGE_URL`, `TERM=xterm-256color`, `COLORTERM=truecolor`, with `NO_COLOR` and the Claude session vars removed.
- **The bro-open skill** is written into each launched harness home (`F:\bro-cli-v2\crates\bro-core\src\skills.rs:11-25`).
- **`ctl_cmd.rs`:** `bro ctl [tool] [k=v…|json]` makes JSON-RPC `tools/list` / `tools/call` requests to `<bridge>/mcp` with a 90 s timeout. It is used by the orchestrator on Pi/omp.

## 2.11 Clipboard — `clip.rs`
- **`copy`** (`:36-73`): writes OSC 52 to **stdout** and also runs an OS tool on a thread (clip.exe with UTF-16LE + BOM; wl-copy, xclip or xsel). It does nothing in tests.
- **`grab_text`** (`:76-105`): text, or on Windows the copied file list in quoted paste form. Used by ctrl+v, ctrl+shift+v and shift+Insert, on a worker (`app\input.rs:18-31`).
- **`grab_image`** (`:110-177`, used by alt+v): saves a clipboard image as `~/.bro/paste/paste-<ms>.png` or returns copied file paths. On Windows it prefers the raw "PNG" format and validates it, falling back to PowerShell STA. The paths are pasted into the terminal with the toast "pasted image as a path · Claude Code and Codex attach it" (`app\mod.rs:1054-1059`).
- **Selection copy:** done after the next draw, from the rendered buffer, with the toast "copied N characters" (`app\draw.rs:514-520`).

---

# 3. Terminal-pane helpers

## 3.1 Link detection — `panes\links.rs`
- **`detect_with(vt100::Screen, files)`** (`:61-111`) scans the visible screen only.
  - Soft-wrapped rows join into one logical line. Hard newlines end links.
  - Wide cells are handled.
  - Result: `Links = HashMap<(row, col), Arc<str>>` (`:10`).
- **Priority:** OSC 8 hyperlink targets (`cell.hyperlink()`, filtered by `target`) win over plain URLs, which win over file paths. A file candidate overlapping a taken cell is skipped.
- **Plain URLs** (`web_urls` `:113-151`):
  - `http(s)://` or `www.` (which gets an `https://` prefix);
  - not preceded by an alphanumeric, `_`, `/` or `@`;
  - stops at whitespace and `<>"'\``;
  - trailing `.,;:!?` and unbalanced closing brackets are trimmed.
- **`target`** (`:38-50`) allows only http/https (with a host), mailto, and file URLs. It rejects strings over 8192 chars and control chars, so `javascript:` and `data:` are out.
- **File candidates:** `bro_bridge::file_links::find_file_links` (`F:\bro-cli-v2\crates\bro-bridge\src\file_links.rs:171-279`) finds:
  - markdown `[label](path)`;
  - backticked or quoted paths (spaces kept);
  - paths with spaces that end in a location suffix;
  - bare paths with a separator;
  - bare `name.ext`, and Dockerfile, Makefile, LICENSE, .gitignore, .env;
  - `file://` URLs;
  - locations `:L`, `:L:C`, `#L12`, `#L12C3`, `(L,C)`.
  - It excludes URLs, emails and versions.
  - Only paths the cache confirms become links.
- **File links** are `file://` URIs with the location in the fragment `#L42C3` (`file_uri` / `parse_file_uri` `:13-35`).
- **Every detected link cell is drawn underlined, always** (`term.rs:1180`). `link_at` adds the `view_top` offset (`term.rs:1218-1220`).
- **Clicking** (`app\mouse.rs`):
  - The press records the link; it opens only on release **at the same cell**, so dragging still selects (`:333-349`, `:444-447`).
  - If the program uses the mouse, ctrl+click is needed. Shift skips link detection, to allow selection (`:371-376`).
  - ctrl+click elsewhere toggles the "priority" size (`:379-386`).
  - `file:` links go to the preview; everything else goes to `links::open` (`:154-171`: ShellExecuteW "open" / `open` / `xdg-open`; the link is passed as data, never through a shell).

## 3.2 File cache — `panes\file_cache.rs`
- One cache per terminal pane, rooted at the session cwd and project root.
- Rendering asks the cache only (`Lookup::get` `:160-174`). Unknown or stale candidates are queued, and `finish` resolves up to 200 per batch on a thread, then wakes the pane (`:177-194`).
- **Freshness:** found entries are trusted for 30 s, missing ones for 8 s. A found file keeps its link while being re-checked, so links do not flicker. The cache resets at 4096 entries.
- **Absolute paths resolve first.** Their folders (up to 8, newest first) are remembered so a bare `name.png` printed later is found next to them (`near`; no `..` allowed).
- **Folders link too.** A click opens them in the file manager through the preview's fallback.

## 3.3 Key and mouse encoding — `panes\keys.rs`, plus `term.rs` paste
- **`encode_key`** (`:8-79`) produces xterm sequences:
  - ctrl+letter gives 1–26; ctrl+space/@/2 gives 0; `[`/3 gives ESC; and so on.
  - shift+Enter gives `ESC CR` (the newline chord from Claude's /terminal-setup).
  - shift+Tab and BackTab give `ESC[Z`; ctrl+Backspace gives 0x08.
  - Arrows, Home and End are CSI, or SS3 when application-cursor (DECCKM) mode is on. Modifiers use `1;m` with m = 1 + shift + 2·alt + 4·ctrl.
  - Insert/Del/PgUp/PgDn are `n~`. F1–F4 are SS3, F5–F12 are `15~`…`24~`. F13+ is dropped.
  - alt prefixes ESC to chars, Enter, Backspace, Tab and Esc.
- **`encode_mouse`** (`:91-124`):
  - only when the program set a mouse mode;
  - SGR 1006 or legacy X10 bytes;
  - modifiers: shift +4, alt +8, ctrl +16;
  - motion only in ButtonMotion/AnyMotion; wheel codes 64/65;
  - coordinates relative to the pane's inner rect, plus `view_top` (`term.rs:1263`).
- **Without a mouse mode,** the wheel scrolls bro's scrollback 3 lines (`term.rs:1253-1260`).
- **Paste** (`term.rs:1236-1246`): CRLF and LF become CR. With bracketed paste, the text is wrapped in `ESC[200~ … ESC[201~` and any inner `ESC[201~` is stripped.
- **`send`** bumps the `typed` counter (auto-resume uses it to back off) and resets scroll to live (`:545-549`).
- **`submit`** (whole message, Enter 300 ms later as a separate write) and **`type_command`** (Enter 400 ms later): `:553-568`.

## 3.4 PTY writer — `panes\writer.rs`
- One "bro-pty-input" thread per pane fed by an mpsc channel. Order is kept and the UI never blocks on a stalled child.
- `flush` is a no-op on the caller's side.
- The module is private (`mod writer;` in `panes\mod.rs:10`).

## 3.5 Remote minimum size — `panes\remote_min.rs` plus `term.rs`
- **Config:** `[bridge] remote_min_cols=80`, `remote_min_rows=50`, `remote_min_grace_secs=15`. A value of 0 turns that axis off (`config.rs:196-204`, `366-369`; `services\mod.rs:553-557`).
- **Effect:** while bridge viewers of the session are above 0, or within the grace period after the last one leaves (`RemoteHold::active` `:29-35`), the PTY grows to at least the minimum on each axis (`effective` `:40-46`).
  - `apply_size` runs from both `poll` (hidden panes too) and `render` (`term.rs:596-611`, `1081-1084`).
  - Each resize goes to the bridge (`set_pty_size` `:656-666`).
- **Tile display:** the tile shows the **bottom-left** of the bigger screen, pinned to the last text or cursor row (`view_top` `term.rs:1143-1157`). Scrolling up first shows the hidden rows, then scrollback.
- **Header** shows "⇲ COLS×ROWS remote" (`term.rs:1015-1018`).
- **Coordinates:** links, prompt-option clicks and mouse reports all add `view_top`.

## 3.6 Insights (transcript following) — `panes\insights.rs`
- **Start:** one "bro-insights" thread per Claude/Codex pane after spawn, not in the demo (`term.rs:400-404`).
- **`BindSpec`** (`term.rs:410-424`): harness, profile/store id, config dir from `CLAUDE_CONFIG_DIR` / `CODEX_HOME` in the spawn environment, pid, cwd, wall start time, and the resume id from the launch args (`resume_id` `:207-214`: Claude `--resume`/`-r`, Codex `resume`/`fork`).
- **Loop** (`run` `:113-150`), every 1 s:
  - `Binder::poll` re-binds when due;
  - `Tail` reads only new bytes; a reset clears the facts;
  - `AgentFacts::ingest` parses them;
  - it sleeps in 100 ms slices so a closed pane's worker exits promptly;
  - a parser panic resets the state.
- **`Snapshot`** (`:21-34`): facts (`Arc<AgentFacts>`: sent, touched, pending, asked, replies, prompt, titles, branch, plan, urls, api_error, turn_open, session_id, …), transcript path, session_id, `generated` (a cached `/recap` still matching the transcript; Claude only), `busy`, and `rev`.
- **`busy`:** Claude's `sessions/<pid>.json` status, or Codex `turn_open`. Pi/omp: None.
- **Readers:**
  - `busy()` and `activity()`: a pending question tool (AskUserQuestion, ExitPlanMode, request_user_input_async) makes the session Blocked (`term.rs:439-445`, `1044-1064`).
  - The agent files overlay.
  - Auto-resume's transcript stall.
  - `app\insights.rs`: the sidebar's second line, toasts, the bridge's attention, recap and agentFiles feeds, and the `/recap` scheduler (gated by `insights.generate_recaps`).

## 3.7 Auto-resume — `panes\auto_resume.rs` (pure, clock injected)
- **Settings:** `[auto_resume]` `enabled` (default true), `prompt` "continue", `max_attempts` 6, `after_limit_reset` true. Only `enabled` is in the settings UI.
- **Classifying errors** (`classify` `:259-278`):
  - "interrupted" → ignored;
  - Fatal: login, auth, billing, context length, 400/401/403/404/413/422;
  - Limit(reset time) when the message has one: `reset_time` `:290-337` parses "resets 2:40am", "Oct 9, 11am", "try again at…" and "in 2 days 3 hours" as local time;
  - RateLimited: 429;
  - Transient: connection, overload, 5xx, timeouts.
- **Stall source** (`term.rs:878-897`):
  - The transcript's `api_error`, keyed by record id, ignoring errors older than the pane.
  - Otherwise, for non-Claude panes (Claude transcripts are trusted alone), a screen banner (`screen_stall` `:349-367`): one of the bottom 12 non-empty rows starting with a known banner or `■`, with a wrapped second line joined; keyed by its text, with a `below` depth.
- **`step`** (`:102-157`):
  - It acts only after the agent has worked in this pane, is idle, and has been quiet for ≥3 s.
  - A fresh occurrence of the same banner text is told apart by how low it sits.
  - Typing by the user cancels it.
  - Backoff ladders: transient 5/15/30/60/120/300 s; rate 30/60/120/300/600 s; other 300/600/1200/1800 s.
  - A limit with a reset time and `after_limit_reset` resumes at reset + 30 s.
  - It gives up after `max_attempts` with the toast "gave up after N retries".
  - Fatal errors toast NeedsYou "stopped: …".
  - If a sent prompt is not picked up within 45 s: an error toast and it stands down.
  - Success: "back to work after N retries".
- **Sending:** `Send` writes the prompt, then Enter 250 ms later as a separate write (`term.rs:829-853`). Toasts are prefixed with the session name.
- **Header and sidebar note** (`note` `:205-215`): "↻ retry N in Ns", "↻ resumes 2:40am", or "↻ resuming".
- **Scope:** agents only (a harness is set), not the demo.

## 3.8 Auto-approve, and clicking prompt options — `term.rs`
- **Auto-approve** (`:856-874`) answers when all of these hold:
  - harness is Claude, not the demo, status Blocked, `auto_approve` on;
  - output has been quiet ≥300 ms, and the last approval was ≥1.5 s ago;
  - `permission_prompt` (`:1322-1331`) finds a "Do you want to …?" question ("do you want me to" is excluded) with "1. Yes" below it.
  - It writes `1\r` and toasts "<name> · approved: <the line above the question, or the question>".
- **Screen scan** (`scan` `:798-826`, every 200 ms from `poll`): Working ("esc to interrupt", or output in the last 1.5 s), Blocked (prompt markers), else Idle. `prompt_title` is the question line.
- **Clicking a numbered option** while Blocked and not scrolled back sends `"<n>\r"`, as the phone's buttons do (`prompt_option_at` `:583-592`, `prompt_option` `:1335-1339`; `app\mouse.rs:389-409`). It shows a hover affordance (`app\draw.rs:416`).

---

# 4. Model/view split, and what a GUI can reuse

For every item below, the code is reusable only after a lib target or extraction, because of finding 0.1.

| Module | Reusable logic (pub, pure, or nearly) | Bound to ratatui/crossterm/vt100 | Visibility or refactor needed |
|---|---|---|---|
| settings | Data lives in bro-core and is pub: `Settings`, `ClaudeEnvSettings` (`choice`, `set_choice`, `use_preset`, `vars`, `settings`, `summary`), `KNOBS`, `PRESETS`, `caveman::Level`, watchdog choices plus `step_choice`/`human` (pub in bro-tui `watchdog`). Services setters are pub. | All of: row list, sections, labels, help, value text, `change`/`step`/`reset`, notes (`blocks`, `integrations`, `smart_paste`, `summary` build `Span`s). Font switching is OSC 50, terminal-only. | Extract a settings model (rows, sections, value text plus a semantic style, step(dir), reset) out of `Span` building. Everything here is private except `step_label` and `spinner` (`pub(crate)`). |
| stats | `bro_core::usage::tokens::{scan, claude_counter, local_date, Tokens, LogTotals, Progress}` (pub, in bro-core). | `token_figures`, `spark`, `compact` (`pub(crate)`), report building, line layout. | Make `token_figures`, `compact`, `spark` and the Report/Login types pub, or move them to bro-core. |
| usage | `State.usage`/`picks`/`usage_at`/`profiles` (pub fields); `ui::pct_color` thresholds 60/85. | The whole render. | Easy rewrite; nothing to expose. |
| profiles | `bro_core::profiles::{valid_name, ProfileKind}`; `svc.create_profile`/`remove_profile` (mpsc receivers), `login_command`, `watch_login`, `login_closed`; `LOGIN_LABEL` (pub). | Mode and pending state machine (private); `open_login` builds a `Term` and `Action::Open`. | Port the state machine; replace `Term` creation with a GUI pane factory. |
| proxy | `tokens`, `hms`, `upstream_label`, `saved` (pub fns); `ProxyState`, `ProxyEvent`, `PoolAccountStatus`. | `trim_line`, the style functions. | Fine as is. |
| bridge | `mask`, `show_url` (pub); `BridgeStatus`; `Bridge::pairing_url()`. | The QR is half-block text in `state.bridge.qr`. | GUI should draw a native QR from `pairing_url`; Services hides the Bridge handle, so add `pairing_url` to State. |
| quick_install | `bro_core::quick_install::{Tool, Group, detect, how, Os}` (pub, in core). | Picked/watch state (private); `install_term`/`bro_term` return a `Term`. | Same pane-factory refactor. |
| setup | `agent_install::installed`/`display_name`. | All private. | Port. |
| file_view | Pub types: `Doc`, `Media`, `Body`, `Payload`, `Loaded`, `Outcome`, `read()`; `FileView` mostly pub fields; bridge `file_preview::*` (pub, in a lib crate). | `Styled = Vec<(ratatui::Style, String)>`; paging and centring are private methods; key/mouse take crossterm events; it sends `pane::Event::FileView`. | Make `highlight` produce renderer-neutral spans (slot id plus bold/italic/underline); make `page()`/`center_on_target` and the request plumbing generic over a notifier. |
| agent_files | `items()`, `shown_path()`, `Item`, `Section`, `Outcome` (pub). | `draw`; key/mouse. | Depends on `panes::insights::Snapshot`. |
| editor | Everything pub and pure (`plan`, `plan_here`, `spawn_gui`, `split_command`, `EditorPane`). | — | `EditorPane.pane` is a bro-tui `PaneId`. |
| links | `file_uri`, `parse_file_uri`, `target`, `open` (pub). | `detect_with` walks a `vt100::Screen`; `web_urls` is private. | Re-implement the walk over the alacritty grid (WRAPLINE flag, `cell.hyperlink()`); make `web_urls` pub. |
| file_cache | `resolve_local`, `FileCache`, `Lookup` (pub). | `finish` takes `pane::Waker`. | Abstract the waker. |
| keys | — | Uses crossterm `KeyEvent`/`MouseEvent`, ratatui `Rect`, vt100 mouse modes. | GUI needs its own winit → bytes encoder that keeps these rules (alacritty_terminal provides no key encoder). |
| writer | Pattern is reusable. | — | Private module. |
| remote_min | Fully pure and pub. | — | — |
| insights | `Insights`/`Snapshot` (pub); `Binder`/`Tail`/`AgentFacts` are pub in bro-core. | `Waker` type. | Abstract the waker. |
| auto_resume | Fully pure and pub (`AutoResume::step`/`note`, `classify`, `reset_time`, `screen_stall` takes `&[String]` rows). | Uses `pane::Activity` and `alerts::Kind`. | Move those two enums with it. |
| term helpers | `permission_prompt`, `prompt_option` (pub, take strings). | `scan` and `auto_approve` read the vt100 parser. | Feed rows from alacritty. |
| clip | `grab_text`, `grab_image`, `paste_form`. | `copy` writes OSC 52 to stdout. | GUI: use the OS clipboard directly; drop OSC 52. |
| Theme | Colours are ratatui `Color` (incl. `Reset`/`Indexed`); slots fg, bg, accent, shine, frame, muted, user, inline, danger, good, ansi[16], skin. | — | Needs a colour conversion. |
| Cx / Action | `Action::Run(Act)` and `LaunchUsage` are fine. | `Action::Open(Box<dyn Pane>)`. | Needs a frontend pane abstraction. |

---

# 5. Must not miss in the GUI port

1. **Image/video sidebar sources:**
   - `facts.sent` (Claude's SendUserFile, with captions);
   - `bro open` (BridgeCommand::OpenFile);
   - clicked file links with media extensions;
   - touched files with image extensions (agents writing PNGs);
   - alt+v pastes in `~/.bro/paste/`.

   Extension lists: `F:\bro-cli-v2\crates\bro-bridge\src\file_preview.rs:290-320`. Needed beyond plain decoders: SVG, AVIF, animated GIF/WebP, video and audio playback, and PDF if wanted. Keep "open in OS" (`o`/⏎) as an action. Do not keep the auto-launch.
2. **`bro open` gating:** the preview pops only for the watched session; otherwise toast with the prefix-`f` key. `last_opened` is global. `OpenInEditor` always opens the editor.
3. **Mention:** focus the source session and send `@path ` with bracketed paste. Path is relative to cwd, else absolute, quoted if it has spaces.
4. **Security:**
   - keep the `links::target` whitelist (http, https, mailto, file only; no control chars; ≤8192);
   - never pass links through a shell;
   - consider blocking OS-open of executables reached through file links.
5. **Link activation:** on release at the same cell; ctrl+click when the program owns the mouse; shift for selection; underline every detected link.
6. **Remote minimum size:** pin to the bottom-left with a `view_top` offset applied to mouse, links and prompt clicks; show the "⇲ remote" header badge; keep the grace period; resize the bridge too.
7. **Timing:**
   - auto-resume Enter 250 ms after the prompt; `submit` 300 ms; `type_command` 400 ms (each a separate write);
   - auto-approve: quiet ≥300 ms, at most once per 1.5 s;
   - scan every 200 ms; terminal poll every 400 ms;
   - insights every 1 s;
   - file cache TTL 30 s found, 8 s missing.
8. **Per-view polling and side effects:**
   - settings: first open runs `check_justtools` and `check_justimagine(true)`; 100 ms ticks while jobs run; 3 s JustImagine watch;
   - stats: 250 ms during a scan;
   - profiles: 50 ms while pending;
   - quick install and setup: 3 s re-checks for 15 min;
   - opening usage → `refresh_usage`; opening profiles → `refresh_data`.
9. **Singleton views:** pressing a view's key again goes back to the previous tab; `Action::Open` of a view replaces the existing one; setup auto-opens at first start.
10. **Login tabs** must keep the `"login · <id>"` label (or an equivalent flag) so closing one calls `login_closed`; `watch_login` toasts on sign-in.
11. **Pane environment** must keep `BRO_SESSION_ID`, `BRO_BRIDGE_URL`, `BRO_BIN`, `BRO=1`, `TERM`/`COLORTERM`, or `bro open`, `bro ctl` and the orchestrator break.
12. **Self-update** swaps bro's binaries (step "replacing bro"). A new GUI executable must be part of the release and swap set.
13. **The terminal font picker** (settings row 2) is OSC 50 for the Just Terminal fork. A GUI loads the 19 bundled fonts itself (`F:\bro-cli-v2\crates\bro-tui\assets\fonts`). Per your global rule, UI chrome uses Inter/Poppins and the session grid stays monospace.
14. **Not in the settings view today** but needed somewhere: `[bridge] remote_min_*`, auto-resume prompt/attempts, trim level and native routing, voice, keys.
15. **Copy text** (labels, help, toasts) lives inline in each view; reuse it verbatim for parity. Main spots: `settings.rs:56-85`, `:320-330`, `:735-913`; knob help in `claude_env.rs:78-257`.
16. **Minor bugs to avoid copying:**
    - usage view mouse-wheel scroll is unclamped (`usage.rs:173`);
    - the bridge view has no mouse support;
    - an unknown watchdog model jumps to "opus" whichever direction you step;
    - the file preview's media card and "⏎ opens it with the system's default app" error screen are unreachable.
