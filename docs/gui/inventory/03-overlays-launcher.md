# Overlays and dialogs in bro-tui: parity inventory for the GUI port

Line refs are relative to `F:\bro-cli-v2\crates\bro-tui\src\` unless marked otherwise. Absolute paths are listed at the end.

## 0. Caveats first

- **Working copy is mid-edit.** Another session has uncommitted changes in `launcher/mod.rs`, `launcher/view.rs`, `launcher/tests.rs`, `palette.rs`, `help.rs` and `app/overlays.rs`. They add:
  - Gemini sign-in (`Outcome::SignIn`).
  - "Typing on the run-on list opens the project picker."
  - Brand glyphs on run-on rows (`account_glyph`).
  - Palette "tiles" entries.

  This inventory describes the working copy.
- **One test is likely stale.** `app/tests.rs:1129-1149` (`launcher_open`) types "openrouter" on the list and expects the model list. With the new behaviour, the snapshot `launcher-openrouter.html` shows the project picker ("nothing here — type a path") instead.
- **bro-tui is a binary-only crate.** `Cargo.toml` has only `[[bin]] bro` and `bro-app`, and `main.rs:7-53` declares private `mod launcher; mod palette; …`. Nothing here can be imported by a GUI crate today (see §5).

## 0.1 Global overlay mechanics (apply to every overlay)

- **One slot.** `App.overlay: Overlay` (`app/mod.rs:133`, enum in `app/overlays.rs:16-32`). At most one overlay is open.
- **Keys** (`app/input.rs:14-35`):
  - ctrl+v, ctrl+shift+v and shift+insert (prefix not armed) start an async clipboard read. The result arrives as `Event::ClipboardText(request, text)` and goes through `App::paste` (`app/mod.rs:1046-1053, 1097-1131`).
  - Otherwise an open overlay takes every key: no global chords, no prefix. Routing is `overlay_key` (`input.rs:124-201`).
  - Key Release is ignored; repeats are delivered (`mod.rs:950`).
- **Pending clipboard reads are cancelled** by any key or any non-move mouse event (`input.rs:17`, `mouse.rs:13-15`). A late paste never lands in a newly opened popup (test `app/tests.rs:2465`).
- **Paste routing** (`mod.rs:1100-1115`):
  - Launcher → `Launcher::paste`.
  - Watchdog → `WatchdogView::paste`.
  - WatchdogPrompt → `PromptEditor::paste`.
  - Palette → appends non-control chars to `query`, `sel=0`, live theme preview.
  - Help, Resume, Confirm, File, AgentFiles swallow the paste. It never leaks to a terminal.
- **Draw order** (`app/draw.rs:107-126`):
  1. Panes.
  2. Launcher, which fills `self.body`: right of the sidebar, below the project-tab strip.
  3. Selection, then backdrop.
  4. Alt hints (hidden whenever any overlay is open, `alt_hints.rs:34`).
  5. Toasts and the armed-prefix hint.
  6. `draw_overlay` for every other overlay, centered on the full screen.
  7. Skin effects.

  So toasts sit above the launcher but under popups.
- **Mouse while an overlay is open** (`mouse.rs:24-112`):
  - Launcher without its project picker: sidebar and project strip clicks and wheel still work ("beside_launcher", `:25-26`, then `launcher_side_click` `:456-494`).
  - Confirm: only a left-click on Yes/No does anything. All other clicks are ignored, inside or outside.
  - File and AgentFiles: their own `mouse()`.
  - Launcher: left-click → `click`, wheel → `scroll`.
  - Watchdog: left-click → `click`. Wheel is ignored.
  - Resume: left-click a row selects it; clicking the selected row chooses it. Wheel and outside clicks are ignored.
  - Palette and Help: **any button-down anywhere, even inside, closes them.** Palette restores the theme. No wheel, no row clicks.
  - WatchdogPrompt: no mouse at all.
- **Mouse back/forward** (`Event::Navigate`, `app/history.rs:87-139`):
  - Closes any non-launcher overlay first.
  - The launcher is a history stop (`Seen::NewSession`). A launcher visit closed without launching is dropped from history (`history.rs:36-41`).
- **Redraw:** while a Watchdog overlay is open, the loop ticks every 125 ms (`mod.rs:894`) for the spinner and live status.
- `run_act` leaves the sidebar whenever an action opened an overlay (`commands.rs:205-207`).

---

## 1. Every Overlay variant

### 1.1 `Overlay::Launcher(Box<Launcher>)`: "New session" / "Continue a session"

Full detail in §2.

- **Opens:**
  - alt+n or prefix c (`Act::NewSession` → `open_launcher(None, Tab)`, `commands.rs:21,322-328`).
  - alt+enter or alt+\ (`Act::Split` → Place::Split).
  - alt+r or prefix R (Continue tab).
  - alt+O or prefix O (★ tab).
  - Sidebar "+ New", "↻ Continue", and the quick icons (`mouse.rs:221-223`).
  - Project strip "+" (`project_tabs.rs:211`).
  - Welcome screen Enter or n (`input.rs:112`).
  - Setup view "Launch" row (`views/setup.rs:106`).
  - Usage rows (`launch_usage`, `commands.rs:411`) and alt+q (`quick_agent`), when a choice is still needed.
  - `add_project` (copied folder ctrl+shift+o, `bro` started in another folder; `sessions.rs:502-515`).
  - History back/forward.
- **Closes:**
  - Esc chain.
  - The × in the frame corner.
  - Launch, Resume, ResumeMany, Install, SignIn, Profiles.
  - A click on a sidebar session row or most other sidebar items (`mouse.rs:456-494`).
  - A click outside `area` (in practice unreachable, because the sidebar and strip are routed).
  - Navigating to a pane via history.

### 1.2 `Overlay::Palette(Box<Palette>)`

Full detail in §3.

- **Opens:**
  - alt+p, prefix `:` or prefix space (`Act::Palette` → `open_palette("")`).
  - prefix t and the settings "Theme" row (`Act::Themes` → `open_palette("theme ")`).
  - Project strip "+N" (`open_palette("tiles ")`, `project_tabs.rs:209`).
  - Welcome `p`.
- **Closes:** Esc, Enter (runs the item), or any mouse button-down.

### 1.3 `Overlay::Help(Help)` (`help.rs`)

- **Opens:** F1, prefix ?, `?` on a non-terminal pane that didn't use the key (`input.rs:100`), welcome `?`.
- **Content** (`help.rs:19-57`), generated from the live keymap:
  - For each `Group::ALL` in order sessions, navigate, panes, views, app (`keymap.rs:110`):
    - A `Head(label)`.
    - Then every `Act` in that group with chords: `Key(chords.join("  "), describe())`.
    - `Jump` appears once as `"alt+1–9"` = "jump to live session 1–9 (sidebar order)".
    - Acts with no chords are skipped.
    - A `Gap` after each group.
  - Then static cheat-sheets: sidebar (alt+b), project tabs, launcher (alt+n), clipboard, usage, terminal, mouse.
  - Note: the launcher sheet says "tab ← →: next column", but ← → actually switch agent tabs.
- **Layout** (`help.rs:80-112`):
  - `centered(screen, 118, screen.h-4 (min 10))`.
  - Frame title "{help icon} bro · keys", top-right status " prefix ctrl+space ", bottom-right subtitle "esc close · j/k scroll".
  - Inner padded 2 columns and 1 row.
  - Two columns when inner width ≥100 (`col_w = w/2-1`).
  - Heads are UPPERCASE, accent bold.
  - Key column is padded to `min(30, col_w/2)` in shine bold; the description is muted.
- **Keys** (`help.rs:67-78`):
  - Esc, F1, q, ?, Enter → close.
  - ↓/j +1, ↑/k -1, PgDn/space +10, PgUp -10, Home/g → top. No End.
  - Scroll is clamped in draw to `len - per_col*(2|1)`.
- **Mouse:** any click closes.

### 1.4 `Overlay::Resume(Box<ResumePicker>)`: "resume in" / "move to another login" (`app/overlays.rs:41-81,122-138,158-200`)

- **State:** `ResumePicker { from: ResumeFrom, targets: Vec<ResumeTarget>, sel, hits: Vec<Rect> }`, all pub.
  - `ResumeFrom::Past(SessionInfo)` or `ResumeFrom::Live { pane, harness, store, cwd, since, title }`.
  - `ResumeTarget { profile_id, name, detail, left: Option<f64>, current }`.
- **Opens:**
  - From `open_resume(idx)` (`sessions.rs:686-699`), after Enter or click on one Continue entry:
    - pi/omp resume directly.
    - Claude/Codex: if there are ≤1 signed-in logins of that family, resume directly; otherwise this picker opens.
  - From `open_switch(pane)` (alt+L or prefix P, `sessions.rs:702-719`), for a live Claude/Codex session. Toasts:
    - "only Claude and Codex sessions can move to another login"
    - "no other {h} login to move to — add one in profiles (alt+o)"
- **Targets** (`sessions.rs:653-675`):
  - Authenticated profiles of the session's family.
  - `detail` = "plan · email".
  - `left` = min(5h, wk) headroom.
  - Sorted current-first, then by % left descending (unknown last).
- **Default selection** (`resume_default`, `sessions.rs:678-683`): current login, unless it has <10% left; then the first non-current login with ≥10% or unknown. Switch (Live) never starts on the current login (`:716`).
- **Layout:**
  - Popup 72 wide × `min(targets+8, 22)` high.
  - Title "{history icon} resume in", or "move to another login" for Live.
  - Line 1: harness glyph (brand colour) + title, bold.
  - Line 2: "  another login continues from a copy of the conversation", or "  the running session closes and continues in the login you pick" for Live.
  - Rows from line 4:
    - "▌" marker, "{n} ", name padded to 14 (bold accent when selected), detail muted.
    - "  current" in shine for the current login.
    - Right side: "{left:.0}% left " coloured by `left_color`.
  - Hints: "⏎ resume · 1-9 pick + go · ↑↓ move · esc cancel".
- **Keys:** Esc closes; ↓/j, ↑/k; `1`-`9` select and choose; Enter chooses.
- **Choose** (`resume_chosen`, `sessions.rs:722-750`):
  - Past + current login → `resume(idx)` in its own login (idx re-looked-up by id). Toast "resuming “title”…".
  - Past + other login → `fork_into` (`:753-761`): stage the transcript into the target login, resume with `fork:true`, name = title fitted to 28. Toast "resuming in {target}…".
  - Live + current → no-op (the overlay is already taken, so it just closes).
  - Live + other → **close the running pane first**, then launch with `find_live` (finds the newest transcript since the session started, `services/launch.rs:78-88`). Permission Skip, browser Off. Toast "moving to {profile}…".
  - If the Live transcript isn't found: error toast "couldn't launch {h}: couldn't find this session's transcript — it moves once the agent has answered at least once". The original session is already gone at that point.

### 1.5 `Overlay::Confirm(Confirm)` (`app/overlays.rs:84-96,140-155,204-222`)

- **State:** `Confirm { text, detail, action: ConfirmAction::{Quit, Close(PaneId), CloseProject(PathBuf)} }`. The button rects are stored in `App.confirm_hits: Vec<(Rect, bool)>`.
- **Sources and copy:**
  - Quit (prefix q), only if any terminal panes exist (`commands.rs:196-203`):
    - Question: "quit bro and end {n} session(s)?"
    - Detail: "running agents are stopped; past sessions stay resumable"
  - Close a non-idle (working or blocked) pane: ctrl+w, alt+w, prefix x, sidebar ×/Delete/middle-click, pane frame × (`ask_close`, `commands.rs:211-219`):
    - Question: "close {title}?"
    - Detail: "the agent is still busy — it will be stopped"
  - Close a project with live sessions: prefix X, sidebar project ×/Delete, strip tab × or middle-click (`sessions.rs:518-529`):
    - Question: "close {name} and its {n} running session(s)?"
    - Detail: "the agents stop; their conversations stay resumable (continue session)"
- **Layout:**
  - Popup `clamp(max(text, detail)+8, 40, 100)` × 7, title "confirm".
  - Question in danger bold; detail muted.
  - Buttons on inner row 3: " Yes  (y) " (11 wide, ink on danger, bold) and, 2 columns later, " No (esc) " (10 wide, fg on frame).
  - Hover underlines the button.
  - If the inner height is <4, no buttons are drawn.
- **Keys:** y, Y or Enter → yes. **Any other key → no** (it closes).
- **Mouse:** left-click Yes or No answers. Other clicks do nothing (test `app/tests.rs:493-512`).
- **Yes actions:** quit, `close(id)`, or `close_project_now(root)`. The last toast is "closed {name} · choose it from New session".

### 1.6 `Overlay::File(Box<FileView>)`: file preview (`file_view.rs`, `app/files.rs`)

- **Opens:**
  - Clicking a `file:` link or path in a terminal (`open_file_link`).
  - An agent's `bro open` (`agent_opened_file`, `files.rs:156-169`). It shows only if you're watching that session; otherwise a toast "{who} opened {path:line} · prefix f to view".
  - prefix f (`show_last_opened`). Toasts: "no file opened yet · agents open files with `bro open path:line`", "that session has closed".
  - Picking an item in AgentFiles.
  - Links on turned-over cards.
- **Load:**
  - A worker thread resolves the path, reads a window of lines, and highlights with syntect using theme-slot colours (1.5 s budget).
  - Pages load near the edges (PREFETCH 20).
  - Media (image, video, audio) hands off to the OS immediately (`apply` → `Outcome::System`).
  - A folder, or a target outside what can be previewed, goes to the `fallback` uri (OS open).
  - Failures show the message; if the file exists, "⏎ opens it with the system's default app".
- **Layout:**
  - Popup `min(w-4, 200)` × `h-2`.
  - Title: file icon + relative name, plus ":line" when targeted.
  - Header: "{lang} · {N} lines · {size}   {abs path}". Right side: "{top}–{last} of {N}", with a "→{left} · " prefix when scrolled right and "loading… · " while loading.
  - Gutter line numbers. The target line gets an accent "▌", a bold number and a tinted background.
  - Lines outside the loaded window show "…".
  - Media body: ▶, ♪ or ▣, then name, then "{video|audio|image} · mime · size", then a how-to line.
  - Clickable hint row (underlined on hover):
    - Text: "e editor · o system app · c copy path · @ mention · ↑↓ pgup pgdn scroll · esc close"
    - Media: "⏎ open · c copy path · @ mention · esc close"
    - Failed with a path: "⏎ system app · c copy path · esc close"
    - Loading: "esc close"
- **Keys** (`file_view.rs:264-292`):
  - Esc or q close.
  - j/k/↑/↓ ±1 line; ctrl+d/ctrl+u half page; PgDn, space or ctrl+f page down; PgUp or b page up.
  - Home/g top, End/G bottom.
  - ←/h and →/l scroll ±8 columns; 0 resets the horizontal scroll.
  - e → editor (media → system app).
  - Enter → editor for text, system app otherwise.
  - o → system app; c or y → copy path; @ or i → mention.
- **Mouse:** wheel ±3 lines; horizontal wheel ±8 columns; left-click a hint runs it; left-click outside the frame closes.
- **Outcomes** (`files.rs:66-109`):
  - Copy → clipboard + toast "copied {path}".
  - Mention → close, focus the source session, paste "@path ". If that session is gone: "that session has closed".
  - Editor → GUI editor spawn (toast "opened x:12 in code") or a terminal editor in a split pane.
  - System → OS open.

### 1.7 `Overlay::AgentFiles(Box<AgentFilesView>)` (`agent_files.rs`, `app/files.rs:172-203`)

- **Opens:** prefix F. Toast "agent files: focus an agent session first" when the focused pane isn't a terminal.
- **Content:**
  - "sent to you" section: Claude's SendUserFile.
  - "touched" section: edit, write, create, delete, move, read.
  - Each section is newest first and has a header "label  N".
- **Empty messages:**
  - "nothing sent or touched yet"
  - "waiting for the agent's transcript (it appears once the agent has answered)"
  - "{h} sessions don't report their files"
  - "a shell has no agent files"
- **Layout:**
  - Popup `clamp(w-6, 40, 110)` × `clamp(items+9, 10, h-2)`.
  - Title "{file icon}agent files · {session}".
  - Row: "▌", then a kind glyph (➤ sent, ✎ edit/write, + create, − delete, → move, · read) in shine, then "path[:line]" (crossed out for delete), then the caption muted. Right side: " {kind}  {ago} ".
  - Hints "⏎ preview · ↑↓ move · esc close" (not clickable).
- **Keys:** Esc/q, j/k/↑/↓, PgUp/PgDn (page = visible rows), Home/g, End/G, Enter opens the File overlay at the line.
- **Mouse:** wheel moves the selection ±3. Left-click a row selects and opens it immediately.
- Opening a file replaces this overlay; Esc from the preview does not return to the list.

### 1.8 `Overlay::Watchdog` and 1.9 `Overlay::WatchdogPrompt`

See §4.

### 1.10 Nested sub-dialogs (not Overlay variants; owned by the launcher)

- **Project picker:** `Launcher.project_picker: Option<FolderPicker>`, drawn as a popup "choose project" 84×22 centered on the launcher body. See §2.9.
- **Continue tab:** `Launcher.continue_picker: ContinuePicker`. See §2.11.

### 1.11 Overlay-like UI that is not in the enum (GUI must still place it)

- **Armed-prefix hint:** a framed box bottom-right titled "ctrl+space …" listing "w watchdog · % split · …" (`draw.rs:528-535`, `keymap.rs:642-648`).
- **Toasts:** top-right, newest first, one line each, fitted to ≤60 columns, kind glyph and colour, 2 s lifetime (`draw.rs:540-560`, `alerts.rs:39`; snapshot `notifications-top-right.html`).
- **Inline rename:** `App.renaming`, max 40 chars (`input.rs:203-228`).
- **Alt hints:** suppressed while any overlay is open.

---

## 2. The launcher in depth (`launcher/mod.rs`, `launcher/view.rs`)

### 2.1 Construction and defaults

- `session_launcher` (`commands.rs:330-335`):

  ```
  Launcher::new(launcher_data(app), cwd, place)
      .with_projects(app.launcher_projects())
      .with_permission(settings.permission)
      .with_continue(app.continue_entries(), Some((project.key, project.name)))
  ```

- `open_launcher` also kicks `svc.refresh_usage()` and `refresh_sessions()` and leaves the sidebar.
- `Launcher::new` (`mod.rs:212-241`):
  - harness = the harness of `recents[0]`, **including orchestrator recents**, else Claude.
  - dir = the `cwd` argument, else `recents[0].cwd`, else the process cwd.
  - browser = `recents[0].browser`, else Off.
  - permission = Skip, overridden in the app by the remembered setting.
  - tab New, focus List, `list_sel 0`.
- The cwd argument (`preferred_dir`, `sessions.rs:430-441`), first match wins:
  1. The sidebar selection while the sidebar is focused.
  2. `cur_project`.
  3. The focused (non-orchestrator) session's project root.
  4. The first open project.
- **Fresh state on every open.** Only permission (settings) and recents-derived defaults carry over.
- **Data** (`app/overlays.rs:225-280`, `launcher::Data` at `mod.rs:85-102`):
  - `profiles`, `providers` from service state.
  - `usage`: profile → % used of the *tighter* window, `100 - min(h5, wk headroom)`. A fresh 5h window with 0 weekly left reads "0% left" (test `app/tests.rs:3876`).
  - `usage_status`, joined with " · ": "weekly" (only weekly is reported), "refreshing…", "stale" (error but a reading exists), "usage unavailable".
  - `installed`: `bro_core::launch::which(label)` per harness, run **on the UI thread at open**. Demo and tests count every harness as installed.
  - `recents`, `models` (provider id + "codex" + "claude" → `Vec<ModelRow>`), `orchestrator_running` (label of the orchestrator pane).
  - `keyed`: provider ids with a key, or `no_key`. Google-login providers count when `gemini::signed_in(creds)` (a file read).

### 2.2 Layout (New tab), top to bottom (`view.rs:36-122`)

- **Frame:** title " New session " or " Continue a session ".
  - Top-right " × " (danger; `Hit::Close`) at `right-5`.
  - Left of it " profiles " + "alt+o " (`Hit::Profiles`), shown only if the width is ≥52.
- **Row 0:** segmented switch " + New " / " ↻ Continue " (`Hit::Tab`). Active = black on accent, bold; inactive = muted on accent tint.
  - Right-aligned project chip (`Hit::Project`): folder icon, name (bold), "  ~\short\path" (only if room ≥20, max 48), "  change ", "alt+p ". Shine tint background.
  - On the ★ tab it becomes a gold, non-clickable chip " ▸ runs in ~\orchestrator ".
- **Row 2:** agent chips "  {glyph} {label}  ", 1 column gap (`Hit::Harness`).
  - Selected = black on brand colour, bold. Installed = brand on brand tint. Not installed = muted on muted tint.
  - Brand colours (`ui.rs:65-73`): claude #D97757, codex #2EC49A, pi #A78BFA, omp #60A5FA.
  - Then, 1 extra column apart, the gold "  ★ orchestrator  " (`Hit::Orchestrator`), with a "●" suffix when one is running. Gold is #FFC83D on dark themes, #B28000 on light (`theme.rs:282`).
- **★ tab only, row 3:** "  runs on " (gold), then small chips " {glyph} {label} " (`Hit::Brain`), then a muted note: "  tools built in" (claude/codex) or "  tools through bro ctl" (pi/omp).
- **Next row:** when the harness isn't installed, "↓ {h} isn't installed yet · enter or click here to install it" in danger (`Hit::Install`).
- **Body:** a side panel when body width ≥84 **and** height ≥11 (`view.rs:32-34,90`). Otherwise the stacked layout.
  - **Side layout:**
    - Panel width: `clamp(w*11/20, 48, 66)` when a model is needed, else `clamp(w*2/5, 40, 48)`.
    - The list gets the rest minus 3, with a "│" divider.
  - **Run-on column** (`view.rs:308-368`):
    - Filter line "› {filter or 'run on… ↑↓ to pick · type a project'}▏" (the ▏ cursor shows only when focused).
    - Rows start 2 lines below it, as a scroll window with the cursor kept at the bottom.
    - Headers: dim bold label + hairline.
    - Recent row: brand glyph, who (provider id / short profile name / "own login"), "  {short model}" muted; right side "{age} ago ".
    - Account row: glyph (muted if not ready), "{label}  " + detail muted. Right side: a ▰▱ meter (10 cells if the list is ≥56 wide, 6 if ≥44, else none) and " {left:>3}% left " coloured. Not ready with no reading shows "not signed in ".
    - Cursor row: tinted background + accent "▌" when the list has focus; a muted "›" otherwise.
    - Empty: "   nothing matches · esc clears the filter".
  - **Panel when no model is needed** (`view.rs:415-501`):
    - Heading "launches".
    - "{glyph} {label_for(spec)}" in brand bold, e.g. "claude · work · opus-5".
    - The account detail.
    - A meter (`min(w-14, 24)` cells) + "N% left".
    - A recent row shows "the way you last ran it here".
    - When there is no spec: "pick where it runs on the left", or "install it first".
    - ★ tab: "★ {label}", "Opens sessions and hands them work," / "then tells you how it went.", plus a meter.
    - ★ tab with an orchestrator already running: "★ {running label}", "Already running. There's only ever one.", "⏎ takes you to it."
  - **Panel when a model is needed:** the model list takes the top. Filter "› {q or 'model… N to pick from'}".
    - Row: model id; right side "{262k|1M} " and "$in/out " per million, trailing zeros trimmed (`view.rs:382-411,630-638`).
    - Empty: "   no models known for this" or "   nothing matches".
  - **Options block** (`view.rs:504-547`), heading "options":
    - "permissions": pills " ask " / " auto " / " skip all " (the skip pill uses danger colour) + "^e". Each pill is `Hit::Perm(p)`.
    - "browser": " off " / " on " / " edge " / " chrome " + "^b". The whole row is `Hit::Browser`.
    - "opens as": " new session " / " split beside this one " + "^s". The whole row is `Hit::Place`.
    - ★ tab: " its own, pinned on top " (gold, not clickable).
  - **Button** on the bottom body row (`view.rs:550-565`):
    - "⏎  Launch {label}" in brand colour.
    - "⏎  Start it on {label without 'orchestrator · '}" (gold).
    - "⏎  Go to the orchestrator" (gold).
    - "⏎  Install {h}" (danger).
    - "pick a model to launch" or "pick where it runs" (muted).
    - The whole row is `Hit::Launch`, or `Hit::Install` when not installed.
  - **Stacked layout** (narrow; `view.rs:568-627`):
    - List full width (45% list / 55% models when a model is needed).
    - Then a rule, a summary "→ {label}" / "→ pick a model" (shine) / "→ pick where to run".
    - One toggles row: " ask  auto  skip all " pills + "  browser off|browser on|edge|chrome" + "  new session|split", each clickable.
- **Hint row** (last inner row): "⏎ {launch|pick a model|go to it} · ↑↓ move · ←→ agent · [shift+←→ runs on ·] type {project|filter} · [tab list ⇄ models ·] alt+p project · esc {close|back}".
  - The "⏎ …" text itself is clickable (`Hit::Launch`, `view.rs:120`).
- `draw` writes `l.hits` and `l.area = body` (`view.rs:21-29`).

### 2.3 Run-on list generation (`mod.rs:425-547`)

- **Groups, in order.** An empty group is omitted together with its header.
  1. "recent": `recents_for(h)` (`:524-542`). Same harness **and** same orchestrator flag, deduped on (profile, provider, model), max 4 (`RECENTS_SHOWN`), newest first.
  2. "your logins":
     - Claude: claude profiles + pool.
     - Codex: codex profiles.
     - pi/omp: a single Native row, "{h}'s own login" / "whatever it's configured with", always ready.
  3. "other logins · via proxy":
     - Claude: codex profiles (detail prefix "chatgpt").
     - Codex: claude profiles (prefix "claude") + pool.
     - pi/omp: claude + codex profiles + pool.
  4. "providers": every provider whose `mode != Native`.
- **Profile row** (`:425-447`):
  - label = profile name.
  - detail = [family if via proxy, plan, "not logged in" if unauthenticated, usage_status] joined " · ".
  - ready = authenticated; left = 100 − used.
- **Pool row** (`:449-452`): only when ≥1 authenticated **ClaudeAccount** exists (ClaudeLocal doesn't count). Label "pool", detail "{n} claude accounts · failover", ready, no meter.
- **Provider row** (`:454-464`):
  - label = provider id.
  - detail = "{n} model(s)", plus " · google ai subscription" (Google login, signed in), " · enter signs in with Google" (Google login, not signed in), or " · no key".
  - ready = keyed.
- **Filtering** (`:495-509`): fuzzy within each group, group order kept. The item text is "profile provider model" for recents and "label detail" for accounts.
  - `list_filter` is effectively unreachable from input now: typing goes to the project picker, and only tests set the filter. Backspace and ctrl+u still "edit" it (see quirks in §6).
- **Selection** is an index over selectable (non-header) rows: `list_sel`, `selectable()`, `current()`, `cursor_row()`.

### 2.4 Model needed? (`mod.rs:562-598`)

- **needs_model:**
  - Provider → always.
  - Profile → yes unless (Claude harness with a `claude:` profile) or (Codex harness with a non-claude profile). So pi/omp always need a model on profiles.
  - Pool → when the harness isn't Claude.
  - Native or Recent → no.
- **Model list key:** provider id; a `codex:` profile → "codex"; anything else (claude profile cross-family, pool) → "claude". "claude" comes from the `anthropic` provider's models (`services/workers.rs:84-112`).
- `models_view` = fuzzy over "id name". `model()` = the model at `model_sel` in the filtered view.
- **reset_models** (`:655-671`), on any row change: clears the model filter, `model_sel=0`, then jumps to the **last model used with that account**, found in recents with matching (profile, provider) and a model.

### 2.5 Spec, launch, outcomes

- **spec()** (`:601-630`) builds a `LaunchSpec`:
  - `harness`.
  - `profile_id` / `provider_id` (pool → provider "pool").
  - `model`: required when needed; otherwise None, meaning the CLI default.
  - `cwd: dir`, `resume None`, `permission`, `browser`, `extra_args []`, `orchestrator`.
  - A Recent row reuses its profile/provider/model but takes the launcher's *current* permission and browser.
- **quick_spec** (`:327-335`), used by one-click paths (sidebar icons, usage rows, alt+q): None when the harness isn't installed, or the row is a not-ready account, or a model is needed. Otherwise `spec()`. A None opens the launcher instead.
- **launch()** (`:739-749`): sign-in needed → `SignIn`; orchestrator → `Launch(spec, Place::Tab)`, forced; else `Launch(spec, place)`.
- **Outcome** (`:132-148`), handled in `input.rs:127-158` (keys) and `mouse.rs:48-82` (clicks):
  - `None`: App calls `remember_launcher_permission()`, which saves `settings.permission` if it changed (`commands.rs:351-360`). The permission persists immediately, not only on launch.
  - `Close`.
  - `Profiles`: close, open/focus the profiles view.
  - `Project(dir)`: `select_launcher_project` (`sessions.rs:453-472`):
    - Adds `project_for(dir).root` to open projects and persists them.
    - Picks it as the current project and selects its sidebar row.
    - Sets `launcher.dir = dir` and the Continue scope.
    - Toast "opened {name}" if it was new.
  - `CreateProject(dir, uuid)`: async folder creation (§2.9).
  - `Launch(spec, place)`: close, then `launch_spec` (`commands.rs:433-452`):
    - Orchestrator: if one is running → focus it + toast "the orchestrator is already running · here it is". If one is pending (<90 s) → toast "the orchestrator is already starting…".
    - `LaunchRequest{remember:true}`, toast "launching {label}…".
    - The build runs in the background. Success opens a pane and pushes to recents (`sessions.rs:83-91`). Failure → error toast "couldn't launch {h}: {e}".
    - An orchestrator launch turns the bridge on if needed. Toast "turned on the bridge: the orchestrator drives bro through it". Failure after 20 s: "the bridge didn't start, and the orchestrator needs it to reach your sessions · check the bridge view".
  - `Resume(idx)`: close → `open_resume` (§2.12).
  - `ResumeMany(Vec)`: close → `resume_many` (§2.12).
  - `Archive(id, on)` / `UndoArchive`: the launcher stays open (§2.11).
  - `Install(h)`: close → `install_agent` (§2.7).
  - `SignIn`: close → `sign_in_gemini` (§2.8).

### 2.6 Keys

Precedence in `Launcher::key` (`mod.rs:845-942`):

1. **Project picker open:** every key goes to `FolderPicker::key`.
2. **alt+p or alt+P:** open the project picker. Works on both tabs. It shadows the global palette chord while the launcher is open.
3. **alt+o, exactly:** `Outcome::Profiles`.
4. **alt+n / alt+N:** New tab. **alt+r / alt+R:** Continue tab.
5. **On the Continue tab:** the key goes to `ContinuePicker::key` (§2.11).
6. **New tab:**
   - **Esc:** clear the focused filter if it has text; else, on Models, go back to List; else Close.
   - **Enter:**
     - Harness not installed → `Install(h)`, from either focus.
     - List + Google sign-in needed → `SignIn`.
     - List + model needed → focus Models (no launch).
     - Otherwise launch. No spec means nothing happens.
   - **Tab / BackTab:** List ⇄ Models, only when a model is needed.
   - **shift+← / shift+→ on the ★ tab:** previous/next "runs on" harness, wrapping (`set_brain`).
   - **← / →:** previous/next tab over claude, codex, pi, omp, ★, wrapping (`set_harness`, `:635-652`). Clears the filter, `list_sel=0`, resets models, focus → List.
   - **↓ ↑ ±1, PgDn PgUp ±8, ctrl+n/ctrl+j down, ctrl+p/ctrl+k up:** move in the focused list. Moving on List resets models.
   - **ctrl+e:** permission cycles Default("ask") → Auto → Skip("skip all").
   - **ctrl+b:** browser cycles Off → Auto("on") → Edge → Chrome.
   - **ctrl+s:** placement Tab ⇄ Split. No-op on the ★ tab.
   - **ctrl+u:** clear the focused filter. **Backspace:** pop the focused filter. Both reset the selection (`filter_changed`).
   - **Plain char on List:** opens the project picker and forwards the char.
   - **Plain char on Models:** model filter, `model_sel=0`.
   - **Paste** (`:712-731`): picker open → picker; Continue tab → continue filter; List → open the picker and paste there; Models → model filter (trimmed).

### 2.7 Mouse (`mod.rs:753-842`)

- **click(pos, mods):**
  - Project picker open: clicking any row selects and opens it (the launcher synthesises Enter after `FolderPicker::click`, `:754-761`). Clicks off the rows do nothing; outside the popup doesn't close it.
  - A click outside `area` → Close.
  - Otherwise look up `hits`:
    - `Profiles` → Profiles. `Project` → open the picker. `Tab(t)` → switch tab.
    - `ContinueScope(all)` → `set_all`. `ContinueRow` → `ContinuePicker::click(pos, mods)`.
    - `Harness(h)` → switch unless it's already current. `Orchestrator` → enter ★. `Brain(h)` → `set_brain`.
    - `Row(i)`: the already-selected row with List focused → acts as Enter; otherwise focus List and select.
    - `Model(k)`: the already-selected model with Models focused → `launch()` directly (this **bypasses the install check**); otherwise focus Models and select.
    - `Perm(p)` → set. `Browser` → ctrl+b. `Place` → ctrl+s.
    - `Install` → Install. `Launch` → Enter. `Close` → Close.
- **scroll(pos, down):**
  - Picker open → 3× ↑/↓ in the picker.
  - Continue tab → 3× ↑/↓ in the continue picker.
  - Otherwise: if the pointer is over a Model hit, focus Models; over a Row hit, focus List. Then `move_sel(±3)` in the focused list.
- **Sidebar and strip while the launcher is open** (`mouse.rs:456-494`):
  - Project row → `select_launcher_project`; the launcher stays.
  - Session row → close the launcher, then go to that session.
  - Sidebar "+ New" / "↻ Continue" → switch the launcher tab.
  - Strip project tab → pick that project.
  - Fable / usage toggle / minimized → act and keep the launcher open.
  - Anything else → close the launcher, then act.
  - Wheel over the sidebar scrolls the sidebar.

### 2.8 ★ orchestrator tab

- **Enter it:** → past omp, `Hit::Orchestrator`, alt+O when none runs (`open_orchestrator`, `commands.rs:338-348`; it focuses the running orchestrator instead if one exists), or `with_orchestrator()`.
- **Brain default** (`brain_default`, `mod.rs:355-358`): the harness of the last orchestrator recent if it's installed, else the first installed of Claude, Codex, Pi, Omp, else Claude.
- **Starting row** (`smart_sel`, `:379-417`):
  1. The first orchestrator recent whose login has ≥10% left, or no reading (NEARLY_OUT = 10).
  2. Else the ready own-family account with the most left; an unknown reading scores 50. "Own family": claude profiles for Claude, codex profiles for Codex, Native/Pool for any.
  3. Else the first ready row, else 0.
- Orchestrator recents show only on ★; plain recents never show on ★.
- **Always one session:** `Place::Tab`, the "opens as" row is fixed, ctrl+s is a no-op, the chip says "runs in ~\orchestrator" (the spec's `cwd` is still the launcher dir).
- **When one is running:** "●" on the chip, Enter hint "go to it", button "Go to the orchestrator". Enter goes to `launch_spec`, which focuses the existing one.

### 2.9 Project choice: FolderPicker inside the launcher (`folder.rs`)

- **Opened by:** alt+p, a click on the chip, typing or pasting on the run-on list.
- **Suggestions:** `dedup([launcher.dir] + projects)`. `projects` = open projects, past-session roots, recents dirs, process cwd (`sessions.rs:444-450`). Dedup is case-insensitive on Windows.
- **refresh()** (`:58-74`), run on every filter change:
  - If the filter "looks like a path" (`~`, `/`, `.`, `\`, `X:`): a **Typed** row (if the path is a dir, or the partial is non-empty, or it has a file name), plus **Completion** rows.
  - Completions are dirs in the parent starting with the partial, case-insensitive. Names starting with `.` or `$` show only when the partial starts with "."; sorted; max 60 (`:210-225`).
  - Otherwise fuzzy over the suggestions (**Suggested** rows).
  - This is **synchronous filesystem I/O on the UI thread.**
- **Keys:**
  - Esc: clear the filter if non-empty, else Close (back to the launcher list).
  - Enter → `open_selected`.
  - Tab → complete to the selected/first completion and add a separator. `~` shortening; uses `/` if the text has `/` and no `\`.
  - ↑↓, ctrl+n/j/p/k, ctrl+u, Backspace, chars. No PgUp/PgDn.
- **Paste:** ignored while creating. Trimmed, matching quotes stripped (Explorer "Copy as path"). Rejected if empty or containing control chars. A full path (has a separator, ends with ":" or is "~") **replaces** the filter; otherwise it appends.
- **open_selected** (`:127-146`):
  - Existing dir → `Open`.
  - A file → error "that path is a file; choose a folder".
  - Missing **and Typed** → `Create(path, uuid)` and pending.
  - Missing Suggested (stale history), or any other error → "couldn't open folder: {err}".
- **Creating a folder:**
  - Only Esc works ("esc cancel"); the line reads "creating folder…".
  - The App runs `create_dir_all` on a thread (`sessions.rs:475-499`).
  - On success, if the request id still matches → `select_launcher_project`. On error, the picker shows "couldn't create folder: {e}" or "that path is a file; choose a folder".
  - Esc during creation discards the request: a later completion is ignored, but the folder is still created on disk.
- **Row rendering:**
  - "▌ " marker.
  - Tag: "file " (danger), "create " (shine), "↳ " (shine, typed existing), "▸ " (completion), none (suggested).
  - name bold (accent when selected), "current  " (shine) when it equals the launcher dir, parent path short (60) muted.
  - Empty: "  nothing here — type a path (~/, C:\, /…)".
  - Hints: "⏎ create and use|use project · ctrl+v paste path · tab complete · ↑↓ move · esc cancel".
- `clipboard_folder` (`:300-316`) backs ctrl+shift+o. Errors become toasts: "nothing on the clipboard · copy a folder's path first", "the clipboard holds more than one line · copy just the folder's path", "{s} is a file, not a folder", "no folder at {s}".

### 2.10 Install-if-missing and Gemini sign-in

- **Install:** the harness isn't on PATH (or in a shim dir), checked once at open; demo counts it installed.
  - Chips are dimmed, the warning line shows, the button reads "⏎  Install {h}".
  - Enter, the warning line, or the button → `Install(h)` → `install_agent` (`sessions.rs:189-197`): a new tab runs `bro install <agent>`. Demo toast: "the demo doesn't install anything".
  - Nothing is re-checked while the launcher stays open; reopening re-checks.
- **Gemini** (Google-login provider, `ProviderMode::Antigravity`), when not signed in:
  - The row is not ready, with " · enter signs in with Google".
  - Enter (List focus) → `SignIn` → a tab "sign in · gemini" runs `bro gemini login --wait`. Demo toast: "the demo doesn't sign in to anything".
  - Once signed in: " · google ai subscription", and a model must be picked.

### 2.11 Continue tab (`continue_picker.rs`)

- **Entries** (`commands.rs:566-589`): every past session that has a project or cwd.
  - Fields: idx, id, harness, title, login (short name), age, project key/name, archived (from `Archive`).
  - Sorted newest first.
- **Scope:** `project` = the launcher dir's project. `all=false` at start. `set_project` keeps `all` (all stays global when the path changes).
- **Visible list:** filtered by `show_archived` and scope, then fuzzy over "title project login harness id". Every refresh resets `sel=0` and the shift anchor.
- **Keys** (`:203-264`):
  - **Esc:** clear the filter; else clear the picks; else Close.
  - **Enter:** with picks → `Resume` (exactly 1) or `ResumeMany(idx in entry order)`; no picks → `Resume(cursor)`.
  - **Pick one:** ctrl+space, ctrl+@ or Null toggles the cursor row's pick.
  - **Range:** shift+↓/↑ ±1, shift+PgDn/PgUp ±10 extend from the anchor. Backing up inside the range shrinks it. A plain move ends the range.
  - **Tab / BackTab:** toggle project ⇄ all (only when there is a project scope).
  - **Delete:** `Archive(id, !archived)`, which restores an already-archived row.
  - **ctrl+z:** Undo. **ctrl+a:** show/hide archived.
  - **Movement:** ↓/↑, PgDn/PgUp ±10, ctrl+n/j/p/k.
  - **Filter:** ctrl+u clears, Backspace, chars.
  - Picks survive searching and are dropped only when the entry vanishes.
- **Mouse** (`:182-201`):
  - ctrl+click: toggle; if nothing was picked and you clicked a different row, the cursor row is picked too.
  - shift+click: range from the anchor/cursor.
  - Click the cursor row: resume. Click another row: move the cursor.
  - Click the scope chips (`scope_hits`): set scope.
  - Wheel: 3 rows.
- **Draw** (`:267-347`):
  - "› {filter or 'type to search earlier sessions'}▏".
  - Scope right-aligned: " {project≤18} " and " All sessions " (reversed accent bold when active) + "  Tab ".
  - Rows from inner+2: "▌" cursor, "✓" picked, harness glyph, title (fitted). Right side: "[{id tail after last '-'}]  ", project (≤16) when showing all, login (≤12), "archived  ", "{age:>4} ".
  - Cursor row background `mix(t.user, rgb(20,20,24), .35)`; picked rows `mix(accent, rgb(20,20,24), .82)`. Archived titles crossed out and muted.
  - Empty: "  No earlier sessions yet" / "  Nothing matches" (all) / "  No sessions here. Press Tab for all sessions."
  - Hints without picks: "⏎ resume · ↑↓ move · ⇧↑↓ ^space pick several · del archive · ^z undo · ^a show archived|hide archived · esc close".
  - Hints with picks: "⏎ resume 1 session|resume all N, one at a time · ⇧↑↓ range · ^space ^click pick / unpick · esc clear picks".
- **Archive in App** (`commands.rs:592-614`):
  - Toasts "archived · ctrl+z undoes", "restored", "nothing to undo", "restored N session(s)".
  - Persisted to `~/.bro/v2-archive.json` (`archive.rs`: id set; undo batches are per-run memory, not persisted).
- **Live refresh:** every `Event::Services` while the launcher is open calls `refresh_continue` → `set_entries`, which keeps the cursor clamped and **drops the shift anchor** (`mod.rs:1019-1022`).

### 2.12 Resume flows

- **Single:** `open_resume` (§1.4). Resume uses `resume_request` (`sessions.rs:876-882`): same login, cwd = session cwd, else project root, else home. Permission Skip, browser Off, name = title fitted to 28. Not added to recents.
- **Fork into another login:** `fork_into` (§1.4) stages the transcript.
- **Queued (several)** (`resume_many`, `sessions.rs:567-650`):
  - One placeholder pane per session right away ("Resuming … in line", showing its place in the line).
  - Two or more are stacked side by side, unless the all-sessions grid is on.
  - They launch one at a time with a 1.5 s gap after each starts (`RESUME_GAP`). A stuck launch frees the line after 20 s (`RESUME_STUCK`).
  - Closing a placeholder drops it from the line. No login picker: each resumes in its own login.
  - Toast "resuming N sessions, one at a time".
- **Switch login (live):** §1.4.

### 2.13 Live data while the launcher is open

- `Event::Services` → `set_data(launcher_data_with_installed(app, l.data.installed))` (`mod.rs:1013-1018`). PATH is **not** searched again.
- `set_data` (`launcher/mod.rs:185-209`):
  - Keeps the selected account by `kind`, or the recent by `Recent::same`.
  - Keeps the model by id.
  - If the selected row vanished: reset models and focus List.
  - Clears `hits`, so a click between the event and the next draw does nothing.
- Shown live: meters, "refreshing…", "stale" (tests `app/tests.rs:68-130`).
- OpenRouter catalogue: models come from the cached catalogue, refreshed in the background when older than 6 h, then republished (`services/workers.rs:84-112`). The model choice survives by id.

### 2.14 Persistence touched by the launcher

- `settings.permission`: `~/.bro/v2.toml`, saved on change.
- Recents: `~/.bro/v2-recents.json` (`recents.rs`). `push` dedups by `same` (harness, profile, provider, model, cwd, orchestrator), newest first, keeps 20, atomic write on a thread. Written only on successful remembered launches.
- Open projects: `~/.bro/v2-projects.json` (`projects.rs`), case-insensitive on Windows.
- Archive: `~/.bro/v2-archive.json`.

---

## 3. Palette (`palette.rs`, App side in `app/commands.rs:479-530`, `app/input.rs:159-182`)

- **Entries**, in final order (builder chain in `commands.rs:484-489`):
  1. **Tiles** (`with_tiles`, spliced at 0):
     - "tiles All — every session side by side · N session(s)"
     - Per project: "tiles {name} — just its sessions · N session(s)"
     - `Cmd::Tiles`, icon window.
  2. **Recents:** the first 5 of *all* recents, orchestrator ones included.
     - Label "launch {harness} · {provider or full profile id} · {short model|default} — {short cwd≤30}", hint "recent", icon history.
  3. **Actions:** every `Act::all()` except Jump 4-9 and ShowTab 1-9.
     - Label `Act::describe()`, hint `km.primary(a)` (the first live chord, e.g. "alt+u", "ctrl+space S", or "").
     - Icon by `icon_for` (`:58-75`).
  4. **Claude env presets** (`with_presets`): "claude preset {label}[ · current] — {help}", icon key.
  5. **Caveman** (`with_caveman`): "caveman mode {id}[ · current] — {help}" for each level, then "caveman off[ · current] — agents write normally".
  6. **Themes:** `theme::names()` (built-ins, then custom files).
     - Label "theme {name}[ · light| · dark (painted dark)][ · yours (custom)][ · current]".
     - No hint; instead a swatch: the theme's background + 5 dots (accent, shine, good, inline, danger).
- **Matching** (`matches`, `:150-160`):
  - Query trimmed == "theme" → only themes, in their own order. Snapshot `theme-picker.html` shows 30.
  - Query trimmed == "tiles" → only tiles.
  - Otherwise `fuzzy::filter(query, "{label} {hint}")`, so chords are searchable.
- **Keys** (`:171-198`):
  - Esc → Cancel. Enter → Run the selected item (`Out::None` if none).
  - ↓, Tab, ctrl+n, ctrl+j → +1. ↑, BackTab, ctrl+p, ctrl+k → -1. PgDn/PgUp ±10.
  - ctrl+u clear. Backspace pop. Plain char append.
  - Every query change sets `sel=0`.
- **Live theme preview** (App, `input.rs:159-182`):
  - After any key that returns `Out::None`: `want = preview() (selected theme) or theme_before`; `set_theme(want, save=false)` if it differs. Moving off a theme row restores the original.
  - Cancel → restore `theme_before`.
  - Run a non-theme command → restore the theme first, then `run_palette`.
  - Run a theme → `set_theme(t, save=true)` + toast "theme: {t}".
  - Paste does the same preview (`mod.rs:1104-1111`).
  - Opening with "theme " doesn't preview until the first key.
- **Run** (`run_palette`, `commands.rs:506-530`):
  - Act → `run_act` (may open another overlay).
  - ClaudePreset → settings + toast "Claude preset: … (new launches)".
  - Caveman → `set_caveman` + toast.
  - Tiles → `show_tab`.
  - Recent(i) → launch that combo as a **non-orchestrator** session with the **current settings permission** and the recent's browser.
- **Layout** (`:200-235`):
  - Popup 76×22, title "{search icon} palette".
  - "› {query or 'type a command, a theme, a recent launch…'}▏", match count on the right.
  - Rows from inner+2: "▌ "/"  ", icon, label (accent bold when selected). Right side: hint (shine bold when selected, muted otherwise) or the swatch.
  - Scroll keeps the cursor on the last visible row. Empty: "  nothing matches".
- **Mouse:** any button-down closes and restores the theme. No row click, no wheel.
- **Fuzzy scoring** (`fuzzy.rs:5-42`):
  - Case-insensitive; whitespace in the query is ignored; an empty query matches everything with score 0.
  - **Greedy** in-order subsequence (first occurrence, not the best alignment).
  - Per matched char: +10; +8 at a word start (index 0 or after a non-alphanumeric); +6 when contiguous with the previous match; +(12−i)/3 when i<12.
  - Final score = sum − len(hay)/8.
  - `filter`: stable sort by score descending, ties by index. A blank query keeps the original order.
  - Used by: palette, launcher run-on (per group), launcher models, Continue, folder suggestions.

---

## 4. Watchdog setup/status overlay and prompt editor

### 4.1 Opening

- prefix w (`Act::Watchdog`, no direct chord) → `open_watchdog` (`app/watchdogs.rs:39-55`).
- Toasts when it can't open:
  - "focus an agent session first: a watchdog keeps one session going"
  - "watchdogs watch agent sessions: focus a Claude or Codex session and try again" (shown for any non-agent pane)
- `WatchdogView::new(pane, sid, title=fit(display_name,48), name=display_title fitted to 28 or "this session", agent=meta.label, existing snapshot, defaults=settings.watchdog, reviewer=reviewer_hint)`.
  - `reviewer_hint` (`watchdogs.rs:24-36`): `pick_reviewer` label + " · N% left", or "no signed-in login has usage left".
- **A snapshot exists:** status page, with the form preloaded from the plan.
- **No snapshot:** setup form with one empty milestone; idle/budget from settings defaults; fresh and second-opinions **hard-coded on** (not from settings).

### 4.2 Form fields (`watchdog/view.rs:66-94,532-647`)

- **Intro** (wrapped, muted):
  - New: "When {name} finishes a turn and goes quiet, bro wakes a fresh reviewer on the login with the most usage left. It reads the chat and the code, checks the work against these milestones and hands {name} its next task."
  - Editing an existing plan: "Edit what {name} works toward. Milestones the reviewer already marked keep their status, and the log and wake-up count carry over."
- "watches  {agent}".
- **MILESTONES** heading, right "{non-blank count} · in order · the first one starts now".
  - Each milestone is a `TextArea`, soft-wrapped to 1-6 rows.
  - 6-column gutter: "▌", "{n:>2} ", and (existing plans only) a status glyph ✓ done / ▸ active / ○ pending.
  - Done milestones are muted.
  - Placeholders: first "the first milestone and what done looks like, e.g. offline sync: writes queue, retry with backoff, tests cover it"; others "the next milestone · enter on an empty one finishes the list".
- **HOW IT WORKS:** label padded to 18, value to 20, then help text.
  - **Wake after:** "‹ {20s|1 min|2 min|5 min|10 min|30 min} idle ›" — "after each stop · a question waits 2 min or more". Choices: `IDLE_CHOICES` 20, 60, 120, 300, 600, 1800 s.
  - **Fresh context:** on/off — on: "may clear the conversation before a new milestone"; off: "keeps one conversation going".
  - **Second opinions:** on/off — on: "may open a helper, say an Opus pass on Codex UI work"; off: "only ever talks to {name}".
  - **Budget:** "‹ {N} wake-ups ›" — "then it stands down and tells you". Choices: `BUDGET_CHOICES` 10, 25, 50, 100, 250.
  - **Reviewer:** read-only, "{reviewer_hint}  picked fresh each wake-up".
  - "off" values are muted; the focused value is bold accent; others use inline colour.
  - **Copy is stale:** wake-ups are now routed to the **orchestrator** (`wake_watchdog` → `watchdog_to_orchestrator`, `watchdogs.rs:138-156`, `app/voice.rs:110-132`). The "fresh reviewer on the roomiest login" intro and the Reviewer row describe the old mechanism.
- **Error line** (danger) above the buttons.
- **Buttons:** " Start watchdog " (new) or " Save plan " (existing), then " Cancel ". Focused = ink on accent; primary unfocused = accent on frame.
- **Scrolling:** keeps the focused block visible; focusing a button scrolls to the bottom.
- **Hints:**
  - Milestone focused: "⏎ next milestone · alt+⏎ new line · alt+↑↓ reorder · ctrl+d delete · tab options · ctrl+s start"
  - Buttons: "⏎ choose · ←→ switch · ctrl+s start · esc cancel"
  - Options: "←→ change · ↑↓ move · ctrl+s start · esc cancel"
  - These say "start" even when the action saves.

### 4.3 Form keys (`key_edit`, `:245-346`)

Every key clears the error first.

- **Anywhere:**
  - **ctrl+s:** submit. An empty plan shows "Add a milestone first. The reviewer checks every handoff against them." and focuses milestone 1.
  - **Esc:** cancel. Existing plan → reload it and return to the status page. New → Close.
  - **Tab / BackTab:** next/previous field. Order: milestones…, Wake, Fresh, Second, Budget, Primary, Cancel. Up from Cancel goes to Budget; Down on Primary stays.
- **On a milestone:**
  - **alt+↑ / alt+↓ or ctrl+↑ / ctrl+↓:** reorder; focus follows the milestone.
  - **ctrl+d:** delete. The last remaining milestone is cleared instead, and its status reset.
  - **Backspace on a truly empty milestone** (and more than one exists): delete it; the cursor goes to the end of the neighbour.
  - **Enter:**
    - Non-blank → insert an empty milestone below and focus it.
    - Blank, with more than one milestone → remove it and focus Wake ("an empty line ends the list").
    - A single blank milestone → focus Wake.
  - **alt+Enter / shift+Enter:** newline inside the milestone.
  - **Anything else:** goes to `TextArea::key(k, field_width)`. ↑/↓ at the top/bottom visual row moves to the previous/next field.
- **On options or buttons:**
  - **←/h, →/l:** change. Wake and Budget step through their choices (`step_choice` snaps off-list values to the nearest). Fresh and Second toggle. On Primary/Cancel, ← and → switch between the two buttons.
  - **Enter / space:** change by +1, toggle, submit (Primary) or cancel (Cancel).
  - **↑/k, ↓/j:** previous/next field.
- **Paste** (`:220-239`, editing only, milestone focused):
  - Multi-line text pasted into a blank milestone, where every line is <200 chars after trimming bullets (`-`, `*`, `•`) and dropping blanks → one milestone per line; focus the last.
  - Otherwise the text is inserted into the current milestone.
- **Click:**
  - A button on the status page → press it.
  - An already-focused option or button → change(0).
  - Otherwise focus the clicked field; the cursor goes to the end, not to the click position.

### 4.4 Status page (`draw_status`, `:649-750`; `key_status`, `:359-406`)

- **Header by state**, with "wake-up {n} of {max}" on the right:
  - **Running:** "{braille spinner} reviewing  {reviewer} · {since}" / "woke because {reason}".
  - **Stood down:** "✓ stood down  {why}" / "e adds milestones or wake-ups and it watches again".
  - **Paused:** "‖ paused  {name} works on without it" / "p resumes watching".
  - **Watching:** "● watching  {agent}" / "wakes when {name} stops and sits idle {idle} · fresh context on|off · second opinions on|off".
- **MILESTONES** + "{done} of {total} done".
  - Each: " {glyph} {n:>2} " + title wrapped to ≤3 rows (the last row ends in "…" when truncated). Done = muted, active = bold.
  - A reviewer note shows as "↳ note" in inline colour.
- **RIGHT NOW** (while running): "  starting the reviewer…", or the last 6 steps. Earlier steps get "·"; the newest gets a spinner.
- **LAST WAKE-UPS:**
  - None: "  None yet. The first comes once {name} stops and sits idle for {idle}."
  - Otherwise newest first: "  {h:mm am/pm}  ✓/✗  {reviewer}", right side "{n} steps", then the summary wrapped to ≤3 rows (danger colour if failed).
- **No scrolling:** content past the bottom is cut off.
- **Buttons:** " Wake now " (primary), " Pause " / " Resume ", " Edit plan ", " Stop watchdog ", " Close ".
  - After the first stop press: "press s again to stop the watchdog; the session keeps running".
  - Hints: "w wake now · p pause · e edit · s s stop · esc close".
- **Keys:**
  - Esc or q → Close.
  - ←, h, BackTab → previous button; →, l, Tab → next button (clamped).
  - Enter or space → press the current button.
  - w → Wake. p → Pause/Resume toggle. e → Edit (reloads the plan into the form). s or x → Stop, which needs two presses; pressing any other button disarms it.
- The page is live: each draw and each key reads `svc.watchdog(sid)`. If the watchdog disappears while the status page is shown, the form is drawn instead.

### 4.5 App outcomes and storage (`watchdogs.rs:76-118`)

- **Start(plan):** `svc.watchdog_start`.
  - Error stays in the form, e.g. "watchdogs need the bridge (it serves the reviewer's tools): turn it on in the bridge view (alt+g)" (`services/mod.rs:682-696`).
  - Success closes the overlay. If the session is busy → toast "{name} · watchdog on · it reviews once {name} stops and sits idle {idle}". If idle → an immediate wake.
- **Save(plan):** `watchdog_set_plan`; toast "{name} · watchdog plan saved · the next reviewer works from it"; the overlay stays open on the status page.
  - The bridge un-finishes a stood-down watchdog when the new plan has unfinished milestones or more wake-ups (`crates/bro-bridge/src/watchdog/mod.rs:265-275`).
- **Wake:** "the user asked for a review now".
- **Pause(p):** toast "{name} · watchdog paused" / "{name} · watchdog watching again".
- **Stop:** toast "{name} · watchdog stopped · the session carries on as it is"; closes.
- **What's saved where:**
  - The plan (`WatchdogPlan { milestones: [{title, status, note}], fresh_context, second_opinions, max_wakes, idle_secs }`) and the log live **in the bridge's memory only**, keyed by session id. Nothing is written to disk (`crates/bro-bridge/src/watchdog/mod.rs:217-298`).
  - The UI caches snapshots in `svc.state().watchdogs`.
  - Defaults live in `~/.bro/v2.toml` `[watchdog]`: prompt (None), idle_secs 60, max_wakes 25, model "opus", reviewer "auto" (`crates/bro-core/src/config.rs:244-261`). They're edited in the Settings view (`views/settings.rs:588-625`).
  - The `plan()` sent drops blank milestones and carries each kept milestone's status and note through edits (`view.rs:154-163`).
- **Trigger** (`watchdog/mod.rs:55-129`):
  - Wakes after idle ≥ `max(idle_secs, 10)` once the session has worked since the last wake.
  - A question on screen waits `max(idle, 120 s)`.
  - Failed reviews retry after 2, 5 and 15 minutes.
  - Not UI, but it explains the copy.

### 4.6 Prompt editor (`watchdog/prompt_editor.rs`, App `watchdogs.rs:57-60,120-134`)

- **Opens:** Settings → Watchdog "Prompt" row, or the palette entry "watchdog prompt: edit what every reviewer is told". No default key.
- **Text at open:** the setting if non-blank, else `bro_bridge::watchdog::DEFAULT_PROMPT`. Enter types a newline; the cursor starts at the top.
- **Keys:**
  - **ctrl+s:** Save. The result is `None` when the trimmed text is empty or equals the trimmed default; otherwise `Some(trimmed)`.
  - **ctrl+r:** reset the text to bro's default (not saved).
  - **Esc:** with unsaved changes, the first press arms ("unsaved changes · esc again discards them"); the second closes. Without changes it closes.
  - **PgUp / PgDn:** `height-2` visual rows.
  - **Anything else:** disarms, then goes to the TextArea.
- **Paste:** insert, disarm.
- **Mouse:** none.
- **Layout:**
  - Popup `min(w-4, 110)` × `h-2`, title "{watchdog icon}watchdog prompt". Nothing is drawn if the inner height is <6.
  - Hints "ctrl+s save · ctrl+r bro's default · esc close".
  - Line 1: "Every reviewer starts with these instructions, then gets a brief: the session, its milestones and the log." Right side: "customized" (shine) or "bro's default".
  - Line 2: status — "unsaved changes · esc again discards them" (danger), "unsaved changes", or "applies from the next wake-up".
  - Then a rule, then the editor. Placeholder: "empty: bro's own instructions are used".
- **App on Save:** `settings.watchdog.prompt = …`, saved to `v2.toml`. Toast "watchdog prompt saved · reviewers use it from the next wake-up" or "watchdog prompt: back to bro's own".

### 4.7 TextArea, shared by milestones and the prompt (`textarea.rs`)

- **Wrapping:** soft wrap at the cell width (min 4). Breaks after the last space on the row; mid-word if one word is too long. Trailing spaces stay on their row.
- **Movement:**
  - ←/→ by char; ctrl/alt+←/→ by word.
  - Home/End: start/end of the visual row. ctrl+Home/End: start/end of the text.
  - ↑/↓: visual rows with a goal column; the TextArea reports "not handled" at the top/bottom edge.
  - PgUp/PgDn: 10 rows.
- **Editing:**
  - ctrl+a / ctrl+e: start/end of the visual row.
  - ctrl+w: delete the word to the left. ctrl+h: delete a char. ctrl+u: delete to the start of the logical line. ctrl+k: delete to the end of the logical line.
  - Backspace (with alt/ctrl: word) and Delete.
  - Enter: newline only when `newline_on_enter` is set or alt/shift is held.
- **Input cleaning** (`clean`): CRLF → LF, tab → 4 spaces, other control chars dropped.
- **Missing:** no undo, selection or copy.
- **Render:** the cursor is a reversed cell; scrolling keeps the cursor row visible; an empty field shows the italic muted placeholder.

---

## 5. Model/view separation (could a GUI reuse the state machines?)

**Verdict:** mostly yes. Key handling and data shaping are pure and well separated from drawing. Four blockers:

1. **Crate structure.** Every module is private to the `bro` binary (`main.rs:7-53`), and app glue is `pub(crate)` or `pub(super)`. Before anything is reusable, either add `crates/bro-tui/src/lib.rs` with `pub mod` and keep the TUI as a thin bin, or extract a `bro-ui-model` crate.
2. **Input types.** Every state machine takes crossterm types (`KeyEvent`, `KeyModifiers`; File and AgentFiles take `MouseEvent`). Hit tests use ratatui `Rect`/`Position`. Those are only compared via `contains`, so a GUI can register its own u16 rects (cells or logical px) and pass the same units to `click`. The other option is an `activate(Hit)` facade (point 4).
3. **Draw functions have model side effects** (hits, layout metrics, scroll clamps, even load triggers). The GUI layout pass must reproduce them.
4. **Private fields and helpers** in some models need accessors or setters.

### Per overlay

- **Launcher:** model in `launcher/mod.rs`, view in `launcher/view.rs`.
  - All fields pub (`:156-181`). `Data`, `Item`, `Account`, `AccountKind`, `Hit`, `Outcome`, `Focus`, `SessionTab` are pub.
  - **Pure pub API:** `new`, `with_*`, `set_data`, `show_continue`, `set_continue_entries`, `select_project`, `quick_spec`, `brain_default`, `smart_sel`, `installed`, `items`, `current`, `cursor_row`, `needs_model`, `models`, `models_view`, `model`, `spec`, `key`, `click`, `scroll`, `paste`.
  - **Private, needed by a hit-free GUI:** `set_harness`, `enter_orchestrator`, `set_brain`, `move_sel`, `reset_models`, `filter_changed`, `launch`, `sign_in_needed`, `selectable`. Suggest extracting `click`'s match into `pub fn activate(&mut self, hit: Hit, mods) -> Outcome` and adding `pub fn wheel(over: Option<Hit>, down)`.
  - **Draw side effects:** writes `hits` and `area`. The scroll window is derived per frame from the cursor (`view.rs:294`); there is no stored scroll.
  - **Data builder:** `app/overlays.rs:225-280` is App-coupled (`pub(super)`) and does UI-thread I/O (`which`, Gemini creds file). Re-home it as a function over (State, recents, orchestrator label).
  - **Copy lives in both places:** account detail strings are in the model; all panel and button copy is in the view.
  - The GUI re-implements `view.rs` (breakpoints, chips, meters, the button) and reuses everything else.
- **FolderPicker** (`folder.rs`): model and view share a file.
  - pub `filter`, `sel`, `dirs`, `hits`; private `view` (accessor `view()`); `pub(crate) pending`, `error`. Those last two are invisible to another crate → add getters.
  - **Not pure:** `refresh`, `open_selected` and even `draw_for_session` (`is_file`/`exists`) touch the filesystem synchronously.
  - `draw_for_session` writes `hits`. Needs `click_row(i)` for a hit-free GUI.
- **ContinuePicker** (`continue_picker.rs`): model and view share a file.
  - pub `filter`, `sel`, `all`, `show_archived`, `project`, `entries`, `hits`, `scope_hits`, `picked`; private `anchor`, `view` (accessor `view()`).
  - Pure. `draw_in` writes `hits` and `scope_hits`. Needs `click_row(k, mods)`.
  - The entry builder `App::continue_entries` is App-coupled.
- **Palette** (`palette.rs`): every field pub; `Item`, `Cmd`, `Out` pub.
  - Pure except that `new` calls `theme::get` for every theme, reading custom theme files.
  - `draw` is read-only with no hits. The theme-preview logic lives in App (`input.rs:159-182`), so a GUI must reimplement it.
  - Swatch colours are ratatui `Color`.
- **Help** (`help.rs`): `lines(&Keymap) -> Vec<Line>` is pure and pub, the ideal reuse point. `Help { pub scroll }` plus `key` are pure; draw clamps the scroll.
- **ResumePicker / Confirm** (`app/overlays.rs`): data structs fully pub. Key and click logic lives in App (`resume_key`, `confirm_key`, `confirm_answer`, `resume_chosen`; `pub(super)`/`pub(crate)`). Draw functions are private. Small enough to reimplement.
- **WatchdogView** (`watchdog/view.rs`): **least reusable as-is.**
  - Only `pane`, `sid` pub. All display state is private: title, name, agent, editing, milestones, kept, idle_secs, max_wakes, fresh, second, focus, button, reviewer, hits, scroll, field_width, stop_armed, error.
  - `Field`, `Button`, `Hit` are private enums.
  - pub: `new`, `plan`, `paste`, `key`, `click`, `set_error`, `Out`.
  - Key handling depends on `field_width`, which draw sets for visual-row movement. Scroll is computed in draw.
  - `hints()` and `intro()` copy are private.
  - Needs read accessors (or pub fields), a `set_field_width`, pub `Field`/`Button` + `activate`, and copy getters.
- **PromptEditor:** every field private. `width` and `height` are set by draw and drive PgUp/PgDn and wrapping. Needs accessors (text, cursor, dirty, armed, custom) and `set_viewport(w, h)`.
- **TextArea:** `text()`, `rows(width)`, `height()`, `key(k, width)`, `insert`, `set_text`, `cursor_to` are pub. The cursor `cur` and `scroll` are private → add `cursor()`.
  - Wrapping is by terminal cell width (unicode-width). **A proportional-font GUI (Inter) needs a measure callback**, or must render editors in monospace.
- **FileView:** pub `pane`, `roots`, `target`, `line`, `column`, `fallback`, `body`, `top`, `left`, `hits`, `frame`; private `req`, `loading`, `center`, `rows`, `theme`, `tx`.
  - Loading posts `crate::pane::Event::FileView` on the app channel.
  - **Draw triggers centring and paging loads** (`file_view.rs:577-582`) and sets `rows`. Needs `set_rows` + `on_layout()`.
  - Highlighted lines are `Vec<(ratatui::Style, String)>`, highlighted with the theme captured at open (no re-highlight on theme change).
- **AgentFilesView:** `items()` builder and `key` are pure. `mouse` relies on private `hits` and `rows` → needs `click_row(i)` / `set_rows`.
- **Fully reusable as-is (once in a lib):** `fuzzy`, `recents`, `archive`, `projects` (background atomic writes), `keymap` (Chord normalisation, live hints for help and palette), `watchdog::{human, step_choice, Trigger, pick_reviewer, reviewer_launch}`, `services::launch::{label_for, short_model}`, `util::{short_path, short_dur}`.

### Key adapter traps (winit → crossterm)

- shift+Tab must arrive as `BackTab`. The watchdog form, palette and launcher match `BackTab` explicitly; Tab+SHIFT would move the watchdog focus forward.
- ctrl+space can arrive as `Char(' ')`+CTRL, `Char('@')`+CTRL or `Null`. The Continue picker accepts all three.
- Alt chords with letters: the launcher checks `Char('p'|'P')`+ALT and `Char('o')` with **exactly** ALT.
- IME commits must arrive as `paste()` or as per-char `Char` events.
- Modifier-only presses must not reach Confirm (any key = "No").

---

## 6. Things a GUI port must not miss

### Behaviour

- **Launcher placement.** It is not a floating popup. It fills the body, keeps the sidebar and project strip live beside it, and sits under toasts. Its project picker is a centered popup over the body. Sidebar clicks while it's open follow `launcher_side_click`'s rules (§2.7).
- **Click-outside semantics differ per overlay** (§0.1). Palette and Help close on any click, inside included; a GUI could improve this, but should decide deliberately.
- **Permission persists on toggle** (settings), not just on launch. Browser defaults to the newest recent's browser. Placement is per entry point: alt+n → Tab, alt+enter → Split. ★ is always Tab.
- **Exact-login launches never fall back to another account.** `with_profile` + `quick_spec` refuse a logged-out exact login (test `launcher/tests.rs:81-91`).
- **One orchestrator only:** pending window of 90 s, focus-instead-of-launch, auto-enables the bridge.
- **Live data refresh preserves choices** by identity (account kind, model id), including the filter, dir, place and browser (test `app/tests.rs:68-101`).
- **Folder creation is asynchronous** with a request id. Esc cancels the UI, but the folder is still created on disk. Only explicitly typed paths may be created; stale history shows an error.
- **Clipboard reads are cancelled** by any input; pastes are stripped of quotes and control characters.
- **Multi-resume runs as a queue** with placeholder panes and stacking; there is no login picker for several sessions.
- **Fork-to-another-login** stages the transcript. Switch-login closes the source session before the transcript lookup can fail.
- **Watchdog details:** two-press stop; the multi-line paste splitter; Enter on a blank milestone ends the list; live redraw at 125 ms; status page truncates without scrolling; editing an existing plan keeps statuses and notes.
- **Prompt editor:** two-press Esc discard. Save maps "default or empty" to `None`.
- **UI-thread blocking I/O** a GUI should move off its render thread: PATH lookup at launcher open, Gemini creds read on every Services event, and folder-picker filesystem calls on each keystroke and in draw.

### Quirks: decide keep or fix

- Backspace or ctrl+u on the run-on list resets the selection to the first row (`launcher/mod.rs:924-927,701-709`), because they edit the hidden `list_filter`.
- Alt+letters with no binding are typed into the folder-picker, Continue and model filters as plain letters. On the run-on list they open the project picker.
- Clicking an already-selected model launches via `launch()`, skipping the not-installed check.
- Enter on a "not logged in" profile still launches it; only `quick_spec` refuses.
- Palette recents include orchestrator combos but launch them as plain sessions with the current permission.
- `Launcher::new` picks the harness from `recents[0]` even when that was an orchestrator launch.
- Stale copy:
  - Help: "tab ← →: next column" (← → actually switch agent tabs).
  - Watchdog hints say "ctrl+s start" when editing, where it saves.
  - The watchdog intro and Reviewer row describe the old fresh-reviewer model.
  - The help mouse sheet describes terminal-specific gestures (shift+drag select etc.).
- `set_data` clears hits, so a click arriving between a service update and the next frame is dropped.
- Services events drop the Continue picker's shift-range anchor.

---

## Files

**Primary area:**
- `F:\bro-cli-v2\crates\bro-tui\src\app\overlays.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\launcher\mod.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\launcher\view.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\launcher\tests.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\palette.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\continue_picker.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\help.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\folder.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\textarea.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\fuzzy.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\watchdog\mod.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\watchdog\view.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\watchdog\prompt_editor.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\recents.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\archive.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\projects.rs`

**Other overlays:**
- `F:\bro-cli-v2\crates\bro-tui\src\file_view.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\agent_files.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\app\files.rs`

**App glue:**
- `F:\bro-cli-v2\crates\bro-tui\src\app\input.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\app\mouse.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\app\commands.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\app\sessions.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\app\watchdogs.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\app\history.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\app\mod.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\app\draw.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\keymap.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\ui.rs`
- `F:\bro-cli-v2\crates\bro-tui\src\services\launch.rs`

**Backing data:**
- `F:\bro-cli-v2\crates\bro-bridge\src\watchdog\mod.rs`
- `F:\bro-cli-v2\crates\bro-core\src\config.rs` (WatchdogSettings, lines 244-261)

**Snapshots** in `F:\bro-cli-v2\crates\bro-tui\snapshots\`:
- `launcher.html`, `launcher-orchestrator.html`, `launcher-in-body.html`, `launcher-openrouter.html` (now shows the project picker)
- `continue-picker.html`, `resume-picker.html`, `palette.html`, `theme-picker.html`, `help.html`
- `watchdog-setup.html`, `watchdog-milestones.html`, `watchdog-status.html`
- `new-session-strip.html`, `notifications-top-right.html`
