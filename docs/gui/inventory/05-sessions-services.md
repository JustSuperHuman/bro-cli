I've read every file you listed, plus the glue code they depend on: `app/mod.rs`, `pane.rs`, `panes/term.rs`, `panes/insights.rs`, `panes/auto_resume.rs`, `panes/remote_min.rs`, `panes/writer.rs`, `watchdog/mod.rs`, `alerts.rs`, `perf.rs`, `paste_burst.rs`, `console_nav.rs`, `fonts.rs`, `recents.rs`, `projects.rs`, `archive.rs`, `util.rs`, `ctl_cmd.rs`, `open_cmd.rs` and parts of `commands.rs`, `input.rs`, `overlays.rs` and `project_tabs.rs`. Nothing was edited and nothing was built.

# Sessions, agent tracking and background services: inventory for GUI parity

## 0. File index

Inline refs below use short names. Prefixes: `T\` = `F:\bro-cli-v2\crates\bro-tui\src\`, `B\` = `F:\bro-cli-v2\crates\bro-bridge\src\`.

- Sessions: `F:\bro-cli-v2\crates\bro-tui\src\app\sessions.rs`
- Insights: `F:\bro-cli-v2\crates\bro-tui\src\app\insights.rs`
- Watchdogs: `F:\bro-cli-v2\crates\bro-tui\src\app\watchdogs.rs`
- Voice: `F:\bro-cli-v2\crates\bro-tui\src\app\voice.rs`
- Files: `F:\bro-cli-v2\crates\bro-tui\src\app\files.rs`
- Loop, `close`, `handle`: `F:\bro-cli-v2\crates\bro-tui\src\app\mod.rs`
- Services: `F:\bro-cli-v2\crates\bro-tui\src\services\mod.rs`, `workers.rs`, `launch.rs`, `guard.rs`, `updates.rs`, `justtools.rs`, `justimagine.rs`, `justpaste.rs`, `justwatch.rs`, `demo.rs` (all in that folder)
- Terminal pane: `F:\bro-cli-v2\crates\bro-tui\src\panes\term.rs`, `insights.rs`, `auto_resume.rs`, `remote_min.rs`, `writer.rs` (same folder)
- Watchdog logic: `F:\bro-cli-v2\crates\bro-tui\src\watchdog\mod.rs`
- Entry points: `F:\bro-cli-v2\crates\bro-tui\src\main.rs`, `cli.rs`, `instance.rs`, `devprobe.rs`, `orchestrator_state.rs`, `perf.rs`, `paste_burst.rs`, `console_nav.rs`, `fonts.rs`, `bin\bro-app.rs` (all under `src\`)
- Bridge API: `F:\bro-cli-v2\crates\bro-bridge\src\lib.rs`

---

## 1. Session lifecycle

### 1.1 Core types

- **`Meta`** (term.rs:46-71) holds:
  - `sid`: uuid v4, which is also the bridge id.
  - `harness`.
  - `profile`: `provider_id` or else `profile_id`.
  - `store`: the login whose folder holds the transcript. `transcript_store` (sessions.rs:885-895) uses the profile when it matches the harness family, else `claude:local` / `codex:local`; None for pi/omp.
  - `model`, `label` ("claude · work · opus-5"), `name`, `renamed`, `cwd`, `project` (ProjectKey), `started`.
  - `route_id`, `cleanup` (staged files), `orchestrator`.
- **`Spawn`** `{program, args, env, env_remove}` (term.rs:84).
- **`Term` constructors:**
  - `new`: spawns lazily at first render (term.rs:232).
  - `queued`: resume placeholder. It never spawns and reports `Activity::Working` (term.rs:285, 1045).
  - `fixed`: static ANSI, for demo snapshots (term.rs:324).
- **`LaunchRequest`** (launch.rs:19-35): `spec`, `place`, `stage`, `reply` (tokio oneshot to a bridge client), `name`, `remember`, `find_live`, `into` (placeholder pane).
- **`Launched`** (launch.rs:53-66): `spec`, `place`, `reply`, `name`, `remember`, `result: Result<CommandSpec,String>`, `route_id`, `note`, `into`.

### 1.2 Where launches come from

- **Launcher** `Outcome::Launch` calls `launch_spec` (commands.rs:433-452):
  - For an orchestrator: focus the running one, or toast "already starting…" if one is pending under 90 s. Otherwise set `orchestrator_pending`.
  - Then `remember=true`, toast "launching {label}…", `svc.launch`.
- **Quick paths** use `Launcher::quick_spec()` and open the launcher if a choice is still needed:
  - sidebar strip icons: `quick_launch` (commands.rs:364)
  - alt+q: `quick_agent` (home dir, last non-orchestrator harness, commands.rs:384)
  - usage rows: `launch_usage` (commands.rs:411)
  - palette recents (commands.rs:523)
- **Resume / fork / move login:** see 1.6.
- **Bridge `Create`:** see section 3.
- **Orchestrator inbox** needs one (voice.rs:135-147). **Restore at startup** (sessions.rs:332).
- **Panes that bypass the launch pipeline** (built directly as `Term::new`):
  - `open_shell` (sessions.rs:177): `settings.shell`, else pwsh, powershell, then cmd.
  - `install_agent`: tab running `bro install --wait <agent>` (sessions.rs:189).
  - `sign_in_gemini`: `bro gemini login --wait` (sessions.rs:200).
  - Login tabs labelled "login · <id>", plus `watch_login` (views/profiles.rs:106-143).
  - Terminal editor panes (files.rs:142).
  - Quick-install tabs (views/quick_install.rs:36-64). These run `std::env::current_exe()`.

### 1.3 Build thread: `launch::run` (launch.rs:69-136), one thread per launch

1. Demo mode: `demo::shell_command` (a harmless shell that prints an ANSI transcript).
2. `find_live`: `sessions::latest_for(harness, store, cwd, since)`.
   - Error text: "couldn't find this session's transcript — it moves once the agent has answered at least once".
   - Same login: plain resume. Different login: fork plus stage.
3. `stage`: `sessions::stage_for_profile(s, target)`. The staged paths are appended to `cmd.cleanup`.
4. Bridge URL:
   - Orchestrator: `orchestrator_bridge` (launch.rs:142-158). If the bridge is off it calls `set_bridge_enabled(true)` and toasts "turned on the bridge: the orchestrator drives bro through it". It then polls `bridge_local_url` every 150 ms for up to 20 s, else fails.
   - Everything else: `bridge_local_url()`, which may be None.
5. `LaunchCtx` is built from State: proxy base/token and `route_native` (only when the proxy is Ready), `claude_env`, `caveman`, `justtools_skill`, `justwatch_skill`, `bridge_url`.
6. `bro_core::launch::build`. If it panics, `fallback_command` is used (native only) with note "… — used the built-in launcher".
7. If `cmd.route` is set: resolve the upstream (`launch::resolve_upstream`, falling back to `map_upstream`), then `proxy.upsert_route`.
   - No proxy: error "this launch goes through the proxy, which isn't running".
   - Publishes `st.proxy.routes`.
8. Sends `Event::Launched`.

### 1.4 Launched becomes a pane: `App::launched` (sessions.rs:56-165)

- **Placeholder check:** the `into` placeholder only counts if it still exists and is still queued. `resume_started` runs for any `into`. An orchestrator spec clears `orchestrator_pending`.
- **On error:**
  - An orchestrator with persist on and none running: `orchestrator_state::clear()` so it doesn't retry every start.
  - Error toast "couldn't launch {harness}: {e}".
  - `reply` gets Err. The placeholder pane is closed.
- **`remember`:** `recents::push` and save to `~/.bro/v2-recents.json` (not in demo).
- **Label:** `cmd.label`, else `label_for`. For the orchestrator the "orchestrator · " prefix is stripped.
- **Meta fields:**
  - `renamed = name.is_some() && reply.is_some()`. A bridge-given name sticks; a resume's starting title gets replaced by the agent's own title.
  - `project`: `orchestrator_project(cwd)` (key `bro:orchestrator`, name "★ orchestrator") or `svc.project_for(cwd)`.
- **Second orchestrator:** added and closed immediately (so its route and staged files get cleaned up), the existing one is focused, toast "the orchestrator is already running".
- **Orchestrator placement:** always `Place::Tab`. With persist on, `orchestrator_state::save(&spec)`.
- **`into` path:** `panes.insert(id, term)` reuses the same PaneId, so it keeps its tab, grid slot and sidebar place. Clears `agent_state`/`last_tick` for it, then `start_now`. If the placeholder was closed meanwhile: add then close (cleanup). Shows the `note` toast.
- **Normal path:** `open(term, place)`, or a Tab if there are no tabs.
  - **Background** (`reply` present, i.e. a remote request): `start_now`, stay on the current tab, `restore_background_tiles` (sessions.rs:317-328: rebuild a tile-all grid or restore the stack exactly), `reply.send(Ok(sid))`.
  - Otherwise `leave_sidebar` (focus goes to the new pane).
- **`start_now`** (sessions.rs:168): `ensure_started` at body size minus 2 (minimum 10x40). Used for hidden or remote panes.

### 1.5 PTY spawn: `Term::start` (term.rs:347-407)

- `portable-pty` (vendored, has a job-handle hook) at the real pane size.
- `resolve()` (term.rs:913): `.cmd`/`.bat` run through `cmd.exe /c`.
- cwd falls back to home when it isn't a directory.
- **Environment:**
  - Set: `TERM=xterm-256color`, `COLORTERM=truecolor`, `BRO=1`, `BRO_SESSION_ID=sid`, `BRO_BIN=current_exe`, `BRO_BRIDGE_URL` (or removed when the bridge is off).
  - Removed: `NO_COLOR` and every `CLAUDE_SESSION_ENV` var (term.rs:31-42).
  - Then the spec's `env_remove` and `env`.
- **Windows:** `bro_core::proc_tree::ProcessTree` job object via `set_windows_job_handle`, then `tree.watch_pty(pid)`.
- **Writer:** `panes/writer.rs` thread "bro-pty-input", so a child that stops reading never blocks the UI.
- **After spawn:**
  - `svc.bridge_register(bridge_meta)` (term.rs:479-493).
  - The Spawn is kept for `/recap`.
  - Claude/Codex panes outside demo start an `Insights` worker (`bind_spec`, term.rs:410).
- **Reader thread** (term.rs:503-542), per chunk:
  - `vt100` process.
  - Answer DSR `\x1b[6n` (ConPTY waits for it) after releasing the parser lock.
  - `svc.bridge_output(sid, bytes)`: raw bytes, in order.
  - Update `last_output`, `waker.wake()`.
  - On EOF: `exited=true`, wake.
- **`Waker`** (pane.rs:121-157) coalesces wakes with a pending flag, so output floods don't queue thousands of redraws ahead of input.

### 1.6 Resume, fork, move login, queued resumes

- **`resume(idx)`** (sessions.rs:559) uses `resume_request` (sessions.rs:876): own login, `Resume{fork:false}`, `Permission::Skip`, `BrowserMode::Off`, name = `fit(title,28)`. Toast "resuming “title”…".
- **`open_resume(idx)`** (Enter/f on an earlier session, sessions.rs:686):
  - pi/omp: resume directly.
  - Otherwise `resume_targets` (sessions.rs:653-675): authenticated logins of the same family; detail "plan · email"; `left` = min(h5, wk) of `headroom_detail`; sorted current first, then most left.
  - One target or none: resume directly. Otherwise open the `ResumePicker` overlay.
  - Default selection `resume_default` (sessions.rs:678): current login unless under 10% left, then the roomiest with at least 10%.
- **Picker keys** (overlays.rs:122-138): ↑↓/j/k, 1-9 picks and goes, Enter, Esc.
- **`resume_chosen`** (sessions.rs:722-750):
  - Past + current login: `resume`.
  - Past + other login: `fork_into` (sessions.rs:753). Sets `stage=Some(s)`, `Resume{fork:true}`, toast "resuming in {target}…".
  - Live + current: no-op.
  - Live + other: `close(pane)`, then launch with `find_live{harness, store, cwd, since}` and name; toast "moving to {profile}…".
- **`open_switch(pane)`** (alt+L / prefix P, sessions.rs:702-719):
  - Needs harness and store, else toast "only Claude and Codex sessions can move to another login".
  - `since = now − started`.
  - One target or none: toast "no other {h} login to move to — add one in profiles (alt+o)".
  - Default selection prefers a non-current login.
- **`resume_many`** (sessions.rs:567-610), several sessions picked together:
  - Opens `Term::queued(meta, ahead)` tabs immediately.
  - Two or more and not tile_all: stacked side by side. Otherwise focus the first.
  - Toast "resuming N sessions, one at a time".
- **`pump_resumes`** (sessions.rs:622-650), runs on every `Event::Tick` and after each report:
  - One launch at a time.
  - `RESUME_GAP` 1.5 s after one reports back; `RESUME_STUCK` 20 s frees the slot.
  - Drops closed placeholders.
  - Writes `queued.ahead` / `queued.starting` for the loader text ("starting now" / "up next" / "in line · N ahead", term.rs:292-321).

### 1.7 Naming and renaming

- `display_title()` (term.rs:727): `renamed ? name : agent_title() ?? name`. `agent_title` is the OSC 0/2 title cleaned by `clean_title` (term.rs:1279): leading spinner glyphs stripped; exe paths and generic names ("claude", "pwsh", …) rejected.
- `display_name()` = `display_title()` or `label`.
- Rename UI: `Act::Rename` (prefix `,`) calls `start_rename` (commands.rs:532). Only Terms can be renamed; the sidebar inline editor allows at most 40 chars.
- `rename_key` Enter (input.rs:203-228): sets `name` (empty clears it and sets `renamed=false`), then `svc.bridge_title(sid, display_name)`. Esc cancels.
- Remote rename: `BridgeCommand::Rename` sets `name`/`renamed` but doesn't call `bridge_title`. The next `Term::poll` (400 ms) pushes the changed display name (term.rs:1092-1099, `title_sent`).
- The card back prefers the transcript's `custom_title`/`ai_title` unless renamed (draw.rs:452).

### 1.8 Orchestrator

- **Start:** `Act::Orchestrator` (alt+O / prefix O) calls `open_orchestrator` (commands.rs:338): focus the running one, else the launcher opens on its tab (`with_orchestrator`).
- **Also started by:** bridge `Create{profile_id:"orchestrator"}`, the inbox (`send_to_orchestrator`), and startup restore.
- **Only one:** `orchestrator_pane()` (sessions.rs:342), the 90 s `orchestrator_pending` window, and the duplicate-close in `launched`.
- **Spec choice** `voice_orchestrator_spec` (voice.rs:151-191), in order:
  1. The persisted spec.
  2. The newest orchestrator recent, with cwd = `bro_core::orchestrator::workspace()`.
  3. `roomiest_login` (voice.rs:195-211): signed-in Claude/Codex whose CLI is on PATH; an unread meter counts as 50%; Claude wins ties. Gets `Permission::Skip`, browser Off.
  4. No profile: Claude if `claude` is on PATH, else Codex if `codex` is on PATH, else None.
- **Persistence** in `~/.bro/v2-orchestrator.json` `{spec, at}` (orchestrator_state.rs):
  - Saved on a successful launch.
  - Cleared when the user closes it (mod.rs:689-695, skipped while `quitting`) and when its launch fails.
  - Restored by `restore_orchestrator` from `main` (sessions.rs:332; main.rs:250). Not in demo.
- **Pinned/excluded:**
  - Project key `bro:orchestrator`; pinned atop the sidebar and numbered first (sidebar.rs:118-121, 185-189).
  - Excluded from project groups/grids/close-all, from `current_project` (sessions.rs:439) and `follow_focus`.
  - Bridge `SessionMeta.kind="orchestrator"`, `project=None`.
  - Gets its own strip tab (`StripTab::Orchestrator`).
  - `wake_watchdog` skips it.
- **Closing it** calls `orchestrator_closed` (voice.rs:308): `schedule_clear_session` and fail anything awaiting an answer.

### 1.9 Projects (affect session placement)

- `start_in(dir)` (sessions.rs:412): the folder bro opened in becomes an open project (saved to `~/.bro/v2-projects.json`) and the current one.
- `current_project` (sessions.rs:430-441): sidebar selection while browsing, else `cur_project` (follows focus via `follow_focus`, mod.rs:863), else the focused non-orchestrator pane, else the first open project. New sessions and bridge creates without a cwd start here.
- `add_project` (sessions.rs:502): from `Event::OpenProject` (instance hand-off) or clipboard folder. Adds and saves, picks it, selects its sidebar row, toasts "opened X" / "X is already open", opens the launcher there.
- `close_project` / `close_project_now` (sessions.rs:518-547): confirms when sessions are live, closes panes with that key, removes the project, toast "closed {name} · choose it from New session".
- Archive (`~/.bro/v2-archive.json`): hides past sessions from the sidebar and continue list. Undo via ctrl+z (commands.rs:592-614).

### 1.10 Demo mode (`bro --demo`)

- `Services::demo`: offline fake State (demo.rs:82-155) plus a ticker posting a fake proxy event every 2.5–6.1 s (demo.rs:229).
- `fallback_settings()`: never reads or writes `v2.toml`.
- No instance hand-off or serve. `persist=false`, so recents, projects, archive and orchestrator state are neither loaded nor saved.
- `open_demo_sessions` (sessions.rs:814-872): 5 sessions across 3 projects.
  - Sids `demo-i`, back-dated `started`, `demo_activity` fixed.
  - Shells print an ANSI transcript (`demo::shell_command`), or `Term::fixed` when `fixed_demo`.
  - The shell is a SplitRight in tab 1; `done` flags preset.
- `seed_projects` opens the demo projects.
- Launches build a demo shell.
- Many actions toast "the demo doesn't …".
- Watchdogs are cached locally without the bridge (services/mod.rs:683-688).
- `Services::offline`: same data, no threads (tests).

### 1.11 Closing and cleanup

- **`App::close(id)`** (mod.rs:619-696):
  - Fix up stack, minimized and tile state.
  - `svc.watchdog_stop(sid)`.
  - `t.kill()`: sets the `closed` flag (which cancels a running recap), `ProcessTree::terminate`, child kill.
  - `t.on_exit()`: `bridge_exit(sid, code)` + `bridge_unregister`.
  - Login tab: `svc.login_closed(profile)`, which reloads profiles.
  - Proxy route removed unless another pane shares the same `route_id`.
  - `meta.cleanup` deleted on a thread: `sessions::cleanup_staged`, with manual removal as fallback.
  - Drop from `last_tick`, `seqs`, `done`, and the tab tree (remove the tab if it was the last leaf).
  - Refocus what remains. Orchestrator handling as in 1.8.
- **Drop:** `Term` drop stops `insights` and kills (term.rs:964). `Live` drop: terminate tree, kill, wait (term.rs:134-141).
- **`reap()`** (mod.rs:1134-1145), after every event batch: panes with `!alive()` are closed.
  - Toast "{name} exited", or Error "{name} exited: {why}" when `early_failure` applies: non-zero exit within 15 s, using the last two non-empty screen lines (term.rs:698-707).
  - `alive` comes from the reader EOF or from `poll`'s `exit_code()`. ConPTY can keep the pipe open after the child exits, so terminals poll every 400 ms even when hidden (term.rs:1068-1080).
- **`ask_close`** (commands.rs:211): confirm overlay when activity isn't Idle ("the agent is still busy — it will be stopped").
- **Quit** (commands.rs:196): confirm if any terminal is live. Then `App::shutdown` (mod.rs:881): `quitting=true`, close every pane, `svc.shutdown()` (proxy shutdown; bridge shutdown waits up to 5 s). Then `fonts::restore` and `restore_terminal`.

---

## 2. Agent tracking

### 2.1 Activity: `Idle | Working | Blocked` (pane.rs:89)

- **`Term::activity()`** (term.rs:1044-1055):
  - Queued placeholder: Working.
  - Otherwise `demo_activity` or the screen status.
  - Forced to Blocked when the transcript shows a pending question tool (`AskUserQuestion`, `ExitPlanMode`, `request_user_input_async`, term.rs:439). Claude's question menu shows "esc to cancel", which would otherwise read as working.
- **`scan()`** (term.rs:798-826), at most every 200 ms inside the 400 ms poll, over the bottom 14 non-empty rows (lowercased):
  - Working markers: "esc to interrupt", "esc to cancel", "ctrl+c/ctrl-c to interrupt".
  - Blocked markers: "do you want to", "allow once", "(y/n)", "❯ 1. yes", "enter to confirm", ….
  - Any marker sets `agent=true`, so a shell can become an "agent".
  - Output within the last 1500 ms counts as Working.
  - `prompt_title` = the nearest row ending in "?" that isn't an option row.
- **`busy()`** (term.rs:1056): the transcript worker's `busy` (Claude `sessions/<pid>.json` status; Codex `turn_open`) and not Blocked, else `activity==Working`.
- **Look:** `status_look` gives the glyph, word and colour (needs you / working spinner / done / idle, term.rs:984). The header also shows: scrolled lines, a remote-grown "⇲ C×R remote" badge, auto-resume note, watchdog badge, and activity (term.rs:1000-1037).

### 2.2 Done / needs-you: `track_agents` (sessions.rs:766-799), runs after every batch

- **seen** = `visible.contains(id) && term_focused`. Window focus matters: FocusGained/FocusLost in mod.rs:974-988.
- **Working → Idle while unseen:** `done.insert` and a `Kind::AgentDone` toast "{title} finished · {reason or recap}".
- **Into Blocked while unseen:** `Kind::NeedsYou` toast "{title} needs you · {reason}". The extra text is collapsed and cut to 72 chars.
- **Clearing done:** a visible pane leaves `done`; so does `focus_pane` (mod.rs:711, 722). Dead ids are pruned.
- **`attention_target`** (sessions.rs:803; alt+a / prefix a): sidebar order rotated past the focused pane; a Blocked pane first, else a done one.
- **`raise()` only toasts** (mod.rs:743). There are no OS notifications. Toasts: at most 2 shown, 2 s each, duplicates by text replaced (alerts.rs).

### 2.3 Transcript insights worker (panes/insights.rs)

- Thread "bro-insights" per Claude/Codex pane, every 1 s (100 ms sleep slices so a closed pane stops quickly).
- `Binder::new(BindSpec{harness, profile_id=store, config_dir from CLAUDE_CONFIG_DIR/CODEX_HOME in the spawn env, pid, cwd, started, resume_id})`, then `Tail` reads new lines and `AgentFacts::ingest` runs.
- Publishes `Snapshot{facts, transcript, session_id, generated, busy, rev}` and wakes the pane.
- `generated` = a cached `/recap` still fresh for this transcript (Claude only).
- A guarded parse panic resets the tail.

### 2.4 `update_insights` (insights.rs:207-270), runs after every batch

- Only for Term panes with a harness. The state comes from `attention_state(activity, done)` (insights.rs:67).
- `View::of` (insights.rs:144): `reason_for` (Blocked uses the pending call / prompt title; Done/Idle uses `live::waiting_reason`), and `recap::choose(facts, generated)`.
- **Sidebar second line** (`notes`): "⏸ reason", else "↳ recap"; nothing while Working (insights.rs:87).
- **`Track` per pane:** state, since, `since_wall`, `last_seen`, a fingerprint of what was sent, `files_sent`, `recap_tried`.
- **Bridge, only on change:**
  - `bridge_set_insights(sid, SessionInsights{attention{state, reason, since ISO}, recap{text, at ISO, source}})` when the fingerprint changes.
  - `bridge_set_agent_files(sid, AgentFiles{transcript, sent[], touched[]})` when (facts.revision, transcript) changes.

### 2.5 Recaps (Claude's own `/recap`)

- **Conditions, all required:**
  - `settings.insights.generate_recaps`
  - Claude
  - state Done or Blocked
  - no recap already running (one at a time across all panes)
  - `recap_due`: unseen for `recap::UNSEEN_AFTER` = 180 s, counted from max(since, last_seen); once per wait
  - `recap::worth_generating`
  - the pane has a launch spawn, session_id and transcript
- **Runner `run_recap`** (insights.rs:297-354):
  - Command: `claude -p --resume <sid> --fork-session --no-session-persistence --output-format json /recap`.
  - Runs in the cwd with the pane's env minus `recap::strip_env` and `CLAUDE_SESSION_ENV`.
  - Windows: `CREATE_NO_WINDOW`, plus a ProcessTree.
  - Polls every 250 ms. Timeout `recap::TIMEOUT` = 120 s. Cancelled when the pane closes (the `closed` flag).
- **Result:** `recap::store(...)`, then `Insights::set_generated`. Thread "bro-recap". A failure isn't retried for the same wait.

### 2.6 Other per-pane agent automation (inside `Term::poll`)

- **auto_approve** (term.rs:856), when `settings.auto_approve` is on:
  - Claude only, Blocked, output quiet at least 300 ms, at least 1.5 s since the last answer.
  - `permission_prompt` must find "Do you want to …?" with a "1. Yes" option.
  - Writes `1\r` and toasts "{name} · approved: {what}".
- **auto_resume** (term.rs:829; panes/auto_resume.rs):
  - Detects a stall from the transcript's `api_error` or a banner on screen.
  - Causes: Transient / RateLimited / Limit(reset) / Fatal.
  - Backoff and `max_attempts`; types `cfg.prompt`, then Enter 250 ms later.
  - Toasts (Info/Usage/NeedsYou/Error). Header note "↻ retry N in Xs".
  - Backs off when the user types (`typed` counter).
- **Click a numbered prompt option** (mouse.rs:387-409, `Term::prompt_option_at`): only while Blocked and not scrolled back. Sends "{n}\r".
- **`submit()`** (bracketed paste, Enter 300 ms later) and **`type_command()`** (keys, Enter 400 ms later): used by the orchestrator inbox (term.rs:553-568).

### 2.7 Notifications and toasts raised by this area

| Kind | Text | Source |
|---|---|---|
| AgentDone | "{title} finished · …" | `track_agents` |
| NeedsYou | "{title} needs you · …" | `track_agents` |
| Info / Error | "{name} exited" / "{name} exited: {why}" | `reap` |
| Error | "couldn't launch {h}: {e}" | `launched` |
| Info | launch `note` | `launched` |
| Info | "the orchestrator is already running" / "…already starting…" | launch dedupe |
| Info | "launching …", "resuming …", "resuming in …", "moving to …" | launch/resume paths |
| Bridge | "title — body" | every bridge notification (bell / OSC 9 / OSC 777 / needs-input / REST `/api/notify` / `notify_user` tool). Overlaps the app's own done/needs-you toasts. |
| Info / Error | "Hugh: …" | orchestrator updates without a tag |
| Info / Error | "listening…", "you: …", "Hugh: …", "voice: …", "voice is off: set {ENV}…" | voice |
| AgentDone / Info / Error | watchdog woke / finished / stood down / failed | watchdog events |
| Info | "{who} opened {path}:{line} · {key} to view" | `agent_opened_file` |
| Info | "{id} signed in as {email}" | `watch_login` |
| Info / Error | update, JustTools, JustImagine, Smart Paste, JustWatch skill, settings-save error, "proxy didn't start", "bridge didn't start" | services |
| Info / Usage / NeedsYou / Error | auto-approve and auto-resume toasts | Term poll |

### 2.8 Everything bro tells the bridge (via Services; all fused, see 5.4)

- `register(SessionMeta{id=sid, title=display_name, shell=program, args, cwd, project=project key (None for the orchestrator), pid, cols, rows, agent=harness label, kind})` at spawn.
- `reregister` on `Event::BridgeStarted` (term.rs:496): program is the harness label or "shell", no args.
- `output` (every PTY chunk, reader thread), `title` (display-name changes, rename commit), `resize` (every pty size change), `exit` + `unregister` (close).
- `set_insights`, `set_agent_files`.
- `set_profiles`: shell, every authenticated profile, pi, omp (services/mod.rs:745).
- `set_known_folders` (mod.rs:579).
- `remote_viewers(sid)` is polled on every size apply.
- `route_orchestrator(true)`, `orchestrator_answer`, `schedule_clear_session`.
- Watchdog calls: `watchdog_brief`, `watchdog_finish`, `watchdog_start`, `watchdog_set_plan`, `watchdog_stop`, `watchdog_pause`.
- **Public bridge API the TUI never calls:** `cwd()`, `set_project()` (so cwd/project changes after start aren't mirrored), `set_orchestrator_launch`, `orchestrator_send(_with)`, `orchestrator_cancel`, `schedules`, `watchdog_wake`, `watchdog`/`watchdogs`, `notify`, `local_hosts`, `pairing_url`.

---

## 3. BridgeCommand variants (B\lib.rs:184-247) and their handling (sessions.rs:220-311)

Commands arrive through `Bridge::start`'s `on_command` callback on a bridge thread, as `Event::Bridge(c)`, handled in `bridge_command`.

| Variant | Handling |
|---|---|
| `Input{id,data}` | `term_id(sid)`, then `t.send(data)`. This bumps `typed`, which counts as user input for auto-resume, and resets scroll. |
| `Resize{id,cols,rows}` | Ignored while the pane is visible (the local view wins). Hidden pane: `t.resize(rows,cols)` sets `base`, then the remote minimum applies. |
| `Kill{id}` | `close(pid)`. Full cleanup; killing the orchestrator clears its persisted state. |
| `Rename{id,title}` | Trimmed title into `name`, `renamed = name.is_some()`. Empty clears it. |
| `Focus{id}` | `focus_pane`. |
| `OpenFile{id,path,line,column}` | `agent_opened_file` (files.rs:156): records `last_opened` (prefix f). If you're watching that session (focused, visible, no sidebar focus, not renaming, overlay None or File): open the preview overlay. Otherwise toast "{who} opened {short}:{line} · {key} to view". |
| `OpenInEditor{…}` | `open_in_editor` (files.rs:113). On a worker thread, `editor::plan_here` picks Gui (spawn, toast "opened x:12 in code"), Terminal (`Event::EditorPane`: a split beside the session) or System (OS open). |
| `Orchestrator(OrchestratorCommand)` | `orchestrator_command` (voice.rs:89-106). `Message{id,text,voice,focused,tag}`: voice messages use the spoken format, others become "[panel] text"; reply goes to `Panel(id)`. `Scheduled{id,name,prompt,persist}` becomes "[schedule \"name\" · persistent\|session trigger id] prompt" with no reply. `Cancel` writes ESC to the orchestrator pane. `ClearContext{handoff}` becomes `What::Clear`. |
| `Tile{project}` | `Some(dir)`: `show_tab(StripTab::Project(project_for(dir).key))`. `None`: `StripTab::All`. A no-op if that tab already shows. |
| `Create{req, reply}` with `profile_id=="orchestrator"` | Running orchestrator: reply `Ok(its sid)`. Pending under 90 s: `Err("The orchestrator is already starting…")`. Else `voice_orchestrator_spec`, then launch with reply and `remember=true` and set pending. No login: `Err("No Claude or Codex login can run the orchestrator…")`. |
| `Create{req, reply}` (other) | cwd = `req.cwd` if it's a directory, else `current_project`, else home. `create_target(profile_id)` (sessions.rs:42-52) accepts harness labels, "claude:*", "codex:*". **Agent:** `LaunchSpec{model=req.model, extra_args=req.args, Permission::Skip, BrowserMode::Off}`, `name=req.title` (sticky), reply on Launched, not remembered. **Shell or unknown:** `req.shell` (custom program; label = file stem; name/renamed from title) else the default shell. `start_now`, stay on the current tab, restore tiles, reply `Ok(sid)`. `req.project_id` is ignored. |

**Invariant:** every `Create` must answer its oneshot, including error paths.

---

## 4. Watchdogs and voice (app side)

### 4.1 Watchdogs (app/watchdogs.rs; watchdog/mod.rs; Services cache)

- **Overlay:** prefix w, `open_watchdog` (watchdogs.rs:39-55).
  - Requires a focused agent pane; otherwise toasts.
  - Builds `WatchdogView::new(pane, sid, title, name, agent label, snapshot, cfg, reviewer_hint)`.
  - `reviewer_hint` (watchdogs.rs:24) uses `pick_reviewer` + `reviewer_launch`. These only feed the hint text now: the orchestrator does the reviews.
  - Keys and clicks produce an `Out` (watchdogs.rs:76-118):
    - `Start(plan)`: `svc.watchdog_start`. If the bridge is off, the error shows inside the view: "watchdogs need the bridge … (alt+g)". Then `watch.insert(Trigger::new(false))`. Busy: toast "watchdog on · it reviews once {name} stops and sits idle {N}". Idle: wake immediately.
    - `Save(plan)`: `watchdog_set_plan` + toast.
    - `Wake`: manual wake ("the user asked for a review now").
    - `Pause(b)`, `Stop`: `watchdog_stop`, remove the trigger, toast.
- **Prompt editor:** `Act::WatchdogPrompt` writes `settings.watchdog.prompt` (None means the default) (watchdogs.rs:120-134).
- **`tick_watchdogs`** after each batch, not in demo (watchdogs.rs:159-190):
  - Per pane with a cached snapshot: `Seen{busy: busy && !blocked, question: prompt_title or "a question on screen" when Blocked, helpers_busy: any dog.helpers sid busy}`.
  - `Trigger::step` (watchdog/mod.rs:74-121):
    - armed after work
    - idle threshold = max(`plan.idle_secs`, 10 s)
    - a question waits max(idle, 120 s)
    - retries after a failed run at 120 s, 300 s, 900 s
    - only when `dog.wakeable()`
  - Fires `wake_watchdog(reason, manual=false)`.
- **`wake_watchdog`** (watchdogs.rs:138-156):
  - Skips the orchestrator pane.
  - Builds `WatchdogWake{reason, reviewer: ReviewerLaunch{kind Claude, program "", label "the orchestrator"}, prompt: custom or DEFAULT_PROMPT, session: WatchedSession{title, agent label, cwd}}`.
  - `watchdog_to_orchestrator` (voice.rs:110-132):
    - `svc.watchdog_brief(sid, wake)` returns `(generation, brief)`. Error: toast (Info when manual, else Error).
    - Writes the brief atomically to `bro_core::orchestrator::dir()/watchdog/{sid stem}-{gen}.md`. On failure: `watchdog_finish(false)`.
    - Queues "[watchdog · {name} (sessionId {sid})] Wake-up: {reason}. Your reviewer brief is in {file}: …" with `Reply::Watchdog(sid, gen)`.
- **`watchdog_event`** (watchdogs.rs:193-214):
  - `Started`: Info "… woke · {reviewer} is reviewing".
  - `Finished ok`: AgentDone with `brief(summary)`. Failed: Error.
  - `StoodDown`: AgentDone.
  - `Step` / `Changed`: ignored (`Changed` is handled in Services and updates `State.watchdogs`, mod.rs:736-742).
- **Elsewhere:**
  - Pane header badge reviewing / done / paused / progress N/M (term.rs:1022-1032).
  - `next_deadline` spins at 125 ms while a watchdog runs or its overlay is open.
  - `close` stops the watchdog.

### 4.2 Voice: push-to-talk and GPT-Live "Hugh"

- **Startup** (`spawn_voice`, workers.rs:340-412):
  - Only from `spawn_all`, only when the bridge is enabled at startup, `voice.enabled`, not demo.
  - Needs `api_key_available(api_key_env)`, else Info toast and the hotkey stays free.
  - `VoiceConfig`:
    - hotkey: default right Win
    - model, language
    - keywords = `voice_keywords()`: Hugh, Mal, Pinky, Toast, Minty, Lilac, Sky, Bitty, bro
    - a transcription prompt
    - `live` unless mode == "transcribe": `LiveConfig{model, voice, instructions: LIVE_INSTRUCTIONS}`
    - log at `~/.bro/voice.log`
- **Callback:**
  - `Listening`: remember `svc.focused_session()` (set every batch by `after_events`, mod.rs:851-852), toast "listening…".
  - `Heard` / `Said`: toasts.
  - `Delegation{id, request}`: `orchestrator_send_voice(request, spoken_to, Some(id))`. An empty request answers "The words didn't come through…".
  - `Transcript(text)`: toast + `orchestrator_send_voice(text, focused, None)`.
  - `Error`: Error toast.
  - `Level`, `Transcribing`, `Cancelled`: ignored. A GUI could show `Level` as a mic meter.
- **After start:** `set_voice(Some)` (dropping it unhooks the global hotkey), then `push_known_folders`, which sets keywords: names plus up to 60 folder names.
- **Bridge orchestrator updates** (workers.rs:437-447):
  - Tagged `Reply` / `Failed`: `voice_delegation_result(tag, brief(text))`.
  - Tagged `Step`: `voice_delegation_progress`.
  - Untagged `Reply`: toast "Hugh: {brief}". Untagged `Failed`: Error toast.
- **Inbox** (`Event::Voice` calls `voice_message`, voice.rs:76): text becomes `spoken_text`: `[voice · looking at "{name}" (sessionId {sid})] words` or `[voice] words`; reply `Voice(tag)`. Then `send_to_orchestrator`.
- **Inbox engine** `pump_voice` (voice.rs:215-295), every batch:
  - **No orchestrator yet:** start one, keeping your focus (`keep_focus`). When it appears: `start_now`, refocus your pane, toast "the orchestrator is up · {key} shows it".
  - **Give up:** after 90 s, or after 1 s if no launch is pending. Calls `inbox_fail`.
  - **Say:** allowed when Idle/Working and age at least 2 s; Blocked never; unknown activity after 30 s; not during the 3 s hold after `/clear`. Calls `submit`. If the reply isn't `None`, pushes `Awaiting{before: last_reply}`.
  - **Clear:** waits until idle at least 1.5 s, then `type_command("/clear")` for Claude or `"/new"` for others. The handoff is re-queued first as "[fresh start · your handoff notes] …". Toast.
  - **Answer:** the newest `facts.reply.text` that differs from `before`, once idle and 2 s have passed. Timeout 240 s (20 min for watchdog replies) answers "Still working on it…" with ok=false.
  - `answer()` (voice.rs:298) routes to `voice_delegation_result`, `orchestrator_answer(panel id)` or `watchdog_finish`.
- **`brief()`** (services/mod.rs:186): first non-offer sentence, markdown stripped, at most 120 chars on a word boundary.

---

## 5. Services

### 5.1 `State` (services/mod.rs:111-133), read via `svc.state()` (a parking_lot RwLock read guard)

- `settings`, `config: Option<Config>`
- `profiles: Avail<Vec<Profile>>`, `providers: Avail<Vec<Provider>>`
- `models: BTreeMap<provider id or "codex"/"claude", Vec<ModelRow>>`
- `past: Avail<Vec<SessionInfo>>`
- `usage: BTreeMap<id, UsageEntry{usage, headroom, error, fetching}>`, `usage_at`, `picks: (large, small)`
- `proxy: ProxyState{status: Avail<ProxyInfo{port, base, token}>, routes, events (cap 400), pool, enabled}`
- `bridge: BridgeState{status: Avail<BridgeStatus>, qr: Option<Vec<String>> (half-block rows), enabled, projects: Vec<ClientProject{key, name, order}>}`
- `justtools`, `justimagine`, `updates`
- `watchdogs: HashMap<sid, WatchdogSnapshot>`
- `issues: Vec<String>`: collected but never shown anywhere (the doc comment claims toasts).
- `Avail::{Loading, Ready, Unavailable(reason)}` with `ready()` and `why()`.

### 5.2 Background workers started by `Services::start` (`workers::spawn_all`, workers.rs:13-30)

| Worker | Trigger / interval | Fills |
|---|---|---|
| Usage loop (workers.rs:135-214) | `recv_timeout(60 s)` or a kick from `refresh_usage`; burst of kicks collapsed. Per login, due per `usage::cache::due` (`REFRESH_EVERY`; `MANUAL_MIN_AGE` when kicked); backoff after errors. At most 4 parallel fetches; each publishes as it finishes. | `usage[*]` (fetching flag first; on error keeps the old reading and sets error), then `picks` via `pick_by_size` and `usage_at` |
| `spawn_data` / `load_data` (workers.rs:54-73) | Once at start; again via `refresh_data` | `config`, `profiles`, `providers` (errors go to `issues`). Then kicks usage, `push_bridge_profiles`, `load_models` (OpenRouter catalogue refreshed on a thread if older than 6 h, then republished), `load_past` (`sessions::list` limit 400) and `push_known_folders` |
| `check_justtools` (justtools.rs:107) | Once at start; settings view | `justtools.{checked, checking, installed, own_skill, paste, latest, offline}`; auto-update when enabled |
| Update checker "bro-update-check" (updates.rs:90-107) | `clean_up_old`, then loop every `self_update::CHECK_EVERY` while `settings.updates.check` | `updates.*` via `check_bro_update(true)`; `justimagine.*` via `check_justimagine(true)`; each announces a new release once |
| Proxy (workers.rs:285-336), if `proxy.enabled` | Start with random token on 127.0.0.1. `on_event` pushes `proxy.events`; `recent(400)`. Then `pool_status` of authenticated `ClaudeAccount` dirs every 5 s until the proxy is gone. | `proxy.status`, `events`, `pool`. Disabled: Unavailable("turned off in settings"). Failure: Unavailable + Error toast unless it's a todo panic. |
| Bridge (workers.rs:415-483), if `bridge.enabled` | Start with data_root `~/.bro/bridge`. Wire `on_notification`, `on_orchestrator_update`, `route_orchestrator(true)`, `on_watchdog_update`. `pairing_qr`, `set_bridge`, push profiles and folders, send `Event::BridgeStarted`. Then every 2 s: `status()` + `client_projects()`; stops when not running or disabled. | `bridge.qr`, `bridge.status`, `bridge.projects` |
| Voice (workers.rs:340), if bridge enabled at start | See 4.2 | (voice handle) |
| Demo ticker (demo.rs:229), demo only | 2.5–6.1 s | `proxy.events` |

**On-demand threads:**

- `launch` (one per request)
- `refresh_sessions`
- `watch_login`: polls `profiles::get` every 2 s for up to 20 min; announces the sign-in, reloads profiles
- `login_closed`, `create_profile`, `remove_profile` (these two return an mpsc Receiver)
- `remove_route`, `save_settings`
- `set_bridge_enabled`: spawn or shut down the bridge
- `check_bro_update`, `install_bro_update` (steps into `updates.job`)
- `check_justimagine` / `refresh_justimagine`
- `install_justtools`, `set_justtools_skill_on`, `set_smart_paste`, `set_justwatch_skill_on`

**App-side background threads** (not in Services; the GUI must also own these):

- TUI input pump "bro-input"
- `instance::serve` listener
- devprobe "bro-devprobe": every 2 s, 150 ms connect, forgets ports after 60 s, sends `Event::Tick` on change
- Per pane: PTY reader, "bro-pty-input" writer, "bro-insights", file-cache resolver
- "bro-recap"
- theme watcher (notify)
- "bro-perf-log"
- "bro-fonts"
- `probe_local_offset`
- one-shot threads for editor, OpenLink, clipboard, folder create, staged cleanup, and recents/projects/archive saves

### 5.3 Every Services method, one line each

**Lifecycle**

- `start(settings, tx)` (pub): real services; `spawn_all`.
- `demo(settings, tx)` (pub): fake State + ticker.
- `offline(settings, tx)` (pub): fake State, no threads.
- `is_demo()` (pub).
- `state()` (pub): read guard. Keep it short.
- `update(f)` (crate): write + `wake`.
- `wake()` (pub): sends `Event::Services`.
- `send(ev)` (crate).
- `settings()` (pub): clone.
- `save_settings(s)` (pub): publish + save on a thread (no-op in demo; error toast).
- `shutdown()` (pub): proxy + bridge shutdown.
- `set_usage_kick` (crate).

**Data**

- `project_for(cwd)` (pub): cached; `bro_core::projects::project_for`, falling back to a local `.git` walk.
- `refresh_data()` (pub).
- `refresh_sessions()` (pub).
- `refresh_usage()` (pub).

**Launch / profiles**

- `launch(req)` (pub).
- `login_command(p)` (pub).
- `watch_login(id)` (pub).
- `login_closed(id)` (pub).
- `create_profile(kind, name)` (pub).
- `remove_profile(id)` (pub).

**Proxy**

- `set_proxy` (crate).
- `with_proxy(what, f)` (crate): guarded.
- `remove_route(id)` (pub).
- `set_trim(on)` (pub): saves and applies live.

**Bridge**

- `set_bridge` (crate): resets the fuse.
- `bridge_running()` (pub).
- `bridge_local_url()` (pub).
- `bridge_register`, `bridge_output`, `bridge_title`, `bridge_remote_viewers`, `bridge_resize`, `bridge_exit`, `bridge_unregister`, `bridge_set_insights`, `bridge_set_agent_files` (all pub).
- `remote_min()` (pub): `[bridge] remote_min_rows/cols/grace_secs`.
- `set_bridge_enabled(on)` (pub).
- `push_bridge_profiles()` (crate).
- `push_known_folders()` (crate): blocking file reads.

**Orchestrator / voice**

- `set_focused_session` (crate), `focused_session` (crate).
- `orchestrator_send_voice` (crate): sends `Event::Voice`.
- `orchestrator_answer`, `schedule_clear_session` (crate).
- `voice_delegation_result`, `voice_delegation_progress` (crate).
- `set_voice` (crate).

**Watchdogs**

- `watchdog(sid)` (pub): from the cache.
- `watchdog_start(sid, plan)` (pub): Err when the bridge is off.
- `watchdog_set_plan`, `watchdog_stop`, `watchdog_pause` (pub).
- `watchdog_brief` (pub), `watchdog_finish` (pub).
- `on_watchdog_update` (crate).

**Updates and tools**

- `check_bro_update(quiet)`, `install_bro_update()` (pub).
- `check_justtools()`, `install_justtools()`, `set_justtools_skill_on(on)` (pub).
- `check_justimagine(quiet)`, `refresh_justimagine()` (pub).
- `set_smart_paste(on)` (pub).
- `set_justwatch_skill_on(on)` (pub).

**Free functions**

- `fallback_settings`, `load_settings`, `local_project_for`, `brief`, `voice_keywords`, `LIVE_INSTRUCTIONS`.
- `launch::{label_for, short_model, to_proxy, map_upstream, fallback_command}`.
- `guard::{guard, guard_res, is_unimplemented, Fuse, install_panic_hook, restore_terminal, TUI_ACTIVE}`.
- `demo::*`, `justimagine::commits`.
- `UpdateState::available`, `JustToolsState::update`, `JustImagineState::update`.

**Hot path:** `with_bridge` uses `guard::Fuse`. The first panic disables bridge calls until `set_bridge` resets it, so one bad dependency costs one caught panic, not one per PTY chunk.

### 5.4 Reuse verdict for a GUI binary

**Reusable as-is** (pure logic, background threads, publish to State):

- `services/{mod, workers, launch, updates, justtools, justimagine, justpaste, justwatch, demo}`
- `services/guard::{guard, guard_res, Fuse}`
- `orchestrator_state`, `recents`, `projects`, `archive`, `devprobe`, `instance` (except the HWND, below), `perf`
- `watchdog/mod.rs` (`Trigger`, `pick_reviewer`, `reviewer_launch`, `human`, `step_choice`)
- `panes/{insights, auto_resume, remote_min, writer}`
- `sidebar.rs` (pure row model)
- `alerts::Toasts`
- Term's pure functions: `permission_prompt`, `prompt_option`, `prompt_question`, `clean_title`, `looks_like_exe_path`, `resolve`, `CLAUDE_SESSION_ENV`, `orchestrator_project`
- All CLI command modules

**Couplings to plan for:**

1. **Event type.** Services, `Waker`, devprobe and instance all send `crate::pane::Event` on an `mpsc::Sender`. `Event::Input` carries crossterm events, and `Launched` / `LaunchRequest` carry `Place` and `PaneId`. A winit loop needs a forwarding thread or a Sender wrapper that pokes an `EventLoopProxy`, plus its own input variant. Keep the pending-flag coalescing.
2. **No lib target.** bro-tui is binary-only: main.rs declares every `mod`, and `bin/bro-app.rs` is standalone. A second binary in the same crate needs a `src/lib.rs` (or `#[path]` modules) to share code.
3. **QR as text.** `BridgeState.qr` is TUI half-block text. The GUI should render from `Bridge::pairing_url()`, which Services doesn't expose yet.
4. **Panic hook.** It is crossterm-aware. With `TUI_ACTIVE=false` it still logs to `v2-panic.log`, but a windows-subsystem GUI has no stderr, so use logging only.
5. **Window handle.** `instance::window::terminal_window()` returns the console HWND. The GUI must reply with its own winit HWND.
6. **`App` methods must be extracted.** These are pure in substance but live on `App` and are tied to layout (tabs/stack/tile_all) and the vt100 `Term`. They should move into a UI-agnostic "workspace" model:
   - `sessions.rs`: `launched`, `bridge_command`, resume queue, `track_agents`, `attention_target`, `resume_targets`/`resume_default`, `create_target`, `transcript_store`
   - `insights.rs`: everything except UI. `Track`, `View`, `RecapJob`, `run_recap`, `recap_due`, `note_line` are already free functions or types.
   - `voice.rs`: the whole inbox
   - `watchdogs.rs`: tick/wake/event (not the overlay)
   - `files.rs`: `agent_opened_file` decision, `open_in_editor`
7. **`Term` must be split** into:
   - a session core: spawn and env, job object, reader tee to the bridge, writer, exit/kill, `bridge_meta`, insights, auto-resume, auto-approve, scan over screen rows, remote-min sizing, `submit`/`type_command`, `early_failure`, `screen_urls`
   - a renderer: vt100+ratatui today, alacritty_terminal+wgpu for the GUI

**TUI-only (replace):** `paste_burst`, `console_nav`, `fonts` OSC 50 mechanism (the font list and data are reusable), `restore_terminal`, `enable_vt_output`, `bin/bro-app.rs`.

---

## 6. Process and entry

### 6.1 `main` (main.rs:57-189)

- `--app-window` is stripped and sets `fonts::APP_WINDOW`.
- `env_cmd::take_flags`: `--env <file|url>` (repeatable), `--env=`, `--save`. Only before the command word. Loaded into the process environment before any thread starts. `--save` writes the user environment and restarts the JustImagine service. `--env x --save` on its own exits 0. Any `--env` forces `--new`.
- Exit codes: parse error is 2.

**CLI commands** (cli.rs):

- `bro [folder] [--new|-n] [--demo|demo]`
- `--help|-h|help`, `--version|-V|version`
- `proxy <upstream> [-m|--model] [--small-model] [--port]`: headless, prints harness env lines, Ctrl-C via tokio
- `ctl [<tool> key=value… | '<json>']`: JSON-RPC to `<bridge>/mcp`, 90 s timeout
- `open <path>[:line[:col]] [--editor|-e] [--session id]`: POST `/api/sessions/{id}/open|open-in-editor`; uses `BRO_SESSION_ID` and `BRO_BRIDGE_URL` or the discovery file
- `install [tools…|recommended|agents|all] [--force] [--wait]`
- `update [--check]`
- `env encrypt|decrypt|list|export`, `--export-env [file] [--all] [--plain] [--force]`
- `imagine`
- `gemini login [--wait]|status|logout`

The GUI depends on these through:

- `BRO_BIN` (= `current_exe`) in every pane. Agents run `bro open` / `bro ctl` through it.
- `bro_term` / `install_term` / `sign_in_gemini`, which run `current_exe` with CLI args in a tab.

If the GUI is a separate windows-subsystem exe, both must point at the console `bro.exe`, not the GUI exe.

### 6.2 `run_tui` (main.rs:191-258)

1. `enable_vt_output` (Windows: VT processing, `DISABLE_NEWLINE_AUTO_RETURN`; sets `TERM=xterm-256color` if unset).
2. Canonicalize the dir and `strip_verbatim`.
3. Hand off to a running bro unless demo or new.
4. Panic hook.
5. `probe_local_offset`: powershell with `CREATE_NO_WINDOW`, off-thread.
6. Load settings (demo uses fallback).
7. Drain keystrokes typed before start.
8. `Services::start` or `demo`.
9. Raw mode, alternate screen, mouse capture, bracketed paste, focus change. Kitty keyboard flags on non-Windows only.
10. `TUI_ACTIVE=true`.
11. Input thread (paste_burst + console_nav, `perf::input_read`).
12. `instance::serve` (not demo).
13. `App::new(Opts{demo, fixed_demo:false, load_recents:!demo})`.
14. `start_in(dir)` + `restore_orchestrator` (not demo).
15. Settings problem toast.
16. `app.run`, then `restore_terminal`.

### 6.3 Single-instance (instance.rs)

- `~/.bro/v2-instance.json` holds `{pid, port, token}`. Loopback TCP; line protocol `token\topen\t<dir>`.
- Replies: `ok <hwnd>`, `denied`, or `closing`.
- Client: 400 ms connect, 2 s read. Ignores its own pid.
- On `ok` it calls `bring_to_front(hwnd)`: `IsWindow`, restore if minimised, an Alt key tap (foreground-lock workaround), `SetForegroundWindow`.
- Server sends `Event::OpenProject(dir)`. The app rings BEL on stdout (tab activity), then `add_project` and `leave_sidebar` (mod.rs:1066-1076). The GUI should flash or request attention instead.
- `Guard` drop removes the file only if the token is still ours.
- `--new`, demo and `--env` skip the hand-off. bro-app always adds `--new`.

### 6.4 `bro-app.exe` (bin/bro-app.rs)

- Windows-subsystem launcher; no console flash.
- **Args:** always inserts `--new` (no duplicate).
- **Which bro:** `find_bro` picks the newest-mtime `bro.exe` among the sibling, `$CARGO_HOME/bin` and PATH. Ties keep the first.
- **Icon:** written to `~/.bro/bro.ico` (or `BRO_DIR`), refreshed when it differs.
- **Terminals tried in order:**
  1. `BRO_TERMINAL`
  2. Fork builds: `WindowsApps\wtdn.exe` (sets `TERMINAL_WEB_DATA_ROOT=%LOCALAPPDATA%\TerminalWebNext` if unset), `wtd.exe`, `Programs\WindowsTerminalDevShim\wt.exe`.
     - Arguments: `-w new --app --title bro --app-icon <ico> --app-relaunch "<bro-app> --new <cwd>" -d <cwd> -- <bro> --app-window <args>`.
     - `;` escaped as `\;`; Windows argument quoting rules applied.
  3. Plain `wt.exe -w new --title bro -d <cwd> -- <bro> <args>`.
  4. `conhost.exe <bro> <args>`.
- **Success check:** a launcher counts as failed if it exits non-zero within 4 s. Exit 1 if all fail.
- **Environment:** `NO_COLOR` removed.
- **Icon embedding:** `build.rs` embeds `assets/bro.ico` and version info via winresource.
- **GUI equivalent:** the GUI replaces this. It needs its own taskbar relaunch/pin identity (AppUserModelID + relaunch command), icon, and `--new` semantics.

### 6.5 Windows console specifics (TUI only, need GUI equivalents)

- **Paste:** `paste_burst` folds key bursts under 8 ms apart, at least 3 keys, into one `Paste`, because the Windows console has no bracketed paste. In the GUI, winit/IME paste is native.
- **`console_nav`:** peeks the console input queue for:
  - `VK_BROWSER_BACK/FORWARD` and mouse buttons 4/5 into `Event::Navigate` (history back/forward)
  - a bare Alt down/up into `Event::Alt` (key hints), excluding AltGr and alt-codes
  - GUI equivalents: winit `MouseButton::Back/Forward` and `ModifiersChanged`
- **Focus:** FocusGained/Lost sets `term_focused`. Critical for done/recap logic; use winit `Focused`.
- **Fonts:** switched via OSC 50 only in the fork's app window (`--app-window` or `BRO_TERMINAL_FONTS=1`), unpacked to `~/.bro/fonts`, restored on exit. The GUI loads the bundled `FONTS` directly.

### 6.6 Panic guard (guard.rs)

- `guard`/`guard_res` turn panics into `Err("what: msg")`; `todo!` maps to "not implemented yet". A thread-local counter marks guarded calls.
- The hook appends every panic to `~/.bro/v2-panic.log` as `{secs} [guarded|panic] msg at file:line`.
  - Guarded panic: silent.
  - Background thread while the TUI is active: logged only.
  - Main thread: `restore_terminal`, then the default hook.
- Proxy/bridge start failures that come from `todo!` panics don't toast (`is_unimplemented`).

### 6.7 Perf log (perf.rs)

- Each loop pass is timed: events (count, worst kind and pane), housekeeping, render, slowest pane render, terminal write, and input wait. Input wait is measured from the input thread's `input_read()` to when the UI takes it.
- A pass over 50 ms, or input waiting over 50 ms, writes one line to `~/.bro/v2-perf.log` on the "bro-perf-log" thread. The log is cut to its newest half past 512 KB.
- `kind()` names every `Event` variant; the GUI needs to extend it for its own input events.

### 6.8 Event loop contract (mod.rs:792-936)

- **Pass:** draw, then `recv_timeout(next_deadline)`, then handle, then drain more events for up to 12 ms, then `after_events`.
- **`after_events` order** (mod.rs:839-859):
  1. screensaver
  2. priority reset
  3. `follow_tab`
  4. locks
  5. `follow_focus`
  6. `record_view`
  7. `set_focused_session`
  8. `reap`
  9. `track_agents`
  10. `tick_watchdogs`
  11. `update_insights`
  12. `pump_voice`
  13. toast expiry
- **Deadlines:**
  - 30 s default.
  - 125 ms while anything is Working, a watchdog runs, or the watchdog overlay is open.
  - 110 ms for rain; toast expiry; sidebar scrollbar; 16 ms while a card flips; alt hints; screensaver.
  - Terminal `tick_every` 400 ms, including hidden ones.
  - Minimum 4 ms.
- **`Event::Tick`:** `pump_resumes`, then poll due panes.
- **Event handlers:**
  - `Event::Wake`: acknowledge, then `poll` that pane.
  - `Event::Services`: offer setup once (first profiles load, nothing signed in, no tabs), refresh the launcher data and continue list if open.
  - `Event::BridgeStarted`: reregister every Term.

---

## 7. What a GUI port must not miss

1. **Window focus feeds attention.** `seen = visible && window focused` drives done marking, needs-you toasts and recap timing.
2. **Keep the raw PTY byte stream.** The bridge mirror needs raw bytes in order (`bridge_output`). alacritty_terminal's own `EventLoop` hides raw bytes: keep your own reader (portable-pty + job object) and feed both alacritty's parser and `bridge_output`.
3. **Terminal mode signals.** The DSR reply (`\x1b[6n`) is required under ConPTY (alacritty answers it via `PtyWrite`). The title comes from alacritty `Event::Title`. Bracketed paste, app cursor and mouse modes come from `TermMode`. OSC 8 links: the vendored vt100 keeps link targets per cell; alacritty supports OSC 8.
4. **Activity scan needs plain text rows.** It runs on the bottom 14 non-empty rows. Provide a "rows as strings" accessor over alacritty's grid for `scan`, `permission_prompt`, `prompt_option_at`, auto-resume `screen_stall`, `early_failure` and `screen_urls`.
5. **Spawn semantics:** lazy spawn at the real cell size; `start_now` at an estimated size for remote creates and queued resumes; the env vars from 1.5; `CLAUDE_SESSION_ENV` stripped; Windows job object; `.cmd` shims through cmd.exe.
6. **Remote minimum size.** While phone/web viewers watch, grow the pty to at least `remote_min` with a grace period. Show the bottom-left of the bigger screen (`view_top`). Ignore bridge `Resize` for visible panes.
7. **Every bridge `Create` reply must be answered.** Remote launches open in the background without stealing focus or the grid (`restore_background_tiles`).
8. **Orchestrator rules:**
   - only one; 90 s pending window
   - restored at startup
   - persisted state cleared on user close or launch failure, kept on quit (`quitting` flag)
   - pinned, own project key, bridge `kind`
   - launch auto-enables the bridge and waits up to 20 s
9. **Close cleanup order:** watchdog stop, kill, bridge exit/unregister, `login_closed` for login tabs, shared-route check, staged-file cleanup thread, orchestrator-closed handling. Plus `reap` with the early-failure message.
10. **Resume queue timings** (1.5 s gap, 20 s stuck), placeholder replaced under the same PaneId, loader text.
11. **The inbox engine** with all its timings (section 4.2) and the exact message prefixes, which the orchestrator prompt relies on.
12. **Instance hand-off must reply with the GUI's HWND.** `bring_to_front` uses the Alt-tap trick.
13. **`BRO_BIN` and in-app CLI tabs** must target a console-capable `bro.exe`.
14. **Don't hold `svc.state()` while calling anything that writes State.** The parking_lot RwLock isn't reentrant, so this would deadlock.
15. **Demo mode:** no persistence, no instance, fallback settings, fixed transcripts for screenshots.
16. **Quirks to decide on:**
    - `issues` is never surfaced.
    - Voice only starts if the bridge was on at startup; toggling the bridge doesn't start or stop voice.
    - A remote `Rename` reaches the bridge title only through the next poll.
    - `CreateRequest.project_id` is ignored.
    - The bridge's `cwd()` / `set_project()` are never called.
    - Bridge notifications and the app's own done/needs-you toasts can both fire for the same event (duplicates only collapse when the text is identical).
