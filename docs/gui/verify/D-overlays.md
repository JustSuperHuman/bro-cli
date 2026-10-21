# Verdicts D: OVL (overlays)

Reviewer D. Section OVL: overlay mechanics, the launcher (New tab, run-on list and models, ★ orchestrator, install and Gemini sign-in, live data), the folder picker, the Continue tab, the resume picker, the palette, the keys overlay (F1), the confirmation, the file preview, the editor hand-off, agent files, the watchdog card, the watchdog prompt editor and the TextArea.

How the window reaches the shared logic (checked in code): keys go `Gui::key` → `App::handle` → `App::key` → `overlay_key`, so every overlay's own state machine (`Launcher::key`, `Palette::key`, `Help::key`, `resume_key`, `confirm_key`, `FileView::key`, `AgentFilesView::key`, `watchdog_key`, `watchdog_prompt_key`, the TextArea) gets the same events as in the terminal; pastes (ctrl+v, IME text, dropped files) go through `App::paste`; clicks hit the overlay's own targets (`Action::Overlay(n)`) and `app/gui/overlays/mod.rs` hands the outcome to the same `App` methods the keys use (`apply_launcher`, `file_view_outcome`, `agent_files_outcome`, `watchdog_outcome`, `watchdog_prompt_outcome`, `run_palette`, `resume_chosen`, `confirm_answer`). "Shared" below means the terminal UI's own code, reached that way.

Demo checks (`bro gui --gui-child --demo`, scripted; shots kept in `F:/tmp/verify-d/`, with the launcher's in `a/`, the folder picker's and Continue's in `b/`, the file preview's and agent files' in `c/`, the watchdog's in `d/`): the palette (all, theme picker, caveman, presets, nothing matches, row click, wheel, hover preview, outside click, running a theme), the keys overlay top and bottom, every confirmation, the resume picker, the launcher on every agent tab with both layouts, the folder picker and the Continue tab in every state, the file preview (opened from a printed path, sideways, too large, mention), agent files (empty states; the populated list through the GPU `shots` test), the watchdog form, status and edit pages, and the prompt editor. States the demo can't reach (a running reviewer, an orchestrator already running, an agent not installed, Gemini signed out, recents in the palette) are checked in code only and say so.

Fixed in this pass: help sheets rewritten for the window (`app/gui/overlays/help.rs`), the confirmation's "No" (`confirm.rs`), toasts on top of the launcher and shift+wheel sideways in the file preview (`app/gui/overlays/mod.rs`), the folder picker's "current" tag (`folder/gui.rs`), the file preview following theme changes (`file_view/gui.rs`), the watchdog form's orchestrator copy and its scroll to the buttons (`watchdog/view/gui.rs`). `cargo test -p bro-tui`: 398 passed, 1 failed (`app::tests::launcher_open`, known and unrelated), 14 ignored; bro-app 11 passed.

Totals: 208 DONE, 8 FIXED, 8 PARTIAL, 17 DELTA, 1 N/A (242 items).

## OVL

### Overlay mechanics

OVL-001 | DONE | shared `Overlay` enum (app/overlays.rs), one slot; app/gui/overlays/mod.rs `draw` dispatches all nine: palette, help, resume and confirm natively there, the other five through their own gui.rs (a module returning false falls back to the terminal UI's drawing)
OVL-002 | DONE | overlays/mod.rs `draw`: launcher::gui::draw gets `body` (fills the content area right of the sidebar, below the strip: launcher.png); every other overlay gets the whole `window` and centres itself across it (the palette sits 18% down like a command palette, resume and confirm 30% down, help centred)
OVL-003 | DELTA | the terminal UI's per-overlay rules hold (overlays/mod.rs `click` takes left clicks only): Confirm answers only on Yes / No (scrim Action::None); Resume selects, runs the selected login, ignores the wheel and outside clicks (resume.rs); the launcher takes clicks and the wheel (launcher/gui.rs); File and AgentFiles have their own targets and wheel (file_view/gui.rs, agent_files/gui.rs). Window additions on purpose (DD-013 spirit): buttons on Resume, Confirm, File and AgentFiles; the wheel scrolls the watchdog card's body (OVL-226) and the prompt editor's text; the prompt editor takes button and caret clicks (TUI: no mouse, OVL-234); sideways wheel still missing (OVL-177)
OVL-004 | DELTA | DD-005: app/gui/mouse.rs `pressed` closes Palette/Help only on their scrim (`Action::CloseOverlay`, any button; the palette restores `theme_before`); inside, palette rows select and the selected one runs (palette.rs `click`), the wheel moves the palette's selection and scrolls the keys sheet (palette.rs / help.rs `wheel`); checked in pal-click1, pal-wheel, pal-outside shots
OVL-005 | DONE | shared `App::paste` (app/mod.rs): ctrl+v / shift+insert go App::key → Event::ClipboardText → paste; dropped files (gui/mouse.rs `dropped`) and IME text (`type_text`) call App::paste too while an overlay is open, so Launcher/Watchdog/WatchdogPrompt/Palette take it, the rest swallow it, nothing reaches a terminal
OVL-006 | DONE | gui/mouse.rs `pressed`: MouseButton::Back/Forward → Event::Navigate → shared App::navigate, which closes any non-launcher overlay first

### Launcher: opening and defaults

OVL-007 | DONE | every opener reaches the shared App::open_launcher / open_continue / open_orchestrator / quick_launch / quick_agent / launch_usage: keys through App::handle; sidebar + New, Continue, quick icons, usage rows and strip + through app/gui/mouse.rs side_click; welcome buttons through app/gui/welcome.rs; Setup "Launch an agent", add_project and history are shared
OVL-008 | PARTIAL | Esc chain (Launcher::key), the × (launcher/gui.rs CLOSE), outcomes (app/gui/overlays/mod.rs apply_launcher), sidebar clicks (app/gui/mouse.rs launcher_side_click) and history (App::navigate) close it, checked; but a click outside the page that isn't the sidebar (the media panel, an empty gap) is swallowed by app/gui/mouse.rs pressed(), and the wheel there scrolls the launcher's list, where the TUI's Launcher::click returns Close outside `area`. Fix in mouse.rs: close the launcher there, or let the media panel work beside it like the sidebar
OVL-009 | DONE | shared App::open_launcher: refresh_usage + refresh_sessions, directory from preferred_dir before leave_sidebar; the window's sidebar clicks don't leave the sidebar first (mouse.rs pressed, in_sidebar)
OVL-010 | DONE | a new Launcher on every open (App::session_launcher); only the permission setting and recents-derived defaults carry over
OVL-011 | DONE | shared Launcher::new + with_permission; demo screenshot opens on claude, skip all, New, List, row 0
OVL-012 | DONE | quirk kept: Launcher::new takes recents[0].harness, orchestrator recents included (shared)
OVL-013 | DONE | shared app/overlays.rs launcher_data_with_installed (tighter window, 0% weekly reads 0% left); drawn as "N% left" in launcher/gui.rs draw_run_on
OVL-014 | DONE | shared usage_status joined " · " lands in each login's detail, drawn by launcher/gui.rs draw_run_on
OVL-015 | PARTIAL | installed is computed once per open, the demo counts every agent installed, not re-checked while open (shared, checked); but launch::which per harness still runs on the window's event/render thread (App::session_launcher -> app/overlays.rs launcher_data), against DD-018. Fix in app/overlays.rs: compute `installed` on a worker (or cache it in Services) and apply it with Launcher::set_data
OVL-016 | PARTIAL | keyed matches the TUI (shared); but bro_core::gemini::signed_in(creds_path()) is read on the event/render thread at every open and on every Event::Services while the launcher is open (app/overlays.rs launcher_data_with_installed), against DD-018. Fix: read it in a services worker and keep the flag in State

### Launcher: New tab layout

OVL-017 | DELTA | DD-028/DD-020: the frame title gives way to the address bar and the New session / Continue switch; the × is a quiet icon button with an "esc" tooltip (DESIGN 8, no danger colour); Profiles + alt+o keycaps sit right of the switch and shrink to an icon when tight (launcher/gui.rs head, profiles_button)
OVL-018 | DONE | launcher/gui.rs draw_tabs: WinUI segmented "New session" (+ icon) / "Continue" (history icon), the chosen one raised (DD-020); clicks TAB_NEW / TAB_CONTINUE
OVL-019 | DELTA | DD-028: launcher/gui.rs address_bar replaces the chip: folder, the path as breadcrumbs (home as a house, middle elided), alt+p keycap and a chevron instead of "change"; a click opens the picker; on ★ a gold folder, "runs in" and the orchestrator folder, not clickable (screenshots)
OVL-020 | DONE | launcher/gui.rs draw_agents: a tile per agent with its real logo (DD-022); chosen = selected fill + accent outline, no brand-coloured fill (DD-023); not installed: muted label, faint logo, download icon and an "isn't installed yet" tooltip
OVL-021 | DONE | launcher/gui.rs draw_agents: ★ orchestrator tile set apart by a divider, gold (t.gold from theme::gold: #FFC83D dark / #B28000 light); a running one shows a pulse icon and a "running" tooltip instead of ● (DD-021)
OVL-022 | DONE | launcher/gui.rs draw_brains: gold "Runs on", small chips with logos, note "tools built in" / "tools through bro ctl" (screenshot)
OVL-023 | DONE | launcher/gui.rs draw_install: alert icon + "{h} isn't installed yet · enter or click here to install it" in danger, the whole box -> Outcome::Install (code only: the demo counts every agent installed)
OVL-024 | DONE | launcher/gui.rs draw_new: side layout when the body is at least 760×300 px (84×11 cells), else draw_stacked (screenshots of both; stacked with the media panel open)
OVL-025 | DONE | launcher/gui.rs draw_side: panel 52% (400-580 px) when a model is needed, else 38% (340-440 px); hairline divider in the gap
OVL-026 | DELTA | launcher/gui.rs input: a WinUI text box (DD-020) with the same placeholder "run on… ↑↓ to pick · type a project", caret only while it has the keyboard; the list keeps the selection in view centred with an eased scroll (gui.rs scroll) instead of pinning it to the bottom row (pixel list)
OVL-027 | DELTA | DESIGN group headings: widgets::section in sentence case ("Your logins"), no hairline (launcher/gui.rs draw_run_on)
OVL-028 | DONE | launcher/gui.rs draw_run_on Item::Recent: logo (DD-022), who (provider id / short profile / "own login"), short model muted, "{age} ago" on the right (screenshot after two demo launches)
OVL-029 | DONE | launcher/gui.rs draw_run_on Item::Account: logo faint when not ready, label + muted detail, "N% left" coloured with segments (72 / 44 / none by list width, DD-025), "not signed in" when not ready with no reading
OVL-030 | DONE | launcher/gui.rs row_fill: selected fill while the list has the keyboard, a quieter fill otherwise (DD-001, no marker); empty "nothing matches · esc clears the filter"
OVL-031 | DONE | launcher/gui.rs draw_summary: "Launches", logo + label_for(spec), the account's detail, segments + "N% left"; a recent row adds "the way you last ran it here" (screenshots)
OVL-032 | DONE | launcher/gui.rs draw_summary: "pick where it runs on the left" / "install it first"
OVL-033 | DONE | launcher/gui.rs draw_summary (orchestrator): ★ + label, "Opens sessions and hands them work," / "then tells you how it went." + segments; running: label, "Already running. There's only ever one.", "⏎ takes you to it." (screenshot of the not-running case)
OVL-034 | DONE | launcher/gui.rs draw_models: list under the field "model… N to pick from", id + context (262k / 1M) + "$in/out" with zeros trimmed, "no models known for this" / "nothing matches" (screenshots)
OVL-035 | DONE | launcher/gui.rs draw_options: "Permissions" Ask / Auto / Skip all (skip in danger when chosen) + ctrl e keycaps, each a click (PERM)
OVL-036 | DONE | launcher/gui.rs draw_options: "Browser" Off / On / Edge / Chrome + ctrl b; a click cycles ctrl+b to that mode (BROWSER, test choices_go_to_what_was_clicked)
OVL-037 | DONE | launcher/gui.rs draw_options: "Opens as" New session / Split beside this one ("Split" when tight) + ctrl s; on ★ "its own, pinned on top" in gold, not clickable
OVL-038 | DONE | launcher/gui.rs main_action + main_button: "Launch {label}" (accent fill, DESIGN 7), "Start it on {label}" / "Go to the orchestrator" (gold), "Install {h}" (danger), muted "pick a model to launch" / "pick where it runs"; also "Sign in with Google" where Enter signs in
OVL-039 | DONE | launcher/gui.rs draw_stacked: list (45% / 55% with models), rule, "→ {label}" / "→ pick a model" (shine) / "→ pick where to run", then option rows or a flow of segmented choices, each clickable (screenshots with the media panel open)
OVL-040 | DONE | launcher/gui.rs draw_footer: the TUI's hints in order (shift+←→ runs on, tab list ⇄ models, esc close / back), the ⏎ one clickable (LAUNCH); ⏎ reads install / sign in when Enter does that

### Launcher: run-on list and models

OVL-041 | DONE | shared Launcher::items: recent, your logins, other logins · via proxy, providers; empty groups and headers left out (screenshots)
OVL-042 | DONE | shared Launcher::recents_for (same harness and orchestrator flag, dedup, max 4, newest first)
OVL-043 | DONE | shared Launcher::items; pi screenshot shows "pi's own login" / "whatever it's configured with"
OVL-044 | DONE | shared Launcher::items / profile_account (via-proxy prefixes "chatgpt" / "claude"; screenshots of claude, codex and pi)
OVL-045 | DONE | shared Launcher::items (providers whose mode isn't Native)
OVL-046 | DONE | shared Launcher::profile_account
OVL-047 | DONE | shared Launcher::pool_account ("pool", "2 claude accounts · failover", no meter in the screenshot)
OVL-048 | DONE | shared Launcher::provider_account ("3 models", "google ai subscription" in the screenshot)
OVL-049 | DONE | shared Launcher::items / item_text (fuzzy per group)
OVL-050 | DONE | quirk kept: shared Launcher::key (the window's field shows the always-empty list filter)
OVL-051 | DONE | shared Launcher::needs_model (screenshots: a provider and a via-proxy codex login on claude both ask for a model)
OVL-052 | DONE | shared Launcher::models / models_view (codex login on claude lists gpt-5.2-codex…)
OVL-053 | DONE | shared Launcher::reset_models, called by the window's row click too (launcher/gui.rs click)

### Launcher: spec and outcomes

OVL-054 | DONE | shared Launcher::spec
OVL-055 | DONE | shared Launcher::quick_spec / with_profile (App::quick_launch, launch_usage, quick_agent)
OVL-056 | DONE | quirk kept: shared Launcher::key / launch
OVL-057 | DONE | shared Launcher::launch
OVL-058 | DONE | app/gui/overlays/mod.rs apply_launcher Outcome::None -> App::remember_launcher_permission, for clicks; keys through app/input.rs
OVL-059 | DONE | apply_launcher Outcome::Profiles -> close + open_view("profiles") (screenshot after clicking Profiles)
OVL-060 | FIXED | shared App::select_launcher_project (keys, picker clicks, sidebar project rows) adds and persists the project, picks it, selects its row, sets the launcher dir and Continue scope; its "opened {name}" toast was drawn under the launcher page (the frame drew toasts before the overlays) until app/gui/overlays/mod.rs `draw` drew the toasts again on top of the launcher, as the TUI does (toast-opened.png)
OVL-061 | DONE | apply_launcher Outcome::Launch -> close, launch_spec (two demo launches)
OVL-062 | DONE | apply_launcher: Archive / UndoArchive keep it open; Resume, ResumeMany, Install, SignIn close it first

### Launcher: keys

OVL-063 | DONE | shared Launcher::key through App::handle (app/gui/input.rs gives the same KeyEvents; alt+p, alt+o, alt+n/N, alt+r/R; key tips show p / n / r / o, DD-026)
OVL-064 | DONE | shared Launcher::key (screenshot: esc on Models goes back to the list)
OVL-065 | DONE | shared Launcher::key (Enter on a provider moves to the models)
OVL-066 | DONE | shared Launcher::key (shift+tab arrives as BackTab, app/gui/input.rs)
OVL-067 | DONE | shared Launcher::set_harness / shift+←→ set_brain (screenshots stepping claude, codex, pi, ★)
OVL-068 | DONE | shared Launcher::move_sel (↓ screenshots)
OVL-069 | DONE | shared Launcher::key (ctrl+e / ctrl+b / ctrl+s; none of them is a window key in Gui::gui_key)
OVL-070 | DONE | shared Launcher::key (typing "k" filters the models in the screenshot)
OVL-071 | DONE | shared Launcher::paste through App::paste (ctrl+v, IME text and dropped files all reach it)
OVL-072 | DONE | quirk kept: shared Launcher::key

### Launcher: mouse

OVL-073 | DONE | launcher/gui.rs click: CLOSE, PROFILES, PROJECT, TAB_NEW / TAB_CONTINUE, CONTINUE (continue_picker/gui.rs scope and rows), HARNESS (switch unless current), ORCHESTRATOR, BRAIN, PERM, BROWSER, PLACE, INSTALL, LAUNCH (= Enter); tests tabs_agents_and_the_orchestrator, choices_go_to_what_was_clicked
OVL-074 | DONE | launcher/gui.rs click ROW: the selected row with the list focused -> Launcher::key(Enter), else focus List and select (+ reset_models); test a_row_again_acts_like_enter_and_a_model_again_launches; screenshot
OVL-075 | DONE | launcher/gui.rs click MODEL: the selected model with Models focused launches, else focus Models and select; same test
OVL-076 | DONE | quirk kept: launcher/gui.rs click MODEL calls Launcher::launch directly (no installed check), as Launcher::click
OVL-077 | DONE | launcher/gui.rs wheel: picker 3 keys, Continue 3 keys, else focus the list under the pointer and move ±3 (test the_wheel_moves_the_list_under_the_pointer; screenshot)

### Launcher: ★ orchestrator tab

OVL-078 | DONE | shared Launcher::set_harness past omp / enter_orchestrator; the ★ tile (ORCHESTRATOR); alt+O -> App::open_orchestrator
OVL-079 | DONE | shared Launcher::brain_default
OVL-080 | DONE | shared Launcher::smart_sel (demo lands on pool: no reading scores 50, beating 9-42%)
OVL-081 | DONE | shared Launcher::recents_for / launch (Place::Tab forced); options show "its own, pinned on top"
OVL-082 | DONE | pulse icon on the ★ tile (DD-021), hint "go to it", button "Go to the orchestrator"; Enter -> launch_spec focuses it (shared). Code only: no orchestrator runs in the demo

### Launcher: install and Gemini sign-in

OVL-083 | DONE | Enter, draw_install's box and the Install button -> Outcome::Install -> App::install_agent (demo toast); reopening runs launcher_data again. Code only in the demo
OVL-084 | DONE | shared provider_account / sign_in_needed / launch -> App::sign_in_gemini (demo toast); the window says "Sign in with Google" and "⏎ sign in" there; signed in shows "google ai subscription" and asks for a model (screenshot)

### Launcher: live data

OVL-085 | DONE | Event::Services -> launcher_data_with_installed (PATH not searched) + Launcher::set_data (shared); the window redraws from that state
OVL-086 | DONE | launcher/gui.rs reads l.data every frame and redraws after each Services event; OpenRouter catalogue refresh in services/workers.rs (shared)
OVL-087 | N/A | the window's clicks go through its own hit list (Gui::hits), which set_data never clears, so no click is dropped; a click right after an update maps by index into the new list (bounds-checked in launcher/gui.rs click)

### Folder picker (inside the launcher)

OVL-088 | DELTA | DD-028: the picker is a dropdown hanging from the launcher's address bar (folder/gui.rs draw, titled "Choose project"), not an 84x22 popup centred on the body. Openers all work the same: alt+p and typing via Launcher::key, a click on the address bar (launcher/gui.rs PROJECT -> open_project_picker), paste via App::paste -> Launcher::paste; checked in demo shots.
OVL-089 | DONE | Shared: App::launcher_projects (app/sessions.rs) + Launcher::open_project_picker + folder::dedup; the demo shot lists the launcher dir first, then the open projects, then the process cwd.
OVL-090 | DONE | Shared FolderPicker::refresh (looks_like_path, Typed + Completion rows, else fuzzy Suggested), reached through Launcher::key; checked in a shot after typing F:\bro-cli-v2\cr (create row and crates completion).
OVL-091 | DONE | Shared folder::complete (case-insensitive prefix, dot/$ dirs only for a "." partial, sorted, max 60); the shot of F:\bro-cli-v2\crates\ lists the crates in sorted order.
OVL-092 | PARTIAL | Draw-time stats are off the UI thread (folder/gui.rs typed_state stats on a worker thread and caches the answer). But FolderPicker::refresh still runs expand(f).is_dir() and complete()'s read_dir on every keystroke, Tab and paste, and open_selected runs fs::metadata on Enter or click. All of these run on the window's event thread through Launcher::key. Fix in folder.rs: refresh/complete/open_selected need a worker with a request id (like create_project_folder) and an Event that applies the result.
OVL-093 | DONE | Shared FolderPicker::key: while the picker is open, every key goes to it (Launcher::key). Esc clears the filter, then closes back to the list (project_outcome Close). Enter, arrows, ctrl+n/j/p/k, ctrl+u, Backspace and chars all work; there is no PgUp/PgDn. ctrl+u and esc were checked in a scripted run.
OVL-094 | DONE | Shared FolderPicker::tab_complete (adds the separator, keeps the ~ shortening, uses / only when typed with /). A shot after Tab shows "F:\bro-cli-v2\crates\" with its subfolders.
OVL-095 | DONE | Shared FolderPicker::paste through App::paste -> Launcher::paste (ctrl+v clipboard event, IME text, and files dropped via gui/mouse.rs dropped). It is ignored while creating, trims, strips quotes, rejects control chars, and a full path replaces the filter (unit tests in folder.rs).
OVL-096 | DONE | Shared FolderPicker::open_selected. Shots show "that path is a file; choose a folder" (README.md), "couldn't open folder: The system cannot find the path specified. (os error 3)" for a missing suggestion, and a typed missing path being created.
OVL-097 | DONE | Shared App::create_project_folder (thread with a request id) and project_folder_created. folder/gui.rs draws a spinner with "creating folder…" and the hint "esc cancel" while it waits. Shots show success (newproj becomes the project) and the failure "couldn't create folder: ... (os error 3)" for Q:\newproj. In the window, the × does what Esc does.
OVL-098 | DONE | Shared: Esc during creation closes the picker (FolderPicker::key returns Close while pending). project_folder_created then drops the stale request, and the worker still creates the folder (app::tests::cancelling_folder_creation_ignores_its_completion_in_a_new_modal passes).
OVL-099 | FIXED | folder/gui.rs draw_rows draws every tag: file (danger pill and file icon), create (shine pill and folder-plus icon), ↳ (shine, typed and existing), a chevron (completion), and a folder (suggested). It also draws the semibold name with the selection as a fill (DD-001) and the parent path fitted from 60 chars down, muted. Before this fix the launcher's folder showed only a check; it now shows a check plus the TUI's word "current" (accent, as the palette marks it). Checked in a shot.
OVL-100 | DONE | folder/gui.rs draw: the empty state says "nothing here — type a path (~/, C:\, /…)". The hints are "⏎ create and use|use project · ctrl+v paste path · tab complete · ↑↓ move · esc cancel", or "esc cancel" while creating. Checked in shots.
OVL-101 | DONE | In launcher/gui.rs click, a row calls folder::gui::click_row and, when nothing went wrong, Enter, so one click on any row selects and opens it. Clicks that miss a row do nothing. Clicks outside the popup (the page, the sidebar) land on the Action::None scrim and change nothing. The wheel moves the selection by 3. Checked in shots.
OVL-102 | DONE | Shared: Act::OpenClipboardFolder (ctrl+shift+o is not taken by Gui::gui_key) runs folder::clipboard_folder on a thread. Event::ClipboardFolder then opens the folder or shows the toast with the TUI words. Checked in code and the app::tests::a_copied_folder_opens_as_a_project test; no shot, because the check would need the user's real clipboard.

### Continue tab

OVL-103 | DONE | Shared App::continue_entries (app/commands.rs), fed at open (session_launcher) and refreshed; the All sessions shot shows the demo's past sessions newest first.
OVL-104 | DONE | Shared ContinuePicker::new/set_project/set_all. The scope starts on the launcher dir's project with all=false. select_launcher_project calls set_project, which keeps all. Tab and BackTab (winit shift+Tab becomes BackTab) toggle the scope only when a project is set. Checked in shots.
OVL-105 | DONE | Shared ContinuePicker::refresh: show_archived plus the scope, then fuzzy over "title project login harness id"; each refresh resets sel and the anchor.
OVL-106 | DONE | Shared ContinuePicker::key Esc chain (filter, then picks, then Close); a scripted esc cleared the picks.
OVL-107 | DONE | Shared resume_outcome (one pick gives Resume, several give ResumeMany in entry order, no picks give the cursor row). continue_picker/gui.rs RESUME (the button and the ⏎ hint) sends the same Enter.
OVL-108 | DONE | Shared toggle: ctrl+space reaches the overlay before App::key's prefix check (winit Space + CONTROL gives Char(' ')). A shot shows the first pick made by ctrl+space.
OVL-109 | DONE | Shared extend/anchor logic (shift+arrows, shift+PgUp/PgDn); a shot shows ctrl+space, down, shift+down twice giving 4 picks and "resume all 4, one at a time".
OVL-110 | DONE | Shared: Delete returns Archive, ctrl+z Undo and ctrl+a toggles show_archived, applied via overlays::apply_launcher -> continue_archive/undo_archive. Shots show a row archived, struck through when shown, and restored.
OVL-111 | DONE | Shared ContinuePicker::key moves, filter editing; picks survive searching and drop only in set_entries when the entry is gone.
OVL-112 | DONE | continue_picker/gui.rs click_row mirrors ContinuePicker::click: ctrl toggles (picking the cursor row first when nothing is picked), shift extends, the cursor row resumes, another row moves the cursor. The scope segments call set_all, and launcher/gui.rs wheel sends 3 key moves. Shots show a click moving the cursor, the wheel moving 3 rows, the project chip setting the scope, and a click on the cursor row opening the resume picker. ctrl/shift clicks are covered by unit tests (the script can't hold ctrl).
OVL-113 | DONE | continue_picker/gui.rs draw has the search field with placeholder "type to search earlier sessions" and the caret. The scope sits on the right as a segmented control (folder + project name fitted to 18, "All sessions", the chosen one raised) followed by a Tab keycap. Checked in shots.
OVL-114 | DONE | continue_picker/gui.rs draw_rows: a pick box column once anything is picked (the TUI's ✓), the agent's logo (DD-022) and the fitted title. On the right: [id tail], project (fit 16, with a folder icon, only for All sessions), login (fit 12), "archived" with an archive icon, and the age. The cursor marker is a fill (DD-001).
OVL-115 | DONE | The cursor row uses the t.selected fill (DESIGN.md selection fill, DD-001) and picked rows a faint accent tint. Archived titles are muted with a strike line (cont-showarch shot).
OVL-116 | DONE | "No earlier sessions yet" / "Nothing matches" (all) / "No sessions here. Press Tab for all sessions." verbatim without the leading spaces; the last two checked in shots.
OVL-117 | DONE | Hints verbatim for both states, including "show archived"/"hide archived" and "resume 1 session"/"resume all N, one at a time" (shots); the window adds a Resume button with the same words.
OVL-118 | FIXED | shared continue_archive / undo_archive raise the TUI's toasts ("archived · ctrl+z undoes", "restored", "nothing to undo", "restored N session(s)"); they were drawn under the launcher page until app/gui/overlays/mod.rs `draw` drew the toasts again on top of a native launcher, as the TUI does (app/draw.rs: launcher, then toasts); toast-launcher.png
OVL-119 | DONE | Shared: App::handle Event::Services -> refresh_continue -> Launcher::set_continue_entries -> ContinuePicker::set_entries (cursor clamped); the window forwards Services events through Gui::app_event.
OVL-120 | DONE | Quirk kept as in the TUI, since this is the same state machine: set_entries -> refresh clears the shift-range anchor. To fix it on purpose, change continue_picker.rs set_entries to keep `anchor` (clamped).

### Resume picker ("resume in" / "move to another login")

OVL-121 | DONE | shared App::open_resume / open_switch (app/sessions.rs); the window reaches them through the Continue tab (overlays/mod.rs `apply_launcher` Outcome::Resume → open_resume) and alt+L / prefix P (keys); resume-switch.png
OVL-122 | DONE | shared App::resume_targets (same-family authenticated logins, "plan · email", min(5h, wk) left, current first, unknown last); resume.rs `row` draws them in that order
OVL-123 | DONE | shared App::resume_default / open_switch; resume-switch.png starts on "personal", not the current "work"
OVL-124 | DONE | resume.rs `draw`: WinUI dialog 620 px wide (DD-003), rows fit the window and scroll with the selection kept in view; the caption "Resume in" / "Move to another login" (sentence case, DESIGN.md) sits above the title, the agent's logo in place of the history icon (DD-022)
OVL-125 | DONE | resume.rs `draw`: agent logo + session title (heading, semibold), then the TUI's note verbatim ("another login continues from a copy of the conversation" / "the running session closes and continues in the login you pick")
OVL-126 | DONE | resume.rs `row`: number as a keycap (1-9; later rows have none since no key picks them), name semibold, "plan · email" muted, "current" as a check + the word (palette.rs `current`), "{left}% left" right in the sidebar's usage colours over segments (DD-025); the selection is a fill, not ▌ (DD-001)
OVL-127 | DONE | keys shared (App::resume_key through App::handle: esc, ↓/j, ↑/k, 1-9, ⏎); resume.rs draws the TUI's hints "⏎ resume · 1-9 pick + go · ↑↓ move · esc cancel" as keycaps beside Cancel / Resume buttons; mouse as in the TUI (click selects, a click on the selected login runs it, wheel and outside clicks ignored: resume.rs `click` / `wheel`, scrim Action::None)

### Palette

OVL-128 | DONE | shared App::open_palette / run_act (alt+p, prefix :, prefix space, prefix t "theme ", welcome p, through App::handle); the tab row's overflow button → StripHit::More → open_palette("tiles ") (gui/sidebar.rs); palette.png, pal-theme.png
OVL-129 | DONE | shared App::open_palette builds the list (tiles, 5 recents, actions, presets, caveman, themes); palette.rs `draw` shows `Palette::matches` in that order (the demo has no recents)
OVL-130 | DONE | shared Palette::with_tiles labels; palette.rs `glyph("window")` window icon, the part after " — " quieter (`label`); palette.png
OVL-131 | DONE | shared recent labels; palette.rs `row` draws "recent" as a quiet pill and the agent's logo in place of the history icon (DD-022); code only (the demo has no recents)
OVL-132 | DONE | quirk kept: shared App::run_palette launches Cmd::Recent as a plain session with the settings permission and the recent's browser; palette.rs `click` runs the same path
OVL-133 | DONE | shared Palette::new (Act::all minus Jump 4-9 / ShowTab 1-9, describe(), km.primary); palette.rs `keys` draws the chord as keycaps (the prefix step outlined), `glyph` covers every icon_for name
OVL-134 | DONE | shared with_presets / with_caveman labels; palette.rs `parts` / `label` turn " · current" into a check + "current" and show the help after " — " muted; presets carry Claude's logo (DD-022), caveman rows its icon; pal-preset.png, pal-caveman.png
OVL-135 | DONE | shared theme labels; palette.rs `parts` shows light / dark / yours as quiet pills and current as a check, `swatch` the theme's background with its 5 colours; pal-theme.png
OVL-136 | DONE | shared Palette::matches ("theme" lists only themes in their order, "tiles" only tiles, else fuzzy over "{label} {hint}"); pal-theme.png lists the 30 themes
OVL-137 | DONE | keys shared: Palette::key through App::handle → overlay_key; palette.rs draws `p.sel` / `p.query` (caret, placeholder)
OVL-138 | DONE | shared overlay_key / App::paste preview; the window also previews on a click or wheel selection (palette.rs `preview`) and while the pointer rests on a theme row (260 ms, put back when it leaves); pal-theme-down.png, pal-hover-a…d.png
OVL-139 | DONE | shared overlay_key (Esc restores; Run restores first unless it's a theme; run_palette saves it and toasts "theme: {t}"); palette.rs `click` on the selected row and gui/mouse.rs's outside click do the same; pal-run.png (toast), pal-outside.png (restored)
OVL-140 | DONE | shared App::run_palette for every Cmd; palette.rs `click` calls it exactly as Enter does
OVL-141 | DELTA | palette.rs `draw`: a WinUI command-palette card (640 px, ≤ 11 rows; DD-003, DD-020) with the search icon in the field instead of a framed "palette" title, the placeholder verbatim, the match count right; the selection is a fill (DD-001) and matched letters are highlighted; hints as keycaps, themes their swatch; the list scrolls only as far as needed to keep the selection in view (the TUI pins the cursor to the last row); "nothing matches" verbatim (pal-empty.png)
OVL-142 | DONE | shared fuzzy::score through Palette::matches; palette.rs `matched` highlights with the same greedy in-order walk
OVL-143 | DONE | shared fuzzy::filter (scores, stable ties, blank keeps order) for the palette, run-on list, models, Continue and folder suggestions; the window ranks nothing itself

### Help

OVL-144 | DONE | shared: F1 (gui/input.rs maps it), prefix ?, ? on a non-terminal pane and welcome ? open Overlay::Help through App::handle; help.png
OVL-145 | DONE | gui help.rs `draw` builds its sections from `help::lines(&app.keymap)` (through `window_lines`): live chords as keycaps with alternatives split by "/", "alt+1–9" once, chordless acts skipped, a gap between groups; help.png
OVL-146 | DONE | the same `help::lines` sheets (sidebar, project tabs, launcher, clipboard, usage, terminal, mouse) drawn as sections; help-end2.png
OVL-147 | FIXED | app/gui/overlays/help.rs `window_lines` / `rewritten` / `added` (DD-012): the launcher sheet now reads "← → switch agent", "shift+← → on ★: what it runs on", "tab list ⇄ models" and "ctrl+e permission: ask → auto → skip all"; the mouse sheet says a double-click on a session's title goes full screen (a double-click inside selects a word) and one on the sidebar edge resets its width, and adds middle-click a title, the back / forward buttons and dropping a file; the terminal sheet adds double/triple-click selection; a new "Window" section lists ctrl+= / ctrl+- / ctrl+0, ctrl+shift+c and F11; test help::tests::the_terminal_sheets_are_rewritten_for_the_window; help-end2.png
OVL-148 | DONE | help.rs `draw`: centred card (window less 80 px, up to 1120), keyboard icon + "bro · keys", "prefix" + its keycaps, the hints "esc close · j/k scroll", × button; two columns when the sheet is ≥ 760 px wide (DD-003); help.png
OVL-149 | DELTA | DESIGN.md type rules: headings in sentence case, semibold (widgets::section; "No ALL-CAPS labels"), keys as keycaps on the right (kit kbd) instead of a padded shine column, descriptions in the text colour
OVL-150 | DONE | keys shared (Help::key through App::handle: esc/F1/q/?/⏎ close, j/k ±1, pgdn/space +10, pgup −10, home/g top, no End); help.rs `draw` clamps `Help::scroll` to the sheet's height (28 px steps); the wheel adds 3 (DD-005)

### Confirm

OVL-151 | DELTA | confirm.rs `draw`: a WinUI ContentDialog (440 px, centred 30% down; DD-003, DD-020): the question is the dialog's title in the text colour (semibold), the detail muted below, no "confirm" frame title; the danger colour moves to the Yes button (DESIGN.md principle 7)
OVL-152 | FIXED | confirm.rs `draw`: the buttons now carry the TUI's words, "Yes" (danger) and "No" (was "Cancel"), with the keys as hints beside them ("y yes · esc no"); WinUI hover fill instead of an underline; the dialog fades and lifts in on every open (`appear`), not only the first; confirm-quit2.png
OVL-153 | DONE | keys shared (App::confirm_key: y/Y/⏎ yes, anything else no); confirm.rs `click` acts on Yes/No only, left button only (overlays/mod.rs `click` drops the others); the scrim (Action::None) swallows every other click and the wheel
OVL-154 | DONE | shared Act::Quit copy (app/commands.rs); confirm-quit2.png "quit bro and end 5 sessions?" / "running agents are stopped; past sessions stay resumable"
OVL-155 | DONE | shared App::ask_close copy; confirm-close.png "close claude · work · opus-5?" / "the agent is still busy — it will be stopped"
OVL-156 | DONE | shared App::close_project copy (prefix X, the sidebar's × through SideHit::CloseProject, the tab's close); confirm-project.png "close bro-cli-v2 and its 3 running sessions?"

### File preview (text)

OVL-157 | DONE | terminal file: links → app/gui/mouse.rs open_link → App::open_file_link (screenshot: path printed in a demo shell, clicked, preview opened); card-back links → open_link from painted.card_links; bro open, prefix f and AgentFiles picks are the shared App paths (app/files.rs)
OVL-158 | DONE | shared App::agent_opened_file (bridge OpenFile through App::handle): last_opened, focused+visible+no sidebar+no rename+overlay None/File gate, else the toast
OVL-159 | DONE | shared App::show_last_opened; screenshot: prefix f shows "no file opened yet · agents open files with `bro open path:line`"
OVL-160 | DONE | shared FileView::request (worker per request); app/gui/mod.rs app_event only intercepts replies whose id matches, stale ones reach FileView::apply and are dropped
OVL-161 | DONE | shared file_view::read → panes/file_cache::resolve_local → bro_bridge resolve_path (file://, ~/, /c/, cwd/project/a/ b/, index suffix)
OVL-162 | DONE | decided: the window follows the TUI (same resolve_local: any absolute or ~/ path on this machine, UNC // and control chars refused); screenshot: F:\bro-cli-v2\… outside the demo session previewed
OVL-163 | DONE | shared FileView::apply (Fallback(uri) when no path and not NOT_FOUND) → Event::OpenLink → app_event (folders aren't executables) → OS file manager; without a fallback the card shows "This is a folder, not a file."
OVL-164 | DONE | shared bro_bridge read_preview via file_view::read (2 MB, NUL = binary, UTF-8 only, CR trimmed, last newline dropped, language_for); screenshot: bro.pdb shows "This file is too large to preview (2 MB limit)."
OVL-165 | DONE | shared read_preview window (CONTEXT_ABOVE 20, DEFAULT_LIMIT 300, markdown from the top to MAX_LIMIT 5000, content cap)
OVL-166 | DONE | shared file_view::highlight (syntect + two-face, extension → name → language token → first line, Plain Text = none)
OVL-167 | DONE | shared Palette slots + file_view/gui.rs line(): fg through kit::color, bold/italic to draw_cell, underline as a quad; screenshot shows keywords/functions/strings in theme colours
OVL-168 | DONE | shared HIGHLIGHT_BUDGET and clean() (tabs to 4-col stops, control chars to ·)
OVL-169 | FIXED | file_view/gui.rs code()/line(): new recolour()+live() map the seven highlight slots of the theme captured at open onto the live theme, so code follows a theme change (custom theme file edited) without a reload; test code_takes_the_live_themes_colours
OVL-170 | DONE | file_view/gui.rs layout() (window less 24 px each side, max 1100 wide; DD-003) and header(): kind icon + FileView::title() (relative name + ":line")
OVL-171 | DONE | file_view/gui.rs header() ("{lang} · N lines · size", full path when it differs from the shown one) and where_you_are() ("top–last of N", →N pill, spinner + "loading…"); screenshots preview.png, sideways.png
OVL-172 | DONE | file_view/gui.rs code(): gutter max(3, digits), target number bold accent, full-width accent tint band (no ▌ bar, DD-001), "…" for unloaded lines
OVL-173 | DONE | shared FileView::set_rows → center_on_target, called by paint() before top is read; test draws_the_text_centred_on_its_line_with_its_buttons (top = line − rows/3)
OVL-174 | DONE | shared FileView::scroll → page() (PREFETCH 20, ±300, 5000 cap, failed page keeps what's shown); GUI wheel/keys call it (edge: see OVL-179 for script files)
OVL-175 | DONE | shared FileView::key through App::handle (no window key shadows them; scripted shots: l ×3 → →24); uppercase G reaches it as Char('G') from winit
OVL-176 | DONE | shared FileView::key outcomes → App::file_view_outcome (editor/system/copy/mention); screenshot: @ mentions
OVL-177 | PARTIAL | wheel ±3 (file_view/gui.rs wheel), hint buttons and a click outside (SCRIM → Close) work; shift+wheel now moves ±8 columns (app/gui/overlays/mod.rs `wheel`, this pass); a real horizontal wheel or touchpad swipe still does nothing: app/gui/mouse.rs `wheel()` returns when dy == 0 — pass dx on (e.g. as a sideways step to overlays::wheel) for ±8 columns
OVL-178 | DONE | file_view/gui.rs buttons()/footer(): the TUI's words per state (text: editor, system app, copy path, mention + inert "↑↓ pgup pgdn scroll"; failed: system app ⏎, copy path; loading: close; media: open ⏎, copy path, mention), close quiet, Enter's action primary on the right, WinUI hover fill instead of underline
OVL-179 | PARTIAL | failures show the message with an explicit system-app button (DD-007, failed.png). Gap in app/gui/mod.rs app_event: for an executable path (bro_media::is_executable, includes .js .py .sh .ps1 .bat) it closes the preview with "bro doesn't run programs · path" instead of showing the failure (e.g. a > 2 MB .js bundle), and also on a failed page of an already shown text script (should keep what's shown) — check matches!(v.body, Body::Text(_)) first, and keep the failure card without the system action for executables
OVL-180 | DELTA | DD-007: the failure card is now reachable (app_event applies the load without auto-opening); media goes to the media sidebar, so the media card stays unreachable (every media_mime/image_mime type is media::is_media)
OVL-181 | PARTIAL | shared App::file_view_outcome Copy → clip::copy + toast "copied {path}" works on Windows (clip.exe) and Linux tools; not the native clipboard (DD-008): it also writes OSC 52 to stdout, and with no pbcopy the macOS window copies nothing while toasting — use window::set_clipboard (arboard) for the window in app/files.rs file_view_outcome (or in clip::copy under the gui feature)
OVL-182 | DONE | shared App::file_view_outcome Mention (close, focus source, leave sidebar, paste "@path "); screenshot: "@F:\…\file_view.rs " typed into the demo shell
OVL-183 | DONE | shared Outcome::System/Fallback → Event::OpenLink → app/gui/mod.rs app_event (refuses executables, DD-007) → App opens it on a thread, errors toasted

### Editor hand-off

OVL-184 | PARTIAL | choice is shared (editor::plan_here on a worker in App::open_in_editor), but for the window it breaks DD-007: Plan::System calls panes::links::open directly (ShellExecute "open"), bypassing app_event's executable check, so Enter/e on a previewed .bat/.cmd/.js/.vbs/.ps1 with no $EDITOR and no code on PATH runs it (same for remote Open on desktop) — in app/files.rs open_in_editor send Event::OpenLink(uri) for Plan::System, or refuse bro_media::is_executable(&path)
OVL-185 | DONE | shared editor::plan (tests editors_get_their_own_line_syntax etc. pass)
OVL-186 | DONE | shared Event::EditorPane → App::editor_pane (split beside the source, Tab when no tabs, "{editor} {file}", session cwd or the file's folder)
OVL-187 | DONE | shared editor::spawn_gui (CREATE_NO_WINDOW) + toast "opened {file:line} in {program}"
OVL-188 | DONE | shared BridgeCommand::OpenInEditor → App::open_in_editor, focused or not (same Plan::System exposure as OVL-184)

### Agent files

OVL-189 | DONE | shared App::show_agent_files via prefix F; screenshot: on the usage view toasts "agent files: focus an agent session first"
OVL-190 | DONE | shared agent_files::items (sent then touched, relative paths); agent_files/gui.rs header(): "Sent to you"/"Touched" sentence case (DESIGN) + count; GPU shot agent-files.png
OVL-191 | DONE | shared empty strings from App::show_agent_files drawn by agent_files/gui.rs empty(); screenshots: waiting for the transcript, a shell has no agent files, pi sessions don't report their files
OVL-192 | DONE | agent_files/gui.rs layout() (window less 48 px, 520–880 wide, height to the list; DD-003) and title() "agent files · {session}" with the file icon
OVL-193 | DONE | agent_files/gui.rs row(): selection fill (DD-001), kind as a monochrome icon (DESIGN: no shine glyphs), path[:line] mono, struck through for delete, caption under it, kind + age right; GPU shots agent-files*.png
OVL-194 | DONE | shared AgentFilesView::key; page = rows in view via set_rows in paint(); hints "⏎ preview · ↑↓ move · esc close" (preview and close also clickable, DD-013 spirit)
OVL-195 | DONE | agent_files/gui.rs wheel (move_by ±3) and click (activate: select + Open at once) → App::agent_files_outcome replaces the overlay with the preview; Esc there closes (tests rows_open_at_once_and_the_wheel_moves_three)

### Watchdog overlay: setup form

OVL-196 | DONE | open_watchdog (app/watchdogs.rs) reached by prefix w through App::handle; both toasts are its own (shot: shell focused shows "watchdogs watch agent sessions: …")
OVL-197 | DONE | WatchdogView::new data from open_watchdog: title in the title row (view/gui.rs title), name in the copy, agent in "watches", settings defaults; the reviewer hint is no longer shown because the Reviewer row names the orchestrator (DD-012, see OVL-201)
OVL-198 | DONE | view/gui.rs draw: status page when !editing && a snapshot exists, else the form; WatchdogView::new preloads the plan / one empty milestone with settings idle+budget, fresh+second on
OVL-199 | DELTA | DD-012: a new plan's intro is rewritten for the orchestrator model (view/gui.rs form_intro, see OVL-201); the old fresh-reviewer sentence stays only in the terminal UI
OVL-200 | DONE | form_intro(existing) uses view.rs intro verbatim, then the Watches block "watches" + agent with its logo (DD-022) (shot: edit page)
OVL-201 | FIXED | view/gui.rs form_intro: "…bro wakes the orchestrator, starting one if none is running. It reads the chat and the code…"; reviewer(): Reviewer row = gold star + "the orchestrator", help "every wake-up goes to it · bro starts one if none is running"; test a_new_plan_says_the_orchestrator_reviews
OVL-202 | DONE | form/milestone(): "Milestones" heading (sentence case) + "{n} · in order · the first one starts now"; fields 1-6 rows (FIELD_ROWS) scrolling inside past 6; number + status icon check/play/ring (DD-021) on existing plans; done text muted (shots: edit, 6-row field)
OVL-203 | DONE | milestone(): both placeholders verbatim, italic, faint
OVL-204 | DONE | option(): "How it works", "Wake after" + help verbatim; the value is a segmented control of the 6 choices (DESIGN: one of a few choices) instead of "‹ 1 min idle ›"
OVL-205 | DONE | opt_copy() on/off help verbatim for Fresh context and Second opinions ("only ever talks to {name}"); switch + On/Off
OVL-206 | DONE | Budget: segmented 10/25/50/100/250 + "wake-ups", help verbatim; Reviewer row reworded per DD-012 (see OVL-201)
OVL-207 | DELTA | values are WinUI controls (segmented selection, switch, focus = accent outline round the row) instead of coloured text (DESIGN components); the error shows above the command band in danger with an alert icon (form())
OVL-208 | FIXED | key_button "Start watchdog"/"Save plan" (primary) + "Cancel" in the command band, focus outlined, focused block kept in view; added: focus on the buttons scrolls the body to its end as the terminal UI does (view/gui.rs follow); test the_body_follows_the_focus_and_shows_the_end_on_the_buttons
OVL-209 | DONE | form(): WatchdogView::hints() as keycaps in the band (milestone / options / buttons sets checked in shots); ctrl+s and esc show as chips on the buttons instead
OVL-210 | DELTA | DD-012: "ctrl+s" only appears as the chip on "Save plan"/"Start watchdog", so editing never says "ctrl+s start"
OVL-211 | DONE | key_edit shared (every key clears the error, ctrl+s submits, empty plan error + focus milestone 1); the primary button runs the same submit (shot: error line)
OVL-212 | DONE | cancel() shared for esc; the Cancel button runs change(Cancel, 0) = the same cancel
OVL-213 | DONE | move_focus shared (shots: tab from milestone to Wake, down x4 to the primary button)
OVL-214 | DONE | key_edit shared (reorder, ctrl+d, backspace on empty); the × shown on hover runs the same remove()
OVL-215 | DONE | key_edit shared; field_width set from this layout so ↑/↓ at the top/bottom visual row leave the field exactly where the drawn rows end (shot: 7-row milestone, ↑ x6)
OVL-216 | DONE | change()/key_edit shared; segmented choices and switches step the same change()
OVL-217 | DONE | WatchdogView::paste through App::paste (shared); test a_pasted_list_becomes_milestones
OVL-218 | DELTA | window affordances (DD-013 spirit): status buttons press at once and option rows focus then change as in the terminal, but Start/Save/Cancel act on the first click, a click in a milestone puts the caret where clicked (terminal: end), choices/switches set directly, × removes (view/gui.rs click)
OVL-219 | DONE | WatchdogView::plan shared (drops blanks, keeps status and note)

### Watchdog overlay: status page

OVL-220 | DONE | view/gui.rs state(): spinner + "reviewing" + "{reviewer} · {since}", "wake-up {n} of {max}" at the right (plus segments), note "woke because {reason}"; running state not reachable in the demo, checked in code
OVL-221 | DONE | state(): stood down = check icon + "stood down" + why / "e adds milestones or wake-ups and it watches again"; paused = pause icon + "paused" + "{name} works on without it" / "p resumes watching" (shot: paused)
OVL-222 | DONE | state(): eye icon (DD-021, no dot) + "watching" + agent with logo; note verbatim (shot: status)
OVL-223 | DONE | status_page(): "Milestones" + "{done} of {total} done"; status icon + number + title wrapped to 3 lines with "…" (shot: long title); done muted, active semibold; "↳ note" in muted rather than inline (DESIGN principle 7)
OVL-224 | DONE | status_page() "Right now": "starting the reviewer…" with a spinner, last 6 steps, earlier ones a faint check (DD-021), newest a spinner; checked in code (no running reviewer in the demo)
OVL-225 | DONE | status_page() "Last wake-ups": none-yet copy verbatim (shot); runs newest first: clock, check/cross icon, reviewer, "{n} steps" right, summary up to 3 lines in danger when failed
OVL-226 | DELTA | quirk fixed on purpose: the status page body scrolls (wheel, scrollbar, edge hairlines: body/end_body/wheel), nothing is cut off
OVL-227 | DONE | status band: Wake now (primary), Pause/Resume, Edit plan, Stop watchdog (danger ink), Close, each with its key as a chip (w, p, e, s s, esc) instead of the hint line (shot: status)
OVL-228 | DONE | key_status shared; the focused button is outlined (s.button)
OVL-229 | DONE | press() shared by keys and clicks; armed: danger line "press s again to stop the watchdog; the session keeps running" and a red Stop button, any other button disarms (shots: armed, then p); test stop_takes_a_second_click
OVL-230 | DONE | overlays/mod.rs draw and click read app.svc.watchdog(sid) every frame, watchdog_key on every key; no snapshot shows the form; redraws come from Services::update (Event::Services) and the running spinner keeps frames ticking

### Watchdog prompt editor

OVL-231 | DONE | Act::WatchdogPrompt → open_watchdog_prompt; palette "watchdog prompt: edit what every reviewer is told" opens it (shot); settings row is views/settings/gui.rs Ctl::Link (other section); no default key (keymap.rs)
OVL-232 | DONE | PromptEditor::new shared: setting or DEFAULT_PROMPT, newline_on_enter, caret at the top (shots: default, reopened after save)
OVL-233 | DONE | PromptEditor::key shared; the Save and "bro's default" buttons send ctrl+s / ctrl+r to it; test buttons_do_what_their_keys_do
OVL-234 | DONE | keys shared; prompt_editor/gui.rs sets height = rows in view so PgUp/PgDn move a page less 2 (shot); armed copy in danger; mouse added (DD-013 spirit): Close = esc (arms first), a click in the text moves the caret and disarms, the wheel scrolls the view
OVL-235 | DONE | prompt_editor/gui.rs draw: card min(window−48, 1100) × (window−48), watchdog icon + "watchdog prompt"; under 420×300 the terminal drawing is used; hints are buttons with key chips (bro's default ctrl+r, Close esc, Save ctrl+s)
OVL-236 | DONE | about line verbatim, "customized" in shine / "bro's default" muted (DESIGN principle 7, terminal: inline); status line danger/inline/muted verbatim; the text box edge stands for the rule; placeholder verbatim, italic (shots)
OVL-237 | DONE | watchdog_prompt_outcome shared (shot: "watchdog prompt saved · reviewers use it from the next wake-up"); save_settings writes v2.toml outside demo

### TextArea (milestones, prompt)

OVL-238 | DONE | DD-004: milestones and the prompt are drawn in the terminal font (Style::mono) and the wrap width in cells = field px / mono_advance, so drawn rows = TextArea::rows = the rows keys move through (shots); glyphs missing from the mono font fall back and may not fill exactly 1 or 2 cells
OVL-239 | DONE | TextArea::key shared; overlay keys reach it before any chord (App::key), width from this layout
OVL-240 | DONE | TextArea::key shared; gui/input.rs gives ctrl+letters, ctrl/alt+Backspace, Delete and shift/alt+Enter as the terminal does
OVL-241 | DONE | TextArea clean() shared for typed and pasted text; the window adds no undo, selection or copy either
OVL-242 | DELTA | the caret is a blinking beam (WinUI text box; DD-010 no reverse video) instead of a reversed cell; the caret row stays in view (window() for milestones, top for the prompt); placeholder italic faint
