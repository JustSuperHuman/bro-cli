# Verdicts B: AGT, SES, WD, SYS

Reviewer B. Sections AGT (agent tracking), SES (sessions and services), WD (watchdogs and voice), SYS (process and entry).

How the window reaches the shared logic (checked in code): every `pane::Event` goes through the `bro-gui-events` forwarder into `Gui::app_event` → `App::handle` (only file-preview media and links to programs are intercepted first); `Gui::after_events` runs `App::after_events` on every loop turn; `Gui::tick` posts `Event::Tick` on the workspace's own deadline; keys go `Gui::key` → `App::handle`; launcher, palette, resume, confirm and watchdog clicks call the same `App` methods the keys do (`overlays/mod.rs apply_launcher`, `watchdog_outcome`, `run_palette`, `resume_chosen`, `confirm_answer`). "Shared" below means the terminal UI's own code, reached that way.

Demo checks (`--demo`, scripted): baseline frame; a blocked session's option row tinted under the pointer and answered by a click (the shell got `1⏎`); sidebar Claude icon → "launching claude · local…" toast and a demo session; × on a working session asks to confirm, × on a plain shell closes at once.

Entry points (`main.rs`, `cli.rs`, `bin/bro-app.rs`, `bin/bro-gui.rs`) are being reworked by the integrator while this was written (`bro` → desktop app, `bro-cli` → terminal UI, `bro-app` → console-free launcher, `bro-gui.rs` removed). SYS verdicts describe the code as it stood when read.

## AGT

AGT-001 | DONE | shared `Term::activity` (queued → Working, `demo_activity`); the window reads it via `Pane::activity` (sidebar.rs `draw_row`, chrome.rs `status_pill`)
AGT-002 | DONE | shared `Term::poll` → `scan` (200 ms gate in the 400 ms poll), driven by `Gui::tick` (Event::Tick) and Event::Wake through `App::handle`
AGT-003 | DONE | shared `Term::scan` markers + 1500 ms output window (demo: a fresh quick launch shows the spinner)
AGT-004 | DONE | shared `Term::scan` blocked markers
AGT-005 | DONE | shared `Term::scan` (`agent = true` on any marker)
AGT-006 | DONE | shared `prompt_question` in `Term::scan`
AGT-007 | DONE | shared `Term::activity` (`asking_question` forces Blocked)
AGT-008 | DONE | shared `Term::busy`; the window uses it for "working" rain (chrome.rs `draw_card`) and watchdogs
AGT-009 | DELTA | DD-021/DD-024: chrome.rs `status_dot`/`status_pill` draw a spinner (accent), a pulsing alert (danger), a check (good); idle and plain shells show nothing; the word is the header tooltip
AGT-010 | DONE | `App::track_agents` in `App::after_events`; seen = `visible()` && `term_focused`, fed by `WindowEvent::Focused` → FocusGained/FocusLost (gui/mod.rs `window_event`)
AGT-011 | DONE | shared `track_agents`; AgentDone toast drawn by gui/toasts.rs (check icon, good)
AGT-012 | DONE | shared `track_agents`; NeedsYou toast drawn by gui/toasts.rs (bell icon, danger)
AGT-013 | DONE | shared `track_agents` (collapse + `ui::fit` 72); toasts.rs collapses whitespace again and wraps to 3 lines
AGT-014 | DONE | shared `track_agents` / `focus_pane`; a click's focus (gui/mouse.rs `focus`) leaves `done` to `track_agents` once the pane is visible, as the terminal UI's mouse does
AGT-015 | DONE | toasts only (gui/toasts.rs). DD-017's optional taskbar flash isn't done for done/needs-you: only a program's bell and a hand-off call `request_user_attention` (gui/mod.rs `after_events`, `app_event`)
AGT-016 | DONE | shared `App::attention_target` via the key (`Gui::key` → `App::handle` → `run_act`)
AGT-017 | DONE | sidebar.rs `draw_row` project row: pulsing alert icon when `attention`; `draw_strip`: blocked/done counts with alert/check icons (DD-021 icons instead of ●)
AGT-018 | DONE | shared panes/insights.rs, started by `Term::start` (not demo) when chrome.rs `draw_terminal` calls `ensure_started`, or by `start_now`
AGT-019 | DONE | shared `Term::bind_spec`
AGT-020 | DONE | shared Binder / Tail / `AgentFacts::ingest` (panes/insights.rs)
AGT-021 | DONE | shared; the pane's Waker posts Event::Wake, forwarded into `App::handle`
AGT-022 | DONE | `App::update_insights` in `App::after_events`
AGT-023 | DONE | shared `insights::View::of` / `reason_for` / `recap::choose`
AGT-024 | DONE | notes from `update_insights`; sidebar.rs `draw_row` second line: pause icon + reason (danger, muted) or ↳ icon + recap (muted), 48 px row
AGT-025 | DONE | shared `insights::Track`
AGT-026 | DONE | shared `update_insights` → `bridge_set_insights` on fingerprint change
AGT-027 | DONE | shared `update_insights` → `bridge_set_agent_files`
AGT-028 | DONE | shared `recap_due` / `worth_generating`; "unseen" uses `term_focused` from the window's focus events
AGT-029 | DONE | shared `insights::run_recap`
AGT-030 | DONE | shared `run_recap` (bro-recap thread, CREATE_NO_WINDOW, ProcessTree, 250 ms poll, `recap::TIMEOUT`, closed signal)
AGT-031 | DONE | shared `start_recap` (`recap::store`, `set_generated`, `recap_tried`)
AGT-032 | DONE | shared `Term::auto_approve` from `Term::poll`
AGT-033 | DONE | shared `Term::auto_approve` timing
AGT-034 | DONE | shared `permission_prompt`
AGT-035 | DONE | shared; `cx.toast` → `App::apply` → toast, drawn by gui/toasts.rs
AGT-036 | DONE | shared panes/auto_resume.rs + `[auto_resume]` settings
AGT-037 | DONE | shared `auto_resume::classify`
AGT-038 | DONE | shared `auto_resume` reset parsing
AGT-039 | DONE | shared `Term::stall` (transcript `api_error`, born filter)
AGT-040 | DONE | shared `screen_stall` over the vt100 model, which `Term::start_reader` still feeds beside the alacritty mirror
AGT-041 | DONE | shared; window input reaches `Term::send` (keys, paste, IME `type_text`, option clicks), which counts as typing
AGT-042 | DONE | shared `auto_resume` backoff ladders
AGT-043 | DONE | shared limit + `after_limit_reset`
AGT-044 | DONE | shared `Term::auto_resume` (`enter_at` 250 ms)
AGT-045 | DONE | shared; toasts drawn by gui/toasts.rs
AGT-046 | DONE | chrome.rs `draw_header` badge `term.resume_note()` in `t.inline` (DD-024 quiet text). The terminal UI shows it in the header only (`AutoResume::note` is "for the pane header"; the sidebar note is insights only), so the sidebar isn't missing anything
AGT-047 | DONE | gui/mouse.rs `body_press`: left, no link, not flipped, no alt, inside the drawn grid, (!wants_mouse ‖ shift) → `prompt_option_at(row)` → `send("{n}\r")`. Demo: clicking "1. Yes" in the blocked pane sent `1⏎`
AGT-048 | DONE | `Term::prompt_option_at` (Blocked, `scroll == 0`, + `view_top`) with the drawn row; the window's scrollback is the same `Term::scroll`
AGT-049 | PARTIAL | chrome.rs `draw_terminal` tints the hovered option row (accent 14%) and mouse.rs `cursor_at` shows a pointer (demo-checked), but the row isn't bold as in the terminal UI. Fix: term/draw.rs `View` takes the hovered option row and draws it bold (chrome.rs passes it)

## SES

SES-001 | DONE | shared `Meta` (panes/term.rs), built in `App::launched` / `open_shell` / bridge Create
SES-002 | DONE | shared `sessions::transcript_store`
SES-003 | DONE | shared services/launch.rs
SES-004 | DONE | shared `App::launch_spec`, reached from overlays/mod.rs `apply_launcher`, gui/mouse.rs `side_click`, overlays/palette.rs `click` → `run_palette`, and keys
SES-005 | DONE | shared `launch_spec`; demo: the sidebar Claude icon toasted "launching claude · local…" and opened the session
SES-006 | DONE | gui/mouse.rs `side_click` → `quick_launch` / `launch_usage`; alt+q via `App::handle`; palette recents via `run_palette`
SES-007 | DONE | shared `open_shell`, `install_agent`, `sign_in_gemini`, login tabs, editor panes, quick install (sidebar terminal icon, `apply_launcher` Install/SignIn, views)
SES-008 | DONE | the window runs inside the console bro.exe (`bro gui`), so `current_exe()` in `quick_install::bro_term` and `BRO_BIN` (`Term::start`) is the console bro. Keep the window inside the console exe in the entry-point rework, or this breaks
SES-009 | DONE | shared `launch::run` thread per launch; demo `shell_command` (demo-checked)
SES-010 | DONE | shared `launch::run` `find_live`
SES-011 | DONE | shared `stage_for_profile` → `cmd.cleanup`
SES-012 | DONE | shared `orchestrator_bridge`
SES-013 | DONE | shared `LaunchCtx` from State
SES-014 | DONE | shared `fallback_command` on a build panic
SES-015 | DONE | shared route upsert / publish
SES-016 | DONE | shared `App::launched` (`into`, `resume_started`, `orchestrator_pending`)
SES-017 | DONE | shared `App::launched` error path; Error toast drawn by gui/toasts.rs
SES-018 | DONE | shared `recents::push` + save (not demo)
SES-019 | DONE | shared `renamed = name && reply`
SES-020 | DONE | shared `orchestrator_project` / `project_for`
SES-021 | DONE | shared duplicate-orchestrator close in `launched`
SES-022 | DONE | shared Place::Tab + `orchestrator_state::save`
SES-023 | DONE | shared `into` path (`start_now` uses `app.body`, which gui/layout.rs keeps in virtual cells)
SES-024 | DONE | shared normal path (`open`, remote `start_now` + `restore_background_tiles`, local `leave_sidebar`)
SES-025 | DONE | shared `resume_request` / `open_resume`; the window's resume picker click → `resume_chosen`
SES-026 | DONE | shared `fork_into`
SES-027 | DONE | shared move-login path
SES-028 | DONE | shared failure toast
SES-029 | DONE | shared `open_switch` toasts
SES-030 | DONE | shared `resume_many` (launcher `Outcome::ResumeMany` via `apply_launcher`)
SES-031 | DONE | shared `pump_resumes` on every Event::Tick (`Gui::tick`); chrome.rs `draw_queued` shows "in line · N ahead" / "up next" / "starting now"
SES-032 | DONE | shared `display_title` / `display_name`; chrome.rs `draw_header` and the sidebar's `LiveInfo.name` use them
SES-033 | DONE | quirk kept: shared `Term::poll` sends the new title on its next poll
SES-034 | DONE | shared `run_act(Orchestrator)`, bridge Create, inbox; `restore_orchestrator` in gui/mod.rs `run` (not demo, not guest)
SES-035 | DONE | shared within one bro (`orchestrator_pane`, 90 s pending, duplicate close). Across processes see SES-037 (the window's guest mode)
SES-036 | DONE | shared `voice_orchestrator_spec` / `roomiest_login`
SES-037 | PARTIAL | save/clear/restore are shared and reached. Gap in the window's guest mode (`--new` beside a running bro, gui/mod.rs `run`): the guest keeps `persist` on, so an orchestrator launched there fails (no bridge in a guest: `orchestrator_bridge` waits 20 s) and `App::launched` then clears `~/.bro/v2-orchestrator.json`, the running bro's restore state; if the bridge is off in settings the guest turns its own on, and a success overwrites the file and runs a second orchestrator. Fix (integrator): in guest mode refuse orchestrator launches (`launch_spec`, `Act::Orchestrator`, `send_to_orchestrator`: toast "another bro is running: it keeps the orchestrator"), or skip `orchestrator_state` save/clear when guest
SES-038 | DONE | sidebar.rs `draw_list` pins the orchestrator row (gold "Orchestrator", numbered from the shared rows); `draw_strip` has the ★ tab; groups, grids, close-all, `current_project`, `follow_focus`, bridge kind and watchdog skip are shared
SES-039 | DONE | shared `App::close` → `orchestrator_closed`
SES-040 | DONE | gui/mod.rs `run` → `app.start_in(dir)` (not demo)
SES-041 | DONE | shared `App::handle` OpenProject / ClipboardFolder → `add_project`; the window also requests attention (DD-009)
SES-042 | DONE | gui/mouse.rs `side_click` CloseProject → `App::close_project`; strip × via `strip_click`
SES-043 | DONE | shared `Services::project_for`
SES-044 | DONE | shared projects.rs
SES-045 | DONE | shared archive.rs
SES-046 | DONE | shared recents.rs
SES-047 | DONE | shared atomic writes; the window's own `~/.bro/v2-gui.json` also uses `util::atomic_write` (gui/prefs.rs)
SES-048 | DONE | shared `App::close`; the window reaches it via header ×, middle-click, sidebar × → `ask_close` → `confirm_answer`
SES-049 | DONE | shared `App::ask_close`; demo: × on the working session asked "close claude · work · opus-5? the agent is still busy — it will be stopped", × on the shell closed at once
SES-050 | DONE | gui/mod.rs `WindowEvent::CloseRequested` → `run_act(Quit)` (confirm when sessions run; `el.exit()` once `quit`)
SES-051 | DONE | gui/mod.rs `run`: `Services::demo` (ticker)
SES-052 | DONE | gui/mod.rs `run`: `fallback_settings`, no hand-off, no instance listener, `load_recents=false` → persist off; the window's prefs and paste scan stay off in demo (`Prefs::load/save(persist)`, `scan_pastes` gated)
SES-053 | DONE | shared `open_demo_sessions`; demo frame shows 5 sessions in 3 projects (+ seeded dotfiles), pwsh split right in tab 1, working/done/needs-you marks
SES-054 | DONE | shared; demo quick launch built a demo shell
SES-055 | DONE | Event::Bridge forwarded unchanged → `App::bridge_command`
SES-056 | DONE | shared Input → `Term::send`
SES-057 | DONE | shared Resize (local view wins while visible)
SES-058 | DONE | shared Kill → `close`
SES-059 | DONE | shared Rename
SES-060 | DONE | shared Focus → `focus_pane`
SES-061 | DONE | shared `agent_opened_file`; pictures and videos then go to the media panel (gui/mod.rs `app_event`, MED)
SES-062 | DONE | shared `open_in_editor` / Event::EditorPane
SES-063 | DONE | shared `orchestrator_command` Message
SES-064 | DONE | shared Scheduled / Cancel / ClearContext
SES-065 | DONE | shared Tile → `show_tab`
SES-066 | DONE | shared Create "orchestrator"
SES-067 | DONE | shared Create cwd + `create_target`
SES-068 | DONE | shared Create agent
SES-069 | DONE | shared Create shell (`start_now` at the virtual-cell body size)
SES-070 | DONE | shared: every Create path answers its oneshot
SES-071 | DONE | quirk kept: `project_id` ignored
SES-072 | DONE | shared `Term::start` → `bridge_register` at the drawn size
SES-073 | DONE | shared output / title / `set_pty_size` → `bridge_resize` (no-op when unchanged, though the window calls `resize` every frame) / exit + unregister in `close`
SES-074 | DONE | shared `push_bridge_profiles` / known folders
SES-075 | DONE | shared bridge calls
SES-076 | DONE | quirk kept
SES-077 | DONE | shared `guard::Fuse` in `with_bridge`
SES-078 | DONE | shared State
SES-079 | DONE | checked the window's code: every `svc.state()` guard is scoped or dropped before anything that writes State (gui/sidebar.rs `draw_footer`, welcome.rs, views/*/gui.rs snapshots)
SES-080 | DONE | quirk kept: issues still aren't shown
SES-081 | DONE | shared usage loop (also runs in a guest)
SES-082 | DONE | shared `spawn_data` (also in a guest)
SES-083 | DONE | shared `check_justtools` + `spawn_update_checks`; a guest runs `check_justtools` only (no update checks, by design)
SES-084 | DONE | shared `spawn_proxy`. Guest: proxy Unavailable("your other bro runs it"), so proxy-routed launches in a guest fail with "this launch goes through the proxy, which isn't running" (guest design, worth knowing)
SES-085 | DONE | shared `spawn_bridge` (not started in a guest)
SES-086 | DONE | shared; Bridge toasts drawn by gui/toasts.rs
SES-087 | DONE | quirk kept
SES-088 | DONE | shared on-demand threads
SES-089 | DONE | instance listener (gui/mod.rs `run`, not demo/guest), devprobe + theme watcher (`App::new`), PTY reader/writer/insights/file cache (`Term::start`; chrome.rs uses `term.file_cache()`), recap, perf log, local-offset probe, one-shots; plus the window's bro-gui-events forwarder
SES-090 | DONE | shared `services::brief`
SES-091 | DONE | shared; Event::Toast → `App::raise` → gui/toasts.rs
SES-092 | DONE | views/bridge/gui.rs draws the QR natively by decoding the half-block rows (`modules`), so `pairing_url` isn't needed; the rest stay unused as in the terminal UI

## WD

WD-001 | DONE | shared `App::watchdog_outcome` (clicks: overlays/mod.rs `click`; keys: `App::handle`); watchdog/view/gui.rs draws `error` over the band
WD-002 | DONE | shared `watchdog_outcome` Start
WD-003 | DONE | shared Save (overlay stays on the status page)
WD-004 | DONE | bro-bridge watchdog
WD-005 | DONE | shared Wake
WD-006 | DONE | shared Pause toasts
WD-007 | DONE | shared Stop
WD-008 | DONE | bro-bridge memory only
WD-009 | DONE | shared `State.watchdogs`; chrome.rs `draw_header` reads `svc.watchdog(sid)` for its badge
WD-010 | DONE | bro-core `WatchdogSettings` defaults
WD-011 | DONE | `App::tick_watchdogs` in `App::after_events` (not demo)
WD-012 | DONE | shared `watchdog::Trigger`
WD-013 | DONE | shared retry ladder
WD-014 | DONE | shared `wake_watchdog`
WD-015 | DONE | shared `watchdog_to_orchestrator`
WD-016 | DONE | shared brief file (`atomic_write`)
WD-017 | DONE | shared exact prefix
WD-018 | DONE | shared `watchdog_event`; toasts drawn by gui/toasts.rs
WD-019 | DONE | `App::close` stops it; `Gui::next_wake` uses `App::next_deadline` (125 ms while one runs)
WD-020 | DONE | shared `spawn_voice` from `Services::start` (not demo; a guest doesn't start voice, by design)
WD-021 | DONE | shared
WD-022 | DONE | shared
WD-023 | DONE | shared
WD-024 | DONE | shared; `set_focused_session` every batch (`App::after_events`); toasts drawn
WD-025 | DONE | shared
WD-026 | DONE | shared
WD-027 | DONE | ignored as in the terminal UI (no mic meter in the window)
WD-028 | DONE | shared
WD-029 | DONE | shared exact prefixes
WD-030 | DONE | quirk kept
WD-031 | DONE | `App::pump_voice` in `App::after_events`
WD-032 | DONE | shared
WD-033 | DONE | shared
WD-034 | DONE | shared
WD-035 | DONE | shared
WD-036 | DONE | shared
WD-037 | DONE | shared

## SYS

SYS-001 | DONE | main.rs `take_flag` + `cli::parse`: `bro gui [folder] [--new|-n] [--demo|demo]`; bin/bro-gui.rs forwards its arguments to `bro gui --gui-child`. Entry points are being reworked (see top)
SYS-002 | DONE | shared main.rs (help, version, exit 2)
SYS-003 | DONE | subcommands dispatch in main.rs before the window; the window is a mode of the console bro.exe
SYS-004 | DONE | main.rs `env_cmd::take_flags` first; `new || env` passed to `gui::launch`; the detached child inherits the loaded environment
SYS-005 | N/A | the window loads fonts itself (DD-011; `fonts::GUI` set in `run`); main.rs still strips `--app-window`
SYS-006 | DONE | CLI in the console bro.exe; the window's panes get `BRO_SESSION_ID`, `BRO_BIN`, `BRO_BRIDGE_URL` from `Term::start`
SYS-007 | DONE | shared open_cmd.rs
SYS-008 | DONE | shared ctl_cmd.rs
SYS-009 | DONE | bro-core skills via `launch::build`
SYS-010 | DONE | gui/mod.rs `run`: canonicalize + `strip_verbatim` (`launch` does the same for the child's folder argument)
SYS-011 | DONE | gui/mod.rs `run`: hand-off unless demo or `--new`. Window-only addition: `--new` beside a running bro opens a guest (`Services::start_guest`, no listener, no orchestrator restore). For the rework: the instance record has no kind, so a desktop launch hands off to a running terminal UI and raises the terminal
SYS-012 | DONE | `run` installs `guard::install_panic_hook` right after the hand-off, before settings and services (same order as `run_tui`)
SYS-013 | DONE | `run` → `util::probe_local_offset`
SYS-014 | DONE | `run`: `fallback_settings` in demo, else `load_settings`; the problem toast after `App::new`
SYS-015 | DONE | `run`: `Services::demo` / `start` (`start_guest` for a second bro), `instance::serve` (not demo/guest), `App::new`, `start_in`, `restore_orchestrator` (not demo/guest)
SYS-016 | DONE | shared instance.rs (`hand_off` / `serve` / `running`)
SYS-017 | DONE | shared client; `run` skips the hand-off for demo and `--new` (main adds `--new` for `--env`)
SYS-018 | DONE | window.rs `remember_handle` stores the winit HWND in `instance::APP_WINDOW` when the window is made; `terminal_window()` replies with it; `bring_to_front` shared. Before the window exists the reply is `ok 0` and the client doesn't raise anything
SYS-019 | DONE | `serve` posts Event::OpenProject (`App::handle` → `add_project`, plus `request_user_attention` in gui/mod.rs `app_event`); the Guard lives in `run` until after `Gui::shutdown`. Risk: `App::handle` still writes BEL to stdout on OpenProject; via `bro gui` stdout is NUL, but the bro-gui.exe/CREATE_NO_WINDOW path inherits whatever handles it gets, so the new launcher should pass null stdio (as `window::detach_from_console` does)
SYS-020 | PARTIAL | bin/bro-gui.rs: windows subsystem, hidden console for bro.exe (CREATE_NO_WINDOW), embedded icon (checked in bro-gui.exe). Missing: AppUserModelID + relaunch command. Nothing calls `SetCurrentProcessExplicitAppUserModelID` or sets the window's `PKEY_AppUserModel_*`, so a pinned window relaunches bare bro.exe and the launcher's pin doesn't group with the window. Fix in app/gui/window.rs `create` (or `run`, before the window): an explicit AUMID, then on the HWND (`SHGetPropertyStoreForWindow`) ID, RelaunchCommand ("<launcher> <folder>", as bro-app's `relaunch_command`), RelaunchDisplayNameResource "bro", RelaunchIconResource "<bro.exe>,0". `--new`: the launcher doesn't add it (a second launch hands off; `--new` means guest), unlike bro-app's always-`--new`; settle it in the rework
SYS-021 | DONE | build.rs (winresource) embeds assets/bro.ico + version info in every bin (checked bro-gui.exe: icon, "bro — agentic terminal workspace"); window.rs sets the window icon from assets/bro.png. `~/.bro/bro.ico` isn't written: only terminals' `--app-icon` read it
SYS-022 | MISSING | bin/bro-gui.rs starts the sibling bro.exe, else bare `bro.exe` from PATH, not bro-app's newest-mtime pick among the sibling, `$CARGO_HOME/bin` and PATH (`find_bro` / `newest_bro`, ties keep the first). The chosen exe becomes `BRO_BIN` and the CLI tabs' program (still a console bro, so SES-008 holds). Not fixed here: bro-gui.rs is being removed; keep `find_bro`/`newest_bro` in the reworked bin/bro-app.rs launcher
SYS-023 | N/A | the window needs no terminal fallbacks; bin/bro-app.rs is being turned into the GUI launcher in the rework, so terminal-UI users lose its terminal hosting unless bro-cli gets an equivalent
SYS-024 | DONE | winit sends keys one by one (no burst folding); ctrl+v / ctrl+shift+v / shift+insert reach `App::key`'s clipboard read (Event::ClipboardText → paste); IME commits go through gui/mouse.rs `type_text`; drops paste the path
SYS-025 | PARTIAL | mouse X1/X2 → Event::Navigate (gui/mouse.rs `pressed`) and bare Alt via `ModifiersChanged` → Event::Alt (AltGr = ctrl+alt excluded) work; the browser Back/Forward keys don't: gui/input.rs `key` returns None for `NamedKey::BrowserBack`/`BrowserForward`. Fix in gui/mod.rs `gui_key`: BrowserBack → `app_event(Event::Navigate(true))`, BrowserForward → `Navigate(false)` (LOOP-037 too)
SYS-026 | N/A | raw mode, alternate screen, mouse capture, bracketed paste, focus events, kitty flags, `enable_vt_output`, the keystroke drain and `restore_terminal` don't apply; `run` calls none of them
SYS-027 | DONE | gui/mod.rs `WindowEvent::Focused` → Event::Input(FocusGained/FocusLost) → `App::handle` (`term_focused`)
SYS-028 | DONE | shared services/guard.rs
SYS-029 | DONE | same hook (`install_panic_hook` in `run`); `TUI_ACTIVE` stays false, so a non-guarded panic also runs the default hook, whose stderr is NUL or the hidden console in the window: log only in effect
SYS-030 | DONE | logged to `v2-panic.log`; the default hook's stderr goes nowhere visible; winit unwinds out of `run` (dropping `Gui` kills the sessions; `Services::shutdown` is skipped)
SYS-031 | DONE | shared workers (`is_unimplemented`)
SYS-032 | PARTIAL | the window times events (`Gui::app_event` → `perf.begin`/`event`), housekeeping (`Gui::after_events`) and the whole frame (`Gui::frame` → `drawn`/`finish`). Missing: input wait (no `perf::input_read`; winit input arrives on the UI thread, so it needs the OS event time), the slowest pane render (chrome.rs `draw_card` never calls `perf.pane_rendered`), and native mouse/wheel/drop handling isn't timed (gui/mouse.rs doesn't go through `perf.event`). Fix: gui/mod.rs `window_event` (time the mouse handlers into the pass) and chrome.rs `draw_card` (`pane_rendered` per card)
SYS-033 | DONE | shared `perf::log` via `Pass::finish`, called every frame
SYS-034 | PARTIAL | `kind()` still names every Event; the window's own input (mouse press/release/move/wheel, drops, IME, `gui_key`) bypasses `perf.event`, so a slow click isn't attributed. Fix: a named timing helper in perf.rs (e.g. `Pass::timed("mouse", f)`) used from gui/mod.rs `window_event`
SYS-035 | PARTIAL | `bro gui` is part of bro.exe, so the window ships in every release and self-update swaps it; `clean_up_old` (bro*.old) runs at start. The launcher isn't shipped or swapped: .github/workflows/release.yml "Package (Windows)" copies only bro.exe and bro-app.exe; packaging/windows/bro.iss `[Files]`, `MoveAside` and the Start-menu shortcut only know bro.exe/bro-app.exe; bro-core `self_update::install` swaps bro + bro-app only. After the rework, ship the GUI launcher (and bro-cli if it's its own exe) in all three
SYS-036 | DONE | `Term::start` sets `BRO_BIN` = `current_exe()` = the console bro.exe running the window
SYS-037 | DONE | `App::new` starts `devprobe::Probe`; chrome.rs `draw_back` reads `app.probe.up(url)`
SYS-038 | DONE | gui/mod.rs `resumed` loads `settings().font` (`fonts::get`) into the renderer and `Gui::sync_font` keeps it in step
